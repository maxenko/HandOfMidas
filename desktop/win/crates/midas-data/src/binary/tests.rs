use super::*;

// ── Size assertions ─────────────────────────────────────────────────

#[test]
fn midas_header_is_128_bytes() {
    assert_eq!(
        std::mem::size_of::<MidasHeader>(),
        128,
        "MidasHeader must be exactly 128 bytes"
    );
}

#[test]
fn candle_record_is_32_bytes() {
    assert_eq!(
        std::mem::size_of::<CandleRecord>(),
        32,
        "CandleRecord must be exactly 32 bytes"
    );
}

#[test]
fn candle_record_alignment_is_8() {
    assert_eq!(
        std::mem::align_of::<CandleRecord>(),
        8,
        "CandleRecord must be aligned to 8 bytes"
    );
}

// ── Helper ──────────────────────────────────────────────────────────

fn sample_buffer(n: usize) -> CandleBuffer {
    let mut buf = CandleBuffer::with_capacity(n);
    for i in 0..n {
        let ts = (i as i64 + 1) * 60_000; // 1-minute candles
        let price = 100.0 + i as f32;
        buf.push(
            ts,
            price,
            price + 5.0,
            price - 5.0,
            price + 1.0,
            (i as u32 + 1) * 100,
        );
    }
    buf
}

// ── Write + Read roundtrip ──────────────────────────────────────────

#[test]
fn write_read_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.midas");

    let original = sample_buffer(50);
    write_midas_file(&path, 1, 60, "AAPL", &original).unwrap();
    let loaded = read_midas_file(&path).unwrap();

    assert_eq!(loaded.len(), original.len());
    assert_eq!(loaded.timestamps, original.timestamps);
    assert_eq!(loaded.opens, original.opens);
    assert_eq!(loaded.highs, original.highs);
    assert_eq!(loaded.lows, original.lows);
    assert_eq!(loaded.closes, original.closes);
    assert_eq!(loaded.volumes, original.volumes);
}

#[test]
fn write_read_empty_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.midas");

    let original = CandleBuffer::new();
    write_midas_file(&path, 42, 300, "EMPTY", &original).unwrap();
    let loaded = read_midas_file(&path).unwrap();

    assert_eq!(loaded.len(), 0);
    assert!(loaded.is_empty());
}

#[test]
fn header_fields_match() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("header.midas");

    let buf = sample_buffer(10);
    write_midas_file(&path, 7, 300, "MSFT", &buf).unwrap();

    let mmap = MmapCandleFile::open(&path).unwrap();
    let h = mmap.header();

    assert_eq!(h.magic, MIDAS_MAGIC);
    assert_eq!(h.version, MIDAS_VERSION);
    assert_eq!(h.symbol_id, 7);
    assert_eq!(h.timeframe_secs, 300);
    assert_eq!(h.candle_count, 10);
    assert_eq!(h.record_size, RECORD_SIZE as u32);
    assert_eq!(h.start_ts, buf.timestamps[0]);
    assert_eq!(h.end_ts, buf.timestamps[9]);
    assert_eq!(&h.symbol_ascii[..4], b"MSFT");
    assert_eq!(h.symbol_ascii[4], 0); // null-terminated
}

#[test]
fn records_match_via_mmap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mmap.midas");

    let buf = sample_buffer(25);
    write_midas_file(&path, 1, 60, "TEST", &buf).unwrap();

    let mmap = MmapCandleFile::open(&path).unwrap();
    assert_eq!(mmap.record_count(), 25);

    for i in 0..25 {
        let r = mmap.record(i).unwrap();
        assert_eq!(r.timestamp, buf.timestamps[i]);
        assert_eq!(r.open, buf.opens[i]);
        assert_eq!(r.high, buf.highs[i]);
        assert_eq!(r.low, buf.lows[i]);
        assert_eq!(r.close, buf.closes[i]);
        assert_eq!(r.volume, buf.volumes[i]);
    }
}

#[test]
fn record_out_of_bounds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oob.midas");

    let buf = sample_buffer(5);
    write_midas_file(&path, 1, 60, "X", &buf).unwrap();

    let mmap = MmapCandleFile::open(&path).unwrap();
    assert!(mmap.record(5).is_err());
    assert!(mmap.record(100).is_err());
}

// ── to_candle_buffer ────────────────────────────────────────────────

#[test]
fn mmap_to_candle_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tobuf.midas");

    let original = sample_buffer(30);
    write_midas_file(&path, 1, 60, "BUF", &original).unwrap();

    let mmap = MmapCandleFile::open(&path).unwrap();
    let loaded = mmap.to_candle_buffer();

    assert_eq!(loaded.len(), original.len());
    assert_eq!(loaded.timestamps, original.timestamps);
    assert_eq!(loaded.opens, original.opens);
    assert_eq!(loaded.highs, original.highs);
    assert_eq!(loaded.lows, original.lows);
    assert_eq!(loaded.closes, original.closes);
    assert_eq!(loaded.volumes, original.volumes);
}

// ── slice_by_time ───────────────────────────────────────────────────

#[test]
fn slice_by_time_subset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slice.midas");

    let buf = sample_buffer(100);
    write_midas_file(&path, 1, 60, "SLICE", &buf).unwrap();

    let mmap = MmapCandleFile::open(&path).unwrap();

    // Request candles with timestamps in [10*60000, 20*60000].
    // That corresponds to indices 9..20 (ts = 10*60000 .. 20*60000).
    let sliced = mmap.slice_by_time(10 * 60_000, 20 * 60_000);
    assert_eq!(sliced.len(), 11); // indices 9 through 19 inclusive
    assert_eq!(sliced.timestamps[0], 10 * 60_000);
    assert_eq!(*sliced.timestamps.last().unwrap(), 20 * 60_000);
}

#[test]
fn slice_by_time_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slice_all.midas");

    let buf = sample_buffer(20);
    write_midas_file(&path, 1, 60, "ALL", &buf).unwrap();

    let mmap = MmapCandleFile::open(&path).unwrap();
    let sliced = mmap.slice_by_time(0, i64::MAX);
    assert_eq!(sliced.len(), 20);
}

#[test]
fn slice_by_time_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slice_none.midas");

    let buf = sample_buffer(10);
    write_midas_file(&path, 1, 60, "NONE", &buf).unwrap();

    let mmap = MmapCandleFile::open(&path).unwrap();
    // All timestamps are in [60000, 600000]. Request a range entirely before.
    let sliced = mmap.slice_by_time(0, 59_999);
    assert_eq!(sliced.len(), 0);
}

#[test]
fn slice_by_time_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty_slice.midas");

    let buf = CandleBuffer::new();
    write_midas_file(&path, 1, 60, "E", &buf).unwrap();

    let mmap = MmapCandleFile::open(&path).unwrap();
    let sliced = mmap.slice_by_time(0, i64::MAX);
    assert_eq!(sliced.len(), 0);
}

// ── Corruption detection ────────────────────────────────────────────

#[test]
fn corrupt_magic_number_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_magic.midas");

    let buf = sample_buffer(5);
    write_midas_file(&path, 1, 60, "BAD", &buf).unwrap();

    // Overwrite the first 4 bytes (magic) with garbage.
    let mut data = std::fs::read(&path).unwrap();
    data[0] = 0xFF;
    data[1] = 0xFF;
    data[2] = 0xFF;
    data[3] = 0xFF;
    std::fs::write(&path, &data).unwrap();

    let err = read_midas_file(&path).unwrap_err();
    assert!(
        matches!(err, BinaryError::InvalidMagic { .. }),
        "expected InvalidMagic, got: {err}"
    );
}

#[test]
fn corrupt_version_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_version.midas");

    let buf = sample_buffer(5);
    write_midas_file(&path, 1, 60, "VER", &buf).unwrap();

    // Set version to 99 (bytes 4..6, little-endian).
    let mut data = std::fs::read(&path).unwrap();
    data[4] = 99;
    data[5] = 0;
    // Also fix checksum so we test the version check, not checksum.
    // Actually, changing the version changes bytes in [0..64), so the
    // checksum will also mismatch. The validation checks magic first,
    // then version, then checksum. Since magic is fine but version=99
    // triggers before checksum is checked, this should produce
    // UnsupportedVersion.
    std::fs::write(&path, &data).unwrap();

    let err = read_midas_file(&path).unwrap_err();
    assert!(
        matches!(err, BinaryError::UnsupportedVersion { .. }),
        "expected UnsupportedVersion, got: {err}"
    );
}

#[test]
fn corrupt_checksum_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_checksum.midas");

    let buf = sample_buffer(5);
    write_midas_file(&path, 1, 60, "CRC", &buf).unwrap();

    // Corrupt a reserved byte in the header (within [0..64)) to trigger
    // checksum mismatch without changing magic or version.
    // The write_seq field is at offset 0x38..0x40 (bytes 56..64).
    let mut data = std::fs::read(&path).unwrap();
    data[56] ^= 0xFF; // flip bits in write_seq
    std::fs::write(&path, &data).unwrap();

    let err = read_midas_file(&path).unwrap_err();
    assert!(
        matches!(err, BinaryError::ChecksumMismatch { .. }),
        "expected ChecksumMismatch, got: {err}"
    );
}

#[test]
fn truncated_file_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("truncated.midas");

    let buf = sample_buffer(10);
    write_midas_file(&path, 1, 60, "TRUNC", &buf).unwrap();

    // Truncate the file to just the header + 5 records (should be 10).
    let data = std::fs::read(&path).unwrap();
    let truncated = &data[..HEADER_SIZE + 5 * RECORD_SIZE];
    std::fs::write(&path, truncated).unwrap();

    let err = read_midas_file(&path).unwrap_err();
    assert!(
        matches!(err, BinaryError::TruncatedBody { .. }),
        "expected TruncatedBody, got: {err}"
    );
}

#[test]
fn file_too_small_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tiny.midas");

    std::fs::write(&path, [0u8; 64]).unwrap();
    let err = read_midas_file(&path).unwrap_err();
    assert!(
        matches!(err, BinaryError::FileTooSmall { .. }),
        "expected FileTooSmall, got: {err}"
    );
}

// ── CRC32C sanity ───────────────────────────────────────────────────

#[test]
fn crc32c_empty() {
    assert_eq!(crc32c(&[]), 0x0000_0000);
}

#[test]
fn crc32c_known_value() {
    // CRC32C of "123456789" is 0xE3069283.
    let data = b"123456789";
    assert_eq!(crc32c(data), 0xE306_9283);
}

// ── Large file roundtrip ────────────────────────────────────────────

#[test]
fn roundtrip_1000_candles() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.midas");

    let original = sample_buffer(1000);
    write_midas_file(&path, 42, 60, "LARGE", &original).unwrap();
    let loaded = read_midas_file(&path).unwrap();

    assert_eq!(loaded.len(), 1000);
    assert_eq!(loaded.timestamps, original.timestamps);
    assert_eq!(loaded.opens, original.opens);
    assert_eq!(loaded.highs, original.highs);
    assert_eq!(loaded.lows, original.lows);
    assert_eq!(loaded.closes, original.closes);
    assert_eq!(loaded.volumes, original.volumes);
}

// ── Long symbol name ────────────────────────────────────────────────

#[test]
fn long_symbol_name_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("long_sym.midas");

    let long_name = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"; // 36 chars
    let buf = sample_buffer(1);
    write_midas_file(&path, 1, 60, long_name, &buf).unwrap();

    let mmap = MmapCandleFile::open(&path).unwrap();
    let h = mmap.header();
    // Should be truncated to 31 bytes + null.
    assert_eq!(&h.symbol_ascii[..31], &long_name.as_bytes()[..31]);
    assert_eq!(h.symbol_ascii[31], 0);
}
