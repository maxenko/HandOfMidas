# Market Data Source & IB String Parsers

## MarketDataSource Trait

```rust
pub trait MarketDataSource: Send {
    fn historical_bars(
        &mut self,
        symbol: &str,
        con_id: i32,
        timeframe: Timeframe,
        start: i64,
        end: i64,
        request_id: u64,
    ) -> Result<HistoricalBarsResult, BrokerError>;
}

pub struct HistoricalBarsResult {
    pub symbol: SymbolKey,
    pub request_id: u64,
    pub bars: Vec<OhlcvBar>,
}
```

Both `TestDataProvider` and the future IB adapter implement this trait. The engine dispatches `RequestHistoricalData` through it — consumer code is identical regardless of data source.

## How the Engine Uses It

1. Consumer sends `BrokerCommand::RequestHistoricalData { symbol, con_id, duration, bar_size, request_id }`
2. Engine parses `bar_size` → `Timeframe` and `duration` → `(start, end)` timestamps
3. Engine calls `data_source.historical_bars(symbol, con_id, timeframe, start, end, request_id)`
4. Engine emits one `BrokerEvent::BarClosed` per bar + `HistoricalDataComplete` at the end

The consumer receives the same events whether data comes from test generation or live IB.

## IB String Parsers (`ib_strings.rs`)

### parse_bar_size

| IB string | Timeframe |
|---|---|
| `"1 secs"` | S1 |
| `"5 secs"` | S5 |
| `"15 secs"` | S15 |
| `"30 secs"` | S30 |
| `"1 min"` | M1 |
| `"5 mins"` | M5 |
| `"15 mins"` | M15 |
| `"30 mins"` | M30 |
| `"1 hour"` | H1 |
| `"4 hours"` | H4 |
| `"1 day"` | D1 |
| `"1 week"` | W1 |
| `"1 month"` | MN1 |

### duration_to_start

Converts an IB duration string + end timestamp into a start timestamp.

| Unit | Example | Meaning |
|---|---|---|
| S | `"3600 S"` | 3600 seconds before end |
| D | `"30 D"` | 30 days |
| W | `"2 W"` | 2 weeks |
| M | `"6 M"` | 6 calendar months (uses chrono) |
| Y | `"1 Y"` | 1 year |

## Seamless Usage

```rust
// Test mode — config is the only difference
let mut config = BrokerConfig::default();
config.data_source = DataSourceConfig::Test;
let handle = start_broker_engine(config);
let mut rx = handle.market_events.subscribe();

// Same command for test or real IB
handle.commands.send(BrokerCommand::RequestHistoricalData {
    symbol: "AAPL".to_string(),
    con_id: 265598,
    duration: "30 D".to_string(),
    bar_size: "1 day".to_string(),
    request_id: 42,
}).await?;

// Receive BrokerEvent::BarClosed events + HistoricalDataComplete
```
