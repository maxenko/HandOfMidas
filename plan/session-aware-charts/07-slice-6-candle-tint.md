# Slice 6 — Session-aware candle coloring

**Goal.** Tint pre-market and post-market candles so they're visually distinguishable from regular-hours candles. No shader change — reuses existing `CandleInstance.color` attribute.

## Scope

### `compute/build_candle_instances` modification

`desktop/win/crates/midas-chart/src/compute/mod.rs`:

```rust
for i in vis_start..vis_end {
    let session = data.session(i);  // default Regular (slice 2)
    let is_bull = close >= open;
    let base = if is_bull { params.bull_color } else { params.bear_color };
    let color = apply_session_tint(base, session, params);
    // ... build CandleInstance with `color` ...
}

fn apply_session_tint(base: [f32;4], kind: SessionKind, params: &Params) -> [f32;4] {
    match kind {
        SessionKind::Regular => base,
        SessionKind::PreMarket => scale_rgb(base, params.pre_market_tint_mult),
        SessionKind::PostMarket => scale_rgb(base, params.post_market_tint_mult),
        _ => base,
    }
}

fn scale_rgb(c: [f32;4], f: f32) -> [f32;4] {
    [c[0]*f, c[1]*f, c[2]*f, c[3]]
}
```

### Params extension

`ChartParams` (existing style struct in `midas-chart`) gains:

```rust
pub pre_market_tint_mult: f32,    // default 0.65
pub post_market_tint_mult: f32,   // default 0.55
```

## Files touched

- `desktop/win/crates/midas-chart/src/compute/mod.rs` — build_candle_instances + helpers.
- `desktop/win/crates/midas-chart/src/style.rs` (or wherever `ChartParams` lives) — new fields.

## Tests

- `pre_market_candle_is_dimmer`: construct a CandleBuffer with one candle tagged PreMarket; build instances; assert its color RGB is `base × 0.65` (within epsilon).
- `post_market_candle_is_dimmest`: same but PostMarket, `× 0.55`.
- `regular_candle_unchanged`: color == base.
- `custom_tint_mult_overrides_default`: pass `pre_market_tint_mult = 0.8`, verify RGB.

## Acceptance

- All tests pass.
- `cargo clippy`, `cargo fmt` clean.
- Visual smoke test (manual): run app with a stock chart, observe dimmed pre-market candles.

## Commit

Single commit: `feat(chart): session-aware candle coloring via CandleData::session`.
