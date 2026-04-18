use super::*;
use std::collections::HashSet;
use std::sync::Arc;

use crate::thumbnail_widget::ThumbnailSnapshot;

#[test]
fn all_returns_seven_columns() {
    assert_eq!(WatchlistColumn::all().len(), 7);
}

#[test]
fn all_column_ids_are_unique() {
    let ids: HashSet<ColumnId> = WatchlistColumn::all().iter().map(|c| c.id()).collect();
    assert_eq!(ids.len(), 7, "every column must have a unique ColumnId");
}

#[test]
fn sortable_columns() {
    let sortable: Vec<_> = WatchlistColumn::all()
        .into_iter()
        .filter(|c| c.sortable())
        .collect();
    assert_eq!(sortable.len(), 4);
    assert!(sortable.contains(&WatchlistColumn::Ticker));
    assert!(sortable.contains(&WatchlistColumn::Price));
    assert!(sortable.contains(&WatchlistColumn::ChangePercent));
    assert!(sortable.contains(&WatchlistColumn::GATR));
}

#[test]
fn non_sortable_columns() {
    assert!(!WatchlistColumn::DragHandle.sortable());
    assert!(!WatchlistColumn::Favorite.sortable());
    assert!(!WatchlistColumn::Delete.sortable());
}

#[test]
fn reorderable_columns() {
    assert!(!WatchlistColumn::DragHandle.reorderable());
    assert!(!WatchlistColumn::Delete.reorderable());
    // The rest should be reorderable.
    assert!(WatchlistColumn::Favorite.reorderable());
    assert!(WatchlistColumn::Ticker.reorderable());
    assert!(WatchlistColumn::Price.reorderable());
    assert!(WatchlistColumn::ChangePercent.reorderable());
    assert!(WatchlistColumn::GATR.reorderable());
    assert!(WatchlistColumn::Thumbnail.reorderable());
}

#[test]
fn resizable_matches_flex_or_thumbnail_columns() {
    // The Thumbnail column is Fixed-width but still resizable so users
    // can widen/narrow it in a config. Every other column follows the
    // `Flex ⇔ resizable` invariant.
    for col in WatchlistColumn::all() {
        let is_flex = matches!(col.width(), ColumnWidth::Flex(_));
        let is_thumbnail = matches!(col, WatchlistColumn::Thumbnail);
        assert_eq!(
            col.resizable(),
            is_flex || is_thumbnail,
            "resizable should match Flex-or-Thumbnail for {col:?}"
        );
    }
}

#[test]
fn fixed_columns_have_correct_widths() {
    assert_eq!(WatchlistColumn::Favorite.width(), ColumnWidth::Fixed(30.0));
    assert_eq!(WatchlistColumn::Delete.width(), ColumnWidth::Fixed(30.0));
}

#[test]
fn numeric_alignment() {
    assert_eq!(WatchlistColumn::Price.align(), Alignment::End);
    assert_eq!(WatchlistColumn::ChangePercent.align(), Alignment::End);
    assert_eq!(WatchlistColumn::GATR.align(), Alignment::End);
    assert_eq!(WatchlistColumn::Ticker.align(), Alignment::Start);
}

#[test]
fn compare_ticker_alphabetical() {
    let a = test_row("AAPL");
    let b = test_row("MSFT");
    assert_eq!(WatchlistColumn::Ticker.compare(&a, &b), Ordering::Less);
    assert_eq!(WatchlistColumn::Ticker.compare(&b, &a), Ordering::Greater);
    assert_eq!(WatchlistColumn::Ticker.compare(&a, &a), Ordering::Equal);
}

#[test]
fn compare_price_numeric() {
    let a = test_row_with_price("AAPL", Some(150.0));
    let b = test_row_with_price("MSFT", Some(300.0));
    let none = test_row_with_price("???", None);
    assert_eq!(WatchlistColumn::Price.compare(&a, &b), Ordering::Less);
    assert_eq!(WatchlistColumn::Price.compare(&b, &a), Ordering::Greater);
    // None sorts after Some.
    assert_eq!(WatchlistColumn::Price.compare(&a, &none), Ordering::Less);
    assert_eq!(WatchlistColumn::Price.compare(&none, &a), Ordering::Greater);
}

#[test]
fn compare_non_sortable_returns_equal() {
    let a = test_row("AAPL");
    let b = test_row("MSFT");
    assert_eq!(WatchlistColumn::DragHandle.compare(&a, &b), Ordering::Equal);
    assert_eq!(WatchlistColumn::Favorite.compare(&a, &b), Ordering::Equal);
    assert_eq!(WatchlistColumn::Delete.compare(&a, &b), Ordering::Equal);
}

#[test]
fn parse_gatr_strips_percent() {
    assert_eq!(parse_gatr("3.45%"), Some(3.45));
    assert_eq!(parse_gatr("3.45"), Some(3.45));
    assert_eq!(parse_gatr("--"), None);
    assert_eq!(parse_gatr(""), None);
}

#[test]
fn cmp_option_f64_ordering() {
    assert_eq!(cmp_option_f64(Some(1.0), Some(2.0)), Ordering::Less);
    assert_eq!(cmp_option_f64(Some(2.0), Some(1.0)), Ordering::Greater);
    assert_eq!(cmp_option_f64(Some(1.0), Some(1.0)), Ordering::Equal);
    assert_eq!(cmp_option_f64(Some(1.0), None), Ordering::Less);
    assert_eq!(cmp_option_f64(None, Some(1.0)), Ordering::Greater);
    assert_eq!(cmp_option_f64(None, None), Ordering::Equal);
}

// ── Test helpers ────────────────────────────────────────────────

fn empty_snapshot() -> ThumbnailSnapshot {
    ThumbnailSnapshot {
        widget_key: 0,
        closes: Arc::new(Vec::new()),
        y_min: 0.0,
        y_max: 1.0,
        color: [0.5, 0.5, 0.5, 0.6],
        generation: 0,
        label: "--".to_string(),
    }
}

fn test_row(symbol: &str) -> WatchlistRow {
    WatchlistRow {
        symbol: symbol.to_owned(),
        favorite: 0,
        price_text: "--".into(),
        change_text: "--".into(),
        change_color: COLOR_NEUTRAL,
        gatr_text: "--".into(),
        gatr_color: COLOR_NEUTRAL,
        wl_id: WatchlistId::new(1),
        price_value: None,
        change_value: None,
        thumbnail: empty_snapshot(),
    }
}

fn test_row_with_price(symbol: &str, price: Option<f64>) -> WatchlistRow {
    WatchlistRow {
        symbol: symbol.to_owned(),
        favorite: 0,
        price_text: price.map(|p| format!("{p:.2}")).unwrap_or("--".into()),
        change_text: "--".into(),
        change_color: COLOR_NEUTRAL,
        gatr_text: "--".into(),
        gatr_color: COLOR_NEUTRAL,
        wl_id: WatchlistId::new(1),
        price_value: price,
        change_value: None,
        thumbnail: empty_snapshot(),
    }
}
