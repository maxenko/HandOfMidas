//! Pure measurement helpers for the decorator layout engine.
//!
//! Walks a `DecoratorItem` tree and computes intrinsic sizes before any
//! positioning happens. Text width uses a `font_size * 0.6 * char_count`
//! heuristic; nested `Stack` groups recurse directly. All helpers are
//! `pub(crate)` — internal to `compute.rs` and its tests.

use super::badge::Badge;
use super::group::{DecoratorItem, FlexDirection, ItemContent};

/// Text-width heuristic: glyph width is approximated as `0.6 * font_size`.
pub(crate) fn measure_text(text: &str, font_size: f32) -> f32 {
    font_size * 0.6 * text.chars().count() as f32
}

/// Intrinsic `(width, height)` of a [`Badge`].
///
/// Width is the sum of per-segment widths (each segment honors its
/// `min_width` floor against its measured text plus `2 * badge.padding`
/// of inner padding) plus one additional `badge.padding` on each outer
/// side. Height is the badge's configured height — segments do not
/// affect it.
pub(crate) fn measure_badge(badge: &Badge) -> (f32, f32) {
    let mut width = badge.padding * 2.0;
    for segment in &badge.segments {
        let text_w = measure_text(&segment.text, segment.font_size);
        let intrinsic = text_w + badge.padding * 2.0;
        let w = match segment.min_width {
            Some(min) => intrinsic.max(min),
            None => intrinsic,
        };
        width += w;
    }
    (width, badge.height)
}

/// Intrinsic `(width, height)` of a [`DecoratorItem`] before placement.
///
/// Dispatches on [`ItemContent`]:
/// - `Badge` recurses into [`measure_badge`].
/// - `Button` reports its fixed `size`.
/// - `Stack` sums child sizes along the nested group's `direction`,
///   adding `gap` between siblings. The cross-axis size is the max of
///   child cross-axis sizes.
/// - `Spacer(w)` contributes `(w, 0.0)` — it consumes main-axis width
///   during layout but emits no primitives.
pub(crate) fn measure_item(item: &DecoratorItem) -> (f32, f32) {
    match &item.content {
        ItemContent::Badge(b) => measure_badge(b),
        ItemContent::Button(b) => (b.size[0], b.size[1]),
        ItemContent::Stack(group) => {
            let mut main = 0.0_f32;
            let mut cross = 0.0_f32;
            for (i, child) in group.items.iter().enumerate() {
                let (cw, ch) = measure_item(child);
                let (m, c) = match group.direction {
                    FlexDirection::Row => (cw, ch),
                    FlexDirection::Column => (ch, cw),
                };
                if i > 0 {
                    main += group.gap;
                }
                main += m;
                if c > cross {
                    cross = c;
                }
            }
            match group.direction {
                FlexDirection::Row => (main, cross),
                FlexDirection::Column => (cross, main),
            }
        }
        ItemContent::Spacer(w) => (*w, 0.0),
    }
}
