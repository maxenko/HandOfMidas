//! Versioned config migrations.
//!
//! Each `migrate_v{N}_to_v{N+1}` function is a pure transform over
//! [`AppConfig`] that bumps `cfg.version` from `N` to `N+1`. The
//! framework entry point [`migrate_to_current`] chains them in
//! order until `cfg.version == CURRENT_CONFIG_VERSION`, returning
//! the labels of every step that ran so the caller can log /
//! backup once.
//!
//! A migration that doesn't apply (e.g. nothing to translate)
//! still bumps the version — the version field tracks "this config
//! has been considered for the v_n → v_{n+1} transform", not "the
//! transform changed something". That keeps the chain monotonic and
//! avoids a forever-running migration.
//!
//! Adding a new migration:
//! 1. Bump [`super::CURRENT_CONFIG_VERSION`].
//! 2. Add `migrate_v{N}_to_v{N+1}` here that mutates `cfg` in place.
//! 3. Append a step to [`migrate_to_current`] dispatching on the
//!    pre-step version.

use super::{
    AccountPanelConfig, AccountTab, AppConfig, LayoutNode, OrdersTabConfig, PanelSlot,
    WindowConfig, WindowGeometryConfig, CURRENT_CONFIG_VERSION,
};
use crate::window_key::WindowKey;

/// Walk `cfg` forward through the migration chain until it reaches
/// [`CURRENT_CONFIG_VERSION`]. Returns the labels of every step
/// that ran, in order — empty when the config was already current.
///
/// Each step bumps `cfg.version` regardless of whether the
/// transform changed anything, so a v1 file with no
/// `order_blotters` still ends up versioned `v2` after load.
pub fn migrate_to_current(cfg: &mut AppConfig) -> Vec<&'static str> {
    let mut steps: Vec<&'static str> = Vec::new();
    while cfg.version < CURRENT_CONFIG_VERSION {
        match cfg.version {
            1 => {
                migrate_v1_to_v2(cfg);
                steps.push("v1→v2 (order_blotters → account_panels)");
            }
            2 => {
                migrate_v2_to_v3(cfg);
                steps.push("v2→v3 (single window → windows[] map; layout indices → ids)");
            }
            // Forward-compat: a config from a *newer* version of
            // the app would have `version > CURRENT_CONFIG_VERSION`
            // and the loop guard above already excludes it. An
            // unknown intermediate version means the framework is
            // missing a step — bail without further mutation rather
            // than spin.
            other => {
                tracing::warn!(
                    "No migration registered from config v{other} → v{}; leaving config alone",
                    other + 1
                );
                break;
            }
        }
    }
    steps
}

/// v1 → v2: legacy `order_blotters` list → `account_panels` with
/// `active_tab = Orders`. See
/// [`migrate_order_blotters_to_account_panels`] for the body; this
/// wrapper bumps `cfg.version` whether or not anything migrated.
fn migrate_v1_to_v2(cfg: &mut AppConfig) {
    migrate_order_blotters_to_account_panels(cfg);
    cfg.version = 2;
}

/// Translate legacy `order_blotters` entries into the new
/// `account_panels` list with `active_tab = Orders`.
///
/// The function also rewrites `legacy_panel_order` and
/// `legacy_layout_tree` so that any
/// `OrderBlotter { order_blotter_index }` slot is remapped to an
/// `Account { account_panel_id }` slot pointing at the newly
/// appended account panel. Index preservation is done by appending
/// blotter `N` at position `len(account_panels_before) + N`, then
/// rewriting every reference through the same offset.
///
/// Returns the number of entries migrated. The function is a no-op
/// when `order_blotters` is empty, so re-running it on an
/// already-migrated config is safe and reports `0`.
pub fn migrate_order_blotters_to_account_panels(cfg: &mut AppConfig) -> usize {
    if cfg.order_blotters.is_empty() {
        return 0;
    }

    // The appended block starts at the current length so existing
    // account_panels (e.g. migrated on a previous run) aren't disturbed.
    let base = cfg.account_panels.len();

    for blotter in cfg.order_blotters.drain(..) {
        cfg.account_panels.push(AccountPanelConfig {
            id: 0,
            // Rename generic "Orders" → "Account" but keep any user-set
            // name. Matches the new button label and keeps customisations.
            name: if blotter.name == "Orders" {
                "Account".to_string()
            } else {
                blotter.name
            },
            active_tab: AccountTab::Orders,
            orders: OrdersTabConfig {
                column_widths: blotter.column_widths,
                symbol_link: blotter.symbol_link,
                hidden_columns: blotter.hidden_columns,
            },
        });
    }

    // Rewrite legacy_panel_order references. PanelSlot is index-based
    // even in v3 (it's only used as v1/v2 input), so we just remap
    // the index within the same enum.
    for slot in cfg.legacy_panel_order.iter_mut() {
        if let PanelSlot::OrderBlotter {
            order_blotter_index,
        } = *slot
        {
            *slot = PanelSlot::Account {
                account_panel_index: base + order_blotter_index,
            };
        }
    }

    // Rewrite legacy_layout_tree references. v1/v2's
    // `account_panel_index: usize` is read into the v3
    // `account_panel_id: u32` field via `serde(alias)`; here we
    // materialise the rewrite as a u32. Indices are bounded by the
    // panel-pool length so the cast is lossless in practice.
    for node in cfg.legacy_layout_tree.iter_mut() {
        if let LayoutNode::OrderBlotter {
            order_blotter_index,
        } = *node
        {
            *node = LayoutNode::Account {
                account_panel_id: (base + order_blotter_index) as u32,
            };
        }
    }

    cfg.account_panels.len() - base
}

/// v2 → v3: single-window layout → per-window `windows` map; layout
/// leaf references switch from positional indices to stable ids.
///
/// The transform is structural-only — no fields move into or out of
/// nested config types like `ChartConfig` or `[experimental]`. ETH
/// and VP additions in those nested locations pass through cleanly
/// regardless of which feature plan lands first
/// (`plan/cross-plan-alignment.md`).
///
/// The transform:
///
/// 1. Assigns `id = position_in_vec` to every panel-pool entry that
///    carries `id == 0` (v1/v2 had no `id` field, so all loaded
///    entries default to 0). After this step every panel has a
///    stable identifier independent of vec position.
/// 2. Rewrites `legacy_layout_tree` leaves: each `chart_id` (which
///    `serde(alias = "chart_index")` brought in as the legacy index
///    value) is overwritten with the chart-at-that-index's freshly-
///    assigned id. Since step 1 assigned id == position_in_vec for
///    fresh ids, this rewrite is a no-op for v2-migrated configs but
///    is correct in the general case where a future migration might
///    reorder the panel pool.
/// 3. If `legacy_layout_tree` is empty but `legacy_panel_order` is
///    not, synthesise a vertical chain of splits from the slot
///    order. This preserves the v1/v2 fallback restoration path.
/// 4. Drains `legacy_window` into the new `windows["Main"].geometry`.
///    Inserts a single entry keyed by [`WindowKey::MAIN_DEFAULT`]
///    flagged `is_main: true`.
fn migrate_v2_to_v3(cfg: &mut AppConfig) {
    // 1. Stamp ids on panel pools.
    let mut next_chart_id: u32 = 0;
    for c in cfg.charts.iter_mut() {
        if c.id == 0 {
            c.id = next_chart_id;
        }
        next_chart_id = next_chart_id.max(c.id) + 1;
    }
    let mut next_wl_id: u32 = 0;
    for wl in cfg.watchlists.iter_mut() {
        if wl.id == 0 {
            wl.id = next_wl_id;
        }
        next_wl_id = next_wl_id.max(wl.id) + 1;
    }
    let mut next_op_id: u32 = 0;
    for op in cfg.order_panels.iter_mut() {
        if op.id == 0 {
            op.id = next_op_id;
        }
        next_op_id = next_op_id.max(op.id) + 1;
    }
    let mut next_ap_id: u32 = 0;
    for ap in cfg.account_panels.iter_mut() {
        if ap.id == 0 {
            ap.id = next_ap_id;
        }
        next_ap_id = next_ap_id.max(ap.id) + 1;
    }

    // 2. Rewrite legacy_layout_tree leaves: index → id.
    for node in cfg.legacy_layout_tree.iter_mut() {
        match node {
            LayoutNode::Chart { chart_id } => {
                if let Some(c) = cfg.charts.get(*chart_id as usize) {
                    *chart_id = c.id;
                }
            }
            LayoutNode::Watchlist { watchlist_id } => {
                if let Some(wl) = cfg.watchlists.get(*watchlist_id as usize) {
                    *watchlist_id = wl.id;
                }
            }
            LayoutNode::OrderPanel { order_panel_id } => {
                if let Some(op) = cfg.order_panels.get(*order_panel_id as usize) {
                    *order_panel_id = op.id;
                }
            }
            LayoutNode::Account { account_panel_id } => {
                if let Some(ap) = cfg.account_panels.get(*account_panel_id as usize) {
                    *account_panel_id = ap.id;
                }
            }
            LayoutNode::Split { .. } | LayoutNode::OrderBlotter { .. } | LayoutNode::Unknown => {}
        }
    }

    // 3. If legacy_layout_tree is empty, synthesise from
    //    legacy_panel_order so v1/v2 configs that only ever wrote
    //    panel_order still restore meaningful layouts.
    let mut layout_tree = std::mem::take(&mut cfg.legacy_layout_tree);
    if layout_tree.is_empty() && !cfg.legacy_panel_order.is_empty() {
        layout_tree = synthesise_from_panel_order(cfg);
    }
    // Whether or not we synthesised, drain the legacy slot list — we
    // either consumed it or it's redundant with layout_tree.
    cfg.legacy_panel_order.clear();

    // 4. Drain legacy_window into windows["Main"].geometry.
    let geometry = cfg
        .legacy_window
        .take()
        .unwrap_or_else(default_window_geometry);

    cfg.windows.insert(
        WindowKey::MAIN_DEFAULT.to_string(),
        WindowConfig {
            is_main: true,
            geometry,
            layout_tree,
        },
    );

    cfg.version = 3;
}

/// Build a vertical-split chain layout from `legacy_panel_order`.
/// Each slot becomes a leaf; consecutive slots are joined by
/// `Split { vertical, 0.5 }`, preserving the legacy "open-and-split-
/// vertically" restore behaviour from `app::MidasApp::new`.
///
/// Slots referencing missing panels are skipped so a corrupted
/// `panel_order` (e.g. a deleted chart whose entry survived) doesn't
/// take down the whole layout.
fn synthesise_from_panel_order(cfg: &AppConfig) -> Vec<LayoutNode> {
    let mut leaves: Vec<LayoutNode> = Vec::new();
    for slot in &cfg.legacy_panel_order {
        match *slot {
            PanelSlot::Chart { chart_index } => {
                if let Some(c) = cfg.charts.get(chart_index) {
                    leaves.push(LayoutNode::Chart { chart_id: c.id });
                }
            }
            PanelSlot::Watchlist { watchlist_index } => {
                if let Some(wl) = cfg.watchlists.get(watchlist_index) {
                    leaves.push(LayoutNode::Watchlist {
                        watchlist_id: wl.id,
                    });
                }
            }
            PanelSlot::OrderPanel { order_panel_index } => {
                if let Some(op) = cfg.order_panels.get(order_panel_index) {
                    leaves.push(LayoutNode::OrderPanel {
                        order_panel_id: op.id,
                    });
                }
            }
            PanelSlot::Account {
                account_panel_index,
            } => {
                if let Some(ap) = cfg.account_panels.get(account_panel_index) {
                    leaves.push(LayoutNode::Account {
                        account_panel_id: ap.id,
                    });
                }
            }
            // OrderBlotter slots should already have been rewritten
            // to Account by the v1→v2 step. Anything still present
            // here is a corrupted config; skip rather than crash.
            PanelSlot::OrderBlotter { .. } | PanelSlot::Unknown => {}
        }
    }
    if leaves.is_empty() {
        return Vec::new();
    }
    if leaves.len() == 1 {
        return leaves;
    }
    // Pre-order traversal of a left-leaning vertical chain:
    //   Split, Leaf0, Split, Leaf1, ..., Split, Leaf{n-2}, Leaf{n-1}
    let mut tree: Vec<LayoutNode> = Vec::with_capacity(leaves.len() * 2 - 1);
    let mut iter = leaves.into_iter();
    let last = iter.next_back().expect("non-empty");
    for leaf in iter {
        tree.push(LayoutNode::Split {
            axis: "vertical".to_string(),
            ratio: 0.5,
        });
        tree.push(leaf);
    }
    tree.push(last);
    tree
}

fn default_window_geometry() -> WindowGeometryConfig {
    WindowGeometryConfig {
        width: 1280,
        height: 800,
        ..Default::default()
    }
}

/// Idempotent post-migration validation. Runs unconditionally on
/// every load (after the version chain has finished) so hand-edits
/// don't slip past.
///
/// - If `windows` is empty, synthesises a default `Main` entry.
/// - If no entry has `is_main: true`, promotes the first by BTreeMap
///   key order.
/// - If more than one entry has `is_main: true`, keeps the first by
///   key order and demotes the rest with a warning.
/// - Drops `LayoutNode::Chart`/`Watchlist`/`OrderPanel`/`Account`
///   leaves that reference an id no longer present in the panel pool
///   (defensive against deleted-config-entry hand edits).
pub fn validate(cfg: &mut AppConfig) {
    if cfg.windows.is_empty() {
        cfg.windows.insert(
            WindowKey::MAIN_DEFAULT.to_string(),
            WindowConfig {
                is_main: true,
                geometry: default_window_geometry(),
                layout_tree: Vec::new(),
            },
        );
    }

    let main_count = cfg.windows.values().filter(|w| w.is_main).count();
    if main_count == 0 {
        if let Some((_, first)) = cfg.windows.iter_mut().next() {
            tracing::warn!("No window had is_main=true after migration; promoting first entry");
            first.is_main = true;
        }
    } else if main_count > 1 {
        let mut kept = false;
        for (_, w) in cfg.windows.iter_mut() {
            if w.is_main {
                if kept {
                    tracing::warn!(
                        "Multiple is_main=true windows after migration; demoting all but first"
                    );
                    w.is_main = false;
                } else {
                    kept = true;
                }
            }
        }
    }

    let chart_ids: std::collections::HashSet<u32> = cfg.charts.iter().map(|c| c.id).collect();
    let wl_ids: std::collections::HashSet<u32> = cfg.watchlists.iter().map(|w| w.id).collect();
    let op_ids: std::collections::HashSet<u32> = cfg.order_panels.iter().map(|o| o.id).collect();
    let ap_ids: std::collections::HashSet<u32> = cfg.account_panels.iter().map(|a| a.id).collect();

    for (_key, w) in cfg.windows.iter_mut() {
        w.layout_tree.retain(|node| match node {
            LayoutNode::Chart { chart_id } => chart_ids.contains(chart_id),
            LayoutNode::Watchlist { watchlist_id } => wl_ids.contains(watchlist_id),
            LayoutNode::OrderPanel { order_panel_id } => op_ids.contains(order_panel_id),
            LayoutNode::Account { account_panel_id } => ap_ids.contains(account_panel_id),
            // Splits stay; OrderBlotter shouldn't survive past v1→v2;
            // Unknown stays (forward-compat).
            LayoutNode::Split { .. } | LayoutNode::OrderBlotter { .. } | LayoutNode::Unknown => {
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AccountTab, ChartConfig, OrderBlotterConfig, OrderPanelConfig, VolumeProfileSettings,
        WatchlistConfig,
    };
    use crate::link::LinkMode;

    #[test]
    fn no_op_when_order_blotters_empty() {
        let mut cfg = AppConfig::default();
        let migrated = migrate_order_blotters_to_account_panels(&mut cfg);
        assert_eq!(migrated, 0);
        assert!(cfg.account_panels.is_empty());
    }

    #[test]
    fn single_blotter_round_trips_to_account_panel() {
        let mut cfg = AppConfig::default();
        cfg.order_blotters.push(OrderBlotterConfig {
            name: "Orders".to_string(),
            column_widths: vec![80.0, 60.0, 120.0],
            symbol_link: LinkMode::ListenAll,
            hidden_columns: vec!["tp".to_string()],
        });
        cfg.legacy_panel_order.push(PanelSlot::OrderBlotter {
            order_blotter_index: 0,
        });
        cfg.legacy_layout_tree.push(LayoutNode::OrderBlotter {
            order_blotter_index: 0,
        });

        let n = migrate_order_blotters_to_account_panels(&mut cfg);

        assert_eq!(n, 1);
        assert!(cfg.order_blotters.is_empty());
        assert_eq!(cfg.account_panels.len(), 1);
        let ap = &cfg.account_panels[0];
        // Renamed from the default legacy "Orders" label.
        assert_eq!(ap.name, "Account");
        assert_eq!(ap.active_tab, AccountTab::Orders);
        assert_eq!(ap.orders.column_widths, vec![80.0, 60.0, 120.0]);
        assert_eq!(ap.orders.symbol_link, LinkMode::ListenAll);
        assert_eq!(ap.orders.hidden_columns, vec!["tp".to_string()]);

        // Slot / layout node rewritten.
        match &cfg.legacy_panel_order[0] {
            PanelSlot::Account {
                account_panel_index,
            } => assert_eq!(*account_panel_index, 0),
            other => panic!("legacy_panel_order not rewritten: {other:?}"),
        }
        match &cfg.legacy_layout_tree[0] {
            LayoutNode::Account { account_panel_id } => assert_eq!(*account_panel_id, 0),
            other => panic!("legacy_layout_tree not rewritten: {other:?}"),
        }
    }

    #[test]
    fn custom_blotter_name_is_preserved() {
        let mut cfg = AppConfig::default();
        cfg.order_blotters.push(OrderBlotterConfig {
            name: "Legacy Blotter".to_string(),
            column_widths: vec![],
            symbol_link: LinkMode::default(),
            hidden_columns: vec![],
        });

        migrate_order_blotters_to_account_panels(&mut cfg);
        assert_eq!(cfg.account_panels[0].name, "Legacy Blotter");
    }

    #[test]
    fn two_blotters_preserve_relative_order() {
        let mut cfg = AppConfig::default();
        cfg.order_blotters.push(OrderBlotterConfig {
            name: "A".into(),
            ..Default::default()
        });
        cfg.order_blotters.push(OrderBlotterConfig {
            name: "B".into(),
            ..Default::default()
        });
        cfg.legacy_panel_order.push(PanelSlot::OrderBlotter {
            order_blotter_index: 1,
        });
        cfg.legacy_panel_order.push(PanelSlot::OrderBlotter {
            order_blotter_index: 0,
        });

        migrate_order_blotters_to_account_panels(&mut cfg);

        assert_eq!(cfg.account_panels.len(), 2);
        assert_eq!(cfg.account_panels[0].name, "A");
        assert_eq!(cfg.account_panels[1].name, "B");
        assert!(matches!(
            cfg.legacy_panel_order[0],
            PanelSlot::Account {
                account_panel_index: 1
            }
        ));
        assert!(matches!(
            cfg.legacy_panel_order[1],
            PanelSlot::Account {
                account_panel_index: 0
            }
        ));
    }

    #[test]
    fn idempotent_second_run_is_no_op() {
        let mut cfg = AppConfig::default();
        cfg.order_blotters.push(OrderBlotterConfig::default());
        let first = migrate_order_blotters_to_account_panels(&mut cfg);
        assert_eq!(first, 1);
        let second = migrate_order_blotters_to_account_panels(&mut cfg);
        assert_eq!(second, 0);
        assert_eq!(cfg.account_panels.len(), 1);
    }

    // ── Framework tests ────────────────────────────────────────────

    #[test]
    fn migrate_to_current_no_op_on_current_config() {
        let mut cfg = AppConfig::default();
        // Default starts at CURRENT_CONFIG_VERSION; framework must
        // be a no-op.
        let steps = migrate_to_current(&mut cfg);
        assert!(steps.is_empty());
        assert_eq!(cfg.version, CURRENT_CONFIG_VERSION);
    }

    #[test]
    fn migrate_to_current_walks_v1_to_current() {
        // Start from a fresh-default cfg but then strip the v3
        // structural state so the chain has work to do from v1.
        let mut cfg = AppConfig {
            version: 1,
            windows: std::collections::BTreeMap::new(),
            ..AppConfig::default()
        };
        cfg.order_blotters.push(OrderBlotterConfig::default());
        let steps = migrate_to_current(&mut cfg);
        assert!(!steps.is_empty());
        assert!(steps[0].starts_with("v1→v2"));
        assert_eq!(cfg.version, CURRENT_CONFIG_VERSION);
        assert_eq!(cfg.account_panels.len(), 1);
        assert!(cfg.order_blotters.is_empty());
        // v3 step also synthesised the Main window entry.
        assert!(cfg.windows.contains_key(WindowKey::MAIN_DEFAULT));
        assert!(cfg.windows[WindowKey::MAIN_DEFAULT].is_main);
    }

    #[test]
    fn migrate_to_current_bumps_version_even_with_nothing_to_translate() {
        // v1 file without legacy `order_blotters` still walks
        // forward — version is "considered for migration", not
        // "transformed by migration".
        let mut cfg = AppConfig {
            version: 1,
            windows: std::collections::BTreeMap::new(),
            ..AppConfig::default()
        };
        let steps = migrate_to_current(&mut cfg);
        // 2 steps from v1 → v3.
        assert_eq!(steps.len(), 2);
        assert_eq!(cfg.version, CURRENT_CONFIG_VERSION);
    }

    // ── v2 → v3 migration tests ────────────────────────────────────

    /// Helper: fresh v2 config with no windows[] entries (the v3
    /// shape introduced in slice B), so the migration has work.
    fn fresh_v2() -> AppConfig {
        AppConfig {
            version: 2,
            windows: std::collections::BTreeMap::new(),
            ..AppConfig::default()
        }
    }

    #[test]
    fn v2_to_v3_assigns_ids_to_charts() {
        let mut cfg = fresh_v2();
        cfg.charts.push(ChartConfig {
            id: 0,
            symbol: "AAPL".into(),
            timeframe: "1D".into(),
            ..ChartConfig {
                id: 0,
                symbol: String::new(),
                timeframe: String::new(),
                levels: vec![],
                camera_time_start: None,
                camera_time_end: None,
                camera_price_low: None,
                camera_price_high: None,
                collapse_gaps: false,
                timeline_border_ratio: 0.20,
                volume_scale: 1.0,
                show_volume_profile: false,
                show_levels: true,
                viewport_width: None,
                viewport_height: None,
                symbol_link: LinkMode::default(),
                timeframe_link: LinkMode::default(),
                bound_symbol: None,
                backend: None,
                show_extended_hours: true,
                show_extended_hours_bands: true,
                volume_profile: VolumeProfileSettings::default(),
            }
        });
        cfg.charts.push(ChartConfig {
            id: 0,
            symbol: "TSLA".into(),
            timeframe: "5m".into(),
            levels: vec![],
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            collapse_gaps: false,
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            show_levels: true,
            viewport_width: None,
            viewport_height: None,
            symbol_link: LinkMode::default(),
            timeframe_link: LinkMode::default(),
            bound_symbol: None,
            backend: None,
            show_extended_hours: true,
            show_extended_hours_bands: true,
            volume_profile: VolumeProfileSettings::default(),
        });

        migrate_v2_to_v3(&mut cfg);

        assert_eq!(cfg.version, 3);
        // First chart keeps id=0 (its initial value); second gets 1.
        assert_eq!(cfg.charts[0].id, 0);
        assert_eq!(cfg.charts[1].id, 1);
    }

    #[test]
    fn v2_to_v3_rewrites_layout_indices_to_ids() {
        let mut cfg = fresh_v2();
        cfg.charts.push(ChartConfig {
            id: 0,
            symbol: "AAPL".into(),
            timeframe: "1D".into(),
            levels: vec![],
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            collapse_gaps: false,
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            show_levels: true,
            viewport_width: None,
            viewport_height: None,
            symbol_link: LinkMode::default(),
            timeframe_link: LinkMode::default(),
            bound_symbol: None,
            backend: None,
            show_extended_hours: true,
            show_extended_hours_bands: true,
            volume_profile: VolumeProfileSettings::default(),
        });
        // Legacy layout: a single Chart leaf with chart_index = 0
        // (deserialised via serde alias into chart_id field).
        cfg.legacy_layout_tree
            .push(LayoutNode::Chart { chart_id: 0 });

        migrate_v2_to_v3(&mut cfg);

        let main = cfg
            .windows
            .get(WindowKey::MAIN_DEFAULT)
            .expect("Main window inserted");
        assert!(main.is_main);
        assert_eq!(main.layout_tree.len(), 1);
        match &main.layout_tree[0] {
            LayoutNode::Chart { chart_id } => assert_eq!(*chart_id, cfg.charts[0].id),
            other => panic!("expected Chart leaf, got {other:?}"),
        }
        // Legacy fields drained.
        assert!(cfg.legacy_layout_tree.is_empty());
        assert!(cfg.legacy_window.is_none());
    }

    #[test]
    fn v2_to_v3_drains_legacy_window_geometry() {
        let mut cfg = fresh_v2();
        cfg.legacy_window = Some(WindowGeometryConfig {
            width: 1920,
            height: 1200,
            maximized: true,
            x: Some(100),
            y: Some(50),
            monitor_width: Some(2560),
            monitor_height: Some(1440),
        });

        migrate_v2_to_v3(&mut cfg);

        let main = cfg.windows.get(WindowKey::MAIN_DEFAULT).expect("Main");
        assert_eq!(main.geometry.width, 1920);
        assert_eq!(main.geometry.height, 1200);
        assert!(main.geometry.maximized);
        assert_eq!(main.geometry.x, Some(100));
        assert_eq!(main.geometry.monitor_width, Some(2560));
        assert!(cfg.legacy_window.is_none());
    }

    #[test]
    fn v2_to_v3_synthesises_layout_from_panel_order_when_layout_tree_absent() {
        let mut cfg = fresh_v2();
        cfg.charts.push(ChartConfig {
            id: 0,
            symbol: "A".into(),
            timeframe: "1D".into(),
            levels: vec![],
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            collapse_gaps: false,
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            show_levels: true,
            viewport_width: None,
            viewport_height: None,
            symbol_link: LinkMode::default(),
            timeframe_link: LinkMode::default(),
            bound_symbol: None,
            backend: None,
            show_extended_hours: true,
            show_extended_hours_bands: true,
            volume_profile: VolumeProfileSettings::default(),
        });
        cfg.charts.push(ChartConfig {
            id: 0,
            symbol: "B".into(),
            timeframe: "1D".into(),
            levels: vec![],
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            collapse_gaps: false,
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            show_levels: true,
            viewport_width: None,
            viewport_height: None,
            symbol_link: LinkMode::default(),
            timeframe_link: LinkMode::default(),
            bound_symbol: None,
            backend: None,
            show_extended_hours: true,
            show_extended_hours_bands: true,
            volume_profile: VolumeProfileSettings::default(),
        });
        cfg.watchlists.push(WatchlistConfig {
            id: 0,
            name: "WL".into(),
            tickers: vec![],
            symbol_link: LinkMode::default(),
            column_widths: vec![],
        });
        cfg.legacy_panel_order
            .push(PanelSlot::Chart { chart_index: 0 });
        cfg.legacy_panel_order
            .push(PanelSlot::Chart { chart_index: 1 });
        cfg.legacy_panel_order
            .push(PanelSlot::Watchlist { watchlist_index: 0 });

        migrate_v2_to_v3(&mut cfg);

        let main = cfg.windows.get(WindowKey::MAIN_DEFAULT).expect("Main");
        // 3 leaves → 2 splits + 3 leaves = 5 nodes.
        assert_eq!(main.layout_tree.len(), 5);
        assert!(matches!(main.layout_tree[0], LayoutNode::Split { .. }));
        assert!(matches!(main.layout_tree[1], LayoutNode::Chart { .. }));
        assert!(matches!(main.layout_tree[2], LayoutNode::Split { .. }));
        assert!(matches!(main.layout_tree[3], LayoutNode::Chart { .. }));
        assert!(matches!(main.layout_tree[4], LayoutNode::Watchlist { .. }));
        // Legacy slots drained.
        assert!(cfg.legacy_panel_order.is_empty());
    }

    #[test]
    fn v2_to_v3_inserts_main_window_with_is_main_true() {
        let mut cfg = fresh_v2();
        migrate_v2_to_v3(&mut cfg);
        assert_eq!(cfg.version, 3);
        let main = cfg.windows.get(WindowKey::MAIN_DEFAULT).expect("Main");
        assert!(main.is_main);
    }

    // ── Validation pass tests ──────────────────────────────────────

    #[test]
    fn validate_synthesises_main_when_windows_empty() {
        let mut cfg = AppConfig::default();
        cfg.windows.clear();
        validate(&mut cfg);
        assert_eq!(cfg.windows.len(), 1);
        assert!(cfg.windows[WindowKey::MAIN_DEFAULT].is_main);
    }

    #[test]
    fn validate_promotes_first_when_no_main() {
        let mut cfg = AppConfig::default();
        cfg.windows.clear();
        cfg.windows.insert(
            "Alpha".to_string(),
            WindowConfig {
                is_main: false,
                geometry: default_window_geometry(),
                layout_tree: Vec::new(),
            },
        );
        cfg.windows.insert(
            "Beta".to_string(),
            WindowConfig {
                is_main: false,
                geometry: default_window_geometry(),
                layout_tree: Vec::new(),
            },
        );
        validate(&mut cfg);
        // BTreeMap order: "Alpha" comes first.
        assert!(cfg.windows["Alpha"].is_main);
        assert!(!cfg.windows["Beta"].is_main);
    }

    #[test]
    fn validate_demotes_extras_when_multiple_main() {
        let mut cfg = AppConfig::default();
        cfg.windows.clear();
        cfg.windows.insert(
            "Alpha".to_string(),
            WindowConfig {
                is_main: true,
                geometry: default_window_geometry(),
                layout_tree: Vec::new(),
            },
        );
        cfg.windows.insert(
            "Beta".to_string(),
            WindowConfig {
                is_main: true,
                geometry: default_window_geometry(),
                layout_tree: Vec::new(),
            },
        );
        validate(&mut cfg);
        assert!(cfg.windows["Alpha"].is_main);
        assert!(!cfg.windows["Beta"].is_main);
    }

    #[test]
    fn validate_drops_dangling_layout_ids() {
        let mut cfg = AppConfig::default();
        // No charts in the pool.
        cfg.windows
            .get_mut(WindowKey::MAIN_DEFAULT)
            .unwrap()
            .layout_tree
            .push(LayoutNode::Chart { chart_id: 999 });
        validate(&mut cfg);
        assert!(cfg.windows[WindowKey::MAIN_DEFAULT].layout_tree.is_empty());
    }

    #[test]
    fn synthesise_single_leaf_no_split() {
        let mut cfg = fresh_v2();
        cfg.charts.push(ChartConfig {
            id: 0,
            symbol: "S".into(),
            timeframe: "1D".into(),
            levels: vec![],
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            collapse_gaps: false,
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            show_levels: true,
            viewport_width: None,
            viewport_height: None,
            symbol_link: LinkMode::default(),
            timeframe_link: LinkMode::default(),
            bound_symbol: None,
            backend: None,
            show_extended_hours: true,
            show_extended_hours_bands: true,
            volume_profile: VolumeProfileSettings::default(),
        });
        cfg.legacy_panel_order
            .push(PanelSlot::Chart { chart_index: 0 });
        migrate_v2_to_v3(&mut cfg);
        let main = cfg.windows.get(WindowKey::MAIN_DEFAULT).unwrap();
        // Single leaf, no surrounding split.
        assert_eq!(main.layout_tree.len(), 1);
        assert!(matches!(main.layout_tree[0], LayoutNode::Chart { .. }));
    }

    #[test]
    fn synthesise_skips_dangling_panel_order_slots() {
        let mut cfg = fresh_v2();
        cfg.charts.push(ChartConfig {
            id: 0,
            symbol: "S".into(),
            timeframe: "1D".into(),
            levels: vec![],
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            collapse_gaps: false,
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            show_levels: true,
            viewport_width: None,
            viewport_height: None,
            symbol_link: LinkMode::default(),
            timeframe_link: LinkMode::default(),
            bound_symbol: None,
            backend: None,
            show_extended_hours: true,
            show_extended_hours_bands: true,
            volume_profile: VolumeProfileSettings::default(),
        });
        // Slot 0 valid; slot 5 dangles.
        cfg.legacy_panel_order
            .push(PanelSlot::Chart { chart_index: 0 });
        cfg.legacy_panel_order
            .push(PanelSlot::Chart { chart_index: 5 });
        migrate_v2_to_v3(&mut cfg);
        let main = cfg.windows.get(WindowKey::MAIN_DEFAULT).unwrap();
        // Only the valid leaf survives — single leaf, no split.
        assert_eq!(main.layout_tree.len(), 1);
    }

    #[test]
    fn order_panel_id_assignment_after_migration() {
        let mut cfg = fresh_v2();
        cfg.order_panels.push(OrderPanelConfig {
            id: 0,
            symbol: "AAPL".into(),
            ..OrderPanelConfig::default()
        });
        migrate_v2_to_v3(&mut cfg);
        assert_eq!(cfg.order_panels[0].id, 0);
        assert_eq!(cfg.version, 3);
    }
}
