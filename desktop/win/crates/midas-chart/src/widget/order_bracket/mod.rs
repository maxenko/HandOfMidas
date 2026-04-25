//! Order bracket annotation: entry + optional TP/SL.
//!
//! Slice A1 of `plan/arch-review-fixes/01-group-a-types-extraction.md`
//! moved the data types (`OrderBracket`, `BracketLeg`, `BracketSide`,
//! `BracketStatus`, `LegRole`, `EntryType`) and the pure-data helpers
//! (`risk_reward`, `dollar_risk`, `dollar_reward`, `leg_style`,
//! `is_leg_on_wrong_side`) into the leaf crate `midas-annotation-types`.
//!
//! What stays here is the chart-only render path: `compute_bracket`,
//! `bracket_zone_rects`, the `format_*_label` helpers, the
//! `quick_create_anchor_line` builder, and the
//! `decorators` submodule that emits `DecoratorGroup`s. These all
//! depend on chart-only `GridLineInstance`/`ComputeContext`/
//! `WidgetOutput` types and cannot live in the data crate.
//!
//! A compound annotation representing a trade idea. The chart crate
//! sees these as pure visual geometry. The app layer maps them to
//! broker orders.

use super::level::LineStyle;
use super::price_line::PriceLine;
use smallvec::smallvec;

pub mod decorators;

// ── Data types: re-exported from the new leaf crate. ──────────────
// `OrderBracket`, `BracketLeg`, `BracketSide`, `BracketStatus`,
// `LegRole`, `EntryType`, `is_leg_on_wrong_side`, and
// `BRACKET_WARNING_COLOR` all live in
// `midas-annotation-types::order_bracket`. The inherent methods
// `risk_reward`, `dollar_risk`, `dollar_reward`, and `leg_style` are
// inherent on `OrderBracket` over there, so call sites that hold an
// `OrderBracket` keep their `bracket.leg_style(role)` form.
//
// A1b added `#[deprecated]` after consumer-side migration so any new
// import through `midas_chart::widget::order_bracket::*` is caught by
// clippy as a hard error under `-D warnings`.
#[deprecated(
    note = "import from midas_annotation_types::order_bracket directly; midas-chart will be deleted in slice 9c"
)]
pub use midas_annotation_types::order_bracket::{
    is_leg_on_wrong_side, BracketLeg, BracketSide, BracketStatus, EntryType, LegRole, OrderBracket,
    BRACKET_LONG_ENTRY_COLOR, BRACKET_LONG_STOP_COLOR, BRACKET_LONG_STOP_LIMIT_COLOR,
    BRACKET_SHORT_ENTRY_COLOR, BRACKET_SHORT_STOP_COLOR, BRACKET_SHORT_STOP_LIMIT_COLOR,
    BRACKET_SL_COLOR, BRACKET_TP_COLOR, BRACKET_WARNING_COLOR,
};

// ── Chart-only zone fill colors (RGBA, linear space) ──────────────
// Stay in midas-chart because only the chart compute path consumes
// them. The line-stroke colors moved with `leg_style` to the data
// crate.

/// Green zone fill at 6% alpha (between entry and TP).
const BRACKET_TP_ZONE: [f32; 4] = [0.20, 0.78, 0.35, 0.06];
/// Orange zone fill at 6% alpha (between entry and SL).
const BRACKET_SL_ZONE: [f32; 4] = [1.0, 0.60, 0.0, 0.06];

/// Minimum vertical gap (logical px) between the TP and SL decorator
/// badges before they are considered visually overlapping.
///
/// The badges are 20 logical px tall (see `Badge { height: 20.0 }` in
/// `decorators.rs`); 22 px leaves a one-pixel clearance on either side
/// of the midpoint when the stacking branch kicks in. Chosen to match
/// the screen-space heuristic described in Slice 0 of the
/// ticker-order-state plan — when the TP and SL price lines land within
/// this many screen pixels of each other, `compute_bracket()` shifts
/// each decorator group's anchor by half of this gap so the badges
/// stack vertically instead of overlapping horizontally.
const BRACKET_BADGE_HEIGHT_PX: f32 = 20.0;
/// Screen-space threshold for the TP/SL overlap stacking heuristic.
/// Pads the badge height by 2 px so stacked badges never touch.
const BADGE_STACK_GAP_PX: f32 = BRACKET_BADGE_HEIGHT_PX + 2.0;

// ── Phase 3: chart rendering helpers ─────────────────────────────────

/// Compute zone fill rectangles for the TP and SL regions of a bracket.
///
/// Zone fills are only drawn when the bracket is `Active`. Each zone is a
/// translucent rectangle spanning from the entry price to the TP or SL
/// price across the full chart width.
///
/// Returns a `Vec` of `(rect, color)` tuples where `rect` is
/// `[left, top, right, bottom]` in screen pixels.
pub fn bracket_zone_rects(
    bracket: &OrderBracket,
    entry_y: f32,
    chart_width: f32,
    price_to_y: impl Fn(f64) -> f32,
) -> Vec<([f32; 4], [f32; 4])> {
    if bracket.status != BracketStatus::Active && bracket.status != BracketStatus::PartialFill {
        return Vec::new();
    }

    let mut zones = Vec::new();

    if let Some(ref tp) = bracket.take_profit {
        let tp_y = price_to_y(tp.line.price);
        let top = entry_y.min(tp_y);
        let bottom = entry_y.max(tp_y);
        zones.push(([0.0, top, chart_width, bottom], BRACKET_TP_ZONE));
    }

    if let Some(ref sl) = bracket.stop_loss {
        let sl_y = price_to_y(sl.line.price);
        let top = entry_y.min(sl_y);
        let bottom = entry_y.max(sl_y);
        zones.push(([0.0, top, chart_width, bottom], BRACKET_SL_ZONE));
    }

    zones
}

// ── Label formatting helpers ─────────────────────────────────────────

/// Entry type label prefix. Market has no prefix (backward compat).
fn entry_type_prefix(entry_type: EntryType) -> &'static str {
    match entry_type {
        EntryType::Market => "",
        EntryType::Limit => "LMT ",
        EntryType::Stop => "STP ",
        EntryType::StopLimit => "STP LMT ",
    }
}

/// Format the price portion of the entry label.
///
/// For StopLimit, shows `"stop/limit"` (e.g., `"185.00/184.50"`).
/// For all other types, shows the single entry price.
fn format_entry_price(bracket: &OrderBracket) -> String {
    if bracket.entry_type == EntryType::StopLimit {
        if let Some(stop_price) = bracket.entry_stop_price {
            return format!("{:.2}/{:.2}", stop_price, bracket.entry.line.price);
        }
    }
    format!("{:.2}", bracket.entry.line.price)
}

/// Format entry label based on bracket status, side, and entry type.
///
/// - **Draft**: `"BUY @ 171.59"` / `"LMT BUY @ 180.00"` / `"STP LMT BUY @ 185.00/184.50"`
/// - **Pending**: `"BUY @ 171.59  ⏳"` / `"LMT BUY @ 180.00  ⏳"`
/// - **PartialFill**: `"BUY @ 171.59  ◑ 50/100sh"`
/// - **Active / Closed / Cancelled**: `"▲ 171.59  100sh"` (original)
pub fn format_entry_label(bracket: &OrderBracket) -> String {
    let prefix = entry_type_prefix(bracket.entry_type);
    let price_str = format_entry_price(bracket);

    match bracket.status {
        BracketStatus::Draft => {
            let verb = match bracket.side {
                BracketSide::Long => "BUY",
                BracketSide::Short => "SELL",
            };
            format!("{}{} @ {}", prefix, verb, price_str)
        }
        BracketStatus::Pending => {
            let verb = match bracket.side {
                BracketSide::Long => "BUY",
                BracketSide::Short => "SELL",
            };
            format!("{}{} @ {}  \u{23F3}", prefix, verb, price_str)
        }
        BracketStatus::PartialFill => {
            let verb = match bracket.side {
                BracketSide::Long => "BUY",
                BracketSide::Short => "SELL",
            };
            let filled = bracket.filled_qty.unwrap_or(0.0);
            let total = bracket.quantity.unwrap_or(0.0);
            format!(
                "{}{} @ {}  \u{25D1} {:.0}/{:.0}sh",
                prefix, verb, price_str, filled, total
            )
        }
        BracketStatus::Active | BracketStatus::Closed | BracketStatus::Cancelled => {
            let arrow = match bracket.side {
                BracketSide::Long => "\u{25B2}",
                BracketSide::Short => "\u{25BC}",
            };
            let qty_str = bracket
                .quantity
                .map(|q| format!("  {:.0}sh", q))
                .unwrap_or_default();
            format!("{} {:.2}{}", arrow, bracket.entry.line.price, qty_str)
        }
    }
}

/// Format TP label: "TP 192.00  +$650" or "TP 192.00".
pub fn format_tp_label(leg: &BracketLeg) -> String {
    let pnl_str = leg
        .projected_pnl
        .map(|pnl| format!("  +${:.0}", pnl.abs()))
        .unwrap_or_default();
    format!("TP {:.2}{}", leg.line.price, pnl_str)
}

/// Format SL label. Draft brackets show `"SL @ {price:.2}"`.
/// Other statuses show `"SL {price:.2}"` with optional P&L.
pub fn format_sl_label(leg: &BracketLeg, status: BracketStatus) -> String {
    if status == BracketStatus::Draft {
        return format!("SL @ {:.2}", leg.line.price);
    }
    let pnl_str = leg
        .projected_pnl
        .map(|pnl| format!("  -${:.0}", pnl.abs()))
        .unwrap_or_default();
    format!("SL {:.2}{}", leg.line.price, pnl_str)
}

// ── Phase 3: compute_bracket ────────────────────────────────────────

use self::decorators::{
    entry_decorator_group, quick_create_above_group, quick_create_below_group, sl_decorator_group,
    tp_decorator_group,
};
use super::compute::{ComputeContext, LabelAnchor, WidgetLabel, WidgetOutput};
use super::decorator::compute_decorator_group;
use super::hit_test::{CursorIcon, HitZone, HitZoneKind};
use super::level::segmented_line;
use super::AnnotationId;
use crate::instances::GridLineInstance;

/// Emit the line geometry and optional drag hit-zone for a single
/// bracket leg.
///
/// Slice 8a-ii: a local analogue of
/// `widget::level::compute_price_line_geometry` that (a) uses the
/// bracket-specific `HitZoneKind` variant for the hover-width detection
/// and drag hit zone, and (b) skips the level-specific selection glow
/// path since brackets do not carry a selection affordance in the same
/// place. The decorator group anchored to this leg's `PriceLine` is
/// emitted separately via `compute_decorator_group()`.
fn emit_bracket_leg_line(
    output: &mut WidgetOutput,
    line: &PriceLine,
    annotation_id: AnnotationId,
    ctx: &ComputeContext<'_>,
    alpha: f32,
    hit_kind: HitZoneKind,
    draw_hit_zone: bool,
) {
    let vp_width = ctx.viewport.width as f32;
    let y = ctx.camera.price_to_y(line.price);

    let hovered = ctx
        .hovered_annotation
        .map(|(aid, kind)| aid == annotation_id && kind == hit_kind)
        .unwrap_or(false);
    let mut width = line.stroke.width;
    if hovered {
        width += 1.0;
    }

    let mut color = line.stroke.color;
    color[3] *= alpha;

    output.lines.extend(segmented_line(
        0.0,
        vp_width,
        y,
        width,
        color,
        &line.stroke.style,
    ));

    if draw_hit_zone {
        output.hit_zones.push(HitZone {
            annotation_id,
            rect: [0.0, y - 6.0, vp_width, y + 6.0],
            kind: hit_kind,
            cursor: CursorIcon::ResizeNS,
        });
    }
}

/// Which slot a quick-create button occupies relative to the entry
/// line. `Above` sits at lower screen Y (higher price); `Below` at
/// higher screen Y (lower price).
pub enum QuickCreateSlot {
    Above,
    Below,
}

/// Build the synthetic `PriceLine` that anchors a quick-create button
/// group. Shared between `compute_bracket` (render path) and the click
/// hit-test paths so both agree on the button's rect — critical so a
/// click does not pass through the button into the underlying entry
/// badge.
pub fn quick_create_anchor_line(
    entry_line: &PriceLine,
    ctx: &ComputeContext<'_>,
    slot: QuickCreateSlot,
) -> PriceLine {
    let base_y = ctx.camera.price_to_y(entry_line.price);
    let offset = BADGE_STACK_GAP_PX * 1.1;
    let target_y = match slot {
        QuickCreateSlot::Above => base_y - offset,
        QuickCreateSlot::Below => base_y + offset,
    };
    PriceLine {
        price: ctx.camera.y_to_price(target_y),
        extent: entry_line.extent,
        stroke: entry_line.stroke.clone(),
    }
}

/// Check if a bracket has data that contradicts its entry_type.
fn needs_render_sanitize(bracket: &OrderBracket) -> bool {
    match bracket.entry_type {
        EntryType::Market | EntryType::Limit | EntryType::Stop => {
            bracket.entry_stop_price.is_some()
        }
        EntryType::StopLimit => false,
    }
}

/// Force bracket data to match entry_type rules at render time.
/// Last line of defense — called only when `needs_render_sanitize` is true.
fn render_sanitize(bracket: &mut OrderBracket) {
    match bracket.entry_type {
        EntryType::Market | EntryType::Limit | EntryType::Stop => {
            bracket.entry_stop_price = None;
        }
        EntryType::StopLimit => {}
    }
}

/// Compute render primitives for a bracket annotation.
///
/// Produces lines for entry/TP/SL, zone fills (Active brackets only),
/// hit zones for interaction, and text labels for prices and R:R.
/// The `alpha` parameter is the annotation's `Presence::alpha()` value.
pub fn compute_bracket(
    bracket: &OrderBracket,
    annotation_id: AnnotationId,
    ctx: &ComputeContext<'_>,
    alpha: f32,
) -> WidgetOutput {
    // ── Rendering-level entry_type guard ────────────────────────────
    // Even if normalization was bypassed, force the data to match the
    // entry_type rules. This is the last line of defense — the chart
    // must never render lines that contradict the order type.
    let mut bracket_cow;
    let bracket = if needs_render_sanitize(bracket) {
        bracket_cow = bracket.clone();
        render_sanitize(&mut bracket_cow);
        &bracket_cow
    } else {
        bracket
    };

    let mut output = WidgetOutput::default();
    let vp_width = ctx.viewport.width as f32;

    // ── Entry line ──────────────────────────────────────────────────
    // Recompute the leg's stroke from the live (status, role) pair so that
    // any stale value stamped at construction time is overwritten before we
    // emit line + decorator primitives for the leg.
    let entry_stroke = bracket.leg_style(LegRole::Entry);
    let entry_line = PriceLine {
        price: bracket.entry.line.price,
        extent: bracket.entry.line.extent,
        stroke: entry_stroke.clone(),
    };
    let entry_y = ctx.camera.price_to_y(entry_line.price);
    // Market entries track last price and aren't user-draggable.
    // Limit / Stop / StopLimit entries are price-settable by the user.
    let entry_draggable = bracket.entry_type != EntryType::Market;
    emit_bracket_leg_line(
        &mut output,
        &entry_line,
        annotation_id,
        ctx,
        alpha,
        HitZoneKind::BracketEntry,
        /* draw_hit_zone */ entry_draggable,
    );
    output.merge(compute_decorator_group(
        &entry_decorator_group(bracket),
        &entry_line,
        annotation_id,
        ctx,
        alpha,
    ));

    // Quick-create buttons: hover-only `^` above and `v` below the
    // entry line, each emitted only when the leg it would create is
    // missing. The synthetic anchor lines are offset by
    // `BADGE_STACK_GAP_PX * 1.1` from the entry — a hair more than one
    // badge height so the button visually clears the entry badge.
    if let Some(group) = quick_create_above_group(bracket) {
        let line = quick_create_anchor_line(&entry_line, ctx, QuickCreateSlot::Above);
        output.merge(compute_decorator_group(
            &group,
            &line,
            annotation_id,
            ctx,
            alpha,
        ));
    }
    if let Some(group) = quick_create_below_group(bracket) {
        let line = quick_create_anchor_line(&entry_line, ctx, QuickCreateSlot::Below);
        output.merge(compute_decorator_group(
            &group,
            &line,
            annotation_id,
            ctx,
            alpha,
        ));
    }

    // ── Stop trigger line (StopLimit only) ─────────────────────────
    if bracket.entry_type == EntryType::StopLimit {
        if let Some(stop_price) = bracket.entry_stop_price {
            let stop_y = ctx.camera.price_to_y(stop_price);
            // Use base width from leg_style, not the potentially hover-boosted entry_width.
            let base_stroke = bracket.leg_style(LegRole::StopTrigger);
            let mut stop_width = base_stroke.width;
            let stop_hovered = ctx
                .hovered_annotation
                .map(|(aid, kind)| aid == annotation_id && kind == HitZoneKind::BracketStopTrigger)
                .unwrap_or(false);
            if stop_hovered {
                stop_width += 1.0;
            }
            // Use entry color but dashed to distinguish from the limit line.
            let stop_style = LineStyle::Pattern(smallvec![4.0, 3.0]);
            let mut ec = entry_stroke.color;
            ec[3] *= alpha;
            output.lines.extend(segmented_line(
                0.0,
                vp_width,
                stop_y,
                stop_width,
                ec,
                &stop_style,
            ));

            // Hit zone for dragging the stop trigger.
            if bracket.status == BracketStatus::Draft {
                output.hit_zones.push(HitZone {
                    annotation_id,
                    rect: [0.0, stop_y - 6.0, vp_width, stop_y + 6.0],
                    kind: HitZoneKind::BracketStopTrigger,
                    cursor: CursorIcon::ResizeNS,
                });
            }

            let stop_label = format!("STP {:.2}", stop_price);
            output.labels.push(WidgetLabel {
                text: stop_label,
                screen_x: vp_width - 10.0,
                screen_y: stop_y,
                bg_color: [0.12, 0.12, 0.15, 0.85 * alpha],
                text_color: ec,
                font_size: 11.0,
                anchor: LabelAnchor::Right,
            });
        }
    }

    // ── Take-profit & stop-loss lines ───────────────────────────────
    //
    // Slice 0 overlap stacking: when TP and SL price lines land so
    // close together in screen-space that their decorator badges would
    // overlap horizontally, we anchor each badge to a synthetic
    // `PriceLine` whose price is offset by half of `BADGE_STACK_GAP`
    // pixels (converted back to price via `camera.y_to_price()`). The
    // underlying leg lines and hit zones still emit at the real prices
    // — only the decorator anchor shifts so the badges stack cleanly
    // above and below the midpoint.
    //
    // Detection runs in screen space so it naturally handles any
    // combination of price scale and viewport height without having to
    // plumb `gatr_abs` through the compute pipeline.
    let tp_sl_badges_overlap = match (bracket.take_profit.as_ref(), bracket.stop_loss.as_ref()) {
        (Some(tp), Some(sl)) => {
            let tp_y = ctx.camera.price_to_y(tp.line.price);
            let sl_y = ctx.camera.price_to_y(sl.line.price);
            (tp_y - sl_y).abs() < BADGE_STACK_GAP_PX
        }
        _ => false,
    };

    if let Some(ref tp) = bracket.take_profit {
        let tp_stroke = bracket.leg_style(LegRole::TakeProfit);
        let tp_line = PriceLine {
            price: tp.line.price,
            extent: tp.line.extent,
            stroke: tp_stroke,
        };
        emit_bracket_leg_line(
            &mut output,
            &tp_line,
            annotation_id,
            ctx,
            alpha,
            HitZoneKind::BracketTP,
            /* draw_hit_zone */ true,
        );
        if let Some(group) = tp_decorator_group(bracket) {
            // When TP/SL badges would overlap, shift the TP badge anchor
            // upward by half a badge gap (i.e. toward higher prices on
            // an inverted-Y chart — screen-Y decreases as price rises).
            let decorator_line = if tp_sl_badges_overlap {
                let base_y = ctx.camera.price_to_y(tp.line.price);
                let target_y = base_y - BADGE_STACK_GAP_PX * 0.5;
                PriceLine {
                    price: ctx.camera.y_to_price(target_y),
                    extent: tp_line.extent,
                    stroke: tp_line.stroke.clone(),
                }
            } else {
                tp_line.clone()
            };
            output.merge(compute_decorator_group(
                &group,
                &decorator_line,
                annotation_id,
                ctx,
                alpha,
            ));
        }
    }

    if let Some(ref sl) = bracket.stop_loss {
        let sl_stroke = bracket.leg_style(LegRole::StopLoss);
        let sl_line = PriceLine {
            price: sl.line.price,
            extent: sl.line.extent,
            stroke: sl_stroke,
        };
        emit_bracket_leg_line(
            &mut output,
            &sl_line,
            annotation_id,
            ctx,
            alpha,
            HitZoneKind::BracketSL,
            /* draw_hit_zone */ true,
        );
        if let Some(group) = sl_decorator_group(bracket) {
            // Symmetric shift: SL badge moves downward in screen space
            // (toward lower prices) to clear the TP badge.
            let decorator_line = if tp_sl_badges_overlap {
                let base_y = ctx.camera.price_to_y(sl.line.price);
                let target_y = base_y + BADGE_STACK_GAP_PX * 0.5;
                PriceLine {
                    price: ctx.camera.y_to_price(target_y),
                    extent: sl_line.extent,
                    stroke: sl_line.stroke.clone(),
                }
            } else {
                sl_line.clone()
            };
            output.merge(compute_decorator_group(
                &group,
                &decorator_line,
                annotation_id,
                ctx,
                alpha,
            ));
        }
    }

    // ── Zone fills (Active brackets only) ───────────────────────────
    let zones = bracket_zone_rects(bracket, entry_y, vp_width, |p| ctx.camera.price_to_y(p));
    for (rect, mut zone_color) in zones {
        zone_color[3] *= alpha;
        output.fills.push(GridLineInstance {
            rect,
            color: zone_color,
        });
    }

    // ── R:R label (when both TP and SL are present) ─────────────────
    if let Some(rr) = bracket.risk_reward() {
        output.labels.push(WidgetLabel {
            text: format!("R:R {:.2}:1", rr),
            screen_x: vp_width - 120.0,
            screen_y: entry_y,
            bg_color: [0.12, 0.12, 0.15, 0.75 * alpha],
            text_color: [0.7, 0.7, 0.7, 1.0 * alpha],
            font_size: 10.0,
            anchor: LabelAnchor::Right,
        });
    }

    output
}

#[cfg(test)]
mod tests;
