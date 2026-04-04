# Hand of Midas -- Data Architecture Plan

> Phase 2 deep-design document. Covers every byte on disk, every struct in memory,
> every algorithm that moves candle data from CSV to GPU. This is the single source
> of truth for `midas-data`, `midas-feed` (CSV import), and the data-related
> portions of `midas-core`.
>
> Revision 0 -- 2026-03-24

---

## Table of Contents

1. [Binary File Format](#1-binary-file-format)
2. [Memory-Mapped Access](#2-memory-mapped-access)
3. [SoA CandleBuffer](#3-soa-candlebuffer)
4. [Level of Detail (Downsampling)](#4-level-of-detail-downsampling)
5. [Data Manager](#5-data-manager)
6. [CSV Import Pipeline](#6-csv-import-pipeline)
7. [Timeframe System](#7-timeframe-system)
8. [Symbol Registry](#8-symbol-registry)
9. [Directory Layout](#9-directory-layout)
10. [Future-Proofing for Real-Time](#10-future-proofing-for-real-time)

---

## 1. Binary File Format

### 1.1 Design Goals

- **O(1) random access** by timestamp in dense mode (no index required).
- **Zero deserialization**: records are `#[repr(C)]` structs that can be read
  directly through a memory-mapped pointer cast.
- **Append-friendly**: new candles are appended to the end of the file; the
  header is updated in-place with a single 8-byte atomic write for `candle_count`
  and `end_ts`.
- **Crash-safe**: a write-ahead sequence number in the header lets readers
  detect partial writes.
- **Compact**: 32 bytes per candle record, aligned to 8 bytes for clean
  pointer arithmetic on all platforms.

### 1.2 File Extension and Magic Number

| Property       | Value                               |
|----------------|-------------------------------------|
| Extension      | `.midas`                            |
| Magic (4 bytes)| `0x4D494441` (ASCII `MIDA`)         |
| Byte order     | **Little-endian** (native on x86)   |

### 1.3 Header Layout -- 128 Bytes

The header is 128 bytes, deliberately over-sized relative to the current field
set. The extra space is reserved for forward-compatible extensions (LOD offsets,
checksum, real-time metadata) without breaking the record stride calculation.

128 bytes is exactly 2x a 64-byte cache line and falls within a single 4096-byte
Windows page, so the header and the first page of data always land in the same
initial page fault.

```
Offset  Size  Type      Field               Description
------  ----  --------  ------------------  -----------------------------------------
0x00    4     u32       magic               0x4D494441 ("MIDA")
0x04    2     u16       version             Format version. Currently 1.
0x06    2     u16       flags               Bit flags (see below)
0x08    4     u32       symbol_id           Internal SymbolId
0x0C    4     u32       timeframe_secs      Candle period in seconds
0x10    8     i64       start_ts            Timestamp of first candle (epoch ms)
0x18    8     i64       end_ts              Timestamp of last candle (epoch ms)
0x20    8     u64       candle_count        Number of candle records in body
0x28    4     u32       record_size         Size of one body record in bytes (32)
0x2C    4     u32       lod_levels          Number of pre-computed LOD levels (0 = none)
0x30    8     u64       lod_offset          Byte offset to start of LOD section (0 = none)
0x38    8     u64       write_seq           Monotonic counter, incremented on every append
0x40    4     u32       checksum            CRC32C of the entire header (bytes 0..0x40)
0x44    28    [u8; 28]  _reserved           Zero-filled; future use
0x60    32    [u8; 32]  symbol_ascii        Symbol ticker as null-terminated ASCII (debug aid)
------  ----
0x80                    <-- body begins at offset 128
```

**Total header**: 128 bytes (0x80).

#### Flags (u16 at offset 0x06)

| Bit | Name            | Meaning                                             |
|-----|-----------------|-----------------------------------------------------|
| 0   | `DENSE`         | File contains sentinel candles for non-trading gaps  |
| 1   | `ADJUSTED`      | Prices are split/dividend adjusted                   |
| 2   | `HAS_LOD`       | Pre-computed LOD pyramid is appended after body      |
| 3   | `FORMING`       | Last candle is a forming (incomplete) candle         |
| 4-15| (reserved)      | Must be zero                                         |

#### Rust Struct

```rust
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FileHeader {
    pub magic:          u32,        // 0x4D494441
    pub version:        u16,        // 1
    pub flags:          u16,
    pub symbol_id:      u32,
    pub timeframe_secs: u32,
    pub start_ts:       i64,
    pub end_ts:         i64,
    pub candle_count:   u64,
    pub record_size:    u32,        // 32
    pub lod_levels:     u32,
    pub lod_offset:     u64,
    pub write_seq:      u64,
    pub checksum:       u32,
    pub _reserved:      [u8; 28],
    pub symbol_ascii:   [u8; 32],
}
// static_assert: size_of::<FileHeader>() == 128
```

### 1.4 Body Record Layout -- 32 Bytes

Each record is 32 bytes, aligned to 8 bytes. The `_padding` field at the end
brings the natural 28-byte payload to a power-of-two-friendly 32 bytes. This
avoids records straddling cache line boundaries at every other record (28 is
not a divisor of 64) and simplifies the O(1) offset calculation.

```
Offset  Size  Type   Field       Description
------  ----  -----  ---------   -----------
0x00    8     i64    timestamp   Candle open time (epoch milliseconds, UTC)
0x08    4     f32    open
0x0C    4     f32    high
0x10    4     f32    low
0x14    4     f32    close
0x18    4     u32    volume      Total volume (capped at u32::MAX for equities)
0x1C    4     u32    _padding    Zero. Reserved for tick_count, VWAP, or flags.
------  ----
0x20 = 32 bytes
```

#### Rust Struct

```rust
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CandleRecord {
    pub timestamp: i64,
    pub open:      f32,
    pub high:      f32,
    pub low:       f32,
    pub close:     f32,
    pub volume:    u32,
    pub _padding:  u32,
}
// static_assert: size_of::<CandleRecord>() == 32
// static_assert: align_of::<CandleRecord>() == 8
```

### 1.5 Dense Mode vs Sparse Mode

**Dense mode** (`flags & DENSE`):

- Every possible candle slot between `start_ts` and `end_ts` has a record,
  even if the market was closed. Non-trading periods are stored as **sentinel
  candles**.
- Enables O(1) random access by timestamp: no binary search needed.
- Sentinel candle: `volume == 0` and `open == high == low == close == NaN`.
  Using `NaN` rather than repeating the previous close makes sentinels trivially
  detectable: `candle.open.is_nan()`.

**Sparse mode** (default, `flags & DENSE == 0`):

- Only actual trading candles are stored. No sentinels.
- Access by timestamp requires binary search on the timestamp array: O(log n).
- More compact for intraday timeframes where weekends/holidays would waste
  significant space.

**Storage comparison** (AAPL, 1 year, 1-minute timeframe):

| Mode   | Records   | Size       | Notes                                     |
|--------|-----------|------------|-------------------------------------------|
| Sparse | ~98,280   | ~3.0 MB    | Trading hours only                        |
| Dense  | ~525,600  | ~16.0 MB   | All 365*24*60 minute slots                |

**Recommendation**: Use **sparse mode** for intraday timeframes (1m, 5m, 15m)
where gaps dominate. Use **dense mode** for daily and above, where nearly every
slot has data and O(1) access is most valuable. The `flags` field records the
mode per file.

### 1.6 O(1) Random Access Formula (Dense Mode)

```
Given: target timestamp T (epoch ms)

slot_index = (T - header.start_ts) / (header.timeframe_secs * 1000)
byte_offset = 128 + slot_index * 32

// Bounds check:
if slot_index >= header.candle_count { out of range }
```

For sparse mode, fall back to binary search on the timestamp column:

```rust
fn find_index_sparse(mmap: &[u8], header: &FileHeader, target_ts: i64) -> Option<usize> {
    let count = header.candle_count as usize;
    let body = &mmap[128..];

    // Binary search over the timestamp at offset 0 of each 32-byte record
    let mut lo = 0usize;
    let mut hi = count;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let ts = i64::from_le_bytes(
            body[mid * 32..mid * 32 + 8].try_into().unwrap()
        );
        if ts < target_ts {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo < count { Some(lo) } else { None }
}
```

### 1.7 File Naming Convention

```
{SYMBOL}_{TIMEFRAME}.midas

Examples:
  AAPL_1m.midas
  AAPL_5m.midas
  AAPL_1D.midas
  SPY_1H.midas
  BTCUSD_1m.midas
```

Timeframe suffixes: `1s`, `5s`, `15s`, `30s`, `1m`, `5m`, `15m`, `30m`,
`1H`, `4H`, `1D`, `1W`, `1M`.

Files live under `data/candles/{SYMBOL}/` (see Section 9).

### 1.8 Pre-Computed LOD Pyramid (Optional)

When `flags & HAS_LOD` is set, the LOD pyramid is appended after the body:

```
Offset: lod_offset (from header)

LOD Directory (16 bytes per level):
  u32   factor          // Downsampling factor (e.g., 4, 16, 64, 256)
  u32   candle_count    // Number of records at this LOD level
  u64   byte_offset     // Absolute byte offset to this level's records

LOD Level Data:
  [CandleRecord; candle_count]  // Same 32-byte record format
```

LOD levels are computed with MinMax bucketing (Section 4). Typical pyramid for
100K base candles:

| Level | Factor | Records | Size    |
|-------|--------|---------|---------|
| 0     | 1      | 100,000 | 3.05 MB |
| 1     | 4      | 25,000  | 781 KB  |
| 2     | 16     | 6,250   | 195 KB  |
| 3     | 64     | 1,563   | 49 KB   |
| 4     | 256    | 391     | 12 KB   |

Total overhead: ~25% additional storage for instant multi-resolution access.

### 1.9 Versioning and Forward Compatibility

- Readers MUST check `magic == 0x4D494441` and `version <= SUPPORTED_VERSION`.
- Unknown flag bits are ignored by readers (allows writers to set new flags
  without breaking old readers, as long as the core layout is unchanged).
- If `record_size` differs from the compiled-in constant, the reader can still
  stride correctly using the header's `record_size` field -- this allows future
  record extensions without a version bump (new fields go into `_padding`).
- The `checksum` field covers only the header (bytes 0..0x40). A full-file
  checksum is impractical for append-heavy mmap'd files. Body integrity is
  validated by checking `write_seq` consistency and optional spot-checks.

### 1.10 Alternatives Considered for Binary Format

Before settling on the custom `.midas` format, several established binary formats were evaluated:

**Apache Parquet.** Parquet is optimized for analytical column scans and compression, not random access. Its variable-length encoding and row group structure mean that reading a single candle requires decompressing an entire row group. There is no O(1) timestamp lookup. The format also carries significant metadata overhead per column chunk that is unnecessary for our fixed-schema, append-only workload.

**Arrow IPC (Feather v2).** Arrow IPC supports memory-mapping and columnar layout, which aligns well with our SoA rendering pipeline. However, it does not provide O(1) timestamp lookup — finding a candle by timestamp still requires scanning or building a secondary index. The format's metadata overhead (schema blocks, dictionary encodings, alignment padding between record batches) adds complexity without benefit for our fixed 32-byte record layout. Arrow IPC is the strongest alternative and could be revisited if interoperability with Python/pandas tooling becomes a priority.

**SQLite.** SQLite is robust, well-tested, and supports indexed lookups. However, it introduces SQL parsing overhead on every read, page-level locking semantics that complicate concurrent reader/writer access patterns, and a B-tree structure that adds indirection for what is fundamentally a sequential-scan workload. The overhead is modest in absolute terms but unnecessary when mmap'd fixed-stride records serve the same purpose with zero abstraction cost.

**Custom `.midas` format (chosen).** Fixed 32-byte `#[repr(C)]` records with a 128-byte header. Provides O(1) random access by timestamp in dense mode (simple arithmetic on the mmap'd pointer), zero-copy reads via `bytemuck` cast, append-friendly design with atomic header updates, and trivial implementation (~200 lines of Rust). The format wins for this use case because the access pattern is simple (sequential scan for rendering, O(1) lookup for navigation) and the schema is fixed.

---

## 2. Memory-Mapped Access

### 2.1 Crate: `memmap2`

We use `memmap2` (v0.9+), the actively maintained fork of `memmap`. It supports:
- Read-only mapping (`Mmap`)
- Read-write mapping (`MmapMut`)
- Unsafe raw mapping for maximum control
- Windows and Unix

### 2.2 Opening a File for Reading

```rust
use memmap2::Mmap;
use std::fs::File;

pub struct MmapCandleFile {
    _file: File,              // Keep file handle alive
    mmap: Mmap,               // Immutable memory-mapped view
    header: FileHeader,        // Validated copy of header
    body_ptr: *const CandleRecord,  // Pointer to first record
    record_count: usize,
}

impl MmapCandleFile {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };

        // Validate minimum size
        if mmap.len() < 128 {
            return Err(DataError::FileTooSmall);
        }

        // Read and validate header
        let header: FileHeader = *bytemuck::from_bytes(&mmap[..128]);
        validate_header(&header)?;

        // Validate body size
        let expected_body = header.candle_count as usize * header.record_size as usize;
        if mmap.len() < 128 + expected_body {
            return Err(DataError::TruncatedBody);
        }

        let body_ptr = mmap[128..].as_ptr() as *const CandleRecord;

        Ok(Self {
            _file: file,
            mmap,
            header,
            body_ptr,
            record_count: header.candle_count as usize,
        })
    }

    /// Zero-copy access to a single record by index.
    #[inline]
    pub fn get(&self, index: usize) -> Option<&CandleRecord> {
        if index >= self.record_count { return None; }
        // SAFETY: We validated the file size in open(). The mmap
        // lifetime is tied to &self. CandleRecord is Pod (no
        // invalid bit patterns). Alignment is guaranteed because
        // the body starts at offset 128 (aligned to 32) and each
        // record is 32 bytes.
        Some(unsafe { &*self.body_ptr.add(index) })
    }

    /// Zero-copy slice of records.
    #[inline]
    pub fn slice(&self, range: std::ops::Range<usize>) -> &[CandleRecord] {
        assert!(range.end <= self.record_count);
        unsafe {
            std::slice::from_raw_parts(
                self.body_ptr.add(range.start),
                range.end - range.start,
            )
        }
    }

    /// Find records covering a time range. Returns index range.
    pub fn time_range(&self, start_ts: i64, end_ts: i64) -> std::ops::Range<usize> {
        if self.header.flags & FLAG_DENSE != 0 {
            // O(1) dense mode
            let tf_ms = self.header.timeframe_secs as i64 * 1000;
            let lo = ((start_ts - self.header.start_ts) / tf_ms).max(0) as usize;
            let hi = (((end_ts - self.header.start_ts) / tf_ms) + 1)
                .min(self.record_count as i64) as usize;
            lo..hi
        } else {
            // O(log n) sparse mode via binary search
            let lo = self.binary_search_ge(start_ts);
            let hi = self.binary_search_gt(end_ts);
            lo..hi
        }
    }
}
```

### 2.3 Zero-Copy Access Patterns

The critical insight: **we never deserialize the file**. The mmap'd bytes
ARE the data. `CandleRecord` is `#[repr(C)]` + `Pod`, so a pointer cast is
sufficient. The OS page cache handles caching -- when we mmap a 3 MB file,
only the pages we actually touch get loaded from disk.

**Access pattern for rendering a visible range**:

1. Chart renderer computes visible time range `[t_start, t_end]` from Camera2D.
2. Call `mmap_file.time_range(t_start, t_end)` to get index range.
3. Call `mmap_file.slice(range)` to get `&[CandleRecord]`.
4. Convert AoS slice to SoA `CandleBuffer` (Section 3) for cache-friendly iteration.

Step 4 is a copy, but it is:
- Only for the visible range (typically 500-5000 candles = 16-160 KB).
- A simple field-extraction loop that the compiler auto-vectorizes.
- Amortized: only re-executed when the viewport changes.

### 2.4 Handling File Growth (Appending New Candles)

Appending candles to an mmap'd file requires careful sequencing:

**Writer side** (runs on the feed/ingest thread):

```rust
pub struct MmapCandleWriter {
    file: File,
    mmap: MmapMut,
    header: FileHeader,
    capacity: usize,        // Pre-allocated record slots
}

impl MmapCandleWriter {
    pub fn append(&mut self, candle: &CandleRecord) -> Result<()> {
        let idx = self.header.candle_count as usize;

        // Grow file if at capacity
        if idx >= self.capacity {
            self.grow()?;
        }

        // Write record
        let offset = 128 + idx * 32;
        self.mmap[offset..offset + 32].copy_from_slice(bytemuck::bytes_of(candle));

        // Update header fields
        self.header.candle_count += 1;
        self.header.end_ts = candle.timestamp;
        self.header.write_seq += 1;

        // Flush record data first, then update header
        // (ensures readers never see header pointing to unwritten data)
        self.mmap.flush_range(offset, 32)?;

        // Write updated header
        self.mmap[..128].copy_from_slice(bytemuck::bytes_of(&self.header));
        self.mmap.flush_range(0, 128)?;

        Ok(())
    }

    fn grow(&mut self) -> Result<()> {
        let new_capacity = (self.capacity * 2).max(4096);
        let new_size = 128 + new_capacity * 32;
        self.file.set_len(new_size as u64)?;

        // Re-map. On Windows, we must unmap first.
        // memmap2 handles this: drop the old MmapMut, create a new one.
        self.mmap = unsafe { MmapMut::map_mut(&self.file)? };
        self.capacity = new_capacity;
        Ok(())
    }
}
```

**Reader side**: Readers hold an immutable `Mmap`. When the writer grows the
file, readers must re-map to see the new data. Two strategies:

1. **Periodic re-map**: Every N seconds (e.g., 1s), check if `write_seq` in the
   on-disk header has advanced past the reader's cached `write_seq`. If so,
   re-map. This is cheap -- `Mmap::map` is ~1 microsecond.

2. **Notification channel**: The writer sends a message on a crossbeam channel
   when it appends, and the reader re-maps on next frame. This is the preferred
   approach for real-time (Phase 7).

For Phase 2 (CSV import only, no concurrent writer), this is not a concern --
files are written once during import, then opened read-only.

### 2.5 Windows Platform Considerations

- **Page granularity**: Windows requires mmap offsets to be aligned to the
  system allocation granularity (typically 64 KB). Since we always map the
  entire file from offset 0, this is satisfied trivially.
- **File locking**: On Windows, a file mapped with `MmapMut` cannot be opened
  by another process for writing. We use a single writer per file. Readers use
  `Mmap` (read-only), which allows concurrent read-only access from multiple
  processes.
- **Large pages**: For very large files (>100 MB), Windows large pages (2 MB)
  can reduce TLB misses. This is a future optimization; `memmap2` does not
  expose it directly, but we can use `VirtualAlloc` with `MEM_LARGE_PAGES` if
  profiling shows TLB pressure.
- **Flush semantics**: `MmapMut::flush()` calls `FlushViewOfFile` on Windows,
  which is asynchronous. `flush_range()` is used for ordered writes (data before
  header). For crash safety, call `file.sync_all()` after flushing to force the
  write to the physical disk.

### 2.6 Corruption Detection and Recovery

**On open**:

1. Check `magic == 0x4D494441`. If not, reject.
2. Check `version <= MAX_SUPPORTED_VERSION`. If not, reject.
3. Verify `checksum` matches CRC32C of header bytes 0..0x40.
4. Verify `file_size >= 128 + candle_count * record_size`. If the file is
   truncated (e.g., crash during grow), reduce `candle_count` to fit the actual
   file size and log a warning.
5. Verify `start_ts <= end_ts`.
6. Spot-check: read the last record and verify its timestamp is `end_ts`.
   If not, the file was likely partially written. Scan backward to find the
   last valid record and truncate `candle_count`.

**On partial write detection** (via `write_seq`):

- If `write_seq` is odd, a write was in progress when the process crashed
  (the writer increments `write_seq` before writing and again after). Discard
  the last record and decrement `candle_count`.

Actually, simpler: use the `write_seq` purely as a monotonic version counter.
Crash safety comes from the write order (data before header) and the atomic
nature of 8-byte writes on x86 (naturally aligned i64/u64 writes are atomic
on all modern x86 CPUs).

---

## 3. SoA CandleBuffer

### 3.1 Purpose

The `CandleBuffer` is the in-memory representation that the renderer and
indicator engine read from. It uses Structure of Arrays (SoA) layout for
cache-friendly sequential access.

**Why not read directly from mmap?** The mmap'd file is AoS (each record has
all fields interleaved). When the renderer scans all highs to find the price
range, it touches 32 bytes per candle but only needs 4 bytes (the `high`
field). SoA layout gives 8x better cache utilization for single-field scans,
and enables SIMD vectorization (8 floats per AVX2 register).

### 3.2 CandleData Trait (defined in midas-core)

The `CandleData` trait abstracts over candle data sources. It lives in
**`midas-core`** (the leaf crate) so that `midas-chart` can program against it
without depending on `midas-data`'s concrete `CandleBuffer` type. This enables:

- **Sans-IO chart logic**: `midas-chart` accepts `&dyn CandleData` (or generics
  bounded by `CandleData`), keeping it free of concrete storage dependencies.
- **Testing**: Test fixtures can implement `CandleData` with hard-coded data,
  avoiding the need to construct a full `CandleBuffer` in chart unit tests.
- **Future streaming**: A real-time adapter wrapping a ring buffer or database
  cursor can implement `CandleData` without converting to `CandleBuffer` first.

```rust
use std::ops::Range;

/// Trait abstracting over candle data sources.
/// Implemented by CandleBuffer, and potentially by streaming adapters,
/// database cursors, or test fixtures.
pub trait CandleData {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool { self.len() == 0 }
    fn timestamp(&self, idx: usize) -> i64;
    fn open(&self, idx: usize) -> f32;
    fn high(&self, idx: usize) -> f32;
    fn low(&self, idx: usize) -> f32;
    fn close(&self, idx: usize) -> f32;
    fn volume(&self, idx: usize) -> u32;
    fn price_range(&self, range: Range<usize>) -> (f32, f32);
    fn find_index_by_time(&self, ts: i64) -> usize;
}
```

**Crate placement**: `midas-core::candle_data` (or `midas-core::data`).
The trait has no dependencies beyond `std`, which is why it belongs in the leaf crate.

**`CandleBuffer` implements `CandleData`** (see the impl block after Section 3.5).
The implementation is trivial: each method delegates to the corresponding
`CandleBuffer` field or method.

### 3.3 Struct Definition

```rust
/// Structure-of-Arrays candle buffer. Cache-friendly for rendering and
/// indicator computation. Each Vec has the same length.
#[derive(Clone, Debug)]
pub struct CandleBuffer {
    pub timestamps: Vec<i64>,    // Epoch milliseconds, monotonically increasing
    pub opens:      Vec<f32>,
    pub highs:      Vec<f32>,
    pub lows:       Vec<f32>,
    pub closes:     Vec<f32>,
    pub volumes:    Vec<u32>,
}
```

### 3.4 Zero-Copy Slice View

```rust
/// Borrowed view into a CandleBuffer or a sub-range thereof.
/// No allocation, no copy. Lifetime tied to the source buffer.
#[derive(Copy, Clone, Debug)]
pub struct CandleSlice<'a> {
    pub timestamps: &'a [i64],
    pub opens:      &'a [f32],
    pub highs:      &'a [f32],
    pub lows:       &'a [f32],
    pub closes:     &'a [f32],
    pub volumes:    &'a [u32],
}
```

### 3.5 Core Methods

```rust
impl CandleBuffer {
    pub fn new() -> Self { /* all Vecs empty */ }

    pub fn with_capacity(n: usize) -> Self {
        Self {
            timestamps: Vec::with_capacity(n),
            opens:      Vec::with_capacity(n),
            highs:      Vec::with_capacity(n),
            lows:       Vec::with_capacity(n),
            closes:     Vec::with_capacity(n),
            volumes:    Vec::with_capacity(n),
        }
    }

    #[inline]
    pub fn len(&self) -> usize { self.timestamps.len() }

    #[inline]
    pub fn is_empty(&self) -> bool { self.timestamps.is_empty() }

    pub fn push(&mut self, ts: i64, o: f32, h: f32, l: f32, c: f32, v: u32) {
        debug_assert!(
            self.timestamps.last().map_or(true, |&prev| ts > prev),
            "timestamps must be monotonically increasing"
        );
        self.timestamps.push(ts);
        self.opens.push(o);
        self.highs.push(h);
        self.lows.push(l);
        self.closes.push(c);
        self.volumes.push(v);
    }

    /// Borrow a sub-range as a CandleSlice. No allocation.
    pub fn slice(&self, range: std::ops::Range<usize>) -> CandleSlice<'_> {
        CandleSlice {
            timestamps: &self.timestamps[range.clone()],
            opens:      &self.opens[range.clone()],
            highs:      &self.highs[range.clone()],
            lows:       &self.lows[range.clone()],
            closes:     &self.closes[range.clone()],
            volumes:    &self.volumes[range],
        }
    }

    /// Binary search for the index of the first candle with timestamp >= target.
    pub fn find_index_ge(&self, target_ts: i64) -> usize {
        self.timestamps.partition_point(|&ts| ts < target_ts)
    }

    /// Binary search for the index of the first candle with timestamp > target.
    pub fn find_index_gt(&self, target_ts: i64) -> usize {
        self.timestamps.partition_point(|&ts| ts <= target_ts)
    }

    /// Return the (min_low, max_high) price range over a given index range.
    /// Hot path: called every frame for Y-axis auto-scaling.
    /// This is a tight loop over contiguous f32 arrays -- the compiler will
    /// auto-vectorize with AVX2 on x86_64.
    pub fn price_range(&self, range: std::ops::Range<usize>) -> (f32, f32) {
        let highs = &self.highs[range.clone()];
        let lows = &self.lows[range];

        let mut min_low = f32::MAX;
        let mut max_high = f32::MIN;

        // These two loops will be fused and vectorized by LLVM.
        for &h in highs {
            if h > max_high { max_high = h; }
        }
        for &l in lows {
            if l < min_low { min_low = l; }
        }

        (min_low, max_high)
    }

    /// Find the visible candle index range for a given time window.
    pub fn visible_range(&self, start_ts: i64, end_ts: i64) -> std::ops::Range<usize> {
        let lo = self.find_index_ge(start_ts);
        let hi = self.find_index_gt(end_ts);
        lo..hi
    }

    /// Replace the last candle (for forming candle updates in real-time mode).
    pub fn update_last(&mut self, ts: i64, o: f32, h: f32, l: f32, c: f32, v: u32) {
        if let Some(last) = self.timestamps.last_mut() {
            *last = ts;
            *self.opens.last_mut().unwrap() = o;
            *self.highs.last_mut().unwrap() = h;
            *self.lows.last_mut().unwrap() = l;
            *self.closes.last_mut().unwrap() = c;
            *self.volumes.last_mut().unwrap() = v;
        }
    }
}
```

#### CandleData trait implementation

`CandleBuffer` implements the `CandleData` trait (defined in `midas-core`,
see Section 3.2). The implementation delegates to the existing methods:

```rust
impl CandleData for CandleBuffer {
    fn len(&self) -> usize { self.len() }
    fn timestamp(&self, idx: usize) -> i64 { self.timestamps[idx] }
    fn open(&self, idx: usize) -> f32 { self.opens[idx] }
    fn high(&self, idx: usize) -> f32 { self.highs[idx] }
    fn low(&self, idx: usize) -> f32 { self.lows[idx] }
    fn close(&self, idx: usize) -> f32 { self.closes[idx] }
    fn volume(&self, idx: usize) -> u32 { self.volumes[idx] }
    fn price_range(&self, range: Range<usize>) -> (f32, f32) { self.price_range(range) }
    fn find_index_by_time(&self, ts: i64) -> usize { self.find_index_ge(ts) }
}
```

This allows `midas-chart` to accept `&dyn CandleData` or `impl CandleData`
without depending on the concrete `CandleBuffer` type from `midas-data`.

### 3.6 AoS-to-SoA Conversion (from mmap)

This is the bridge between the on-disk AoS format and the in-memory SoA format.
It runs when a chart loads its data or when the visible range changes significantly.

```rust
impl CandleBuffer {
    /// Convert a slice of mmap'd AoS CandleRecords to SoA CandleBuffer.
    /// This is a simple scatter: one sequential read pass over the AoS data,
    /// writing into 6 contiguous output arrays.
    ///
    /// Performance: ~0.5ms for 100K records on modern x86 (limited by memory
    /// bandwidth, not computation).
    pub fn from_records(records: &[CandleRecord]) -> Self {
        let n = records.len();
        let mut buf = Self::with_capacity(n);

        for rec in records {
            // Skip sentinel candles (NaN open)
            if rec.open.is_nan() {
                continue;
            }
            buf.timestamps.push(rec.timestamp);
            buf.opens.push(rec.open);
            buf.highs.push(rec.high);
            buf.lows.push(rec.low);
            buf.closes.push(rec.close);
            buf.volumes.push(rec.volume);
        }

        buf
    }
}
```

### 3.7 When Does Conversion Happen?

**On load** (eager, not lazy). Rationale:

- The visible candle count is typically 500-5000. Converting 5000 records takes
  ~25 microseconds. This is negligible compared to the page fault cost of first
  touching the mmap'd data (~100-500 microseconds for cold pages).
- Lazy per-field conversion adds complexity for no measurable gain at these sizes.
- The SoA buffer is the primary data source for both the renderer AND the
  indicator engine. Converting once serves both consumers.

**Conversion triggers**:

| Event                      | Action                                      |
|---------------------------|----------------------------------------------|
| Chart opens a symbol      | Load full file, convert to CandleBuffer       |
| Viewport scrolls/zooms    | Re-slice the existing CandleBuffer (no copy)  |
| Significant range change  | If user scrolls far, may need a wider buffer  |
| New candle appended       | `push()` to existing buffer (no re-convert)   |
| Timeframe change          | Load different file, full re-convert           |

**Over-fetch strategy**: When loading from the mmap, fetch 2x the visible range
(1x before viewport, 1x after). This means small pans do not trigger a reload.
Only when the user scrolls past the pre-fetched boundary do we re-fetch from the
mmap. This is a simple sliding window.

```
|---pre-fetched buffer (2x viewport)---|
        |---visible viewport---|
```

---

## 4. Level of Detail (Downsampling)

### 4.1 The Problem

At maximum zoom-out, a chart might contain 500,000 one-minute candles but only
have 2,000 pixels of width. Drawing all 500K candles means 250 candles per pixel
column -- the GPU wastes time rasterizing sub-pixel geometry that is invisible.
More critically, uploading 500K CandleInstance structs (16 MB) to the GPU every
frame is wasteful.

**Goal**: Never process more than `2 * viewport_width` candles for rendering.
At 4K resolution (3840px), that is at most ~8000 candles.

### 4.2 MinMax Bucketing for Candles

MinMax bucketing is the correct downsampling algorithm for OHLCV data because
it preserves the **price envelope** -- the visual extremes that define chart
shape.

**Algorithm**:

```
Input:  candles[0..N], target_count T
Output: super_candles[0..T]

bucket_size = N / T  (integer division, handle remainder in last bucket)

for i in 0..T:
    bucket = candles[i * bucket_size .. min((i+1) * bucket_size, N)]

    super_candle.timestamp = bucket[0].timestamp       // First timestamp
    super_candle.open      = bucket[0].open             // First open
    super_candle.high      = max(bucket[*].high)        // Highest high
    super_candle.low       = min(bucket[*].low)          // Lowest low
    super_candle.close     = bucket[last].close          // Last close
    super_candle.volume    = sum(bucket[*].volume)       // Total volume
```

This is identical to how higher timeframe candles are computed from lower
timeframe candles. A "super-candle" spanning 10 one-minute candles is
equivalent to a 10-minute candle.

**Rust pseudocode**:

```rust
pub fn downsample_minmax(src: &CandleSlice, target_count: usize) -> CandleBuffer {
    let n = src.timestamps.len();
    if n <= target_count {
        // No downsampling needed; copy as-is
        return CandleBuffer::from_slice(src);
    }

    let bucket_size = n / target_count;
    let mut out = CandleBuffer::with_capacity(target_count);

    let mut i = 0;
    while i < n {
        let end = (i + bucket_size).min(n);
        let bucket_highs = &src.highs[i..end];
        let bucket_lows = &src.lows[i..end];

        // SIMD-friendly: these are contiguous f32 scans
        let high = bucket_highs.iter().copied().fold(f32::MIN, f32::max);
        let low = bucket_lows.iter().copied().fold(f32::MAX, f32::min);
        let volume: u32 = src.volumes[i..end].iter().sum();

        out.push(
            src.timestamps[i],     // first timestamp
            src.opens[i],          // first open
            high,                  // max high
            low,                   // min low
            src.closes[end - 1],   // last close
            volume,                // sum volume
        );

        i = end;
    }

    out
}
```

**Performance**: For 100K candles downsampled to 4K:
- 25 candles per bucket
- 100K / 8 (AVX2 f32 lanes) = 12,500 SIMD iterations for high scan
- ~50 microseconds total. Well under the 1ms budget.

### 4.3 LTTB for Line Data (Indicators)

For single-valued line data (SMA, EMA, RSI output), MinMax bucketing produces
a jagged result. The **Largest Triangle Three Buckets** (LTTB) algorithm
preserves visual shape by selecting the point in each bucket that maximizes
the triangle area with its neighbors.

```rust
/// LTTB downsampling for line data.
/// Input: timestamps and values of equal length.
/// Output: indices of selected points (caller extracts the actual values).
pub fn lttb_indices(
    timestamps: &[i64],
    values: &[f32],
    target_count: usize,
) -> Vec<usize> {
    let n = timestamps.len();
    if n <= target_count {
        return (0..n).collect();
    }

    let mut selected = Vec::with_capacity(target_count);
    selected.push(0);  // Always keep first point

    let bucket_size = (n - 2) as f64 / (target_count - 2) as f64;

    let mut prev_selected = 0usize;

    for i in 1..(target_count - 1) {
        // Current bucket range
        let bucket_start = ((i - 1) as f64 * bucket_size + 1.0) as usize;
        let bucket_end = ((i as f64) * bucket_size + 1.0).min(n as f64) as usize;

        // Next bucket range (for computing the average point)
        let next_start = bucket_end;
        let next_end = (((i + 1) as f64) * bucket_size + 1.0).min(n as f64) as usize;

        // Average of next bucket (the "C" point of the triangle)
        let avg_ts: f64 = timestamps[next_start..next_end].iter()
            .map(|&t| t as f64).sum::<f64>() / (next_end - next_start) as f64;
        let avg_val: f64 = values[next_start..next_end].iter()
            .map(|&v| v as f64).sum::<f64>() / (next_end - next_start) as f64;

        // Previous selected point (the "A" point)
        let a_ts = timestamps[prev_selected] as f64;
        let a_val = values[prev_selected] as f64;

        // Find point in current bucket that maximizes triangle area
        let mut max_area = -1.0f64;
        let mut max_idx = bucket_start;

        for j in bucket_start..bucket_end {
            let area = ((a_ts - avg_ts) * (values[j] as f64 - a_val)
                      - (a_ts - timestamps[j] as f64) * (avg_val - a_val))
                      .abs();
            if area > max_area {
                max_area = area;
                max_idx = j;
            }
        }

        selected.push(max_idx);
        prev_selected = max_idx;
    }

    selected.push(n - 1);  // Always keep last point
    selected
}
```

### 4.4 Auto-LOD Selection

Given the viewport width and total candle count, determine whether downsampling
is needed and at what factor.

```rust
/// Determine optimal target candle count for the given viewport.
///
/// Rules:
///   - If total candles fit in 2 * viewport_width pixels, no downsampling.
///   - Otherwise, target = 2 * viewport_width (2 candles per pixel gives
///     sub-pixel fidelity for anti-aliased wicks).
///   - Clamp to a minimum of 256 candles (prevents degenerate ultra-zoom-out).
///
/// Returns: (target_count, bucket_size) where bucket_size == 1 means no LOD.
pub fn compute_lod(total_candles: usize, viewport_width: u32) -> (usize, usize) {
    let max_useful = (viewport_width as usize) * 2;

    if total_candles <= max_useful {
        return (total_candles, 1);
    }

    let target = max_useful.max(256);
    let bucket_size = total_candles / target;

    (target, bucket_size)
}
```

### 4.5 Pre-Computed LOD Pyramid vs On-the-Fly

| Approach         | Pros                              | Cons                           |
|------------------|-----------------------------------|--------------------------------|
| Pre-computed     | Zero runtime cost; instant access | 25% extra disk space; stale on append |
| On-the-fly       | Always fresh; no extra storage    | ~50-200 microsecond compute per frame |

**Decision**: Use **on-the-fly** computation for Phase 2. Reasons:

1. The 50-200 microsecond cost per frame is acceptable (well within the 14ms budget).
2. On-the-fly avoids the complexity of invalidating and rebuilding the LOD
   pyramid when new candles are appended (critical for Phase 7 real-time).
3. The LOD result can be cached in the DataManager and only recomputed when the
   visible range or zoom level changes -- which happens less than once per frame
   during smooth scrolling.

**Future optimization** (if profiling shows LOD is a bottleneck for 20+ charts):
Pre-compute the pyramid on import and store in the `.midas` file (Section 1.8).
The on-the-fly code path remains as a fallback for files without a pre-computed
pyramid.

### 4.6 LOD Cache Integration

The DataManager (Section 5) maintains a per-chart LOD cache:

```rust
struct LodCache {
    /// The source range (indices into the full CandleBuffer) that this LOD covers
    source_range: std::ops::Range<usize>,
    /// The bucket size used for downsampling
    bucket_size: usize,
    /// The downsampled result
    buffer: CandleBuffer,
}
```

The cache is invalidated when:
- The visible range shifts by more than 25% of its width.
- The zoom level changes such that the optimal bucket_size differs.
- New candles are appended.

---

## 5. Data Manager

### 5.1 Role

The DataManager is the central coordinator for all candle data access. It sits
between the disk (mmap'd binary files) and the consumers (chart renderer,
indicator engine). Its responsibilities:

- Load and cache mmap'd files on demand.
- Maintain per-symbol SoA CandleBuffers in an LRU cache.
- Apply LOD downsampling as needed.
- Provide a thread-safe interface for the render thread to request visible candles.
- Coordinate background loading so the UI never blocks on I/O.

### 5.2 Interface

```rust
use std::sync::Arc;
use parking_lot::RwLock;

/// The public interface that chart renderers call.
pub struct DataManager {
    data_dir: PathBuf,
    registry: SymbolRegistry,
    /// LRU cache of loaded candle data, keyed by (SymbolId, Timeframe).
    cache: RwLock<LruCache<(SymbolId, Timeframe), Arc<CandleBuffer>>>,
    /// Background loader handle
    loader_tx: crossbeam::channel::Sender<LoadRequest>,
}

pub struct LoadRequest {
    pub symbol: SymbolId,
    pub timeframe: Timeframe,
    pub reply: oneshot::Sender<Result<Arc<CandleBuffer>>>,
}

impl DataManager {
    pub fn new(data_dir: PathBuf) -> Self { ... }

    /// Get the full candle buffer for a symbol/timeframe.
    /// Returns immediately if cached; otherwise triggers background load
    /// and returns None (caller should display a loading state).
    pub fn get(
        &self,
        symbol: SymbolId,
        tf: Timeframe,
    ) -> Option<Arc<CandleBuffer>> {
        // Check cache (read lock, fast path)
        if let Some(buf) = self.cache.read().get(&(symbol, tf)) {
            return Some(Arc::clone(buf));
        }
        // Not cached: trigger background load
        self.request_load(symbol, tf);
        None
    }

    /// Synchronous load -- blocks until data is available.
    /// Use only during initialization or testing.
    pub fn get_blocking(
        &self,
        symbol: SymbolId,
        tf: Timeframe,
    ) -> Result<Arc<CandleBuffer>> {
        if let Some(buf) = self.cache.read().get(&(symbol, tf)) {
            return Ok(Arc::clone(buf));
        }
        let buf = self.load_from_disk(symbol, tf)?;
        let arc = Arc::new(buf);
        self.cache.write().put((symbol, tf), Arc::clone(&arc));
        Ok(arc)
    }

    /// Get visible candles, applying LOD if needed.
    /// This is the primary interface for the chart renderer.
    pub fn get_visible(
        &self,
        symbol: SymbolId,
        tf: Timeframe,
        time_range: (i64, i64),
        viewport_width: u32,
    ) -> Option<CandleBuffer> {
        let full = self.get(symbol, tf)?;

        // Find visible range
        let range = full.visible_range(time_range.0, time_range.1);
        let visible_count = range.end - range.start;

        // Apply LOD if needed
        let (target, bucket_size) = compute_lod(visible_count, viewport_width);

        if bucket_size <= 1 {
            // No downsampling: return a sub-slice copy
            Some(CandleBuffer::from_slice(&full.slice(range)))
        } else {
            // Downsample
            Some(downsample_minmax(&full.slice(range), target))
        }
    }

    /// Import a CSV file, convert to binary, and add to cache.
    pub fn import_csv(
        &self,
        csv_path: &Path,
        symbol: &str,
        tf: Timeframe,
    ) -> Result<SymbolId> { ... }

    /// List all available symbols (discovered from data directory).
    pub fn available_symbols(&self) -> Vec<(SymbolId, String)> {
        self.registry.all()
    }

    // --- Internal ---

    fn load_from_disk(&self, symbol: SymbolId, tf: Timeframe) -> Result<CandleBuffer> {
        let path = self.binary_path(symbol, tf);
        let mmap_file = MmapCandleFile::open(&path)?;
        let records = mmap_file.slice(0..mmap_file.record_count());
        Ok(CandleBuffer::from_records(records))
    }

    fn binary_path(&self, symbol: SymbolId, tf: Timeframe) -> PathBuf {
        let ticker = self.registry.ticker(symbol);
        self.data_dir
            .join("candles")
            .join(&ticker)
            .join(format!("{}_{}.midas", ticker, tf.file_suffix()))
    }

    fn request_load(&self, symbol: SymbolId, tf: Timeframe) {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.loader_tx.send(LoadRequest {
            symbol,
            timeframe: tf,
            reply: reply_tx,
        });
        // The background thread will load and insert into cache.
        // The chart will pick it up on the next frame.
    }
}
```

### 5.3 LRU Cache Strategy

```rust
use lru::LruCache;
use std::num::NonZeroUsize;

// Configuration
const MAX_CACHED_SERIES: usize = 64;  // 64 symbol/timeframe combinations

// Memory budget: 64 series * ~100K candles avg * 24 bytes/candle = ~150 MB
// Actual usage is much less: most series have 10K-50K candles.
```

Eviction policy: standard LRU. When a chart is closed and its symbol is not
displayed in any other chart, the entry becomes eligible for eviction. It stays
in the cache until the slot is needed by a new load request.

### 5.4 Thread Safety Model

```
Main Thread (iced event loop)
  |
  |-- reads from DataManager.get_visible()  [RwLock read, fast path]
  |-- calls DataManager.get() to trigger loads
  |
Background Loader Thread (spawned once)
  |
  |-- receives LoadRequest from channel
  |-- opens mmap file, converts to CandleBuffer
  |-- inserts into cache [RwLock write]
  |-- sends reply on oneshot channel
```

**Lock contention analysis**:

- The read path (`get()`, `get_visible()`) takes a `RwLock::read()`. Multiple
  charts can read concurrently.
- The write path (background loader inserting into cache) takes `RwLock::write()`.
  This blocks readers briefly, but the critical section is just a HashMap
  insert (~100ns). With `parking_lot::RwLock`, reader-writer fairness prevents
  starvation.
- During normal rendering (all data loaded), the write lock is never taken.
  Zero contention.

### 5.5 Background Loader

```rust
fn spawn_loader(
    rx: crossbeam::channel::Receiver<LoadRequest>,
    cache: Arc<RwLock<LruCache<(SymbolId, Timeframe), Arc<CandleBuffer>>>>,
    data_dir: PathBuf,
    registry: Arc<SymbolRegistry>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("midas-data-loader".to_string())
        .spawn(move || {
            for req in rx {
                let path = make_path(&data_dir, &registry, req.symbol, req.timeframe);
                let result = load_and_convert(&path);

                match result {
                    Ok(buf) => {
                        let arc = Arc::new(buf);
                        cache.write().put(
                            (req.symbol, req.timeframe),
                            Arc::clone(&arc)
                        );
                        let _ = req.reply.send(Ok(arc));
                    }
                    Err(e) => {
                        let _ = req.reply.send(Err(e));
                    }
                }
            }
        })
        .expect("failed to spawn data loader thread")
}
```

---

## 6. CSV Import Pipeline

### 6.1 Supported Formats

The CSV importer handles common historical data export formats:

| Source          | Date Format               | Columns                               |
|-----------------|---------------------------|---------------------------------------|
| Yahoo Finance   | `2024-01-15`              | Date, Open, High, Low, Close, Adj Close, Volume |
| Polygon.io      | epoch ms or `2024-01-15`  | timestamp, open, high, low, close, volume, vwap, transactions |
| Alpha Vantage   | `2024-01-15`              | timestamp, open, high, low, close, volume |
| TradingView     | `2024-01-15T09:30:00-05:00` | time, open, high, low, close, volume |
| Generic         | Any parseable date/time   | Auto-detected by header               |

### 6.2 Auto-Detection Algorithm

```rust
pub fn detect_csv_format(header: &str, first_row: &str) -> CsvFormat {
    let columns: Vec<&str> = header.split(',').map(str::trim).collect();

    // Find OHLCV columns by name (case-insensitive)
    let date_col = find_col(&columns, &["date", "time", "timestamp", "datetime"]);
    let open_col = find_col(&columns, &["open", "o"]);
    let high_col = find_col(&columns, &["high", "h"]);
    let low_col  = find_col(&columns, &["low", "l"]);
    let close_col = find_col(&columns, &["close", "c", "adj close"]);
    let vol_col  = find_col(&columns, &["volume", "vol", "v"]);

    // Detect date format from first row
    let date_sample = first_row.split(',').nth(date_col).unwrap().trim();
    let date_format = detect_date_format(date_sample);

    CsvFormat {
        delimiter: ',',     // Could also detect tab, semicolon
        date_col,
        open_col,
        high_col,
        low_col,
        close_col,
        volume_col: vol_col,
        date_format,
        has_header: true,
    }
}
```

### 6.3 Date Parsing and Timezone Handling

Date/time parsing is the hardest part of CSV import. Rules:

1. **If the CSV contains epoch milliseconds** (detected as a large integer):
   use directly. No timezone conversion needed.

2. **If the CSV contains epoch seconds**: multiply by 1000.

3. **If the CSV contains a date string with explicit timezone** (e.g.,
   `2024-01-15T09:30:00-05:00`): parse and convert to UTC epoch ms.

4. **If the CSV contains a date string WITHOUT timezone** (e.g., `2024-01-15`
   or `2024-01-15 09:30:00`):
   - **Daily and above**: Treat as UTC. Daily bars conventionally use midnight UTC
     or the exchange's market close time. Use midnight UTC (00:00:00Z) as the
     canonical timestamp.
   - **Intraday**: Assume **US Eastern** (America/New_York) unless the user
     specifies otherwise. This handles DST correctly: EST (UTC-5) in winter,
     EDT (UTC-4) in summer. Use the `chrono-tz` crate for DST-aware conversion.

5. **DST edge case**: On the spring-forward day (e.g., March 10 2024), the
   timestamp `02:30 ET` does not exist. On the fall-back day, `01:30 ET`
   occurs twice. The parser must handle these via `chrono`'s `MappedLocalTime`
   enum (return the earliest mapping for ambiguous times, return the next valid
   time for non-existent times).

```rust
use chrono::{NaiveDateTime, TimeZone, MappedLocalTime};
use chrono_tz::America::New_York;

fn parse_naive_to_utc_ms(
    naive: NaiveDateTime,
    assume_tz: &chrono_tz::Tz,
) -> i64 {
    match assume_tz.from_local_datetime(&naive) {
        MappedLocalTime::Single(dt) => dt.timestamp_millis(),
        MappedLocalTime::Ambiguous(dt, _) => dt.timestamp_millis(),  // Use earlier
        MappedLocalTime::None => {
            // Non-existent time (DST spring-forward gap).
            // Advance to the next valid time.
            let advanced = naive + chrono::Duration::hours(1);
            assume_tz.from_local_datetime(&advanced)
                .earliest()
                .unwrap()
                .timestamp_millis()
        }
    }
}
```

### 6.4 Validation Rules

After parsing, before writing to binary, validate every candle:

```rust
pub fn validate_candle(ts: i64, o: f32, h: f32, l: f32, c: f32, v: u32,
                       prev_ts: Option<i64>) -> Result<()> {
    // 1. No negative prices
    if o < 0.0 || h < 0.0 || l < 0.0 || c < 0.0 {
        return Err(ImportError::NegativePrice);
    }

    // 2. OHLC relationship: high >= max(open, close), low <= min(open, close)
    if h < o.max(c) || l > o.min(c) {
        return Err(ImportError::InvalidOhlc);
    }

    // 3. High >= Low
    if h < l {
        return Err(ImportError::HighBelowLow);
    }

    // 4. Timestamp is positive and reasonable (after year 2000, before year 2100)
    let min_ts = 946_684_800_000i64;   // 2000-01-01 UTC
    let max_ts = 4_102_444_800_000i64; // 2100-01-01 UTC
    if ts < min_ts || ts > max_ts {
        return Err(ImportError::TimestampOutOfRange);
    }

    // 5. Monotonically increasing timestamps
    if let Some(prev) = prev_ts {
        if ts <= prev {
            return Err(ImportError::NonMonotonicTimestamp { prev, current: ts });
        }
    }

    // 6. No NaN or Infinity
    if o.is_nan() || h.is_nan() || l.is_nan() || c.is_nan() {
        return Err(ImportError::NanPrice);
    }
    if !o.is_finite() || !h.is_finite() || !l.is_finite() || !c.is_finite() {
        return Err(ImportError::InfinitePrice);
    }

    Ok(())
}
```

### 6.5 Full Import Pipeline

```rust
pub fn import_csv(
    csv_path: &Path,
    symbol_id: SymbolId,
    symbol_ticker: &str,
    timeframe: Timeframe,
    output_dir: &Path,
) -> Result<ImportReport> {
    // 1. Read CSV file
    let content = std::fs::read_to_string(csv_path)?;
    let mut lines = content.lines();

    // 2. Detect format from header + first data row
    let header = lines.next().ok_or(ImportError::EmptyFile)?;
    let first_row = lines.next().ok_or(ImportError::NoData)?;
    let format = detect_csv_format(header, first_row);

    // 3. Parse all rows into CandleBuffer
    let mut buffer = CandleBuffer::new();
    let mut errors = Vec::new();
    let mut prev_ts: Option<i64> = None;

    // Re-read from the beginning (first_row was consumed for detection)
    for (line_num, line) in content.lines().skip(1).enumerate() {
        match parse_csv_row(line, &format) {
            Ok((ts, o, h, l, c, v)) => {
                match validate_candle(ts, o, h, l, c, v, prev_ts) {
                    Ok(()) => {
                        buffer.push(ts, o, h, l, c, v);
                        prev_ts = Some(ts);
                    }
                    Err(e) => {
                        errors.push((line_num + 2, e));
                        // Skip this row, continue importing
                    }
                }
            }
            Err(e) => {
                errors.push((line_num + 2, e.into()));
            }
        }
    }

    if buffer.is_empty() {
        return Err(ImportError::NoValidCandles);
    }

    // 4. Sort by timestamp (some CSVs are in reverse chronological order)
    if !is_sorted(&buffer.timestamps) {
        sort_buffer_by_timestamp(&mut buffer);
    }

    // 5. Write binary file
    let output_path = output_dir
        .join(symbol_ticker)
        .join(format!("{}_{}.midas", symbol_ticker, timeframe.file_suffix()));

    std::fs::create_dir_all(output_path.parent().unwrap())?;
    write_binary_file(&output_path, symbol_id, symbol_ticker, timeframe, &buffer)?;

    // 6. Return report
    Ok(ImportReport {
        candles_imported: buffer.len(),
        candles_skipped: errors.len(),
        errors,
        time_range: (buffer.timestamps[0], *buffer.timestamps.last().unwrap()),
        output_path,
    })
}
```

### 6.6 Write Binary File

```rust
fn write_binary_file(
    path: &Path,
    symbol_id: SymbolId,
    symbol_ticker: &str,
    timeframe: Timeframe,
    buffer: &CandleBuffer,
) -> Result<()> {
    let n = buffer.len();
    let file_size = 128 + n * 32;

    // Create file and pre-allocate
    let file = File::create(path)?;
    file.set_len(file_size as u64)?;

    let mut mmap = unsafe { MmapMut::map_mut(&file)? };

    // Build header
    let mut header = FileHeader::zeroed();
    header.magic = 0x4D494441;
    header.version = 1;
    header.flags = 0;  // Sparse mode for now
    header.symbol_id = symbol_id.0;
    header.timeframe_secs = timeframe.as_secs();
    header.start_ts = buffer.timestamps[0];
    header.end_ts = *buffer.timestamps.last().unwrap();
    header.candle_count = n as u64;
    header.record_size = 32;
    header.lod_levels = 0;
    header.lod_offset = 0;
    header.write_seq = 1;

    // Copy symbol ticker into fixed-size ASCII field
    let ticker_bytes = symbol_ticker.as_bytes();
    let copy_len = ticker_bytes.len().min(31);
    header.symbol_ascii[..copy_len].copy_from_slice(&ticker_bytes[..copy_len]);

    // Compute checksum over first 64 bytes of header
    header.checksum = crc32c(&bytemuck::bytes_of(&header)[..0x40]);

    // Write header
    mmap[..128].copy_from_slice(bytemuck::bytes_of(&header));

    // Write body records (SoA -> AoS conversion)
    for i in 0..n {
        let record = CandleRecord {
            timestamp: buffer.timestamps[i],
            open:      buffer.opens[i],
            high:      buffer.highs[i],
            low:       buffer.lows[i],
            close:     buffer.closes[i],
            volume:    buffer.volumes[i],
            _padding:  0,
        };
        let offset = 128 + i * 32;
        mmap[offset..offset + 32].copy_from_slice(bytemuck::bytes_of(&record));
    }

    // Flush to disk
    mmap.flush()?;
    file.sync_all()?;

    Ok(())
}
```

---

## 7. Timeframe System

### 7.1 Timeframe Enum

```rust
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Timeframe {
    S1,   // 1 second
    S5,   // 5 seconds
    S15,  // 15 seconds
    S30,  // 30 seconds
    M1,   // 1 minute
    M5,   // 5 minutes
    M15,  // 15 minutes
    M30,  // 30 minutes
    H1,   // 1 hour
    H4,   // 4 hours
    D1,   // 1 day
    W1,   // 1 week
    MN1,  // 1 month
}

impl Timeframe {
    /// Duration in seconds. For D1/W1/MN1 these are nominal values
    /// (actual calendar duration varies for months).
    pub const fn as_secs(&self) -> u32 {
        match self {
            Self::S1  => 1,
            Self::S5  => 5,
            Self::S15 => 15,
            Self::S30 => 30,
            Self::M1  => 60,
            Self::M5  => 300,
            Self::M15 => 900,
            Self::M30 => 1800,
            Self::H1  => 3600,
            Self::H4  => 14400,
            Self::D1  => 86400,
            Self::W1  => 604800,
            Self::MN1 => 2592000,  // 30 days nominal
        }
    }

    /// File suffix for binary file naming.
    pub const fn file_suffix(&self) -> &'static str {
        match self {
            Self::S1  => "1s",
            Self::S5  => "5s",
            Self::S15 => "15s",
            Self::S30 => "30s",
            Self::M1  => "1m",
            Self::M5  => "5m",
            Self::M15 => "15m",
            Self::M30 => "30m",
            Self::H1  => "1H",
            Self::H4  => "4H",
            Self::D1  => "1D",
            Self::W1  => "1W",
            Self::MN1 => "1M",
        }
    }

    /// Display name for UI.
    pub const fn display(&self) -> &'static str {
        match self {
            Self::S1  => "1s",
            Self::S5  => "5s",
            Self::S15 => "15s",
            Self::S30 => "30s",
            Self::M1  => "1m",
            Self::M5  => "5m",
            Self::M15 => "15m",
            Self::M30 => "30m",
            Self::H1  => "1H",
            Self::H4  => "4H",
            Self::D1  => "1D",
            Self::W1  => "1W",
            Self::MN1 => "1M",
        }
    }

    /// Parse from file suffix string (e.g., "1m" -> M1).
    pub fn from_suffix(s: &str) -> Option<Self> {
        match s {
            "1s"  => Some(Self::S1),
            "5s"  => Some(Self::S5),
            "15s" => Some(Self::S15),
            "30s" => Some(Self::S30),
            "1m"  => Some(Self::M1),
            "5m"  => Some(Self::M5),
            "15m" => Some(Self::M15),
            "30m" => Some(Self::M30),
            "1H"  => Some(Self::H1),
            "4H"  => Some(Self::H4),
            "1D"  => Some(Self::D1),
            "1W"  => Some(Self::W1),
            "1M"  => Some(Self::MN1),
            _     => None,
        }
    }

    /// Whether this timeframe is calendar-aligned (boundary depends on
    /// calendar, not just modular arithmetic).
    pub const fn is_calendar(&self) -> bool {
        matches!(self, Self::W1 | Self::MN1)
    }
}
```

### 7.2 Boundary Alignment

Given an arbitrary timestamp, compute the start of the candle period that
contains it.

```rust
impl Timeframe {
    /// Align a timestamp (epoch ms) to the start of its candle period.
    ///
    /// For sub-daily timeframes: pure modular arithmetic on UTC.
    /// For D1: floor to midnight UTC.
    /// For W1: floor to Monday 00:00 UTC.
    /// For MN1: floor to 1st of the month 00:00 UTC.
    pub fn floor_timestamp(&self, ts_ms: i64) -> i64 {
        match self {
            // Sub-daily: modular arithmetic
            Self::S1 | Self::S5 | Self::S15 | Self::S30 |
            Self::M1 | Self::M5 | Self::M15 | Self::M30 |
            Self::H1 | Self::H4 => {
                let period_ms = self.as_secs() as i64 * 1000;
                ts_ms - (ts_ms.rem_euclid(period_ms))
            }

            // Daily: floor to midnight UTC
            Self::D1 => {
                let day_ms = 86_400_000i64;
                ts_ms - (ts_ms.rem_euclid(day_ms))
            }

            // Weekly: floor to Monday 00:00 UTC
            // Epoch (1970-01-01) was a Thursday. Monday is day 4 of that week.
            // Days since epoch: ts / 86400000
            // Day of week: (days + 3) % 7  (0=Monday, 6=Sunday)
            Self::W1 => {
                let day_ms = 86_400_000i64;
                let days = ts_ms.div_euclid(day_ms);
                let dow = (days + 3).rem_euclid(7); // 0=Mon
                let monday_ts = (days - dow) * day_ms;
                monday_ts
            }

            // Monthly: floor to 1st of the month
            Self::MN1 => {
                let dt = chrono::DateTime::from_timestamp_millis(ts_ms).unwrap();
                let floored = dt.date_naive()
                    .with_day(1)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap();
                floored.and_utc().timestamp_millis()
            }
        }
    }

    /// Compute the timestamp of the next candle boundary after `ts_ms`.
    pub fn next_boundary(&self, ts_ms: i64) -> i64 {
        match self {
            Self::MN1 => {
                let dt = chrono::DateTime::from_timestamp_millis(ts_ms).unwrap();
                let d = dt.date_naive();
                let next_month = if d.month() == 12 {
                    chrono::NaiveDate::from_ymd_opt(d.year() + 1, 1, 1).unwrap()
                } else {
                    chrono::NaiveDate::from_ymd_opt(d.year(), d.month() + 1, 1).unwrap()
                };
                next_month.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis()
            }
            _ => {
                let floored = self.floor_timestamp(ts_ms);
                floored + self.as_secs() as i64 * 1000
            }
        }
    }
}
```

### 7.3 Timeframe Aggregation

Aggregation computes higher timeframe candles from lower timeframe candles.

**When to aggregate**: On import (pre-aggregate), not on-demand. Reasons:
- Importing is a one-time cost (seconds per symbol).
- On-demand aggregation adds latency on every timeframe switch.
- Disk space for pre-aggregated timeframes is negligible (see Section 1.8
  storage comparison -- all standard timeframes for one symbol cost ~6 MB/year).

**Algorithm**:

```rust
/// Aggregate candles from a source buffer to a target timeframe.
/// The source buffer must be at a lower timeframe than the target.
pub fn aggregate(
    src: &CandleBuffer,
    src_tf: Timeframe,
    target_tf: Timeframe,
) -> CandleBuffer {
    assert!(target_tf.as_secs() > src_tf.as_secs());

    let mut out = CandleBuffer::new();
    if src.is_empty() { return out; }

    let mut i = 0;
    while i < src.len() {
        let boundary_start = target_tf.floor_timestamp(src.timestamps[i]);
        let boundary_end = target_tf.next_boundary(boundary_start);

        // Collect all source candles within this target period
        let mut open = src.opens[i];
        let mut high = src.highs[i];
        let mut low = src.lows[i];
        let mut close = src.closes[i];
        let mut volume: u64 = src.volumes[i] as u64;

        i += 1;
        while i < src.len() && src.timestamps[i] < boundary_end {
            high = high.max(src.highs[i]);
            low = low.min(src.lows[i]);
            close = src.closes[i];
            volume += src.volumes[i] as u64;
            i += 1;
        }

        out.push(
            boundary_start,
            open,
            high,
            low,
            close,
            volume.min(u32::MAX as u64) as u32,
        );
    }

    out
}
```

### 7.4 Aggregation Hierarchy

On CSV import of 1-minute data, pre-aggregate the following hierarchy:

```
1m (source)
 ├── 5m   (aggregate from 1m)
 ├── 15m  (aggregate from 1m)
 ├── 30m  (aggregate from 1m)
 ├── 1H   (aggregate from 1m)
 ├── 4H   (aggregate from 1m)
 ├── 1D   (aggregate from 1m)
 ├── 1W   (aggregate from 1D)
 └── 1M   (aggregate from 1D)
```

If the CSV contains daily data, aggregate W1 and MN1 from it. The lower
timeframes (intraday) will not be available unless intraday source data is
provided.

Each aggregated timeframe is written to its own `.midas` binary file.

---

## 8. Symbol Registry

### 8.1 Purpose

The symbol registry maps between human-readable ticker strings ("AAPL", "SPY")
and compact `SymbolId(u32)` values used throughout the codebase. SymbolIds are
used in:

- Binary file headers (`symbol_id` field).
- The DataManager's LRU cache keys.
- The chart state (each chart references a SymbolId, not a string).
- Real-time feed subscriptions.

### 8.2 Data Structure

```rust
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct SymbolId(pub u32);

pub struct SymbolRegistry {
    ticker_to_id: HashMap<String, SymbolId>,
    id_to_ticker: HashMap<SymbolId, String>,
    next_id: u32,
    persist_path: PathBuf,
}

impl SymbolRegistry {
    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        let persist_path = data_dir.join("symbols.json");
        if persist_path.exists() {
            let json = std::fs::read_to_string(&persist_path)?;
            let entries: Vec<(String, u32)> = serde_json::from_str(&json)?;
            let mut reg = Self {
                ticker_to_id: HashMap::new(),
                id_to_ticker: HashMap::new(),
                next_id: 0,
                persist_path,
            };
            for (ticker, id) in entries {
                reg.ticker_to_id.insert(ticker.clone(), SymbolId(id));
                reg.id_to_ticker.insert(SymbolId(id), ticker);
                reg.next_id = reg.next_id.max(id + 1);
            }
            Ok(reg)
        } else {
            Ok(Self {
                ticker_to_id: HashMap::new(),
                id_to_ticker: HashMap::new(),
                next_id: 0,
                persist_path,
            })
        }
    }

    /// Get or create a SymbolId for the given ticker.
    pub fn get_or_insert(&mut self, ticker: &str) -> SymbolId {
        let upper = ticker.to_uppercase();
        if let Some(&id) = self.ticker_to_id.get(&upper) {
            return id;
        }
        let id = SymbolId(self.next_id);
        self.next_id += 1;
        self.ticker_to_id.insert(upper.clone(), id);
        self.id_to_ticker.insert(id, upper);
        self.save().ok(); // Best-effort persist
        id
    }

    /// Look up a ticker by ID.
    pub fn ticker(&self, id: SymbolId) -> Option<&str> {
        self.id_to_ticker.get(&id).map(|s| s.as_str())
    }

    /// Look up an ID by ticker.
    pub fn id(&self, ticker: &str) -> Option<SymbolId> {
        self.ticker_to_id.get(&ticker.to_uppercase()).copied()
    }

    /// List all registered symbols.
    pub fn all(&self) -> Vec<(SymbolId, String)> {
        self.id_to_ticker.iter()
            .map(|(&id, ticker)| (id, ticker.clone()))
            .collect()
    }

    fn save(&self) -> Result<()> {
        let entries: Vec<(&str, u32)> = self.id_to_ticker.iter()
            .map(|(id, ticker)| (ticker.as_str(), id.0))
            .collect();
        let json = serde_json::to_string_pretty(&entries)?;
        std::fs::write(&self.persist_path, json)?;
        Ok(())
    }
}
```

### 8.3 Discovery from Data Directory

On startup, the registry scans `data/candles/` for symbol directories and
ensures they all have entries:

```rust
impl SymbolRegistry {
    pub fn discover(&mut self, data_dir: &Path) -> Result<Vec<SymbolId>> {
        let candles_dir = data_dir.join("candles");
        if !candles_dir.exists() {
            return Ok(vec![]);
        }

        let mut discovered = Vec::new();
        for entry in std::fs::read_dir(&candles_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let ticker = entry.file_name().to_string_lossy().to_uppercase();
                if is_valid_ticker(&ticker) {
                    let id = self.get_or_insert(&ticker);
                    discovered.push(id);
                }
            }
        }
        Ok(discovered)
    }
}

fn is_valid_ticker(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 10
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}
```

### 8.4 Persistence Format

`data/symbols.json`:

```json
[
    ["AAPL", 0],
    ["SPY", 1],
    ["MSFT", 2],
    ["BTCUSD", 3]
]
```

JSON is used for simplicity and human readability. The file is tiny (a few KB
even with thousands of symbols) and is only read/written on startup and when
new symbols are imported. Performance is irrelevant here.

---

## 9. Directory Layout

### 9.1 Structure

```
data/                               # Root data directory (configurable)
├── symbols.json                    # Symbol registry (Section 8)
├── config.toml                     # User configuration
│
├── candles/                        # Binary candle files
│   ├── AAPL/
│   │   ├── AAPL_1m.midas
│   │   ├── AAPL_5m.midas
│   │   ├── AAPL_15m.midas
│   │   ├── AAPL_30m.midas
│   │   ├── AAPL_1H.midas
│   │   ├── AAPL_4H.midas
│   │   ├── AAPL_1D.midas
│   │   ├── AAPL_1W.midas
│   │   └── AAPL_1M.midas
│   │
│   ├── SPY/
│   │   ├── SPY_1D.midas
│   │   ├── SPY_1W.midas
│   │   └── SPY_1M.midas
│   │
│   └── BTCUSD/
│       └── ...
│
├── import/                         # Staging area for CSV imports
│   ├── AAPL_daily.csv              # User drops CSV files here
│   └── SPY_1min_2024.csv
│
└── logs/                           # Application logs
    └── midas.log
```

### 9.2 Design Decisions

**One file per symbol per timeframe** (not one file per symbol):

- Simpler mmap management: each file has a single record stride, single header.
- Independent append: writing new 1-minute candles does not touch the daily file.
- Selective loading: opening a daily chart only maps one small file, not the
  entire 50 MB 1-minute archive.
- Clean deletion: to remove all intraday data for a symbol, delete specific files.

**Files named `{SYMBOL}_{TIMEFRAME}.midas`**, placed in `candles/{SYMBOL}/`:

- The symbol directory groups all timeframes for a symbol.
- The filename is redundant with the directory (both contain the symbol ticker)
  but this is intentional: it makes files self-identifying when moved or shared.

**The `import/` directory**:

- A convenience for users. They can drop CSV files here and use a UI button or
  CLI command to import. Files are NOT automatically watched for changes in
  Phase 2 (this is a Phase 8 polish item).

### 9.3 Data Directory Location

The default data directory is platform-specific:

```rust
pub fn default_data_dir() -> PathBuf {
    // Windows: %APPDATA%/Midas/data
    // macOS:   ~/Library/Application Support/Midas/data
    // Linux:   ~/.local/share/midas/data
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Midas")
        .join("data")
}
```

Overridable via `MIDAS_DATA_DIR` environment variable or `config.toml`.

---

## 10. Future-Proofing for Real-Time

This section documents design decisions made in Phase 2 that specifically
prepare for Phase 7 (real-time streaming). Nothing here is implemented in
Phase 2 -- this is architecture documentation for future reference.

### 10.1 Append-Friendly Binary Format

The `.midas` binary format is designed for efficient append:

- **Header at fixed offset 0**: The `candle_count`, `end_ts`, and `write_seq`
  fields are updated in-place after each append. No file restructuring needed.
- **Sequential body layout**: New records are appended at offset
  `128 + candle_count * 32`. No insertion, no rebalancing.
- **Pre-allocated capacity**: The writer allocates extra space beyond the
  current `candle_count` (typically 2x growth). Most appends require zero
  file growth.
- **write_seq monotonic counter**: Readers poll this to detect new data
  without re-reading the entire header.

### 10.2 Forming Candle Slot

The `FORMING` flag (bit 3 of `flags`) indicates that the last record in the
file is a forming (incomplete) candle. Real-time writers:

1. Set `flags |= FORMING`.
2. Update the last record in-place as new ticks arrive (overwrite open/high/
   low/close/volume).
3. When the candle period closes, clear `flags &= !FORMING`, append the next
   forming candle.

The CandleBuffer's `update_last()` method (Section 3.5) supports this pattern
in memory.

### 10.3 Triple-Buffer Boundary

The real-time data flow has three zones:

```
Zone 1: Disk (mmap'd .midas files)
  - Historical candles. Append-only.
  - Written by the feed thread after a candle closes.

Zone 2: In-memory CandleBuffer (owned by DataManager)
  - Full series including the forming candle.
  - Updated by the feed thread via message passing.
  - Read by the render thread via Arc<CandleBuffer>.

Zone 3: GPU buffer (owned by ChartRenderer)
  - The CandleInstance array uploaded to the GPU.
  - Rebuilt from Zone 2 data on each dirty frame.
```

The **triple-buffer** sits between Zone 2's writer (feed thread) and Zone 2's
reader (render thread). It allows the feed thread to write continuously without
blocking the render thread.

```
Feed Thread                          Render Thread
  |                                    |
  | writes forming candle updates      | reads CandleBuffer
  | into "write" CandleBuffer          | from "read" CandleBuffer
  |                                    |
  |--- atomic swap (write -> ready) ---|
  |                                    |--- atomic swap (ready -> read) ---
```

### 10.4 Interfaces That Phase 7 Will Need

The following interfaces must exist in Phase 2's code (even if their real-time
implementations are stubbed out):

```rust
// In midas-data/src/lib.rs

/// Trait for anything that can provide candle data.
/// Phase 2: only FileDataSource implements this.
/// Phase 7: adds WebSocketDataSource.
pub trait DataSource: Send + Sync {
    /// Get the full historical buffer.
    fn history(&self, symbol: SymbolId, tf: Timeframe) -> Option<Arc<CandleBuffer>>;

    /// Subscribe to updates. Returns a receiver for candle events.
    /// Phase 2: returns a receiver that never sends (no real-time source).
    fn subscribe(&self, symbol: SymbolId, tf: Timeframe)
        -> crossbeam::channel::Receiver<CandleEvent>;
}

pub enum CandleEvent {
    /// A new closed candle has been appended.
    CandleClosed {
        timestamp: i64,
        open: f32, high: f32, low: f32, close: f32, volume: u32,
    },
    /// The forming (last) candle has been updated.
    FormingUpdated {
        timestamp: i64,
        open: f32, high: f32, low: f32, close: f32, volume: u32,
    },
}
```

### 10.5 Constraints for Phase 7

Decisions made in Phase 2 that Phase 7 must respect:

1. **Timestamps are UTC epoch milliseconds everywhere.** Real-time feeds must
   convert to this format before entering the data layer.

2. **CandleBuffer timestamps must be monotonically increasing.** Real-time
   appends must never insert a candle before the last one. Out-of-order ticks
   are handled by the TickAggregator, not by the buffer.

3. **The render thread never blocks on data access.** If data is not yet
   available (still loading, WebSocket not yet connected), the chart displays
   a loading state. The DataManager's `get()` returns `Option`, not `Result`
   that blocks.

4. **The binary file format is append-only.** Real-time data never modifies
   historical candles (except the forming candle slot). Split adjustments and
   corrections are handled by a separate "rebuild" process.

5. **The DataManager's LRU cache uses `Arc<CandleBuffer>`.** When real-time
   updates arrive, the feed thread creates a new `CandleBuffer` (via clone +
   push or update_last), wraps it in a new `Arc`, and swaps it into the cache.
   The render thread's existing `Arc` reference remains valid until it drops
   it at the end of the frame. This is effectively a copy-on-write scheme
   with no locking on the hot path.

### 10.6 What Phase 2 Explicitly Does NOT Do

- No WebSocket connections.
- No TickAggregator implementation.
- No triple-buffer crate dependency.
- No forming candle UI indicator.
- No real-time data provider trait implementations.
- No background polling of `write_seq` for file changes.

These are all Phase 7 scope. Phase 2 provides the data structures, file
format, and interfaces that Phase 7 will build on.

---

## Appendix A: Performance Budgets

| Operation                          | Target      | Notes                          |
|------------------------------------|-------------|--------------------------------|
| Open and validate .midas file      | < 1 ms      | mmap + header read             |
| AoS-to-SoA conversion (5K records) | < 50 us     | Tight loop, auto-vectorized    |
| AoS-to-SoA conversion (100K records)| < 500 us   | Memory bandwidth limited       |
| Binary search by timestamp (100K)  | < 1 us      | 17 iterations max              |
| price_range over 5K candles        | < 10 us     | AVX2 auto-vectorized           |
| MinMax downsample 100K -> 4K       | < 200 us    | 25 candles/bucket              |
| LTTB downsample 100K -> 4K         | < 500 us    | More computation per bucket    |
| CSV import 100K rows               | < 2 s       | I/O + parse + validate + write |
| Full pipeline: file open -> GPU ready | < 5 ms   | For 5K visible candles         |

## Appendix B: Crate Dependencies for Phase 2

```toml
[dependencies]
# In midas-data/Cargo.toml
memmap2 = "0.9"
bytemuck = { version = "1", features = ["derive"] }
chrono = { version = "0.4", features = ["serde"] }
chrono-tz = "0.10"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
parking_lot = "0.12"
crossbeam = "0.8"
lru = "0.12"
crc32c = "0.6"
thiserror = "2"
tracing = "0.1"

# In midas-feed/Cargo.toml (CSV import subset)
csv = "1"
chrono = { version = "0.4", features = ["serde"] }
chrono-tz = "0.10"
thiserror = "2"
tracing = "0.1"
```

**Workspace dependency note**: `parking_lot` and `lru` should be declared in
the workspace `[workspace.dependencies]` table (in the root `Cargo.toml`) rather
than pinned independently in each crate. Both are used by `midas-data` and may
also be needed by `midas-app` (for shared layout state locks) and `midas-core`
(for future caching). Centralizing the version in workspace deps avoids version
skew and duplicate compilations:

```toml
# In workspace Cargo.toml [workspace.dependencies]
parking_lot = "0.12"
lru = "0.12"

# In midas-data/Cargo.toml
[dependencies]
parking_lot = { workspace = true }
lru = { workspace = true }
```

## Appendix C: Key Invariants

These invariants must be upheld by all code that touches candle data. Any
violation is a bug.

1. **All six SoA arrays in a CandleBuffer have the same length.**
   `timestamps.len() == opens.len() == highs.len() == lows.len() == closes.len() == volumes.len()`

2. **Timestamps are monotonically strictly increasing.**
   `for i in 1..len: timestamps[i] > timestamps[i-1]`

3. **OHLC prices satisfy: high >= max(open, close) and low <= min(open, close).**

4. **No NaN or Infinity in price fields** (except sentinel candles in dense-mode
   binary files, which are filtered out during AoS-to-SoA conversion).

5. **CandleRecord is exactly 32 bytes, 8-byte aligned.**

6. **FileHeader is exactly 128 bytes.**

7. **Body data in a .midas file begins at byte offset 128.**

8. **The binary file size is exactly `128 + candle_count * 32` bytes**
   (plus optional LOD data if `HAS_LOD` flag is set).

9. **SymbolIds are stable across sessions.** Once assigned, a SymbolId never
   changes or is reused for a different ticker.

10. **The DataManager never blocks the render thread.** All potentially slow
    operations (file I/O, mmap, conversion) happen on background threads.
