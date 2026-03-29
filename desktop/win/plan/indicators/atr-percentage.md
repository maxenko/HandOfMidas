# ATR Percentage Display

Simple overlay showing the current ATR as a percentage of price,
displayed as a text label on the chart (e.g., "ATR: 2.3%").

---

## What It Shows

A single text value updated per bar:

```
ATR% = (ATR / close) * 100
```

Displayed as e.g. `ATR: 2.34%` or `+/- 2.34%` in a fixed position
on the chart (top-right corner or similar info area).

Useful for quickly gauging volatility regime — a stock with 1% ATR
behaves very differently from one with 5% ATR.

---

## Algorithm

### Step 1: True Range

```
TR = max(high - low, |high - close_prev|, |low - close_prev|)
```

First bar (no previous close): `TR = high - low`.

### Step 2: ATR (Wilder's Smoothing)

```
RMA[i] = TR * (1/length) + RMA[i-1] * (1 - 1/length)
```

First `length` bars: SMA of TR values.

This is the same Wilder's smoothing used by Predictive Ranges —
shared implementation.

### Step 3: Percentage

```
ATR_pct = (ATR / source) * 100
```

Where `source` is typically `close`.

---

## Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| Length | 14 | ATR smoothing period (standard default) |
| Source | close | Denominator for percentage calculation |

---

## Display Options

- **Position**: top-right info area or alongside the OHLCV data overlay
- **Format**: `ATR: X.XX%` or `ATR(14): X.XX%`
- **Color coding** (optional): green when below median, red when above —
  or just neutral white/gray for simplicity in v1

Could also show the raw ATR value alongside: `ATR: 4.52 (2.34%)`

---

## Implementation Notes

### State per instance

- `rma: f64` — Wilder's ATR accumulator
- `bar_count: usize` — for SMA initialization period
- `sum: f64` — running sum during SMA init phase

Same ATR state as Predictive Ranges. Extract a shared `WildersATR`
struct that both indicators use.

### Per-bar compute

One True Range calculation, one RMA update, one division. O(1).

### Rendering

Single text label — fits naturally into the existing `OhlcvOverlay`
area or as a new field on `ChartScene`. No GPU primitives needed
beyond text rendering already in place for price/time labels.

### Shared code with Predictive Ranges

Both indicators need Wilder's ATR. Implement once as:

```rust
pub struct WildersAtr {
    length: usize,
    rma: f64,
    sum: f64,
    count: usize,
}

impl WildersAtr {
    pub fn update(&mut self, high: f64, low: f64, prev_close: f64) -> f64;
    pub fn value(&self) -> f64;
}
```

Lives in a shared indicators module (or `midas-chart::indicators`).
