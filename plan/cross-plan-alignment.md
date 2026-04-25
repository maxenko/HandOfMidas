# Cross-plan alignment — feature plans in flight

Three feature plans are landing during the same execution window. This
is a coordination doc so an agent picking up any one of them can see
what else is in motion without re-deriving the matrix.

This is **not** an implementation plan. It owns no slices, no tests, no
shipping behaviour. It owns the answers to "is this going to collide
with the other thing being worked on?".

If a NEW cross-plan touchpoint surfaces during execution, update this
file in the same commit so the next agent doesn't trip on it.

## In-flight plans

| Plan | Path | Scope (1-line) |
|---|---|---|
| ETH session shading | `plan/session-aware-charts/eth-shading.md` | Pre/post-market band overlay on the legacy chart. |
| Anchored Volume Profile | `plan/volume-profile-anchored/00-index.md` (+ 6 sub-files) | Per-chart anchored VP on legacy + new stacks. |
| Multi-window support | `plan/multi-window-support/README.md` | Arbitrary user-named windows; retire `floating_charts`. |

## Order independence

Any plan can land first. All cross-plan modifications are additive — no
plan blocks another, and no migration step in one plan rewrites a
struct another plan adds fields to.

Specifically:

- Multi-window's `AppConfig` v2→v3 migration **only** rewrites
  `LayoutNode::*` leaves (index→id) and inserts the `windows`
  BTreeMap. It does NOT touch nested `ChartConfig` fields and it does
  NOT touch top-level `[experimental]`. So ETH's
  `chart.show_extended_hours*` and VP's `chart.volume_profile` and
  `experimental.disable_anchored_vp` flow through unchanged regardless
  of order.
- ETH and VP both widen `ChartConfig` and `ChartInput` with disjoint
  nested keys / field names. Whichever plan's PR lands second is a
  trivial merge.

## Shared touchpoints

| File / area | Plans touching it | What each plan does |
|---|---|---|
| `midas-core/src/config/mod.rs` (`AppConfig`) | VP, multi-window | VP adds `experimental.disable_anchored_vp: bool`; multi-window bumps `version = 3`, adds `windows: BTreeMap<String, WindowConfig>`, retires `window` / `layout_tree` / `panel_order` to legacy-rename fields. **Disjoint sub-trees.** |
| `midas-core/src/config/mod.rs` (`ChartConfig`) | ETH, VP | ETH adds `show_extended_hours: bool`, `show_extended_hours_bands: bool`. VP adds `volume_profile: VolumeProfileSettings`. **Disjoint keys.** |
| `desktop/win/crates/midas-chart/src/input.rs` (`ChartInput`) | ETH, VP | ETH adds `show_extended_hours_bands: bool`, `bar_duration_ms: i64`, `pre_market_band_color: [f32; 4]`, `post_market_band_color: [f32; 4]`. VP adds `effective_vp_anchor`. Same per-frame builder; **additive**. |
| `MarketDataRouter::historical_bars` (`midas-market-data/src/router/mod.rs`) | ETH | New `use_rth: bool` parameter. Neither VP nor multi-window touches the router. |
| `desktop/win/crates/midas-app/Cargo.toml` | ETH | Promotes `midas-calendar`, `midas-bars`, `midas-bars-adapter` from `optional = true` to baseline. No-op for VP / multi-window. |
| `desktop/win/crates/midas-app/src/session_chart/scene_builder.rs` | VP, multi-window F2 (gated) | VP S3 wires `VolumeProfileLayer` for the first time and computes the kill-switch override. Multi-window F2 may fold `session_chart_window.rs` rendering into a `pane_grid` cell — `scene_builder` is unchanged either way, so VP S3's wiring composes regardless of F2's outcome. |
| `floating_charts`, `floating_session_charts` | multi-window F1, F2 | Deleted. VP's "Floating session-chart preset windows" non-goal becomes **moot** once F1/F2 land — those windows become regular named windows that inherit VP through the standard per-chart config path. |
| `desktop/win/crates/midas-devloop-proto/src/lib.rs` | all three | Multi-window adds `#[serde(default)] window: Option<String>` to most existing commands (default = main). VP adds `SetVpSettings` (slice 1). ETH adds a smoke script + fixture. See "Devloop convention" below. |
| `LayoutNode::Chart {chart_index}` → `{chart_id}` | multi-window v3 | Multi-window's leaf-rewrite is **invisible** to ETH and VP. Neither plan reads or writes `LayoutNode`. |

## Devloop convention

Multi-window's slice G adds an optional `window: Option<String>` field
to most existing harness commands. This is the canonical pattern for
window-aware harness drivers going forward.

- **If multi-window lands first**: VP's `SetVpSettings` and ETH's smoke
  scripts must include `#[serde(default)] window: Option<String>` from
  the start.
- **If VP or ETH lands first**: those devloop additions ship without
  the field. A follow-up commit (alongside or after multi-window's
  slice G) adds the field. VP S1's `SetVpSettings` is the most exposed —
  it targets a chart, and a chart can live in any window once
  multi-window lands.

The wire format is plain JSON, so adding `window` to a command later
is non-breaking for fixture files that omit it (the `#[serde(default)]`
covers the gap).

## Per-chart vs per-window state

The three plans agree on this distribution and don't fight over it:

- **Per-symbol**, shared across charts and windows: `TickerState`,
  `AnnotationStore`, `ChartViewStore`. (Multi-window confirms.)
- **Per-chart-instance** (per `ChartId`): VP settings (anchor, width,
  reserved value-area fields), ETH visibility flags
  (`show_extended_hours`, `show_extended_hours_bands`). Persisted on
  `ChartConfig`.
- **Per-window**: layout (pane grid), geometry, name. Persisted on
  `WindowConfig` (multi-window v3).
- **App-global**: broker connection, theme, status bar, recent
  symbols, VP global kill-switch (`experimental.disable_anchored_vp`).

A single ChartId can move between windows (multi-window slice E and
the existing pop-out path); its VP and ETH settings travel with it
because they're keyed by `ChartId`, not by window.

## Open coordination items

(Empty for now. Add an entry the moment a real cross-plan question
surfaces during slice execution — e.g., "VP S1 needs a new
`ChartInput` field that conflicts with an ETH name", "multi-window F2
broke VP S3's calendar lookup", etc. Each entry should name an owning
plan and a resolution date.)
