# Test Data Provider

Deterministic, realistic per-ticker market data. ~10 years (2016–2026), multi-timeframe.

## Quick Start

```rust
use midas_broker::testdata::TestDataProvider;
use midas_broker_core::Timeframe;

let mut provider = TestDataProvider::new();

// Daily bars for the last 6 months
let daily = provider.bars_last_months("AAPL", Timeframe::D1, 6);

// 5-minute bars for a date range (epoch seconds)
let intraday = provider.bars("TSLA", Timeframe::M5, 1704067200, 1704153600);

// Same ticker always gives the same data
let a = provider.bars_last_days("MSFT", Timeframe::H1, 30);
let b = provider.bars_last_days("MSFT", Timeframe::H1, 30);
assert_eq!(a.len(), b.len());
```

## API

| Method | Returns |
|---|---|
| `TestDataProvider::new()` | Empty provider, generates lazily on first access |
| `bars(ticker, tf, start, end)` | `Vec<OhlcvBar>` in `[start, end)` |
| `bars_last_days(ticker, tf, days)` | Convenience for recent data |
| `bars_last_months(ticker, tf, months)` | Convenience for recent data |
| `daily_bars(ticker)` | `&[OhlcvBar]` — all ~2,700 daily bars |
| `date_range(ticker)` | `(start_ts, end_ts)` of available data |
| `aggregate_bars(source, target_tf)` | Standalone OHLCV aggregation utility |

**Supported timeframes:** S30, M1, M5, M15, M30, H1, H4, D1, W1, MN1.

## How It Works

### 1. Ticker Seeding

Ticker name → FNV-1a hash → `u64` seed. Same ticker always produces identical data. Different tickers are different.

### 2. Personality Selection

The seed selects and varies one of 5 base presets:

| Preset | Like | Start Price | Character |
|---|---|---|---|
| Growth | TSLA | $8–$38 | Aggressive uptrend, high vol, fast regime changes |
| Blue Chip | AAPL | $60–$300 | Moderate drift, low vol, long stable trends |
| Volatile | GME | $4–$20 | Wild swings, erratic regime switching |
| Steady | KO | $28–$138 | Low vol, mostly consolidation |
| Cyclical | XOM | $35–$175 | Balanced bull/bear cycles |

Each ticker gets ±15–20% random variation on all parameters.

### 3. Daily Generation (regime switching + GARCH)

4 regimes: **Bull**, **Bear**, **Consolidation**, **Crash**. Each has its own drift and volatility. The system transitions between regimes via a Markov chain with high self-persistence (regimes last weeks/months).

- **GARCH(1,1)** volatility clustering — big moves beget big moves
- **Momentum** — slight return autocorrelation for realistic trending
- **Overnight gaps** — open != previous close
- **Volume** — correlated with returns, regime-adjusted, lognormal noise

### 4. Intraday Generation (Brownian bridge)

For each trading day, given the daily OHLC:
1. Generate a Brownian bridge from Open → Close over 780 S30 bars (6.5h market hours, 14:30–21:00 UTC)
2. Scale the path to touch the daily High and Low
3. Add small intrabar wicks
4. U-shaped volume profile (high at open/close, low midday)

### 5. Aggregation

Coarser timeframes aggregate from S30 (intraday) or D1 (daily/weekly/monthly). OHLCV rules: first open, max high, min low, last close, sum volume.

## MarketDataSource Adapter

`TestDataProvider` implements `MarketDataSource`, so it works seamlessly through the engine. See [market-data.md](market-data.md) for the channel-based usage pattern.
