# Slice 5 — Cross-Stack Visual Parity Tests

**Goal:** Devloop screenshot tests that verify the **legacy** and **new** stacks produce visually consistent output for the same fixture + same anchor mode. Also covers fallback cases (D11 collapse_gaps, D12 anchor-too-fine).

**Per-stack reference PNGs are owned by S2 and S3** (see their Done-when criteria). S5 reuses them; S5 does NOT own or regenerate them. S5 owns only the cross-stack parity PNGs and the fallback-case PNGs.

**Depends on:** S2 + S3 (both render paths must work; their reference PNGs must exist).

## Files to modify

- `.github/workflows/rust.yml` — confirm both feature-gate combos (`default` and `--features session_chart`) run in CI on Windows. If devloop scripts are part of CI today (verify with `grep "devloop" .github/workflows/`), add the new scripts. If devloop is manual-only, document and skip.

## Files to create

### Fixtures (in `desktop/win/tests/data/` — verify exact fixture dir with `Glob "desktop/win/tests/data/*.json"` during slice kickoff)

- `vp_daily_aapl_5m_3days.json` — AAPL 5-minute candles spanning 3 NYSE trading days (well-defined POCs per day; spans NYSE close 16:00 ET so the boundary detector is exercised).
- `vp_weekly_spy_d1_3months.json` — SPY daily candles spanning ~12 weeks (for weekly anchor).
- `vp_monthly_spy_d1_2years.json` — SPY daily, 2 years (for monthly anchor).
- `vp_yearly_spy_d1_5years.json` — SPY daily, 5 years (for yearly anchor).
- `vp_collapse_gaps_aapl_5m_3days.json` — same as the daily fixture but `collapse_gaps = true` in the saved config (covers D11 fallback).
- `vp_anchor_too_fine_spy_d1.json` — SPY daily candles + `anchor = Daily` (covers D12 fallback).

### Reference PNGs owned by S5 (in `desktop/win/tests/data/refs/`)

Only **2** PNGs (the per-stack PNGs are owned by S2 and S3):

| File | Stack | Anchor | Source fixture | Notes |
|------|-------|--------|----------------|-------|
| `vp-collapse-gaps-fallback.png` | Legacy | Daily (with gap-collapse) | `vp_collapse_gaps_aapl_5m_3days` | Verifies D11 silent fallback to Viewport while gap-collapse is on. Should be visually identical to `vp-legacy-viewport.png` (owned by S2). |
| `vp-anchor-too-fine.png` | Either | Daily on D1 TF | `vp_anchor_too_fine_spy_d1` | Verifies D12 silent fallback. Should be visually identical to the corresponding stack's `*-viewport.png`. |

The cross-stack parity check (`devloop-vp-cross-stack-parity.sh`) **does not commit a third reference PNG** — it diffs the live legacy screenshot against the live new screenshot directly (SSIM ≥ 0.85, plus per-day POC y-position within 5 pixels).

### Devloop scripts (in `desktop/win/tools/`)

- `devloop-vp-anchored-legacy.sh` — **owned and shipped in full by S2**, including the per-anchor sweep across all four anchor modes. S5 does not modify it.
- `devloop-vp-anchored-new.sh` — **owned and shipped in full by S3**. S5 does not modify it.
- `devloop-vp-cross-stack-parity.sh` — **new in this slice**. Boots the daily fixture, captures Legacy/Daily, flips backend to New, captures New/Daily, asserts both produce a profile per day with overlapping POC bands. Tolerance: SSIM ≥ 0.85, per-day POC y-position within 5 pixels.
- `devloop-vp-fallback-cases.sh` — **new in this slice**. Covers D11 (collapse_gaps) + D12 (anchor too fine), captures the two reference PNGs above.

## Key implementation details

### Determinism (critical for screenshot stability)

- **Fixed window size:** every script invokes `cargo run -p midas-app --features dev_harness` with a `MIDAS_DEVLOOP_WINDOW_SIZE=1280x720` env var (or whatever the existing devloop convention is — verify in `tools/devloop-smoke.sh`). If no convention exists, add it via a new devloop fixture field or env var.
- **Force DPI = 100%:** Windows 11 sometimes scales windows. The fixture sets `app.window_dpi_override = 1.0` (or whatever the existing fixture field allows). If absent, document the limitation.
- **`WaitForIdle` to drain animations:** before every `Screenshot`, `midas-devloop-cli WaitForIdle --timeout-ms 500`. The camera lerp on ticker switch (and the scroll wheel inertia) takes ~300ms.
- **Disable cursor blink and other timed effects:** if any UI elements animate (tooltip fade-in, hover highlight), pick screenshots from a "clean" frame after the cursor has moved off-chart.

### SSIM threshold

- **Per-stack tests (S2/S3):** SSIM ≥ 0.98 (near-exact pixel match against the stack's own reference).
- **Cross-stack parity (this slice):** SSIM ≥ 0.85 — the two stacks render with different pipelines, fonts, and color palettes; we're checking *structural* similarity, not pixel equality. Per-period count, POC y-positions, and rough histogram shapes should match.

### Backend-flip script — what to assert

`devloop-vp-cross-stack-parity.sh`:
1. Boot daily fixture with `backend = Legacy`, `anchor = Daily`.
2. `WaitForIdle`, `Screenshot legacy.png`.
3. `ToggleChartBackend chart_id=0` → backend now `New`.
4. `WaitForIdle`, `Screenshot new.png`.
5. Diff `legacy.png` vs `new.png` with SSIM ≥ 0.85.
6. Also compute a per-day POC y-position from each PNG (extract the bright POC band) and assert the per-day positions are within 5 pixels of each other.

(POC extraction can be a small Python helper invoked from the script — same pattern existing devloop scripts use for diff analysis. Verify with `cat tools/devloop-smoke.sh`.)

### Fixture format

Verify with an existing fixture file (`Glob "desktop/win/tests/data/fixtures/*.json"`). Likely shape: a JSON with `{ symbols: [...], candles: [...], chart_config: {...} }`. New fixtures must include `volume_profile = { anchor = "Daily", ... }` in the embedded `chart_config` so the chart loads with the right anchor without needing devloop `SetVpSettings` (faster, simpler).

## Testing

This slice IS the testing.

### Acceptance commands

```bash
# Each script returns 0 on success
bash tools/devloop-vp-anchored-legacy.sh
bash tools/devloop-vp-anchored-new.sh
bash tools/devloop-vp-cross-stack-parity.sh
bash tools/devloop-vp-fallback-cases.sh
```

### Reference-PNG generation procedure

Document in a comment at the top of each script:
```
# To regenerate references after a deliberate visual change:
#   MIDAS_DEVLOOP_REGENERATE=1 bash tools/devloop-vp-anchored-legacy.sh
# This overwrites the reference PNGs without running diffs.
# Commit the new PNGs and document why in the PR.
```

## Done when

- All four scripts run green locally on the developer machine (Windows 11, x86_64, primary monitor 100% DPI).
- The two S5-owned reference PNGs (`vp-collapse-gaps-fallback.png`, `vp-anchor-too-fine.png`) committed under `desktop/win/tests/data/refs/`.
- The ten per-stack PNGs committed by S2 (5) and S3 (5) are in place and unmodified by this slice.
- Cross-stack parity script asserts SSIM ≥ 0.85 + per-day POC y-position within 5 pixels.
- Fallback scripts assert collapse_gaps + Daily anchor produces the same screenshot as Viewport mode (D11), and Daily-on-D1 produces the same as Viewport (D12).
- `.github/workflows/rust.yml` either runs the scripts or has them documented as manual gates.

## Risks

- **S6 P6 will replace `vp-collapse-gaps-fallback.png`** — once the full collapse_gaps fix lands (S6 P6), this slice's `vp-collapse-gaps-fallback.png` becomes obsolete (the case stops being a fallback). P6's Done-when explicitly handles the cleanup; S5 reviewers should expect this PNG to be deleted/renamed when P6 ships, not flag it as a regression.
- **Reference PNG drift on driver/font updates** — Windows update can ship a new font version; expect occasional reference regeneration. The "regenerate" env var procedure makes this routine.
- **GPU non-determinism** — accepted via SSIM ≥ 0.98 threshold for per-stack tests, ≥ 0.85 for cross-stack.
- **Headless CI** — devloop requires a real GPU and window manager. If CI is headless, devloop scripts run only manually pre-PR. Document.
- **Reference PNG repo size** — twelve PNGs at maybe 50KB each ≈ 600KB. Acceptable; existing `tests/data/refs/` already carries similar weight.
- **Devloop CLI argument shape** — verify `Click`, `WaitForIdle`, `Screenshot` argv match the existing convention (`tools/devloop-smoke.sh`). If `Click --target <name>` is missing, fall back to `--x --y` against the fixed window size.
- **`MIDAS_DEVLOOP_WINDOW_SIZE` env var** — if no such convention exists, add a fixture-config field instead. Either way, every screenshot test runs at the same window size.
- **POC extraction Python helper** — if Python isn't on the dev/CI machine, drop the per-day POC check and rely on SSIM alone. (Slightly weaker test; acceptable.)
