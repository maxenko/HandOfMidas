//! One-shot config migrations.
//!
//! Each function here is a pure transform over [`AppConfig`]. They are
//! idempotent: applying twice leaves the config unchanged. [`AppConfig::load`]
//! drives them unconditionally and the caller is expected to save the
//! migrated form back to disk after a successful load.

use super::{AccountPanelConfig, AccountTab, AppConfig, LayoutNode, OrdersTabConfig, PanelSlot};

/// Translate legacy `order_blotters` entries into the new
/// `account_panels` list with `active_tab = Orders`.
///
/// The function also rewrites `panel_order` and `layout_tree` so that
/// any `OrderBlotter { order_blotter_index }` slot is remapped to an
/// `Account { account_panel_index }` slot pointing at the newly
/// appended account panel. Index preservation is done by appending
/// blotter `N` at position `len(account_panels_before) + N`, then
/// rewriting every reference to the old blotter index through the
/// same offset.
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

    // Rewrite panel_order references.
    for slot in cfg.panel_order.iter_mut() {
        if let PanelSlot::OrderBlotter {
            order_blotter_index,
        } = *slot
        {
            *slot = PanelSlot::Account {
                account_panel_index: base + order_blotter_index,
            };
        }
    }

    // Rewrite layout_tree references.
    for node in cfg.layout_tree.iter_mut() {
        if let LayoutNode::OrderBlotter {
            order_blotter_index,
        } = *node
        {
            *node = LayoutNode::Account {
                account_panel_index: base + order_blotter_index,
            };
        }
    }

    cfg.account_panels.len() - base
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AccountTab, OrderBlotterConfig};
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
        cfg.panel_order.push(PanelSlot::OrderBlotter {
            order_blotter_index: 0,
        });
        cfg.layout_tree.push(LayoutNode::OrderBlotter {
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
        match &cfg.panel_order[0] {
            PanelSlot::Account {
                account_panel_index,
            } => assert_eq!(*account_panel_index, 0),
            other => panic!("panel_order not rewritten: {other:?}"),
        }
        match &cfg.layout_tree[0] {
            LayoutNode::Account {
                account_panel_index,
            } => assert_eq!(*account_panel_index, 0),
            other => panic!("layout_tree not rewritten: {other:?}"),
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
        cfg.panel_order.push(PanelSlot::OrderBlotter {
            order_blotter_index: 1,
        });
        cfg.panel_order.push(PanelSlot::OrderBlotter {
            order_blotter_index: 0,
        });

        migrate_order_blotters_to_account_panels(&mut cfg);

        assert_eq!(cfg.account_panels.len(), 2);
        assert_eq!(cfg.account_panels[0].name, "A");
        assert_eq!(cfg.account_panels[1].name, "B");
        assert!(matches!(
            cfg.panel_order[0],
            PanelSlot::Account {
                account_panel_index: 1
            }
        ));
        assert!(matches!(
            cfg.panel_order[1],
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
}
