# 03 — Market Data & Account Simulation

> Tick generation, bar streaming, position tracking, and account values.

---

## Table of Contents

- [1. Market Data Generation](#1-market-data-generation)
- [2. Bar Streaming](#2-bar-streaming)
- [3. Position Tracking](#3-position-tracking)
- [4. Account Values](#4-account-values)
- [5. P&L Calculation](#5-pnl-calculation)

---

## 1. Market Data Generation

### 1.1 Tick Generation from TestDataProvider

The test broker generates L1 ticks by interpolating from the existing
`TestDataProvider` OHLCV data:

```rust
/// Generate a synthetic tick for a symbol at a given timestamp.
fn generate_tick(&self, symbol: &str, timestamp: i64) -> TickSnapshot {
    let inner = self.inner.lock().unwrap();
    let dp = &inner.data_provider;
    let bars = dp.daily_bars(symbol);

    // Find the current bar
    let bar = bars.iter().rev().find(|b| b.timestamp <= timestamp)
        .unwrap_or_else(|| bars.last().unwrap());

    // Interpolate within the bar's OHLC range
    let progress = /* fraction of bar period elapsed */ ;
    let price = interpolate_ohlc(bar, progress);

    // Bid-ask spread (configurable, default 1 cent for stocks)
    let spread = self.config.default_spread;
    let bid = price - spread / 2.0;
    let ask = price + spread / 2.0;

    TickSnapshot {
        symbol: symbol.to_string(),
        bid: Some(bid),
        ask: Some(ask),
        last: Some(price),
        volume: Some(bar.volume / /* bars per day */),
        timestamp: Utc::now(),
    }
}
```

### 1.2 OHLC Interpolation

Within a single bar period, price follows a simplified path:

```
Open ──→ High ──→ Low ──→ Close
 0%       25%      75%     100%

Progress 0.00-0.25: linear from Open to High
Progress 0.25-0.75: linear from High to Low
Progress 0.75-1.00: linear from Low to Close
```

This creates a realistic-looking price path that respects the bar's
OHLC boundaries.

### 1.3 Tick Subscription Management

```rust
fn subscribe_market_data(&self, symbol: &str, _con_id: i32) {
    let mut inner = self.inner.lock().unwrap();
    inner.subscriptions.insert(symbol.to_string());

    // Seed initial market price from TestDataProvider
    let price = Self::get_or_seed_price_inner(&mut inner, symbol);

    // Push initial tick snapshot
    inner.callbacks.push_back(BrokerCallback::Tick {
        symbol: symbol.to_string(),
        con_id: _con_id,
        bid: Some(price - 0.005),
        ask: Some(price + 0.005),
        last: Some(price),
        volume: Some(0),
    });
}

fn unsubscribe_market_data(&self, symbol: &str) {
    let mut inner = self.inner.lock().unwrap();
    inner.subscriptions.remove(symbol);
}
```

### 1.4 Auto-Tick Mode

When `tick_interval_ms > 0`, the test broker generates periodic ticks
for all subscribed symbols. This drives price-triggered fills:

```rust
/// Called periodically by the engine's poll loop.
fn generate_auto_ticks(&self) {
    let mut inner = self.inner.lock().unwrap();
    let subs: Vec<String> = inner.subscriptions.iter().cloned().collect();
    for symbol in subs {
        let tick = Self::generate_tick_inner(&inner, &symbol, Utc::now().timestamp());
        inner.callbacks.push_back(BrokerCallback::Tick {
            symbol: tick.symbol,
            con_id: 0,
            bid: tick.bid,
            ask: tick.ask,
            last: tick.last,
            volume: tick.volume,
        });

        // Update market price (drives limit/stop fill checks)
        if let Some(last) = tick.last {
            inner.market_prices.insert(symbol.clone(), last);
            Self::check_limit_fills_inner(&mut inner, &symbol, last);
            Self::check_stop_triggers_inner(&mut inner, &symbol, last);
        }
    }
}
```

### 1.5 Commission Model

Simple per-share commission (matching IB's IBKR Pro tiered pricing):

| Config Field | Default | Description |
|---|---|---|
| `commission_per_share` | 0.005 | USD per share (IB Pro minimum) |

Commission = shares x commission_per_share, minimum $1.00 per trade.

---

## 2. Bar Streaming

### 2.1 Historical Data Request

Delegates to the existing `TestDataProvider` (already implemented):

```rust
fn request_historical_data(
    &self,
    symbol: &str,
    con_id: i32,
    duration: &str,
    bar_size: &str,
    request_id: u64,
) -> Result<HistoricalBarsResult, BrokerError> {
    let mut inner = self.inner.lock().unwrap();
    inner.data_provider.historical_bars(symbol, con_id, timeframe, start, end, request_id)
}
```

### 2.2 Streaming Bar Updates

When `keep_up_to_date` is simulated, the test broker emits:
- `BarUpdated` for the forming bar (on each tick)
- `BarClosed` when a bar period completes

```rust
fn update_forming_bar(&self, symbol: &str, price: f64, volume: i64) {
    let mut inner = self.inner.lock().unwrap();
    let bar = inner.forming_bars.entry(symbol.to_string()).or_insert(FormingBar::new(price));
    bar.high = bar.high.max(price);
    bar.low = bar.low.min(price);
    bar.close = price;
    bar.volume += volume;

    inner.callbacks.push_back(BrokerCallback::BarUpdated {
        symbol: symbol.to_string(),
        timestamp: Utc::now().timestamp(),
        open: bar.open,
        high: bar.high,
        low: bar.low,
        close: bar.close,
        volume: bar.volume,
    });
}
```

---

## 3. Position Tracking

### 3.1 Position State

```rust
struct PositionState {
    symbol: String,
    con_id: i32,
    quantity: f64,    // positive = long, negative = short
    avg_cost: f64,    // weighted average entry price
}
```

### 3.2 Fill → Position Update

After each execution, the test broker updates positions:

```rust
fn update_position_on_fill(&self, symbol: &str, con_id: i32, action: &str, shares: f64, price: f64) {
    let mut inner = self.inner.lock().unwrap();
    let pos = inner.positions.entry(symbol.to_string()).or_insert(PositionState {
        symbol: symbol.to_string(),
        con_id,
        quantity: 0.0,
        avg_cost: 0.0,
    });

    let signed_shares = match action {
        "BUY" => shares,
        "SELL" => -shares,
        _ => return,
    };

    // Weighted average cost calculation
    if pos.quantity.signum() == signed_shares.signum() || pos.quantity == 0.0 {
        // Adding to position: weighted average
        let total_cost = pos.avg_cost * pos.quantity.abs() + price * shares;
        pos.quantity += signed_shares;
        if pos.quantity.abs() > 0.0 {
            pos.avg_cost = total_cost / pos.quantity.abs();
        }
    } else {
        // Reducing/flipping position
        let prev_quantity = pos.quantity;
        pos.quantity += signed_shares;
        if pos.quantity.abs() < f64::EPSILON {
            pos.avg_cost = 0.0;  // flat
        }
        // If flipped, avg_cost becomes the fill price for the new direction
        if pos.quantity.signum() != prev_quantity.signum() && pos.quantity.abs() > 0.0 {
            pos.avg_cost = price;
        }
    }

    // Emit position update callback
    inner.callbacks.push_back(BrokerCallback::Position {
        symbol: pos.symbol.clone(),
        con_id: pos.con_id,
        quantity: pos.quantity,
        avg_cost: pos.avg_cost,
    });
}
```

### 3.3 Position Queries

```rust
fn request_positions(&self) {
    let mut inner = self.inner.lock().unwrap();
    let snapshots: Vec<_> = inner.positions.values().map(|pos| {
        BrokerCallback::Position {
            symbol: pos.symbol.clone(),
            con_id: pos.con_id,
            quantity: pos.quantity,
            avg_cost: pos.avg_cost,
        }
    }).collect();
    for cb in snapshots {
        inner.callbacks.push_back(cb);
    }
}
```

---

## 4. Account Values

### 4.1 Account State

```rust
struct AccountState {
    cash: f64,
    initial_cash: f64,
    realized_pnl: f64,
}
```

### 4.2 Fill → Account Update

```rust
fn update_account_on_fill(&self, action: &str, shares: f64, price: f64, commission: f64) {
    let mut inner = self.inner.lock().unwrap();
    let notional = shares * price;

    match action {
        "BUY" => inner.cash -= notional + commission,
        "SELL" => inner.cash += notional - commission,
        _ => return,
    }

    // Emit account value updates
    inner.callbacks.push_back(BrokerCallback::AccountValue {
        key: "CashBalance".to_string(),
        value: format!("{:.2}", inner.cash),
        currency: "USD".to_string(),
    });

    let net_liq = inner.cash + Self::position_market_value_inner(&inner);
    inner.callbacks.push_back(BrokerCallback::AccountValue {
        key: "NetLiquidation".to_string(),
        value: format!("{:.2}", net_liq),
        currency: "USD".to_string(),
    });
}
```

### 4.3 Account Summary Query

```rust
fn request_account_summary(&self) {
    let mut inner = self.inner.lock().unwrap();
    let cash = inner.cash;
    let net_liq = cash + Self::position_market_value_inner(&inner);
    let unrealized = Self::unrealized_pnl_inner(&inner);
    let realized = inner.realized_pnl;

    for (key, value) in [
        ("NetLiquidation", format!("{:.2}", net_liq)),
        ("CashBalance", format!("{:.2}", cash)),
        ("BuyingPower", format!("{:.2}", cash * 4.0)),  // 4x margin
        ("UnrealizedPnL", format!("{:.2}", unrealized)),
        ("RealizedPnL", format!("{:.2}", realized)),
    ] {
        inner.callbacks.push_back(BrokerCallback::AccountValue {
            key: key.to_string(),
            value,
            currency: "USD".to_string(),
        });
    }
}
```

---

## 5. P&L Calculation

### 5.1 Unrealized P&L

```rust
fn unrealized_pnl(&self) -> f64 {
    let inner = self.inner.lock().unwrap();
    inner.positions.values().map(|pos| {
        let market_price = inner.market_prices.get(&pos.symbol).copied().unwrap_or(pos.avg_cost);
        (market_price - pos.avg_cost) * pos.quantity
    }).sum()
}
```

### 5.2 Realized P&L

Tracked cumulatively as positions are closed:

```rust
fn update_realized_pnl_on_close(&self, symbol: &str, closed_qty: f64, entry_price: f64, exit_price: f64) {
    let pnl = (exit_price - entry_price) * closed_qty;
    let mut inner = self.inner.lock().unwrap();
    inner.realized_pnl += pnl;
}
```
