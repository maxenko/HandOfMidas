//! Slice 8a-ii: decorator-group constructors for order brackets.
//!
//! Three builders translate an `OrderBracket`'s domain state into
//! visual `DecoratorGroup`s consumed by `compute_decorator_group()`:
//!
//! - [`entry_decorator_group`] — always present; the pointed-left entry
//!   badge with `[type | qty | price]` segments plus hover-only close
//!   button, quick-create stack, and (for draft brackets) `Submit` /
//!   `Save` action buttons.
//! - [`tp_decorator_group`] — emitted when the bracket has a TP leg. The
//!   TP badge carries a role glyph, a black-circle position counter, a
//!   percentage segment and the price.
//! - [`sl_decorator_group`] — emitted when the bracket has an SL leg.
//!   Orange-filled pointed-left badge with `[role | dollar_risk | price]`
//!   and a hover-only `RemoveStopLoss` close button.
//!
//! The GPU renderer treats each group as opaque; status-driven line
//! styling still flows through `OrderBracket::leg_style()`. These
//! constructors own the visual badge/button tree anchored on top of
//! each leg's `PriceLine` and carry the per-item `DecoratorAction`
//! wiring that Slice 8b relies on to route clicks through
//! `ChartAction::DecoratorClick`.

use super::{
    BracketSide, BracketStatus, EntryType, OrderBracket, BRACKET_LONG_ENTRY_COLOR,
    BRACKET_LONG_STOP_COLOR, BRACKET_LONG_STOP_LIMIT_COLOR, BRACKET_SHORT_ENTRY_COLOR,
    BRACKET_SHORT_STOP_COLOR, BRACKET_SHORT_STOP_LIMIT_COLOR, BRACKET_SL_COLOR, BRACKET_TP_COLOR,
};
use crate::widget::compute::ComputeContext;
use crate::widget::decorator::{
    Badge, BadgeBorder, BadgeSegment, BadgeShape, Button, DecoratorAction, DecoratorAnchor,
    DecoratorGroup, DecoratorItem, FlexDirection, ItemContent, Visibility,
};
use smallvec::smallvec;

/// Nested-stack group id for the entry-decorator quick-create column.
///
/// Top-level bracket groups use `0` (entry), `1` (TP), `2` (SL). Nested
/// stacks inside a top-level group get ids namespaced above `0x80` so
/// they cannot collide with a sibling top-level id — `03-data-model.md`
/// requires unique `group_id` values within one annotation's decorator
/// set, including nested groups reached through `ItemContent::Stack`.
pub(crate) const ENTRY_QUICK_CREATE_STACK_GROUP_ID: u16 = 0x80;

// ── Color helpers ────────────────────────────────────────────────────

/// Linearly interpolate `color` toward white by `amount` (`0.0..=1.0`).
pub(crate) fn lighten(color: [f32; 4], amount: f32) -> [f32; 4] {
    let a = amount.clamp(0.0, 1.0);
    [
        color[0] + (1.0 - color[0]) * a,
        color[1] + (1.0 - color[1]) * a,
        color[2] + (1.0 - color[2]) * a,
        color[3],
    ]
}

/// Linearly interpolate `color` toward black by `amount` (`0.0..=1.0`).
pub(crate) fn darken(color: [f32; 4], amount: f32) -> [f32; 4] {
    let a = amount.clamp(0.0, 1.0);
    [
        color[0] * (1.0 - a),
        color[1] * (1.0 - a),
        color[2] * (1.0 - a),
        color[3],
    ]
}

/// Base (full-alpha) entry color for a given `(side, entry_type)` pair.
///
/// Mirrors the dispatch table inside `OrderBracket::leg_style()` but
/// returns the unmodulated base color — status-driven alpha handling is
/// done separately on the line stroke and never leaks into decorators.
pub(crate) fn entry_base_color(side: BracketSide, entry_type: EntryType) -> [f32; 4] {
    match (side, entry_type) {
        (BracketSide::Long, EntryType::Stop) => BRACKET_LONG_STOP_COLOR,
        (BracketSide::Long, EntryType::StopLimit) => BRACKET_LONG_STOP_LIMIT_COLOR,
        (BracketSide::Long, _) => BRACKET_LONG_ENTRY_COLOR,
        (BracketSide::Short, EntryType::Stop) => BRACKET_SHORT_STOP_COLOR,
        (BracketSide::Short, EntryType::StopLimit) => BRACKET_SHORT_STOP_LIMIT_COLOR,
        (BracketSide::Short, _) => BRACKET_SHORT_ENTRY_COLOR,
    }
}

/// Single-character glyph for the entry type segment.
pub(crate) fn entry_type_glyph(et: EntryType) -> char {
    match et {
        EntryType::Market => 'M',
        EntryType::Limit => 'L',
        EntryType::Stop => 'S',
        EntryType::StopLimit => 'X',
    }
}

/// Format quantity for the middle segment. `None` renders as an em-dash.
pub(crate) fn format_quantity(q: Option<f64>) -> String {
    match q {
        Some(v) => format!("{:.0}", v),
        None => "\u{2014}".to_owned(),
    }
}

/// Count of filled/working legs for the TP "position" badge. The plan
/// shows a black circle with an integer inside it; we approximate with
/// "number of bracket legs currently attached" (entry + tp + optional
/// sl). This is cosmetic until Slice 8b wires in execution-report data.
fn position_count(bracket: &OrderBracket) -> u32 {
    let mut n = 1_u32; // entry is always present
    if bracket.take_profit.is_some() {
        n += 1;
    }
    if bracket.stop_loss.is_some() {
        n += 1;
    }
    n
}

// ── Entry decorator group ────────────────────────────────────────────

/// Build the entry-leg decorator group.
///
/// Structure (main axis = Row, anchored on the right edge — `items[0]`
/// is placed right-most, subsequent items pack leftward):
/// 1. `OnGroupHover` close button ('X').
/// 2. Always-visible `PointLeft` badge with three segments
///    `[type_glyph | quantity | price]`.
/// 3. `OnGroupHover` nested column with the `▲` (CreateTakeProfit) and
///    `▼` (CreateStopLoss) quick-create buttons. `CreateStopLoss` is
///    the Slice 8b successor to the legacy `[SL]` toggle button — it is
///    only attached here when the bracket does not yet carry an SL leg,
///    which matches the legacy overflow rule (the `[SL]` button was
///    only drawn when `bracket.stop_loss.is_none()`).
///
/// Draft brackets also get hover-only `Submit` and `Save` action
/// buttons: `Submit` is gated on a non-zero entry price (market data
/// available); `Save` is always emitted for draft brackets. Both are
/// wired to the matching `DecoratorAction` so clicks route through
/// `ChartAction::DecoratorClick`.
pub fn entry_decorator_group(bracket: &OrderBracket) -> DecoratorGroup {
    let color_main = entry_base_color(bracket.side, bracket.entry_type);
    let color_light = lighten(color_main, 0.3);
    let color_dark = darken(color_main, 0.3);

    let is_draft = bracket.status == BracketStatus::Draft;
    let submit_bg: [f32; 4] = match bracket.side {
        BracketSide::Long => [0.20, 0.78, 0.35, 1.0],
        BracketSide::Short => [0.90, 0.25, 0.25, 1.0],
    };
    let save_bg: [f32; 4] = [0.35, 0.45, 0.65, 1.0];

    let mut items: smallvec::SmallVec<[DecoratorItem; 6]> = smallvec![
        // Hover-only close button on the far right of the group.
        DecoratorItem {
            visibility: Visibility::OnGroupHover,
            action: Some(DecoratorAction::CloseAnnotation),
            content: ItemContent::Button(Button {
                shape: BadgeShape::Rounded { radius: 2.0 },
                fill: color_main,
                hover_fill: Some(color_light),
                glyph: 'X',
                glyph_color: [1.0, 1.0, 1.0, 1.0],
                glyph_size: 12.0,
                size: [18.0, 18.0],
                border: None,
            }),
        },
    ];

    // Draft-only `Submit` button (requires a non-zero entry price).
    if is_draft && bracket.entry.line.price != 0.0 {
        items.push(DecoratorItem {
            visibility: Visibility::OnGroupHover,
            action: Some(DecoratorAction::Submit),
            content: ItemContent::Button(Button {
                shape: BadgeShape::Rounded { radius: 2.0 },
                fill: submit_bg,
                hover_fill: Some(lighten(submit_bg, 0.2)),
                glyph: '\u{2713}',
                glyph_color: [1.0, 1.0, 1.0, 1.0],
                glyph_size: 12.0,
                size: [48.0, 18.0],
                border: None,
            }),
        });
    }

    // Draft-only `Save` button.
    if is_draft {
        items.push(DecoratorItem {
            visibility: Visibility::OnGroupHover,
            action: Some(DecoratorAction::Save),
            content: ItemContent::Button(Button {
                shape: BadgeShape::Rounded { radius: 2.0 },
                fill: save_bg,
                hover_fill: Some(lighten(save_bg, 0.2)),
                glyph: '\u{2B50}',
                glyph_color: [1.0, 1.0, 1.0, 1.0],
                glyph_size: 12.0,
                size: [36.0, 18.0],
                border: None,
            }),
        });
    }

    items.push(DecoratorItem {
        visibility: Visibility::Always,
        action: None,
        content: ItemContent::Badge(Box::new(Badge {
            shape: BadgeShape::PointLeft { point_width: 8.0 },
            fill: color_main,
            border: None,
            height: 20.0,
            padding: 6.0,
            segments: smallvec![
                BadgeSegment {
                    text: entry_type_glyph(bracket.entry_type).to_string(),
                    text_color: [1.0, 1.0, 1.0, 1.0],
                    font_size: 11.0,
                    min_width: Some(14.0),
                    fill_override: Some(color_light),
                    shape_override: None,
                    action: Some(DecoratorAction::CycleEntryType),
                },
                BadgeSegment {
                    text: format_quantity(bracket.quantity),
                    text_color: [1.0, 1.0, 1.0, 1.0],
                    font_size: 11.0,
                    min_width: Some(44.0),
                    fill_override: None,
                    shape_override: None,
                    action: Some(DecoratorAction::EditQuantity),
                },
                BadgeSegment {
                    text: format!("{:.2}", bracket.entry.line.price),
                    text_color: [1.0, 1.0, 1.0, 1.0],
                    font_size: 11.0,
                    min_width: None,
                    fill_override: Some(color_dark),
                    shape_override: None,
                    action: Some(DecoratorAction::EditPrice),
                },
            ],
            divider_color: Some(color_dark),
        })),
    });

    // Hover-only vertical stack of quick-create buttons. Both are
    // always attached so the user can (re-)create either leg at any
    // time — a click on `CreateStopLoss` with an existing SL leg is
    // treated as "replace" by the app layer.
    let stack_items: smallvec::SmallVec<[DecoratorItem; 4]> = smallvec![
        DecoratorItem {
            visibility: Visibility::Always,
            action: Some(DecoratorAction::CreateTakeProfit),
            content: ItemContent::Button(Button {
                shape: BadgeShape::Rect,
                fill: [0.15, 0.85, 0.85, 1.0],
                hover_fill: Some([0.25, 0.95, 0.95, 1.0]),
                glyph: '\u{25B2}',
                glyph_color: [1.0, 1.0, 1.0, 1.0],
                glyph_size: 10.0,
                size: [14.0, 10.0],
                border: None,
            }),
        },
        DecoratorItem {
            visibility: Visibility::Always,
            action: Some(DecoratorAction::CreateStopLoss),
            content: ItemContent::Button(Button {
                shape: BadgeShape::Rect,
                fill: [0.15, 0.85, 0.85, 1.0],
                hover_fill: Some([0.25, 0.95, 0.95, 1.0]),
                glyph: '\u{25BC}',
                glyph_color: [1.0, 1.0, 1.0, 1.0],
                glyph_size: 10.0,
                size: [14.0, 10.0],
                border: None,
            }),
        },
    ];

    // Nested stack group_ids are namespaced above 0x80 so they never
    // collide with the top-level TP (1) / SL (2) group_ids used by
    // their sibling constructors.
    items.push(DecoratorItem {
        visibility: Visibility::OnGroupHover,
        action: None,
        content: ItemContent::Stack(Box::new(DecoratorGroup {
            group_id: ENTRY_QUICK_CREATE_STACK_GROUP_ID,
            anchor: DecoratorAnchor::RightEdge,
            direction: FlexDirection::Column,
            gap: 1.0,
            items: stack_items,
        })),
    });

    DecoratorGroup {
        group_id: 0,
        anchor: DecoratorAnchor::RightEdge,
        direction: FlexDirection::Row,
        gap: 3.0,
        items: items.into_iter().collect(),
    }
}

// ── Take-profit decorator group ──────────────────────────────────────

/// Build the TP-leg decorator group or `None` if TP is not present.
///
/// Segments: `[role_glyph 'T' | position_count_circle | pct | price]`.
pub fn tp_decorator_group(bracket: &OrderBracket) -> Option<DecoratorGroup> {
    let tp = bracket.take_profit.as_ref()?;
    let main = BRACKET_TP_COLOR;
    let dark = darken(main, 0.3);
    let light = lighten(main, 0.3);

    // Percentage move from entry to TP (based on entry price).
    let pct_text = {
        let entry_price = bracket.entry.line.price;
        if entry_price.abs() > f64::EPSILON {
            let pct = (tp.line.price - entry_price) / entry_price * 100.0;
            format!("{:.1}%", pct.abs())
        } else {
            "\u{2014}".to_owned()
        }
    };

    Some(DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::RightEdge,
        direction: FlexDirection::Row,
        gap: 3.0,
        items: smallvec![DecoratorItem {
            visibility: Visibility::Always,
            action: None,
            content: ItemContent::Badge(Box::new(Badge {
                shape: BadgeShape::PointLeft { point_width: 8.0 },
                fill: main,
                border: None,
                height: 20.0,
                padding: 6.0,
                segments: smallvec![
                    BadgeSegment {
                        text: "T".to_owned(),
                        text_color: [1.0, 1.0, 1.0, 1.0],
                        font_size: 11.0,
                        min_width: Some(14.0),
                        fill_override: Some(light),
                        shape_override: None,
                        action: None,
                    },
                    BadgeSegment {
                        text: format!("{}", position_count(bracket)),
                        text_color: [1.0, 1.0, 1.0, 1.0],
                        font_size: 11.0,
                        min_width: Some(18.0),
                        fill_override: Some([0.0, 0.0, 0.0, 1.0]),
                        shape_override: Some(BadgeShape::Circle),
                        action: None,
                    },
                    BadgeSegment {
                        text: pct_text,
                        text_color: [1.0, 1.0, 1.0, 1.0],
                        font_size: 11.0,
                        min_width: None,
                        fill_override: None,
                        shape_override: None,
                        action: None,
                    },
                    BadgeSegment {
                        text: format!("{:.2}", tp.line.price),
                        text_color: [1.0, 1.0, 1.0, 1.0],
                        font_size: 11.0,
                        min_width: None,
                        fill_override: Some(dark),
                        shape_override: None,
                        action: Some(DecoratorAction::EditPrice),
                    },
                ],
                divider_color: Some(dark),
            })),
        }],
    })
}

// ── Stop-loss decorator group ────────────────────────────────────────

/// Build the SL-leg decorator group or `None` if SL is not present.
///
/// Segments: `[role_glyph 'S' | dollar_risk | price]`. Uses the orange
/// bracket-SL base color.
pub fn sl_decorator_group(bracket: &OrderBracket) -> Option<DecoratorGroup> {
    let sl = bracket.stop_loss.as_ref()?;
    let main = BRACKET_SL_COLOR;
    let dark = darken(main, 0.3);
    let light = lighten(main, 0.3);

    let risk_text = match bracket.dollar_risk() {
        Some(r) => format!("${:.0}", r),
        None => "\u{2014}".to_owned(),
    };

    Some(DecoratorGroup {
        group_id: 2,
        anchor: DecoratorAnchor::RightEdge,
        direction: FlexDirection::Row,
        gap: 3.0,
        items: smallvec![
            // Hover-only close button to detach the SL leg.
            DecoratorItem {
                visibility: Visibility::OnGroupHover,
                action: Some(DecoratorAction::RemoveStopLoss),
                content: ItemContent::Button(Button {
                    shape: BadgeShape::Rounded { radius: 2.0 },
                    fill: main,
                    hover_fill: Some(light),
                    glyph: 'X',
                    glyph_color: [1.0, 1.0, 1.0, 1.0],
                    glyph_size: 12.0,
                    size: [18.0, 18.0],
                    border: None,
                }),
            },
            DecoratorItem {
                visibility: Visibility::Always,
                action: None,
                content: ItemContent::Badge(Box::new(Badge {
                    shape: BadgeShape::PointLeft { point_width: 8.0 },
                    fill: main,
                    border: None,
                    height: 20.0,
                    padding: 6.0,
                    segments: smallvec![
                        BadgeSegment {
                            text: "S".to_owned(),
                            text_color: [1.0, 1.0, 1.0, 1.0],
                            font_size: 11.0,
                            min_width: Some(14.0),
                            fill_override: Some(light),
                            shape_override: None,
                            action: None,
                        },
                        BadgeSegment {
                            text: risk_text,
                            text_color: [1.0, 1.0, 1.0, 1.0],
                            font_size: 11.0,
                            min_width: None,
                            fill_override: None,
                            shape_override: None,
                            action: None,
                        },
                        BadgeSegment {
                            text: format!("{:.2}", sl.line.price),
                            text_color: [1.0, 1.0, 1.0, 1.0],
                            font_size: 11.0,
                            min_width: None,
                            fill_override: Some(dark),
                            shape_override: None,
                            action: Some(DecoratorAction::EditPrice),
                        },
                    ],
                    divider_color: Some(dark),
                })),
            },
        ],
    })
}

// ── Pin-toggle decorator group ───────────────────────────────────────

/// Top-level group id for the pin-toggle decorator.
///
/// Entry uses `0`, TP uses `1`, SL uses `2`; `3` is reserved for the
/// always-visible pin button anchored on the entry leg's `PriceLine`.
pub(crate) const PIN_TOGGLE_GROUP_ID: u16 = 3;

/// Gold fill used for the active (pinned) pin badge. Matches the
/// "active" convention the other decorator builders use for emphasis.
const PIN_ACTIVE_FILL: [f32; 4] = [0.95, 0.78, 0.18, 1.0];

/// Hover fill for the active pin button — a lighter gold.
const PIN_ACTIVE_HOVER_FILL: [f32; 4] = [1.0, 0.87, 0.30, 1.0];

/// Outline color used when the intent is not pinned. Renders as a
/// thin-stroke badge with a near-transparent fill so the entry line
/// underneath remains visible.
const PIN_OUTLINE_COLOR: [f32; 4] = [0.78, 0.78, 0.85, 1.0];

/// Background fill for the inactive (outlined) pin button. Kept very
/// dark so the border dominates the silhouette on a light chart.
const PIN_OUTLINE_FILL: [f32; 4] = [0.12, 0.12, 0.15, 0.85];

/// Hover fill for the inactive pin button — slightly brighter than
/// the resting fill.
const PIN_OUTLINE_HOVER_FILL: [f32; 4] = [0.20, 0.20, 0.24, 0.95];

/// Unicode "pushpin" glyph rendered inside the pin button.
///
/// Uses U+1F4CC ("ROUND PUSHPIN"). If the active font lacks this code
/// point, downstream label rendering falls back to a tofu box — the
/// cosmetic gap is noted in the Slice 4 visual-review checklist.
const PIN_GLYPH: char = '\u{1F4CC}';

/// Build the always-visible pin-toggle decorator group anchored on the
/// entry leg's `PriceLine`.
///
/// Visual state is derived entirely from `ctx.pinned`:
///
/// - **Pinned (`ctx.pinned == true`)**: a small gold-filled button
///   with a pushpin glyph. Signals that the GATR drift-snap rule is
///   suppressed for this symbol.
/// - **Unpinned (`ctx.pinned == false`)**: an outlined button in the
///   same silhouette with a dim fill and a light border, inviting the
///   user to click and lock the bracket in place.
///
/// A click on the button emits `DecoratorAction::TogglePin`, which the
/// app layer routes to `Message::ChartBracketTogglePin(...)` and
/// ultimately to `OrderIntentAppMsg::TogglePin`. Because the pin state
/// lives on `TickerOrderIntent`, the next frame's
/// `ComputeContext.pinned` reflects the flipped value and the badge
/// swaps variants without any chart-local state.
pub fn pin_toggle_group(bracket: &OrderBracket, ctx: &ComputeContext<'_>) -> DecoratorGroup {
    let _ = bracket; // bracket kept in the signature for symmetry with
                     // entry_/tp_/sl_decorator_group; future revisions
                     // may tint the badge from the bracket side color.

    let (fill, hover_fill, glyph_color, border) = if ctx.pinned {
        (
            PIN_ACTIVE_FILL,
            Some(PIN_ACTIVE_HOVER_FILL),
            [0.12, 0.10, 0.02, 1.0],
            None,
        )
    } else {
        (
            PIN_OUTLINE_FILL,
            Some(PIN_OUTLINE_HOVER_FILL),
            PIN_OUTLINE_COLOR,
            Some(BadgeBorder {
                color: PIN_OUTLINE_COLOR,
                thickness: 1.25,
            }),
        )
    };

    DecoratorGroup {
        group_id: PIN_TOGGLE_GROUP_ID,
        anchor: DecoratorAnchor::RightEdge,
        direction: FlexDirection::Row,
        gap: 3.0,
        items: smallvec![DecoratorItem {
            visibility: Visibility::Always,
            action: Some(DecoratorAction::TogglePin),
            content: ItemContent::Button(Button {
                shape: BadgeShape::Rounded { radius: 3.0 },
                fill,
                hover_fill,
                glyph: PIN_GLYPH,
                glyph_color,
                glyph_size: 11.0,
                size: [18.0, 18.0],
                border,
            }),
        }],
    }
}
