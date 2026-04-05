//! Binary `.midas` file format: header, records, read/write, and memory-mapped access.
//!
//! The format is designed for zero-deserialization reads via `bytemuck` casts on
//! `#[repr(C)]` structs. Each candle record is 32 bytes; the header is 128 bytes.
//! See the data-architecture plan (Section 1) for full specification.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use memmap2::Mmap;

use crate::candle::CandleBuffer;

// ─── Constants ──────────────────────────────────────────────────────────

/// Magic number: ASCII `MIDA` = `0x4D494441` (little-endian on disk).
pub const MIDAS_MAGIC: u32 = 0x4D49_4441;

/// Current format version.
pub const MIDAS_VERSION: u16 = 1;

/// Size of the file header in bytes.
pub const HEADER_SIZE: usize = 128;

/// Size of one candle record in bytes.
pub const RECORD_SIZE: usize = 32;

// ─── Error type ─────────────────────────────────────────────────────────

/// Errors produced by binary file I/O operations.
#[derive(Debug, thiserror::Error)]
pub enum BinaryError {
    /// Underlying I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// File is smaller than the required 128-byte header.
    #[error("file too small: expected at least {HEADER_SIZE} bytes, got {size}")]
    FileTooSmall { size: usize },

    /// Magic number does not match `0x4D494441`.
    #[error("invalid magic number: expected 0x{MIDAS_MAGIC:08X}, got 0x{got:08X}")]
    InvalidMagic { got: u32 },

    /// Version is newer than what this reader supports.
    #[error("unsupported version: file has {got}, max supported is {MIDAS_VERSION}")]
    UnsupportedVersion { got: u16 },

    /// Header checksum does not match the computed CRC32C.
    #[error("header checksum mismatch: stored 0x{stored:08X}, computed 0x{computed:08X}")]
    ChecksumMismatch { stored: u32, computed: u32 },

    /// The body portion of the file is truncated.
    #[error("truncated body: expected {expected} bytes, file has {actual}")]
    TruncatedBody { expected: usize, actual: usize },

    /// Record index is out of bounds.
    #[error("record index {index} out of bounds (count = {count})")]
    IndexOutOfBounds { index: usize, count: usize },
}

// ─── Header ─────────────────────────────────────────────────────────────

/// 128-byte file header for `.midas` binary candle files.
///
/// All multi-byte fields are little-endian (native on x86). The struct is
/// `#[repr(C)]` and derives `bytemuck::Pod`, enabling zero-copy reads from
/// memory-mapped files.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MidasHeader {
    /// Magic number. Must be [`MIDAS_MAGIC`] (`0x4D494441`).
    pub magic: u32,
    /// Format version. Currently [`MIDAS_VERSION`] (1).
    pub version: u16,
    /// Bit flags (DENSE, ADJUSTED, HAS_LOD, FORMING).
    pub flags: u16,
    /// Internal symbol identifier.
    pub symbol_id: u32,
    /// Candle period in seconds.
    pub timeframe_secs: u32,
    /// Timestamp of the first candle (epoch milliseconds, UTC).
    pub start_ts: i64,
    /// Timestamp of the last candle (epoch milliseconds, UTC).
    pub end_ts: i64,
    /// Number of candle records in the body section.
    pub candle_count: u64,
    /// Size of one body record in bytes (always 32 for v1).
    pub record_size: u32,
    /// Number of pre-computed LOD levels (0 = none).
    pub lod_levels: u32,
    /// Byte offset to the start of the LOD section (0 = none).
    pub lod_offset: u64,
    /// Monotonic write sequence counter.
    pub write_seq: u64,
    /// CRC32C of header bytes `[0..0x40)` (i.e. the first 64 bytes).
    pub checksum: u32,
    /// Reserved for future use. Must be zero.
    pub _reserved: [u8; 28],
    /// Symbol ticker as null-padded ASCII (debug aid).
    pub symbol_ascii: [u8; 32],
}

// ─── Candle Record ──────────────────────────────────────────────────────

/// 32-byte on-disk candle record (AoS layout).
///
/// Aligned to 8 bytes so records never straddle cache-line boundaries.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CandleRecord {
    /// Candle open time (epoch milliseconds, UTC).
    pub timestamp: i64,
    /// Opening price.
    pub open: f32,
    /// Highest price.
    pub high: f32,
    /// Lowest price.
    pub low: f32,
    /// Closing price.
    pub close: f32,
    /// Total volume (capped at `u32::MAX` for equities).
    pub volume: u32,
    /// Padding. Reserved for future fields (tick_count, VWAP, flags).
    pub _padding: u32,
}

// ─── CRC32C ─────────────────────────────────────────────────────────────

/// Compute a CRC32C (Castagnoli) checksum over `data`.
///
/// Uses the polynomial 0x82F63B78 (reflected). This is a simple
/// table-less implementation suitable for the 64-byte header prefix.
fn crc32c(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0x82F6_3B78;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// ─── Validation ─────────────────────────────────────────────────────────

/// Validate a header's magic, version, and checksum.
fn validate_header(header: &MidasHeader) -> Result<(), BinaryError> {
    if header.magic != MIDAS_MAGIC {
        return Err(BinaryError::InvalidMagic { got: header.magic });
    }
    if header.version > MIDAS_VERSION {
        return Err(BinaryError::UnsupportedVersion {
            got: header.version,
        });
    }

    // Checksum covers bytes [0..64) of the header.
    let header_bytes = bytemuck::bytes_of(header);
    let computed = crc32c(&header_bytes[..0x40]);
    if header.checksum != computed {
        return Err(BinaryError::ChecksumMismatch {
            stored: header.checksum,
            computed,
        });
    }

    Ok(())
}

/// Build a [`MidasHeader`] from the given parameters and compute its checksum.
fn build_header(
    symbol_id: u32,
    timeframe_secs: u32,
    symbol: &str,
    candles: &CandleBuffer,
) -> MidasHeader {
    let count = candles.len();

    let start_ts = if count > 0 { candles.timestamps[0] } else { 0 };
    let end_ts = if count > 0 {
        candles.timestamps[count - 1]
    } else {
        0
    };

    // Copy symbol name into the fixed-size ASCII field.
    let mut symbol_ascii = [0u8; 32];
    let bytes = symbol.as_bytes();
    let copy_len = bytes.len().min(31); // leave room for null terminator
    symbol_ascii[..copy_len].copy_from_slice(&bytes[..copy_len]);

    let mut header = MidasHeader {
        magic: MIDAS_MAGIC,
        version: MIDAS_VERSION,
        flags: 0,
        symbol_id,
        timeframe_secs,
        start_ts,
        end_ts,
        candle_count: count as u64,
        record_size: RECORD_SIZE as u32,
        lod_levels: 0,
        lod_offset: 0,
        write_seq: 1,
        checksum: 0, // placeholder -- computed below
        _reserved: [0u8; 28],
        symbol_ascii,
    };

    // Compute checksum over bytes [0..64).
    let header_bytes = bytemuck::bytes_of(&header);
    header.checksum = crc32c(&header_bytes[..0x40]);

    header
}

// ─── Write ──────────────────────────────────────────────────────────────

/// Write a complete `.midas` file from a [`CandleBuffer`].
///
/// Creates (or overwrites) the file at `path`. The file will contain a
/// 128-byte header followed by `candles.len()` 32-byte records.
pub fn write_midas_file(
    path: &Path,
    symbol_id: u32,
    timeframe_secs: u32,
    symbol: &str,
    candles: &CandleBuffer,
) -> Result<(), BinaryError> {
    let header = build_header(symbol_id, timeframe_secs, symbol, candles);

    let mut file = File::create(path)?;

    // Write header.
    file.write_all(bytemuck::bytes_of(&header))?;

    // Write records.
    let count = candles.len();
    for i in 0..count {
        let record = CandleRecord {
            timestamp: candles.timestamps[i],
            open: candles.opens[i],
            high: candles.highs[i],
            low: candles.lows[i],
            close: candles.closes[i],
            volume: candles.volumes[i],
            _padding: 0,
        };
        file.write_all(bytemuck::bytes_of(&record))?;
    }

    file.sync_all()?;
    Ok(())
}

// ─── Read (full file into memory) ───────────────────────────────────────

/// Read a `.midas` file fully into a [`CandleBuffer`].
///
/// Validates the header (magic, version, checksum) and verifies the body
/// is not truncated before converting AoS records to SoA layout.
pub fn read_midas_file(path: &Path) -> Result<CandleBuffer, BinaryError> {
    let data = std::fs::read(path)?;

    if data.len() < HEADER_SIZE {
        return Err(BinaryError::FileTooSmall { size: data.len() });
    }

    let header: &MidasHeader = bytemuck::from_bytes(&data[..HEADER_SIZE]);
    validate_header(header)?;

    let count = header.candle_count as usize;
    let expected_body = count * header.record_size as usize;
    let actual_body = data.len() - HEADER_SIZE;
    if actual_body < expected_body {
        return Err(BinaryError::TruncatedBody {
            expected: expected_body,
            actual: actual_body,
        });
    }

    let body = &data[HEADER_SIZE..HEADER_SIZE + expected_body];
    let records: &[CandleRecord] = bytemuck::cast_slice(body);

    let mut buf = CandleBuffer::with_capacity(count);
    for r in records {
        buf.push(r.timestamp, r.open, r.high, r.low, r.close, r.volume);
    }

    Ok(buf)
}

// ─── Memory-Mapped Access ───────────────────────────────────────────────

/// Memory-mapped read-only accessor for `.midas` files.
///
/// Keeps the file handle and mmap alive for the lifetime of the struct.
/// Provides zero-copy access to individual records and efficient
/// conversion to [`CandleBuffer`] for rendering.
pub struct MmapCandleFile {
    /// Keep the file handle alive so the mmap remains valid.
    _file: File,
    /// Immutable memory-mapped view of the entire file.
    mmap: Mmap,
    /// Validated copy of the file header.
    header: MidasHeader,
    /// Number of candle records (cached from header).
    record_count: usize,
}

impl MmapCandleFile {
    /// Open a `.midas` file with memory-mapped read-only access.
    ///
    /// Validates the header on open. Returns an error if the file is
    /// corrupt, truncated, or uses an unsupported format version.
    pub fn open(path: &Path) -> Result<Self, BinaryError> {
        let file = File::open(path)?;

        // SAFETY: The file is opened read-only and we hold the File handle
        // for the lifetime of the Mmap. The mmap is immutable. We validate
        // all data before dereferencing through bytemuck casts.
        let mmap = unsafe { Mmap::map(&file)? };

        if mmap.len() < HEADER_SIZE {
            return Err(BinaryError::FileTooSmall { size: mmap.len() });
        }

        let header: MidasHeader = *bytemuck::from_bytes(&mmap[..HEADER_SIZE]);
        validate_header(&header)?;

        let count = header.candle_count as usize;
        let expected_body = count * header.record_size as usize;
        let actual_body = mmap.len() - HEADER_SIZE;
        if actual_body < expected_body {
            return Err(BinaryError::TruncatedBody {
                expected: expected_body,
                actual: actual_body,
            });
        }

        Ok(Self {
            _file: file,
            mmap,
            header,
            record_count: count,
        })
    }

    /// Return a reference to the validated file header.
    #[inline]
    pub fn header(&self) -> &MidasHeader {
        &self.header
    }

    /// Number of candle records in the file.
    #[inline]
    pub fn record_count(&self) -> usize {
        self.record_count
    }

    /// Access a single record by index (zero-copy from mmap).
    ///
    /// Returns an error if `idx` is out of bounds.
    pub fn record(&self, idx: usize) -> Result<&CandleRecord, BinaryError> {
        if idx >= self.record_count {
            return Err(BinaryError::IndexOutOfBounds {
                index: idx,
                count: self.record_count,
            });
        }
        let offset = HEADER_SIZE + idx * RECORD_SIZE;
        let record: &CandleRecord = bytemuck::from_bytes(&self.mmap[offset..offset + RECORD_SIZE]);
        Ok(record)
    }

    /// Convert all records to a [`CandleBuffer`] (AoS to SoA).
    pub fn to_candle_buffer(&self) -> CandleBuffer {
        let mut buf = CandleBuffer::with_capacity(self.record_count);
        let body = &self.mmap[HEADER_SIZE..HEADER_SIZE + self.record_count * RECORD_SIZE];
        let records: &[CandleRecord] = bytemuck::cast_slice(body);
        for r in records {
            buf.push(r.timestamp, r.open, r.high, r.low, r.close, r.volume);
        }
        buf
    }

    /// Extract candles within a time range `[start_ts, end_ts]` into a [`CandleBuffer`].
    ///
    /// Uses binary search on the AoS records to find the range bounds.
    pub fn slice_by_time(&self, start_ts: i64, end_ts: i64) -> CandleBuffer {
        if self.record_count == 0 {
            return CandleBuffer::new();
        }

        let body = &self.mmap[HEADER_SIZE..HEADER_SIZE + self.record_count * RECORD_SIZE];
        let records: &[CandleRecord] = bytemuck::cast_slice(body);

        // Binary search: first record with timestamp >= start_ts
        let lo = records.partition_point(|r| r.timestamp < start_ts);
        // Binary search: first record with timestamp > end_ts
        let hi = records.partition_point(|r| r.timestamp <= end_ts);

        let mut buf = CandleBuffer::with_capacity(hi.saturating_sub(lo));
        for r in &records[lo..hi] {
            buf.push(r.timestamp, r.open, r.high, r.low, r.close, r.volume);
        }
        buf
    }
}

#[cfg(test)]
mod tests;
