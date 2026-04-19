//! `.tws.pcap` format — our own binary log of wire bytes.
//!
//! *Not* libpcap. Append-only, cheap to produce in `tokio::io::copy_bidirectional`
//! tee paths, cheap to consume.
//!
//! # Layout
//!
//! ```text
//! [TwsPcapHeader]           24 bytes, little-endian
//!   magic        : [u8; 4]  = b"TWSC"
//!   version      : u16      (PCAP_VERSION, currently 1)
//!   server_version_neg: u16 (negotiated TWS server version)
//!   start_ts_nanos   : i128 (wall clock at capture start, ns since UNIX epoch)
//!
//! [TwsPcapRecord × N]       14 + len bytes, little-endian
//!   ts_nanos_since_start: u64  (monotonic offset from start_ts_nanos)
//!   direction          : u8   (0 = client→sim, 1 = sim→client)
//!   flags              : u8   (reserved, 0)
//!   len                : u32  (payload byte count, excluding header)
//!   payload            : [u8; len]
//! ```
//!
//! # Compression
//!
//! Files may be zstd-compressed at rest. The reader sniffs the first 4 bytes:
//!
//! - `b"TWSC"` → raw pcap
//! - `28 B5 2F FD` (zstd frame magic) → decode through `zstd::Decoder`
//!
//! Writers choose between [`TwsPcapWriter::create`] (raw) and
//! [`TwsPcapWriter::create_zstd`] (zstd-compressed). The in-memory API is
//! identical, so callers need only pick the compression strategy once.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// `b"TWSC"` — magic prefix of an uncompressed `.tws.pcap` file.
pub const PCAP_MAGIC: [u8; 4] = *b"TWSC";

/// Current pcap format version. Bumped on any on-disk layout change.
pub const PCAP_VERSION: u16 = 1;

/// Zstd frame magic number (little-endian `0xFD2FB528`).
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// Byte length of the on-disk [`TwsPcapHeader`].
pub const HEADER_BYTES: usize = 4 + 2 + 2 + 16;

/// Byte length of the on-disk [`TwsPcapRecord`] header (excluding its payload).
pub const RECORD_HEADER_BYTES: usize = 8 + 1 + 1 + 4;

/// File-level metadata at the start of every `.tws.pcap` stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TwsPcapHeader {
    pub magic: [u8; 4],
    pub version: u16,
    pub server_version_neg: u16,
    pub start_ts_nanos: i128,
}

impl TwsPcapHeader {
    /// Construct a fresh header with the current version and supplied negotiated server version.
    pub fn new(server_version_neg: u16, start_ts_nanos: i128) -> Self {
        Self {
            magic: PCAP_MAGIC,
            version: PCAP_VERSION,
            server_version_neg,
            start_ts_nanos,
        }
    }

    /// Serialise as 24 little-endian bytes.
    pub fn encode(&self) -> [u8; HEADER_BYTES] {
        let mut out = [0u8; HEADER_BYTES];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.version.to_le_bytes());
        out[6..8].copy_from_slice(&self.server_version_neg.to_le_bytes());
        out[8..24].copy_from_slice(&self.start_ts_nanos.to_le_bytes());
        out
    }

    /// Parse from a 24-byte buffer. Returns an error on bad magic.
    pub fn decode(buf: &[u8; HEADER_BYTES]) -> io::Result<Self> {
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&buf[0..4]);
        if magic != PCAP_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bad pcap magic: {magic:?}, expected {PCAP_MAGIC:?}"),
            ));
        }
        let version = u16::from_le_bytes([buf[4], buf[5]]);
        let server_version_neg = u16::from_le_bytes([buf[6], buf[7]]);
        let mut ts_buf = [0u8; 16];
        ts_buf.copy_from_slice(&buf[8..24]);
        let start_ts_nanos = i128::from_le_bytes(ts_buf);
        Ok(Self {
            magic,
            version,
            server_version_neg,
            start_ts_nanos,
        })
    }
}

/// Direction of a captured byte chunk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Direction {
    /// Bytes flowing from the API client into the sim.
    ClientToSim = 0,
    /// Bytes flowing from the sim back out to the client.
    SimToClient = 1,
}

impl Direction {
    fn from_u8(b: u8) -> io::Result<Self> {
        match b {
            0 => Ok(Self::ClientToSim),
            1 => Ok(Self::SimToClient),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid direction byte: {other}"),
            )),
        }
    }
}

/// One captured frame (direction + timestamp + raw bytes).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TwsPcapRecord {
    pub ts_nanos_since_start: u64,
    pub direction: Direction,
    pub flags: u8,
    pub payload: Vec<u8>,
}

impl TwsPcapRecord {
    pub fn new(ts_nanos_since_start: u64, direction: Direction, payload: Vec<u8>) -> Self {
        Self {
            ts_nanos_since_start,
            direction,
            flags: 0,
            payload,
        }
    }
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Sink side of a pcap stream. Generic over any `Write` implementation —
/// concrete constructors cover the two common on-disk cases (raw + zstd).
pub struct TwsPcapWriter<W: Write> {
    inner: W,
    /// Number of records written so far. Diagnostic only.
    pub records_written: u64,
    /// Total payload bytes written. Diagnostic only.
    pub bytes_written: u64,
}

impl TwsPcapWriter<BufWriter<File>> {
    /// Create a new raw pcap at `path`. Fails if the file already exists.
    pub fn create(path: impl AsRef<Path>, header: TwsPcapHeader) -> io::Result<Self> {
        let file = File::create(path)?;
        Self::with_writer(BufWriter::new(file), header)
    }
}

/// Zstd auto-finishing writer — wraps the inner file so dropping the writer
/// flushes the final zstd frame.
pub type ZstdFile = zstd::stream::AutoFinishEncoder<'static, BufWriter<File>>;

impl TwsPcapWriter<ZstdFile> {
    /// Create a new zstd-compressed pcap at `path`. Compression level 3 is a
    /// good default for log-type workloads.
    pub fn create_zstd(path: impl AsRef<Path>, header: TwsPcapHeader) -> io::Result<Self> {
        let file = File::create(path)?;
        let encoder = zstd::stream::Encoder::new(BufWriter::new(file), 3)?.auto_finish();
        Self::with_writer(encoder, header)
    }
}

impl<W: Write> TwsPcapWriter<W> {
    /// Build a writer over an arbitrary `Write`. Immediately serialises the
    /// header into the stream.
    pub fn with_writer(mut inner: W, header: TwsPcapHeader) -> io::Result<Self> {
        inner.write_all(&header.encode())?;
        Ok(Self {
            inner,
            records_written: 0,
            bytes_written: 0,
        })
    }

    /// Append a record to the stream. `payload` may be empty (legal — logs
    /// zero-byte `read()`s for completeness during proxy idle periods).
    pub fn write_record(&mut self, record: &TwsPcapRecord) -> io::Result<()> {
        let len_u32: u32 = record.payload.len().try_into().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "record payload exceeds u32::MAX bytes",
            )
        })?;
        let mut head = [0u8; RECORD_HEADER_BYTES];
        head[0..8].copy_from_slice(&record.ts_nanos_since_start.to_le_bytes());
        head[8] = record.direction as u8;
        head[9] = record.flags;
        head[10..14].copy_from_slice(&len_u32.to_le_bytes());
        self.inner.write_all(&head)?;
        self.inner.write_all(&record.payload)?;
        self.records_written += 1;
        self.bytes_written += u64::from(len_u32);
        Ok(())
    }

    /// Flush the underlying writer. For zstd streams this does NOT finalise
    /// the trailing frame — drop the writer (or let it go out of scope) for
    /// that.
    pub fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }

    /// Return the underlying writer, forcing any buffered data to flush first.
    pub fn into_inner(mut self) -> io::Result<W> {
        self.inner.flush()?;
        Ok(self.inner)
    }
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// Source side of a pcap stream. Generic over any `Read`.
pub struct TwsPcapReader<R: Read> {
    inner: R,
    header: TwsPcapHeader,
}

impl<R: Read> std::fmt::Debug for TwsPcapReader<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TwsPcapReader")
            .field("header", &self.header)
            .finish_non_exhaustive()
    }
}

impl TwsPcapReader<BufReader<File>> {
    /// Open a pcap file by path. Sniffs the first four bytes to decide
    /// between raw and zstd layouts; callers don't need to know which they
    /// have.
    pub fn open(path: impl AsRef<Path>) -> io::Result<TwsPcapReader<Box<dyn Read>>> {
        let path: PathBuf = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let mut magic = [0u8; 4];
        file.read_exact(&mut magic)?;
        file.seek(SeekFrom::Start(0))?;
        let raw: Box<dyn Read> = if magic == ZSTD_MAGIC {
            Box::new(zstd::stream::Decoder::new(BufReader::new(file))?)
        } else if magic == PCAP_MAGIC {
            Box::new(BufReader::new(file))
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unrecognised file magic at {}: {magic:?}", path.display()),
            ));
        };
        TwsPcapReader::with_reader(raw)
    }
}

impl<R: Read> TwsPcapReader<R> {
    /// Build a reader over an arbitrary `Read`. Immediately reads and
    /// validates the header.
    pub fn with_reader(mut inner: R) -> io::Result<Self> {
        let mut buf = [0u8; HEADER_BYTES];
        inner.read_exact(&mut buf)?;
        let header = TwsPcapHeader::decode(&buf)?;
        if header.version != PCAP_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported pcap version {} (expected {})",
                    header.version, PCAP_VERSION
                ),
            ));
        }
        Ok(Self { inner, header })
    }

    /// Return the file header.
    pub fn header(&self) -> &TwsPcapHeader {
        &self.header
    }

    /// Read one record. Returns `Ok(None)` at clean EOF.
    pub fn read_record(&mut self) -> io::Result<Option<TwsPcapRecord>> {
        let mut head = [0u8; RECORD_HEADER_BYTES];
        match self.inner.read_exact(&mut head) {
            Ok(()) => {}
            // UnexpectedEof on a fresh read = clean EOF, treat as end of stream.
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        let ts_nanos_since_start = u64::from_le_bytes([
            head[0], head[1], head[2], head[3], head[4], head[5], head[6], head[7],
        ]);
        let direction = Direction::from_u8(head[8])?;
        let flags = head[9];
        let len = u32::from_le_bytes([head[10], head[11], head[12], head[13]]) as usize;
        let mut payload = vec![0u8; len];
        if len > 0 {
            self.inner.read_exact(&mut payload)?;
        }
        Ok(Some(TwsPcapRecord {
            ts_nanos_since_start,
            direction,
            flags,
            payload,
        }))
    }

    /// Drain the entire stream into a `Vec`.
    pub fn read_all(mut self) -> io::Result<Vec<TwsPcapRecord>> {
        let mut out = Vec::new();
        while let Some(r) = self.read_record()? {
            out.push(r);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::TempDir;

    fn sample_records() -> Vec<TwsPcapRecord> {
        vec![
            TwsPcapRecord::new(100, Direction::ClientToSim, b"API\0".to_vec()),
            TwsPcapRecord::new(
                250,
                Direction::SimToClient,
                b"176\x0020260418 09:30:00\0".to_vec(),
            ),
            TwsPcapRecord::new(
                300_000,
                Direction::ClientToSim,
                b"71\0\x01\0payload\0".to_vec(),
            ),
            TwsPcapRecord::new(0, Direction::SimToClient, Vec::new()),
        ]
    }

    #[test]
    fn header_roundtrip_matches_struct_layout() {
        let hdr = TwsPcapHeader::new(201, 1_700_000_000_000_000_000);
        let bytes = hdr.encode();
        assert_eq!(bytes.len(), HEADER_BYTES);
        let parsed = TwsPcapHeader::decode(&bytes).unwrap();
        assert_eq!(parsed, hdr);
    }

    #[test]
    fn header_rejects_bad_magic() {
        let mut bytes = [0u8; HEADER_BYTES];
        bytes[0..4].copy_from_slice(b"NOPE");
        let err = TwsPcapHeader::decode(&bytes).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn writer_reader_in_memory_roundtrip() {
        let header = TwsPcapHeader::new(210, 1);
        let records = sample_records();

        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = TwsPcapWriter::with_writer(&mut buf, header).unwrap();
            for r in &records {
                w.write_record(r).unwrap();
            }
            w.flush().unwrap();
        }

        let mut r = TwsPcapReader::with_reader(Cursor::new(&buf)).unwrap();
        assert_eq!(*r.header(), header);
        let mut read = Vec::new();
        while let Some(rec) = r.read_record().unwrap() {
            read.push(rec);
        }
        assert_eq!(read, records);
    }

    #[test]
    fn writer_reader_raw_file_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.tws.pcap");
        let header = TwsPcapHeader::new(221, 42);
        let records = sample_records();

        {
            let mut w = TwsPcapWriter::create(&path, header).unwrap();
            for r in &records {
                w.write_record(r).unwrap();
            }
        }

        let reader = TwsPcapReader::open(&path).unwrap();
        assert_eq!(*reader.header(), header);
        let read = reader.read_all().unwrap();
        assert_eq!(read, records);
    }

    #[test]
    fn writer_reader_zstd_file_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.tws.pcap.zst");
        let header = TwsPcapHeader::new(221, 42);
        let records = sample_records();

        {
            let mut w = TwsPcapWriter::create_zstd(&path, header).unwrap();
            for r in &records {
                w.write_record(r).unwrap();
            }
            // Drop to flush the final zstd frame.
        }

        // First four bytes on disk must be the zstd frame magic.
        let mut f = File::open(&path).unwrap();
        let mut magic = [0u8; 4];
        f.read_exact(&mut magic).unwrap();
        assert_eq!(magic, ZSTD_MAGIC);

        let reader = TwsPcapReader::open(&path).unwrap();
        assert_eq!(*reader.header(), header);
        let read = reader.read_all().unwrap();
        assert_eq!(read, records);
    }

    #[test]
    fn reader_eof_returns_none_cleanly() {
        let header = TwsPcapHeader::new(200, 0);
        let mut buf: Vec<u8> = Vec::new();
        {
            let _ = TwsPcapWriter::with_writer(&mut buf, header).unwrap();
        }
        let mut r = TwsPcapReader::with_reader(Cursor::new(&buf)).unwrap();
        assert!(r.read_record().unwrap().is_none());
    }

    #[test]
    fn reader_rejects_unknown_direction_byte() {
        let header = TwsPcapHeader::new(200, 0);
        let mut buf = header.encode().to_vec();
        // Craft a manual record with direction=9.
        buf.extend_from_slice(&0u64.to_le_bytes());
        buf.push(9);
        buf.push(0);
        buf.extend_from_slice(&0u32.to_le_bytes());
        let mut r = TwsPcapReader::with_reader(Cursor::new(&buf)).unwrap();
        let err = r.read_record().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn writer_rejects_mismatched_pcap_version() {
        // Hand-craft a header claiming version 999 and feed it to a reader.
        let mut buf = [0u8; HEADER_BYTES];
        buf[0..4].copy_from_slice(&PCAP_MAGIC);
        buf[4..6].copy_from_slice(&999u16.to_le_bytes());
        let err = TwsPcapReader::with_reader(Cursor::new(&buf[..])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
