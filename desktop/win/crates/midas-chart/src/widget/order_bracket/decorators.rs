//! Decorator-group constructors for order brackets.
//!
//! Visual model (see `plan/bracket-visuals.md` and the V2 design pass):
//!
//! - [`entry_decorator_group`] — the pointed-left entry badge showing
//!   `[type_glyph | price]`. For filled positions the glyph becomes `P`
//!   and a black `qty` segment is inserted between the glyph and the
//!   price. Hover reveals an `X` close button for filled positions and
//!   `Submit` / `Save` buttons for drafts.
//! - [`tp_decorator_group`] — teal pointed-left badge with
//!   `[T | counter | pct | price]`. Counter and percentage segments are
//!   painted black. Hover reveals a teal `X` remove-TP button.
//! - [`sl_decorator_group`] — orange pointed-left badge with
//!   `[SL | price]`. Hover reveals an orange `X` remove-SL button.
//! - [`quick_create_above_group`] / [`quick_create_below_group`] —
//!   hover-only single-button groups that sit one badge-height above and
//!   below the entry line. Glyph (`^`/`v`), fill color (teal or orange)
//!   and action (create TP or create SL) are all driven by bracket side:
//!   for Long the up arrow adds TP and the down arrow adds SL; for Short
//!   the assignment flips. Each group is only emitted by `compute_bracket`
//!   when the corresponding leg is absent.
//!
//! The old ENTRY_QUICK_CREATE_STACK_GROUP_ID stack and the `pin_toggle_group`
//! badge are gone — both replaced by the cleaner V2 layout.

use super::{
    BracketSide, BracketStatus, EntryType, OrderBracket, BRACKET_LONG_ENTRY_COLOR,
    BRACKET_LONG_STOP_COLOR, BRACKET_LONG_STOP_LIMIT_COLOR, BRACKET_SHORT_ENTRY_COLOR,
    BRACKET_SHORT_STOP_COLOR, BRACKET_SHORT_STOP_LIMIT_COLOR, BRACKET_SL_COLOR, BRACKET_TP_COLOR,
};
use crate::widget::decorator::{
    Badge, BadgeSegment, BadgeShape, Button, DecoratorAction, DecoratorAnchor, DecoratorGroup,
    DecoratorItem, FlexDirection, ItemContent, Visibility,
};
use smallvec::smallvec;

// ── Group ids ────────────────────────────────────────────────────────

/// Entry badge + hover buttons.
pub(crate) const ENTRY_GROUP_ID: u16 = 0;
/// Take-profit leg badge + hover close.
pub(crate) const TP_GROUP_ID: u16 = 1;
/// Stop-loss leg badge + hover close.
pub(crate) const SL_GROUP_ID: u16 = 2;
/// Quick-create button positioned above the entry line.
pub(crate) const QUICK_CREATE_ABOVE_GROUP_ID: u16 = 4;
/// Quick-create button positioned below the entry line.
pub(crate) const QUICK_CREATE_BELOW_GROUP_ID: u16 = 5;

// ── Palette extensions for the V2 design ─────────────────────────────

/// Filled-position fill for long brackets (the "P" badge).
pub(crate) const POSITION_LONG_COLOR: [f32; 4] = [0.20, 0.40, 0.95, 1.0];
/// Filled-position fill for short brackets (the "P" badge).
pub(crate) const POSITION_SHORT_COLOR: [f32; 4] = [0.95, 0.20, 0.20, 1.0];
/// Black fill used for highlighted inset segments (counter, pct, qty).
const INSET_BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

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
#[allow(dead_code)]
pub(crate) fn darken(color: [f32; 4], amount: f32) -> [f32; 4] {
    let a = amount.clamp(0.0, 1.0);
    [
        color[0] * (1.0 - a),
        color[1] * (1.0 - a),
        color[2] * (1.0 - a),
        color[3],
    ]
}

/// True when the bracket represents an open position — i.e. it has been
/// (partially) filled and shows a quantity-backed "P" badge.
fn is_filled_position(bracket: &OrderBracket) -> bool {
    matches!(
        bracket.status,
        BracketStatus::Active | BracketStatus::PartialFill
    ) && bracket.filled_qty.unwrap_or(0.0) > 0.0
}

/// Fill color for the entry badge. Filled positions use the blue/red
/// "position" palette; everything else uses the `(side, entry_type)`
/// table.
pub(crate) fn entry_badge_fill(bracket: &OrderBracket) -> [f32; 4] {
    if is_filled_position(bracket) {
        return match bracket.side {
            BracketSide::Long => POSITION_LONG_COLOR,
            BracketSide::Short => POSITION_SHORT_COLOR,
        };
    }
    entry_base_color(bracket.side, bracket.entry_type)
}

/// Base (full-alpha) entry color for a given `(side, entry_type)` pair.
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

/// Label for the entry-badge glyph segment.
///
/// - Filled positions → `"P"` (with the qty segment attached alongside).
/// - Limit orders → `"L"`.
/// - Stop orders → `"S"` (the orange SL leg uses `"SL"` so there is no
///   visual collision in the same badge row).
/// - StopLimit orders → `"X"`.
/// - Market orders carry the side glyph — `"B"` for long, `"S"` for short.
pub(crate) fn entry_glyph_label(bracket: &OrderBracket) -> &'static str {
    if is_filled_position(bracket) {
        return "P";
    }
    match bracket.entry_type {
        EntryType::Limit => "L",
        EntryType::Stop => "S",
        EntryType::StopLimit => "X",
        EntryType::Market => match bracket.side {
            BracketSide::Long => "B",
            BracketSide::Short => "S",
        },
    }
}

/// Format quantity as a bare integer, or an em-dash if missing.
pub(crate) fn format_quantity(q: Option<f64>) -> String {
    match q {
        Some(v) => format!("{:.0}", v),
        None => "\u{2014}".to_owned(),
    }
}

// ── Entry decorator group ────────────────────────────────────────────

/// Build the entry-leg decorator group.
///
/// Non-filled brackets render as `[glyph | price]`. Filled positions add
/// a middle qty segment on black, and a hover-only `X` close button to
/// the right of the badge. Draft brackets additionally get hover-only
/// `Submit` and `Save` action buttons wired to the matching
/// `DecoratorAction` variants.
pub fn entry_decorator_group(bracket: &OrderBracket) -> DecoratorGroup {
    let body_color = entry_badge_fill(bracket);
    let is_draft = bracket.status == BracketStatus::Draft;
    let is_position = is_filled_position(bracket);

    let submit_bg: [f32; 4] = match bracket.side {
        BracketSide::Long => [0.20, 0.78, 0.35, 1.0],
        BracketSide::Short => [0.90, 0.25, 0.25, 1.0],
    };
    let save_bg: [f32; 4] = [0.35, 0.45, 0.65, 1.0];

    // Main badge: [glyph | qty? | price].
    let mut segments: smallvec::SmallVec<[BadgeSegment; 3]> = smallvec![BadgeSegment {
        text: entry_glyph_label(bracket).to_owned(),
        text_color: [1.0, 1.0, 1.0, 1.0],
        font_size: 11.0,
        min_width: Some(18.0),
        fill_override: None,
        shape_override: None,
        action: Some(DecoratorAction::CycleEntryType),
    }];
    if is_position {
        segments.push(BadgeSegment {
            text: format_quantity(bracket.quantity.or(bracket.filled_qty)),
            text_color: [1.0, 1.0, 1.0, 1.0],
            font_size: 11.0,
            min_width: Some(44.0),
            fill_override: Some(INSET_BLACK),
            // Force Rect so the inset doesn't inherit the parent's
            // PointLeft triangle and render a nose in the middle of
            // the badge.
            shape_override: Some(BadgeShape::Rect),
            action: Some(DecoratorAction::EditQuantity),
        });
    }
    segments.push(BadgeSegment {
        text: format!("{:.2}", bracket.entry.line.price),
        text_color: [1.0, 1.0, 1.0, 1.0],
        font_size: 11.0,
        min_width: None,
        fill_override: None,
        shape_override: None,
        action: Some(DecoratorAction::EditPrice),
    });

    // Items pack right-to-left from the viewport edge (see
    // `compute_decorator_group`: `Row + RightEdge` is right-to-left).
    // `items[0]` is therefore the rightmost visual element. The entry
    // badge owns that slot so it sits flush with the priceline; close /
    // submit / save buttons land to its left as hover affordances.
    let mut items: smallvec::SmallVec<[DecoratorItem; 6]> = smallvec![DecoratorItem {
        visibility: Visibility::Always,
        action: None,
        content: ItemContent::Badge(Box::new(Badge {
            shape: BadgeShape::PointLeft { point_width: 8.0 },
            fill: body_color,
            border: None,
            height: 20.0,
            padding: 6.0,
            segments,
            divider_color: None,
        })),
    }];

    // Hover-only close button for filled positions (close the trade).
    if is_position {
        items.push(DecoratorItem {
            visibility: Visibility::OnGroupHover,
            action: Some(DecoratorAction::CloseAnnotation),
            content: ItemContent::Button(Button {
                shape: BadgeShape::Rounded { radius: 2.0 },
                fill: body_color,
                hover_fill: Some(lighten(body_color, 0.25)),
                glyph: 'X',
                glyph_color: [1.0, 1.0, 1.0, 1.0],
                glyph_size: 12.0,
                size: [18.0, 18.0],
                border: None,
            }),
        });
    }

    // Draft-only controls: cancel + submit + save, all hover-only. Order
    // matters — items are appended in the order they should appear
    // right-to-left, so cancel sits closest to the badge, followed by
    // submit, with save furthest left.
    if is_draft {
        items.push(DecoratorItem {
            visibility: Visibility::OnGroupHover,
            action: Some(DecoratorAction::CloseAnnotation),
            content: ItemContent::Button(Button {
                shape: BadgeShape::Rounded { radius: 2.0 },
                fill: body_color,
                hover_fill: Some(lighten(body_color, 0.25)),
                glyph: 'X',
                glyph_color: [1.0, 1.0, 1.0, 1.0],
                glyph_size: 12.0,
                size: [18.0, 18.0],
                border: None,
            }),
        });
        if bracket.entry.line.price != 0.0 {
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
                    size: [18.0, 18.0],
                    border: None,
                }),
            });
        }
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
                size: [18.0, 18.0],
                border: None,
            }),
        });
    }

    DecoratorGroup {
        group_id: ENTRY_GROUP_ID,
        anchor: DecoratorAnchor::RightEdge,
        direction: FlexDirection::Row,
        gap: 3.0,
        items: items.into_iter().collect(),
    }
}

// ── Take-profit decorator group ──────────────────────────────────────

/// Build the TP-leg decorator group or `None` if TP is not present.
///
/// Segments: `[T | counter | pct | price]` on teal. Counter is drawn as
/// a black circle; pct is drawn on a black rectangle inset. A teal
/// hover-only close button sits to the right.
pub fn tp_decorator_group(bracket: &OrderBracket) -> Option<DecoratorGroup> {
    let tp = bracket.take_profit.as_ref()?;
    let main = BRACKET_TP_COLOR;

    let pct_text = {
        let entry_price = bracket.entry.line.price;
        if entry_price.abs() > f64::EPSILON {
            let pct = (tp.line.price - entry_price) / entry_price * 100.0;
            format!("{:.0}%", pct.abs())
        } else {
            "\u{2014}".to_owned()
        }
    };

    let position_count = tp_position_count(bracket);

    // Right-to-left packing: items[0] is the rightmost visual element,
    // so the badge owns that slot and the close button sits to its left.
    let items: smallvec::SmallVec<[DecoratorItem; 4]> = smallvec![
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
                        text: "T".to_owned(),
                        text_color: [1.0, 1.0, 1.0, 1.0],
                        font_size: 11.0,
                        min_width: Some(16.0),
                        fill_override: None,
                        shape_override: None,
                        action: None,
                    },
                    BadgeSegment {
                        text: format!("{}", position_count),
                        text_color: [1.0, 1.0, 1.0, 1.0],
                        font_size: 11.0,
                        min_width: Some(18.0),
                        fill_override: Some(INSET_BLACK),
                        shape_override: Some(BadgeShape::Circle),
                        action: None,
                    },
                    BadgeSegment {
                        text: pct_text,
                        text_color: [1.0, 1.0, 1.0, 1.0],
                        font_size: 11.0,
                        min_width: Some(34.0),
                        fill_override: Some(INSET_BLACK),
                        // Inset rectangle inside the PointLeft parent —
                        // override so the fragment shader doesn't draw
                        // the parent's triangle here.
                        shape_override: Some(BadgeShape::Rect),
                        action: None,
                    },
                    BadgeSegment {
                        text: format!("{:.2}", tp.line.price),
                        text_color: [1.0, 1.0, 1.0, 1.0],
                        font_size: 11.0,
                        min_width: None,
                        fill_override: None,
                        shape_override: None,
                        action: Some(DecoratorAction::EditPrice),
                    },
                ],
                divider_color: None,
            })),
        },
        DecoratorItem {
            visibility: Visibility::OnGroupHover,
            action: Some(DecoratorAction::CloseAnnotation),
            content: ItemContent::Button(Button {
                shape: BadgeShape::Rounded { radius: 2.0 },
                fill: main,
                hover_fill: Some(lighten(main, 0.2)),
                glyph: 'X',
                glyph_color: [1.0, 1.0, 1.0, 1.0],
                glyph_size: 12.0,
                size: [18.0, 18.0],
                border: None,
            }),
        },
    ];

    Some(DecoratorGroup {
        group_id: TP_GROUP_ID,
        anchor: DecoratorAnchor::RightEdge,
        direction: FlexDirection::Row,
        gap: 3.0,
        items,
    })
}

/// Number drawn inside the TP badge's black-circle counter. Currently
/// approximates leg-count but is a cosmetic placeholder until execution
/// reports drive the real position index.
fn tp_position_count(bracket: &OrderBracket) -> u32 {
    let mut n = 1_u32; // entry is always present
    if bracket.take_profit.is_some() {
        n += 1;
    }
    if bracket.stop_loss.is_some() {
        n += 1;
    }
    n
}

// ── Stop-loss decorator group ────────────────────────────────────────

/// Build the SL-leg decorator group or `None` if SL is not present.
///
/// Segments: `[SL | price]` on orange. Hover-only orange close button
/// to the right of the badge.
pub fn sl_decorator_group(bracket: &OrderBracket) -> Option<DecoratorGroup> {
    let sl = bracket.stop_loss.as_ref()?;
    let main = BRACKET_SL_COLOR;

    Some(DecoratorGroup {
        group_id: SL_GROUP_ID,
        anchor: DecoratorAnchor::RightEdge,
        direction: FlexDirection::Row,
        gap: 3.0,
        // Right-to-left packing: badge is rightmost, close-X sits to its
        // left as a hover affordance.
        items: smallvec![
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
                            text: "SL".to_owned(),
                            text_color: [1.0, 1.0, 1.0, 1.0],
                            font_size: 11.0,
                            min_width: Some(22.0),
                            fill_override: None,
                            shape_override: None,
                            action: None,
                        },
                        BadgeSegment {
                            text: format!("{:.2}", sl.line.price),
                            text_color: [1.0, 1.0, 1.0, 1.0],
                            font_size: 11.0,
                            min_width: None,
                            fill_override: None,
                            shape_override: None,
                            action: Some(DecoratorAction::EditPrice),
                        },
                    ],
                    divider_color: None,
                })),
            },
            DecoratorItem {
                visibility: Visibility::OnGroupHover,
                action: Some(DecoratorAction::RemoveStopLoss),
                content: ItemContent::Button(Button {
                    shape: BadgeShape::Rounded { radius: 2.0 },
                    fill: main,
                    hover_fill: Some(lighten(main, 0.2)),
                    glyph: 'X',
                    glyph_color: [1.0, 1.0, 1.0, 1.0],
                    glyph_size: 12.0,
                    size: [18.0, 18.0],
                    border: None,
                }),
            },
        ],
    })
}

// ── Quick-create buttons (above/below entry line) ────────────────────

/// Quick-create button group that sits one badge-height above the entry
/// line. The button is hover-only, packs right-to-left from the viewport
/// edge, and carries the "create the leg that goes *above* the entry"
/// semantics: TP for Long brackets (teal + `^`) and SL for Short
/// brackets (orange + `^`). Returns `None` when the leg this button
/// would create is already attached.
///
/// The caller (`compute_bracket`) is responsible for placing the group's
/// anchor `PriceLine` at the correct screen offset — see
/// `bracket_quick_create_line` in `mod.rs`.
pub fn quick_create_above_group(bracket: &OrderBracket) -> Option<DecoratorGroup> {
    let (leg_present, action, fill) = match bracket.side {
        BracketSide::Long => (
            bracket.take_profit.is_some(),
            DecoratorAction::CreateTakeProfit,
            BRACKET_TP_COLOR,
        ),
        BracketSide::Short => (
            bracket.stop_loss.is_some(),
            DecoratorAction::CreateStopLoss,
            BRACKET_SL_COLOR,
        ),
    };
    if leg_present {
        return None;
    }
    Some(single_button_group(
        QUICK_CREATE_ABOVE_GROUP_ID,
        '^',
        fill,
        action,
    ))
}

/// Mirror of [`quick_create_above_group`] for the slot below the entry
/// line. Adds SL for Long brackets (orange + `v`) and TP for Short
/// brackets (teal + `v`). Returns `None` when the leg is already present.
pub fn quick_create_below_group(bracket: &OrderBracket) -> Option<DecoratorGroup> {
    let (leg_present, action, fill) = match bracket.side {
        BracketSide::Long => (
            bracket.stop_loss.is_some(),
            DecoratorAction::CreateStopLoss,
            BRACKET_SL_COLOR,
        ),
        BracketSide::Short => (
            bracket.take_profit.is_some(),
            DecoratorAction::CreateTakeProfit,
            BRACKET_TP_COLOR,
        ),
    };
    if leg_present {
        return None;
    }
    Some(single_button_group(
        QUICK_CREATE_BELOW_GROUP_ID,
        'v',
        fill,
        action,
    ))
}

fn single_button_group(
    group_id: u16,
    glyph: char,
    fill: [f32; 4],
    action: DecoratorAction,
) -> DecoratorGroup {
    DecoratorGroup {
        group_id,
        anchor: DecoratorAnchor::RightEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![DecoratorItem {
            visibility: Visibility::OnLineHover,
            action: Some(action),
            content: ItemContent::Button(Button {
                shape: BadgeShape::Rounded { radius: 2.0 },
                fill,
                hover_fill: Some(lighten(fill, 0.2)),
                glyph,
                glyph_color: [1.0, 1.0, 1.0, 1.0],
                glyph_size: 11.0,
                size: [22.0, 14.0],
                border: None,
            }),
        }],
    }
}
