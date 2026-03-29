# Predictive Ranges (Lux Algo)

Overlay indicator that draws 5 horizontal levels forming a predicted
support/resistance box around price. Levels hold flat until price breaks
out, then recalculate instantly. No repainting.

Based on: [Predictive Ranges [LuxAlgo]](https://www.tradingview.com/script/lIdNGLiV-Predictive-Ranges-LuxAlgo/)

---

## Visual Output

Five horizontal levels per range box:

| Level | Position | Color convention |
|-------|----------|-----------------|
| R2 | avg + band_width * range_multi | outer resistance (red) |
| R1 | avg + band_width | inner resistance (red) |
| Avg | step function output | central level (blue) |
| S1 | avg - band_width | inner support (green) |
| S2 | avg - band_width * range_multi | outer support (green) |

Levels are constant until a breakout event. On breakout, the entire box
jumps to a new position. The direction of the step (up/down) signals
trend direction.

---

## Algorithm

### Step 1: Compute scaled ATR

```
tr = TrueRange(high, close_prev, low)
ATR = WildersSmoothing(tr, length)     // EMA with alpha = 1/length
nATR = ATR * factor
```

Wilder's smoothing (= Pine's `ta.rma`):
```
RMA[i] = value * (1/length) + RMA[i-1] * (1 - 1/length)
```
First `length` bars initialized as SMA.

### Step 2: Step-function central level

```
prev_avg = avg[i-1]  (first bar: use source)

if (source - prev_avg) > nATR:
    avg = prev_avg + nATR          // breakout above, step UP
elif (prev_avg - source) > nATR:
    avg = prev_avg - nATR          // breakout below, step DOWN
else:
    avg = prev_avg                 // within range, hold
```

### Step 3: Lock band width on step

```
if avg != prev_avg:                // step just occurred
    band_width = nATR / 2
else:
    band_width = band_width[i-1]   // frozen between steps
```

### Step 4: Compute five levels

```
R2 = avg + band_width * range_multi
R1 = avg + band_width
C  = avg
S1 = avg - band_width
S2 = avg - band_width * range_multi
```

### Step 5: Visual gap on step bars

On the bar where `avg` changes, all levels are set to NaN (hidden),
creating a visual break between the old and new range box.

---

## Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| Length | 200 | Wilder's ATR smoothing period |
| Factor | 6.0 | ATR multiplier for breakout threshold |
| Range Multi | 2.0 | Outer band scale relative to inner |
| Source | close | Price input (close, hl2, hlc3, etc.) |

---

## Implementation Notes

### State per instance

- `avg: f64` — current central level
- `band_width: f64` — locked half-width
- `rma: f64` — Wilder's ATR accumulator
- `bar_count: usize` — for SMA initialization period

### Per-bar compute

One ATR update, one comparison, trivial arithmetic. O(1) per bar.

### Rendering

5 horizontal lines — existing `LevelRender` / `GridLineInstance`
infrastructure should work directly. Each range box is a set of
constant-Y lines until the next step event.

### Licensing

Original Pine Script is CC BY-NC-SA 4.0 (non-commercial). The
algorithm itself (ATR + step function + bands) is a standard
mathematical construction. Clean-room implementation from the
algorithm description above is license-free.
