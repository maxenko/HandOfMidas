# DuckDB Schema Design for Trading Data

## Core Schema

```sql
-- Schema namespaces for logical grouping
CREATE SCHEMA IF NOT EXISTS market;
CREATE SCHEMA IF NOT EXISTS cache;
CREATE SCHEMA IF NOT EXISTS meta;

-- ================================================================
-- CANDLE DATA: Primary OHLCV storage
-- ================================================================
CREATE TABLE market.candles (
    symbol         VARCHAR    NOT NULL,
    timeframe_secs INTEGER    NOT NULL,   -- 60=1m, 300=5m, 3600=1H, 86400=1D
    timestamp_ms   BIGINT     NOT NULL,   -- Epoch milliseconds (matches CandleBuffer)
    open           FLOAT      NOT NULL,   -- f32, matches existing CandleBuffer
    high           FLOAT      NOT NULL,
    low            FLOAT      NOT NULL,
    close          FLOAT      NOT NULL,
    volume         UINTEGER   NOT NULL,   -- u32, matches existing CandleBuffer

    PRIMARY KEY (symbol, timeframe_secs, timestamp_ms)
);

-- ================================================================
-- CACHE METADATA: Quick inventory queries
-- ================================================================
CREATE TABLE meta.data_ranges (
    symbol         VARCHAR    NOT NULL,
    timeframe_secs INTEGER    NOT NULL,
    candle_count   INTEGER    NOT NULL,
    first_ts       BIGINT     NOT NULL,
    last_ts        BIGINT     NOT NULL,
    source         VARCHAR    NOT NULL,   -- 'csv', 'ib_historical', 'ib_stream', 'test'
    updated_at     TIMESTAMP  DEFAULT CURRENT_TIMESTAMP,

    PRIMARY KEY (symbol, timeframe_secs)
);

-- ================================================================
-- SYMBOL CATALOG
-- ================================================================
CREATE TABLE meta.symbols (
    symbol     VARCHAR PRIMARY KEY,
    name       VARCHAR,
    sec_type   VARCHAR NOT NULL DEFAULT 'STK',  -- stores SecurityType::Display form; parse via from_str()
    exchange   VARCHAR NOT NULL DEFAULT 'SMART',
    currency   VARCHAR NOT NULL DEFAULT 'USD',
    con_id     INTEGER,                -- IB contract ID
    min_tick   DOUBLE,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- ================================================================
-- SCHEMA VERSION (for migrations)
-- ================================================================
CREATE TABLE IF NOT EXISTS schema_version (
    version    INTEGER NOT NULL,
    applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
```

### Design Decisions

**Timestamps as BIGINT (not TIMESTAMP):** The entire codebase uses `i64` epoch milliseconds. Storing as BIGINT avoids conversion overhead at the Rust boundary. DuckDB's `make_timestamp()` and `epoch_ms()` functions bridge to TIMESTAMP when SQL analytics need date functions.

**Prices as FLOAT (not DOUBLE):** Matches `CandleBuffer`'s `Vec<f32>` exactly. No precision loss on roundtrip. Cast to DOUBLE in SQL when computing indicators.

**Volume as UINTEGER:** Matches `CandleBuffer`'s `Vec<u32>`.

**`timeframe_secs` as INTEGER:** Maps directly to `Timeframe::as_secs()`. Avoids string parsing.

**`sec_type` as VARCHAR:** DuckDB has no Rust enum mapping. Values store the `Display` form of `SecurityType` (`'STK'`, `'OPT'`, `'FUT'`, `'CASH'`) per the project convention "SecurityType enum over strings." On read, parse via `SecurityType::from_str()`. This keeps the enum as the canonical type in Rust while using VARCHAR for SQL queryability.

**All TIMESTAMP columns are UTC.** The `updated_at` fields in `meta.data_ranges` and `meta.symbols` use `CURRENT_TIMESTAMP` which returns UTC. Timezone conversion requires the ICU extension, which is deliberately omitted in v1. Do not write timezone-dependent queries against these columns without installing ICU first.

## Future Tables

### Tick Data (IB Streaming)

```sql
CREATE TABLE market.ticks (
    symbol      VARCHAR     NOT NULL,
    timestamp_ms BIGINT     NOT NULL,
    price       DOUBLE      NOT NULL,
    size        UINTEGER    NOT NULL,
    exchange    VARCHAR,
    conditions  VARCHAR
);
-- No PK: ticks can share timestamps. Sorted by (symbol, timestamp_ms).
```

### Indicator Cache

```sql
CREATE TABLE cache.indicators (
    symbol         VARCHAR NOT NULL,
    timeframe_secs INTEGER NOT NULL,
    indicator      VARCHAR NOT NULL,   -- 'ATR_14', 'SMA_200', 'RSI_14'
    timestamp_ms   BIGINT  NOT NULL,
    value          DOUBLE  NOT NULL,

    PRIMARY KEY (symbol, timeframe_secs, indicator, timestamp_ms)
);
```

## Partitioning Strategy

**Single table with compound PK** (recommended over table-per-symbol):

- DuckDB stores zone maps (min/max per row group per column) automatically
- Query `WHERE symbol = 'AAPL' AND timeframe_secs = 86400` skips non-matching row groups
- Cross-symbol queries work naturally without dynamic table names
- Simplifies all SQL and the Rust query layer

## Query Patterns

### Range scan: "Last 5000 daily candles for AAPL"

```sql
SELECT timestamp_ms, open, high, low, close, volume
FROM market.candles
WHERE symbol = 'AAPL' AND timeframe_secs = 86400
ORDER BY timestamp_ms DESC
LIMIT 5000;
```

### Time bucket aggregation: "1min -> 5min candles"

```sql
SELECT
    symbol,
    300 AS timeframe_secs,
    (timestamp_ms / 300000) * 300000 AS timestamp_ms,  -- floor to 5min boundary
    FIRST(open ORDER BY timestamp_ms) AS open,
    MAX(high) AS high,
    MIN(low) AS low,
    LAST(close ORDER BY timestamp_ms) AS close,
    SUM(volume)::UINTEGER AS volume
FROM market.candles
WHERE symbol = 'AAPL' AND timeframe_secs = 60
GROUP BY symbol, (timestamp_ms / 300000) * 300000
ORDER BY timestamp_ms;
```

### Window function: "14-period ATR"

```sql
WITH bars AS (
    SELECT timestamp_ms, high, low, close,
           LAG(close) OVER (ORDER BY timestamp_ms) AS prev_close
    FROM market.candles
    WHERE symbol = 'AAPL' AND timeframe_secs = 86400
),
tr AS (
    SELECT timestamp_ms,
           GREATEST(high - low, ABS(high - prev_close), ABS(low - prev_close)) AS true_range
    FROM bars WHERE prev_close IS NOT NULL
)
SELECT timestamp_ms,
       AVG(true_range) OVER (ORDER BY timestamp_ms ROWS BETWEEN 13 PRECEDING AND CURRENT ROW) AS atr_14
FROM tr;
```

Note: This is SMA-based ATR. Wilder's smoothed ATR requires recursive computation -- better done in Rust with the existing `WildersAtr` implementation.

### Cross-symbol scan: "All symbols with volume > 1M today"

```sql
SELECT symbol, timestamp_ms, volume, close
FROM market.candles
WHERE timeframe_secs = 86400
  AND timestamp_ms = (SELECT MAX(timestamp_ms) FROM market.candles WHERE timeframe_secs = 86400)
  AND volume > 1000000
ORDER BY volume DESC;
```

### ASOF JOIN: Align tick data with candle boundaries

```sql
SELECT c.timestamp_ms AS candle_ts, c.open, c.close,
       t.price AS pre_open_tick, t.timestamp_ms AS tick_ts
FROM market.candles c
ASOF JOIN market.ticks t
    ON c.symbol = t.symbol AND c.timestamp_ms >= t.timestamp_ms
WHERE c.symbol = 'AAPL' AND c.timeframe_secs = 300;
```

## Bulk Operations

### CSV Import

```sql
INSERT INTO market.candles
SELECT 'AAPL', 86400,
       epoch_ms(Date::TIMESTAMP) AS timestamp_ms,
       Open::FLOAT, High::FLOAT, Low::FLOAT, Close::FLOAT, Volume::UINTEGER
FROM read_csv('data/AAPL.csv', AUTO_DETECT=TRUE);
```

### Appender API (Rust, for streaming)

```rust
let mut appender = conn.appender("market.candles")?;
for i in 0..buf.len() {
    appender.append_row(params![
        symbol, timeframe_secs, buf.timestamps[i],
        buf.opens[i], buf.highs[i], buf.lows[i], buf.closes[i], buf.volumes[i],
    ])?;
}
appender.flush()?;
```

### Parquet Export (archival)

```sql
COPY (SELECT * FROM market.candles WHERE timeframe_secs = 86400)
TO 'archive/daily/' (FORMAT PARQUET, PARTITION_BY (symbol), COMPRESSION 'ZSTD');
```

### Upsert (overlapping data)

```sql
INSERT OR REPLACE INTO market.candles VALUES (?, ?, ?, ?, ?, ?, ?, ?);
```

For large overlaps, DELETE + INSERT is faster:
```sql
DELETE FROM market.candles WHERE symbol = ? AND timeframe_secs = ? AND timestamp_ms BETWEEN ? AND ?;
INSERT INTO market.candles SELECT * FROM staging;
```

## Cache Invalidation

- **Append-only for time-series:** New candles only added at the end. `INSERT OR IGNORE` skips duplicates.
- **Forming candle:** Lives in memory (`CandleBuffer.update_last()`). Flushed to DuckDB only when candle period closes.
- **Stock splits/adjustments:** UPDATE historical prices, then invalidate all indicator caches for that symbol.
- **Staleness detection:** Compare `meta.data_ranges.last_ts` against expected market close time.

## SQLite Interop

DuckDB can ATTACH the existing broker SQLite database read-only:

```sql
ATTACH 'data/broker.db' AS broker (TYPE SQLITE, READ_ONLY);

-- Cross-domain query: unrealized P&L
SELECT m.symbol, m.close AS last_price, b.quantity, b.avg_cost,
       (m.close - b.avg_cost) * b.quantity AS unrealized_pnl
FROM market.candles m
JOIN broker.positions b ON m.symbol = b.symbol
WHERE m.timeframe_secs = 86400
  AND m.timestamp_ms = (SELECT MAX(timestamp_ms) FROM market.candles
                         WHERE symbol = m.symbol AND timeframe_secs = 86400);
```

## Database File Layout

```
data/
  cache.duckdb           -- DuckDB: candles, ticks, indicators, metadata
  cache.duckdb.wal       -- DuckDB WAL (auto-managed)
  broker.db              -- SQLite: orders, fills, positions (unchanged)
  candles/               -- .midas binary: hot cache for active charts
    AAPL/1D.midas
    MSFT/5m.midas
  config.toml            -- App config (unchanged)
```
