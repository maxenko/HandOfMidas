use super::*;
use crate::schema::run_migrations;
use midas_core::Timeframe;

fn test_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    run_migrations(&conn).unwrap();
    conn
}

fn sample_key() -> DataKey {
    DataKey {
        symbol: "AAPL".into(),
        timeframe: Timeframe::D1,
    }
}

fn sample_buffer(n: usize) -> CandleBuffer {
    let mut buf = CandleBuffer::with_capacity(n);
    for i in 0..n {
        let ts = 1_700_000_000_000i64 + (i as i64 * 86_400_000);
        let price = 150.0 + (i as f32 * 0.01);
        buf.push(
            ts,
            price,
            price + 2.0,
            price - 1.5,
            price + 0.5,
            (1000 + i) as u32,
        );
    }
    buf
}

#[test]
fn test_bulk_insert_roundtrip() {
    let conn = test_conn();
    let key = sample_key();
    let buf = sample_buffer(100);
    let inserted = bulk_insert(&conn, &key, &buf, "test").unwrap();
    assert_eq!(inserted, 100);

    let loaded = query_all(&conn, &key).unwrap();
    assert_eq!(loaded.len(), 100);
    assert_eq!(loaded.timestamps[0], buf.timestamps[0]);
    assert_eq!(loaded.opens[0], buf.opens[0]);
    assert_eq!(loaded.volumes[99], buf.volumes[99]);
}

#[test]
fn test_empty_query() {
    let conn = test_conn();
    let key = DataKey {
        symbol: "NONEXISTENT".into(),
        timeframe: Timeframe::M5,
    };
    let result = query_all(&conn, &key).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_range_query() {
    let conn = test_conn();
    let key = sample_key();
    let buf = sample_buffer(1000);
    bulk_insert(&conn, &key, &buf, "test").unwrap();

    let start = buf.timestamps[100];
    let end = buf.timestamps[199];
    let range = query_range(&conn, &key, start, end).unwrap();
    assert_eq!(range.len(), 100);
    assert_eq!(range.timestamps[0], start);
    assert_eq!(range.timestamps[99], end);
}

#[test]
fn test_data_ranges_updated() {
    let conn = test_conn();
    let key = sample_key();
    let buf = sample_buffer(50);
    bulk_insert(&conn, &key, &buf, "test").unwrap();

    let cached = list_cached(&conn).unwrap();
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].candle_count, 50);
    assert_eq!(cached[0].first_ts, buf.timestamps[0]);
    assert_eq!(cached[0].last_ts, buf.timestamps[buf.len() - 1]);
    assert_eq!(cached[0].source, "test");
}

#[test]
fn test_delete_symbol() {
    let conn = test_conn();
    let key = sample_key();
    let buf = sample_buffer(10);
    bulk_insert(&conn, &key, &buf, "test").unwrap();

    delete_symbol(&conn, &key).unwrap();

    let result = query_all(&conn, &key).unwrap();
    assert!(result.is_empty());
    let cached = list_cached(&conn).unwrap();
    assert!(cached.is_empty());
}

#[test]
fn test_multiple_symbols() {
    let conn = test_conn();
    let key_aapl = DataKey {
        symbol: "AAPL".into(),
        timeframe: Timeframe::D1,
    };
    let key_msft = DataKey {
        symbol: "MSFT".into(),
        timeframe: Timeframe::D1,
    };

    bulk_insert(&conn, &key_aapl, &sample_buffer(50), "test").unwrap();
    bulk_insert(&conn, &key_msft, &sample_buffer(30), "test").unwrap();

    assert_eq!(query_all(&conn, &key_aapl).unwrap().len(), 50);
    assert_eq!(query_all(&conn, &key_msft).unwrap().len(), 30);
}

#[test]
fn test_multiple_timeframes() {
    let conn = test_conn();
    let key_d1 = DataKey {
        symbol: "AAPL".into(),
        timeframe: Timeframe::D1,
    };
    let key_m5 = DataKey {
        symbol: "AAPL".into(),
        timeframe: Timeframe::M5,
    };

    bulk_insert(&conn, &key_d1, &sample_buffer(50), "test").unwrap();
    let mut buf_m5 = CandleBuffer::with_capacity(30);
    for i in 0..30 {
        let ts = 1_700_000_000_000i64 + (i as i64 * 300_000);
        buf_m5.push(ts, 100.0, 101.0, 99.0, 100.5, 500);
    }
    bulk_insert(&conn, &key_m5, &buf_m5, "test").unwrap();

    assert_eq!(query_all(&conn, &key_d1).unwrap().len(), 50);
    assert_eq!(query_all(&conn, &key_m5).unwrap().len(), 30);
}

#[test]
fn test_large_buffer_insert() {
    let conn = test_conn();
    let key = sample_key();
    let buf = sample_buffer(50_000);
    let inserted = bulk_insert(&conn, &key, &buf, "test").unwrap();
    assert_eq!(inserted, 50_000);

    let loaded = query_all(&conn, &key).unwrap();
    assert_eq!(loaded.len(), 50_000);
    assert_eq!(loaded.timestamps[0], buf.timestamps[0]);
    assert_eq!(
        loaded.timestamps[loaded.len() - 1],
        buf.timestamps[buf.len() - 1]
    );
}

#[test]
fn test_reinsert_overwrites() {
    let conn = test_conn();
    let key = sample_key();
    let buf1 = sample_buffer(50);
    bulk_insert(&conn, &key, &buf1, "test").unwrap();

    let mut buf2 = CandleBuffer::with_capacity(20);
    for i in 0..20 {
        let ts = 2_000_000_000_000i64 + (i as i64 * 86_400_000);
        buf2.push(ts, 200.0, 210.0, 190.0, 205.0, 5000);
    }
    bulk_insert(&conn, &key, &buf2, "test").unwrap();

    let loaded = query_all(&conn, &key).unwrap();
    assert_eq!(loaded.len(), 20);
    assert_eq!(loaded.timestamps[0], buf2.timestamps[0]);
}

#[test]
fn test_f32_roundtrip() {
    let conn = test_conn();
    let key = sample_key();

    let test_values: Vec<f32> = vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        f32::MIN_POSITIVE,
        f32::MAX,
        f32::MIN,
        std::f32::consts::PI,
        0.1,
        100.005,
    ];

    let mut buf = CandleBuffer::with_capacity(test_values.len());
    for (i, &val) in test_values.iter().enumerate() {
        let ts = 1_700_000_000_000i64 + (i as i64 * 86_400_000);
        buf.push(ts, val, val, val, val, 1);
    }
    bulk_insert(&conn, &key, &buf, "test").unwrap();

    let loaded = query_all(&conn, &key).unwrap();
    for (i, &expected) in test_values.iter().enumerate() {
        assert_eq!(
            loaded.opens[i].to_bits(),
            expected.to_bits(),
            "f32 roundtrip failed at index {i}: expected {expected}, got {}",
            loaded.opens[i]
        );
    }
}

#[test]
#[ignore = "DuckDB debug-mode assertion in compress_string.cpp; passes in release"]
fn test_reconcile_data_ranges() {
    let conn = test_conn();
    let key = sample_key();
    let buf = sample_buffer(100);
    bulk_insert(&conn, &key, &buf, "test").unwrap();

    // Corrupt metadata.
    conn.execute("DELETE FROM meta.data_ranges", []).unwrap();
    assert!(list_cached(&conn).unwrap().is_empty());

    // Reconcile should detect the mismatch and rebuild.
    reconcile_data_ranges(&conn).unwrap();
    let cached = list_cached(&conn).unwrap();
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].candle_count, 100);
}
