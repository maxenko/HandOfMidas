# Slice 6 — Polish (micro-PRs)

**Goal:** Quality pass. Each item is independently optional. Ship a subset if scope is tight; each has its own done-when so partial delivery is unambiguous.

**Depends on:** S2 + S3.

Each item is sized as ~half a day. PR-per-item is encouraged.

---

## P0 — Baseline VP compute-time measurement (PREREQ for P1)

**Why:** P1 (cache) has Done-when criteria like "compute time drops by ≥ 80%" and a skip-condition "if recompute < 1ms per frame, drop P1". Both require a baseline. Without P0, P1's Done-when is unverifiable and the prioritization advice has no trigger.

**Approach:**
- Build a `criterion`-style benchmark in `crates/midas-scene/benches/volume_profile.rs` (and a sibling for the legacy stack in `desktop/win/crates/midas-chart/benches/volume_profile.rs`): paint VP for a 50-period scrollback fixture, all 4 anchor modes, on the dev machine.
- Run with default features (legacy stack) and `--features session_chart` (new stack).
- Record results in this file as a comment block (mean ± stddev per anchor mode per stack).
- Decision rule: if mean recompute time < 1ms per frame on the dev machine for ALL anchor modes, **drop P1 entirely** (mark as Won't Do in this file). Otherwise, P1 proceeds.

**Done when:**
- Benchmarks in repo and runnable via `cargo bench --bench volume_profile`.
- Baseline numbers committed in this file as a "P0 results" comment.
- Go/no-go decision on P1 documented.

---

## P1 — Cache completed-period profiles

**Why:** Only the rightmost in-progress period mutates on tick updates. Recomputing the other (immutable) periods every frame is wasteful.

**Approach:**
- Add `cache: HashMap<(PeriodKey, usize /*row_size*/), VolumeProfile>` to `VolumeProfileLayer` (new stack) and a parallel cache inside `compute_anchored_volume_profiles` (legacy stack).
- Cache key: `(period_start_unix_seconds, anchor, num_bins, instrument_id)`.
- Invalidate on series-replace (history reload) or candle-mutation in completed periods (rare; e.g., revision tick for a closed bar).
- Skip caching the trailing period (always recompute).

**Done when:**
- Cache hit-rate > 90% on a static viewport over a 50-period scrollback fixture.
- Per-frame VP compute time drops by ≥ 80% in benchmark.
- No visual regression vs reference PNGs.

**Skip if:** measurements show recompute < 1ms per frame on the dev machine.

---

## P2 — Narrow-period 1-pixel POC tick degradation

**Why:** Today's behaviour skips per-period rendering when `period_pixel_span < MIN_PERIOD_PX_TO_RENDER` (= 12). TradingView convention: degrade to a 1-pixel POC tick rather than vanishing.

**Approach:**
- In `paint()` (new stack) and `anchored_profiles_to_instances` (legacy), change the skip branch:
  - If `period_pixel_span < MIN_PERIOD_PX_TO_RENDER` AND `period_pixel_span >= 1`: emit one tiny `QuadInstance` at the POC y, width = 1 px, color = POC color. Skip all other bins.
- Constant `MIN_POC_TICK_PX = 1`.

**Done when:**
- Yearly anchor on 5-year viewport (very narrow per-month spans) shows visible POC ticks instead of gaps.
- Reference PNG `vp-narrow-poc-tick.png` committed.
- **Any S2/S3 reference PNG whose narrow-period region changed is regenerated** (likely none for the standard 3-day fixture; check before committing).
- `cargo clippy` clean.

---

## P3 — Hover tooltip on POC

**Why:** Discoverability — users want to know POC price + total volume of a period without squinting.

**Approach:**
- Add hit-test region: each per-period histogram emits a hover region covering its bins.
- On hover, render a tooltip via `midas_ui::Tooltip` (`desktop/win/crates/midas-ui/src/tooltip.rs:23`, re-exported from `midas-ui/src/lib.rs:36` per Recon R8). Hover surface is a per-period region, not a button — use the bare `Tooltip::new(content, "POC: $XX.XX | Vol: 1.2M | Period: 2025-01-15").position(Position::Top).view(&ui_theme)` form, NOT `IconButton::tooltip(...)`. (R8 reserves the icon-button builder for clickable toolbar buttons.)
- Hit-test priority: bracket drag handles WIN over VP hover (per Critique B 10b). Brackets register first; VP hover only fires if no bracket region claims the cursor.

**Done when:**
- Hover near a POC line shows the tooltip within 200ms.
- Tooltip dismisses on cursor-leave.
- Bracket drag on top of a VP region still works (no hover-tooltip interception).
- One screenshot test for tooltip-visible state.

---

## P4 — Value Area (VAH / VAL) rendering

**Why:** Reserved schema fields (`show_value_area`, `value_area_pct`) ship in S1 but render code ignores them in v1.

**Approach:**
- For each period, compute the value area: starting from POC, expand outward (alternating up/down) until cumulative volume ≥ `value_area_pct * total_volume`.
- Bins inside the value area render with a brighter shade (alpha bump: 0.45 → 0.65 for legacy, similar for new stack).
- Optional: thin horizontal lines at VAH and VAL prices (separate `LineInstance`s), reserved for a future polish item.
- UI: add "Show Value Area" checkbox to the gear popup (Slice 4's `build_vp_settings_panel`).

**Done when:**
- `show_value_area = true` produces visibly distinct VA bins per period.
- POC remains the brightest single bin.
- VA percentage tunable via popup checkbox; reference PNG committed.

---

## P5 — Up/Down split in the new stack

**Why:** Legacy has buy/sell split (`close >= open` heuristic); new stack is aggregate-only. Parity requires porting the split.

**Approach:**
- In `compute_bins_for_range`, track `(buy_volume, sell_volume)` per bin instead of total.
- Emit two `QuadInstance`s per non-zero bin: one buy (left), one sell (right of buy width).
- Style: extend `VolumeProfileStyle` with `buy_color`, `sell_color` (already exists in legacy; mirror).

**Done when:**
- New-stack screenshot matches legacy stack for the same fixture (cross-stack SSIM bumps from 0.85 to 0.90+).
- Reference PNG `vp-new-up-down-split.png` committed.
- **All S3-owned `vp-new-*.png` reference PNGs regenerated** (this change alters pixel output of every new-stack screenshot). Document regeneration in the PR.

---

## P6 — Full `collapse_gaps` per-period anchoring fix

**Why:** D11 silently disables per-period anchoring when `collapse_gaps == true` because `Camera2D::time_to_x` is linear-time and ignores collapse. Full fix uses the index→x transform from `compute_collapsed_scene`.

**Approach (in order — closure-lift is the bulk of the work):**

1. **Lift `index_to_x` from a closure to a callable helper** (~½ day, the bulk of P6). Verified location: `desktop/win/crates/midas-chart/src/compute/mod.rs:424` — `let index_to_x = |local_idx: usize| -> f32 { x_from_idx(vis_start + local_idx) };` is a local closure inside `compute_collapsed_scene`. Options:
   - Extract as a free function `fn index_to_x(local_idx: usize, vis_start: usize, x_from_idx: impl Fn(usize) -> f32) -> f32` that takes its closure inputs explicitly.
   - Move to a method on `Camera2D` if `x_from_idx` itself can be derived from camera state.
   Pick whichever produces the smaller diff in `compute_collapsed_scene` callers.
2. **Plumb the helper into `compute_anchored_volume_profiles`** (~1-2 hours) so per-period left-x can be computed via candle index, not raw timestamp.
3. **Remove the `!state.collapse_gaps` guard from `build_volume_profile`** — anchored mode is now safe in collapsed mode.
4. **Update D11/D13** consequences in the UI: remove the "Gap-collapse is on; per-period anchors disabled" note from the popup; restore the `VP·D/W/M/Y` toolbar suffix while collapse_gaps is on.

**Done when:**
- `Anchor = Daily` + `collapse_gaps = true` renders per-day profiles correctly aligned with the collapsed candle positions.
- Reference PNG `vp-collapse-gaps-daily-fixed.png` committed.
- **`vp-collapse-gaps-fallback.png` (originally committed by S5) is deleted** since the case is no longer a fallback. Update S5's owned-PNGs table or its README to reflect the removal.
- Popup note for collapse_gaps removed.
- Toolbar suffix shows in collapsed mode again.
- All S2/S3 reference PNGs whose pixel output changed (none expected; the fix is collapsed-mode-only) are regenerated.

---

## Item ordering / prioritisation

Recommended order if shipping a subset:
1. **P0 (baseline)** — required IFF P1 is in scope; otherwise skip. Gates P1's Done-when only.
2. **P5 (up/down split)** — biggest cross-stack parity win; smallest scope.
3. **P2 (narrow POC tick)** — tiny code change, big UX polish.
4. **P4 (value area)** — reserved schema fields ship rendering. Highest user-perceived value.
5. **P6 (collapse_gaps fix)** — removes the embarrassing popup note + restores toolbar suffix.
6. **P3 (hover tooltip)** — quality-of-life; depends on hit-test infra.
7. **P1 (cache)** — last; only if P0 measurements show recompute > 1ms/frame.

## Risks

- **Cache invalidation correctness (P1)** — getting "what counts as completed" right matters. A late tick correction that mutates an already-cached period must invalidate the entry. Add a test for revision-tick behaviour.
- **Hit-test conflicts (P3)** — coordinate with the bracket plan. If brackets are in flux, defer P3 until they stabilise.
- **VA computation edge cases (P4)** — periods with 0 volume, periods with one giant-volume bin, ties at the expansion frontier. Add unit tests for each.
- **Index→x function shape (P6)** — assumes the legacy collapsed scene exposes a callable function, not just inline math. Verify in P6 kickoff; if it's inline, factor first.
- **Up/down split changes total bar widths (P5)** — visual regression risk vs reference PNGs. Regenerate references for new-stack tests.
