use super::*;

#[test]
fn new_watchlist_is_empty() {
    let wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
    assert!(wl.tickers.is_empty());
    assert_eq!(wl.name, "Test");
    assert!(wl.add_ticker_input.is_empty());
}

#[test]
fn add_ticker_normalizes_to_uppercase() {
    let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
    assert!(wl.add_ticker("aapl"));
    assert_eq!(wl.tickers[0].symbol, "AAPL");
}

#[test]
fn add_ticker_trims_whitespace() {
    let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
    assert!(wl.add_ticker("  msft  "));
    assert_eq!(wl.tickers[0].symbol, "MSFT");
}

#[test]
fn add_ticker_rejects_empty() {
    let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
    assert!(!wl.add_ticker(""));
    assert!(!wl.add_ticker("   "));
    assert!(wl.tickers.is_empty());
}

#[test]
fn add_ticker_rejects_duplicate() {
    let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
    assert!(wl.add_ticker("AAPL"));
    assert!(!wl.add_ticker("aapl"));
    assert!(!wl.add_ticker("Aapl"));
    assert_eq!(wl.tickers.len(), 1);
}

#[test]
fn remove_ticker_case_insensitive() {
    let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
    wl.add_ticker("AAPL");
    wl.add_ticker("MSFT");
    wl.remove_ticker("aapl");
    assert_eq!(wl.tickers.len(), 1);
    assert_eq!(wl.tickers[0].symbol, "MSFT");
}

#[test]
fn adjust_favorite_clamps_to_range() {
    let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
    wl.add_ticker("AAPL");
    assert_eq!(wl.tickers[0].favorite, 0);
    for expected in 1..=5 {
        wl.adjust_favorite("aapl", 1);
        assert_eq!(wl.tickers[0].favorite, expected);
    }
    // Already at max — further increments clamp.
    wl.adjust_favorite("AAPL", 1);
    assert_eq!(wl.tickers[0].favorite, 5);
    for expected in (0..=4).rev() {
        wl.adjust_favorite("AAPL", -1);
        assert_eq!(wl.tickers[0].favorite, expected);
    }
    // Already at min — further decrements clamp.
    wl.adjust_favorite("AAPL", -1);
    assert_eq!(wl.tickers[0].favorite, 0);
}

#[test]
fn freeze_sort_snapshots_current_favorites() {
    let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
    wl.add_ticker("AAPL");
    wl.add_ticker("MSFT");
    wl.adjust_favorite("AAPL", 3);
    wl.adjust_favorite("MSFT", 1);

    wl.freeze_sort();
    let snap = wl
        .sort_freeze
        .as_ref()
        .expect("freeze_sort populates state");
    assert_eq!(snap.get("AAPL"), Some(&3));
    assert_eq!(snap.get("MSFT"), Some(&1));

    // Live values can change after freeze; snapshot stays put.
    wl.adjust_favorite("AAPL", 1);
    assert_eq!(wl.tickers[0].favorite, 4);
    assert_eq!(wl.sort_freeze.as_ref().unwrap().get("AAPL"), Some(&3));

    wl.unfreeze_sort();
    assert!(wl.sort_freeze.is_none());
}

#[test]
fn freeze_sort_is_idempotent_until_unfreeze() {
    // Back-to-back on_enter events (e.g. iced redelivering on a re-render)
    // must not overwrite the original snapshot — otherwise scrolling
    // would still re-sort because the snapshot would track the live value.
    let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
    wl.add_ticker("AAPL");
    wl.adjust_favorite("AAPL", 2);

    wl.freeze_sort();
    wl.adjust_favorite("AAPL", 1); // live=3, snap=2
    wl.freeze_sort(); // re-entrant — must NOT re-snapshot
    assert_eq!(wl.sort_freeze.as_ref().unwrap().get("AAPL"), Some(&2));

    wl.unfreeze_sort();
    wl.freeze_sort(); // after exit/re-enter, snapshot updates
    assert_eq!(wl.sort_freeze.as_ref().unwrap().get("AAPL"), Some(&3));
}

#[test]
fn has_ticker_case_insensitive() {
    let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
    wl.add_ticker("AAPL");
    assert!(wl.has_ticker("AAPL"));
    assert!(wl.has_ticker("aapl"));
    assert!(wl.has_ticker("Aapl"));
    assert!(!wl.has_ticker("MSFT"));
}

#[test]
fn config_roundtrip() {
    let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Main".into());
    wl.add_ticker("AAPL");
    wl.add_ticker("MSFT");
    wl.adjust_favorite("AAPL", 3);

    let config = wl.to_config();
    let restored = WatchlistPanel::from_config(WatchlistId::new(2), &config);

    assert_eq!(restored.name, "Main");
    assert_eq!(restored.tickers.len(), 2);
    assert_eq!(restored.tickers[0].symbol, "AAPL");
    assert_eq!(restored.tickers[0].favorite, 3);
    assert_eq!(restored.tickers[1].symbol, "MSFT");
    assert_eq!(restored.tickers[1].favorite, 0);
    // Transient state is not persisted.
    assert!(restored.add_ticker_input.is_empty());
    assert!(restored.selected_symbol.is_none());
}

#[test]
fn symbol_link_roundtrip() {
    let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Linked".into());
    wl.symbol_link = LinkMode::Color(midas_core::LinkColor::Blue);
    let config = wl.to_config();
    let restored = WatchlistPanel::from_config(WatchlistId::new(2), &config);
    assert_eq!(
        restored.symbol_link,
        LinkMode::Color(midas_core::LinkColor::Blue)
    );
}

#[test]
fn from_config_empty() {
    let config = WatchlistConfig {
        name: "Empty".into(),
        tickers: Vec::new(),
        symbol_link: LinkMode::Unlinked,
        column_widths: vec![],
    };
    let wl = WatchlistPanel::from_config(WatchlistId::new(1), &config);
    assert_eq!(wl.name, "Empty");
    assert!(wl.tickers.is_empty());
    // Grid state should have default widths.
    let defaults = default_column_widths();
    for (&id, &expected) in defaults.iter() {
        assert_eq!(wl.grid_state.column_width(id), expected);
    }
}

#[test]
fn column_widths_roundtrip() {
    let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
    wl.grid_state
        .set_column_width(COL_TICKER, 120.0, 20.0, None); // widen ticker column
    let config = wl.to_config();
    let restored = WatchlistPanel::from_config(WatchlistId::new(2), &config);
    assert_eq!(restored.grid_state.column_width(COL_TICKER), 120.0);
    assert_eq!(
        restored.grid_state.column_width(COL_DRAG),
        default_column_widths()[&COL_DRAG],
    ); // unchanged
}

#[test]
fn column_widths_minimum_enforced() {
    let config = WatchlistConfig {
        name: "Narrow".into(),
        tickers: vec![],
        symbol_link: LinkMode::Unlinked,
        column_widths: vec![5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0],
    };
    let wl = WatchlistPanel::from_config(WatchlistId::new(1), &config);
    for &id in &WATCHLIST_COLUMN_ORDER {
        let w = wl.grid_state.column_width(id);
        assert!(w >= 20.0, "column width {w} for {id} should be >= 20.0");
    }
}
