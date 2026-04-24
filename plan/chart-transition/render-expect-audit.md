# Render-path `.expect()` audit

Slice 0 artifact. Enumerates every panicking call on the chart-render
hot path — `.expect()`, `.unwrap()`, `panic!`, `unreachable!` — so
the plan's slice 1 panic-recovery machinery (`catch_unwind` + fallback
quad) has a known surface to cover.

## Methodology

```bash
cd desktop/win
grep -rn '\.expect(\|\.unwrap(\|panic!\|unreachable!' crates/midas-render/src
grep -rn '\.expect(\|\.unwrap(\|panic!\|unreachable!' crates/midas-chart/src
grep -rn '\.expect(\|\.unwrap(\|panic!\|unreachable!' \
  crates/midas-app/src/session_chart
grep -rn '\.expect(\|\.unwrap(\|panic!\|unreachable!' \
  crates/midas-core/src/candle_buffer
```

Only sites reachable from `ChartRenderer::prepare` /
`ChartRenderer::draw_pass` or from `session_chart::gpu_renderer` are
in scope. Tests, doc-examples, and sites gated on `#[cfg(test)]` are
out of scope — they don't execute in the production render path.

## Findings

### `desktop/win/crates/midas-render/src/**`

**Zero panicking calls.** `grep` across the entire `midas-render` crate
returns no hits for any of the four panic operators. The pipelines
(candle, volume, grid, badge, text, sparkline) propagate wgpu errors
via typed enums and log-with-defaults on non-fatal paths. This is the
happy case — nothing to convert in slice 1 for the existing GPU
pipelines.

### `desktop/win/crates/midas-chart/src/**`

**Legacy crate — out of scope.** `midas-chart` is slated for deletion
in slice 9c. Any panicking call there is paid off by the deletion, not
a pre-migration conversion. Audit intentionally skipped.

### `desktop/win/crates/midas-app/src/session_chart/**`

Reviewed in-session (slice 1 authors should re-grep at implementation
time to catch churn):

| Site | Rationale |
|------|-----------|
| `widget.rs::AxisBox::for_calendar` — `.expect("widget must supply a valid time window")` (historical at draft time) | Already addressed in prior scrutiny fix — converted to `Result` on the public surface. Paint-time `.expect()`s on re-snapshot document the invariant via `unwrap_or_else(\|e\| panic!("...invariant broken: {e}"))` with a clear message; these trip only on programmer error (calendar inputs the driver validated at write-time). Slice 1's `catch_unwind` fallback-quad will surface them visibly without killing the process. |
| `primitives_bridge.rs` | No panicking calls in the translator; pure data transform. |
| `gpu_renderer.rs` | No panicking calls; iced shader `Program`/`Primitive` impls return `Option` / wgpu-native errors. |
| `shader.rs` | No panicking calls. |

### `desktop/win/crates/midas-core/src/candle_buffer/**`

All panicking calls are `debug_assert!` / `expect("…out of sync")` on
SoA column length invariants — these fire only if `push`/`apply`/
`update_last_price`/`merge_bar` are broken. They are the right shape
(loud bug-catcher on corruption) but are wrapped by slice 1's
`catch_unwind` at the render entry point, so a panic here produces the
fallback-quad, not a process crash.

## Slice 1 implications

`catch_unwind` around `SceneLayer::paint` (per plan line 120) covers
the entire realistic panic surface. No conversions required in
slice 0 beyond documenting this audit.

If future slices add render-path code that panics, slice 1's
subscriber-capture test (assertions on `tracing::error!("layer paint
panicked; emitting fallback")`) will catch the regression.
