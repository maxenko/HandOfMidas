# midas-core API

Shared foundation types used by all crates. Zero dependency on `ibapi`.

## Types

| Type | Purpose |
|---|---|
| `ContractSpec` | Serializable instrument identifier (Stock, Option, Future, Forex) |
| `SecurityType` | Type-safe IB security type — Display/FromStr as `"STK"`/`"OPT"`/`"FUT"`/`"CASH"` |
| `OptionRight` | Call/Put discriminant |
| `SymbolKey` | Compact `(contract_id, symbol)` for hot-path lookups |
| `Timeframe` | Bar durations S1..MN1 with `as_secs()` |
| `OhlcvBar` | Single candlestick bar (timestamp, open, high, low, close, volume) |

## ContractSpec

```rust
pub enum ContractSpec {
    Stock { symbol: String, exchange: String, currency: String },
    Option { symbol: String, expiry: String, strike: OrderedFloat<f64>, right: OptionRight, exchange: String },
    Future { symbol: String, expiry: String, exchange: String },
    Forex { pair: String },
}
```

- `symbol()` returns the primary symbol for any variant.
- All variants are `Hash + Eq` (thanks to `OrderedFloat` for the strike price).

## SecurityType

```rust
pub enum SecurityType { Stock, Option, Future, Forex }
```

- `Display`: `Stock` → `"STK"`, `Option` → `"OPT"`, `Future` → `"FUT"`, `Forex` → `"CASH"`
- `FromStr`: parses the IB strings back
- Use this instead of raw strings for `sec_type` fields.

## SymbolKey

```rust
pub struct SymbolKey { pub contract_id: i32, pub symbol: String }
```

Used in `BrokerEvent` bar/tick variants for efficient hot-path lookups.

## Timeframe

```rust
pub enum Timeframe { S1, S5, S15, S30, M1, M5, M15, M30, H1, H4, D1, W1, MN1 }
```

- `as_secs()` returns the duration: `M5` → 300, `D1` → 86400
- `Display`: `H4` → `"4h"`, `MN1` → `"1M"`

## OhlcvBar

```rust
pub struct OhlcvBar {
    pub timestamp: i64,  // UTC epoch seconds
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
}
```

Used by both the broker (historical data) and the UI (chart rendering).
