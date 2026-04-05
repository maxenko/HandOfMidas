use super::*;
use midas_core::Timeframe;

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

#[tokio::test]
async fn test_dbhandle_open_memory() {
    let handle = DbHandle::open_memory();
    let cached = handle.list_cached().await.unwrap();
    assert!(cached.is_empty());
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_dbhandle_roundtrip() {
    let handle = DbHandle::open_memory();
    let key = sample_key();
    let buf = sample_buffer(100);

    let inserted = handle
        .insert_candles(key.clone(), buf.clone(), "test")
        .await
        .unwrap();
    assert_eq!(inserted, 100);

    let loaded = handle.query_candles(key).await.unwrap();
    assert_eq!(loaded.len(), 100);
    assert_eq!(loaded.timestamps[0], buf.timestamps[0]);

    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_dbhandle_open_file() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.duckdb");

    let config = StoreConfig {
        path: Some(db_path),
        ..Default::default()
    };
    let handle = DbHandle::open(config);

    let key = sample_key();
    handle
        .insert_candles(key.clone(), sample_buffer(10), "test")
        .await
        .unwrap();
    let loaded = handle.query_candles(key).await.unwrap();
    assert_eq!(loaded.len(), 10);

    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_dbhandle_concurrent_queries() {
    let handle = DbHandle::open_memory();
    let key = sample_key();
    handle
        .insert_candles(key.clone(), sample_buffer(100), "test")
        .await
        .unwrap();

    let mut tasks = Vec::new();
    for _ in 0..10 {
        let h = handle.clone();
        let k = key.clone();
        tasks.push(tokio::spawn(async move {
            h.query_candles(k).await.unwrap().len()
        }));
    }

    for task in tasks {
        assert_eq!(task.await.unwrap(), 100);
    }
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_dbhandle_fire_and_forget_insert() {
    let handle = DbHandle::open_memory();
    let key = sample_key();

    handle
        .fire_and_forget_insert(key.clone(), sample_buffer(50), "test")
        .await
        .unwrap();

    // Small delay for actor to process.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let loaded = handle.query_candles(key).await.unwrap();
    assert_eq!(loaded.len(), 50);
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_dbhandle_shutdown_clean() {
    let handle = DbHandle::open_memory();
    handle
        .insert_candles(sample_key(), sample_buffer(10), "test")
        .await
        .unwrap();
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_dbhandle_list_cached() {
    let handle = DbHandle::open_memory();
    handle
        .insert_candles(sample_key(), sample_buffer(100), "test")
        .await
        .unwrap();

    let cached = handle.list_cached().await.unwrap();
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].key.symbol, "AAPL");
    assert_eq!(cached[0].candle_count, 100);

    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_dbhandle_query_range() {
    let handle = DbHandle::open_memory();
    let key = sample_key();
    let buf = sample_buffer(1000);
    handle
        .insert_candles(key.clone(), buf.clone(), "test")
        .await
        .unwrap();

    let start = buf.timestamps[100];
    let end = buf.timestamps[199];
    let range = handle.query_candles_range(key, start, end).await.unwrap();
    assert_eq!(range.len(), 100);

    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_dbhandle_permanent_failure() {
    let config = StoreConfig {
        path: Some(std::path::PathBuf::from("/nonexistent/path/db.duckdb")),
        ..Default::default()
    };
    let handle = DbHandle::open(config);

    // First call fails.
    let result = handle.list_cached().await;
    assert!(result.is_err());

    // Second call also fails without retrying — permanent failure.
    let result2 = handle.list_cached().await;
    assert!(result2.is_err());
}
