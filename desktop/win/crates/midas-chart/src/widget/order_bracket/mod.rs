//! Order bracket annotation: entry + optional TP/SL.
//!
//! A compound annotation representing a trade idea. The chart crate
//! sees these as pure visual geometry. The app layer maps them to
//! broker orders.

use super::level::LineStyle;
use super::price_line::{LineStroke, PriceLine};
use serde::{Deserialize, Serialize};
use smallvec::smallvec;

pub mod decorators;

/// Entry order type for a bracket. Defined in midas-chart (not broker)
/// to maintain sans-IO boundary. The app layer maps `EntryType` →
/// broker `OrderKind` at the bridge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntryType {
    /// Market order — entry tracks last price, not draggable.
    #[default]
    Market,
    /// Limit order — entry at user-specified limit price.
    Limit,
    /// Stop order — entry at user-specified stop trigger price.
    Stop,
    /// Stop-Limit order — stop trigger + limit execution price.
    StopLimit,
}

/// An order bracket: entry line + optional take-profit and stop-loss.
///
/// The chart crate uses `BracketStatus` for visual styling only.
/// The app layer maps brackets to broker order instances.
///
/// # `entry.price` semantics by `entry_type`
///
/// - **Market / Limit**: target fill price (exact for risk calculations).
/// - **Stop**: trigger price (fill is approximate — market at trigger).
/// - **StopLimit**: limit execution price; the stop trigger lives in
///   `entry_stop_price`. Risk calculations use `entry.price` as-is,
///   which is exact for Limit and approximate for Stop (acceptable V1).
// TODO: consider EntryPrice enum to make per-type semantics compiler-enforced
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderBracket {
    /// The entry price line. Always present.
    pub entry: BracketLeg,
    /// Take-profit target. None if user hasn't set one yet.
    pub take_profit: Option<BracketLeg>,
    /// Stop-loss level. None if user hasn't set one yet.
    pub stop_loss: Option<BracketLeg>,
    /// Trade direction. Determines which side TP/SL go on.
    pub side: BracketSide,
    /// Visual status. Used for styling only in chart crate.
    pub status: BracketStatus,
    /// Display quantity (informational label, not order routing).
    pub quantity: Option<f64>,
    /// Whether the bracket has been explicitly saved/pinned by the user.
    /// Saved drafts survive [X] toggle and render at higher alpha.
    #[serde(default)]
    pub saved: bool,
    /// Quantity filled so far. Set by the app layer from execution
    /// reports. Used by `format_entry_label()` for partial-fill display.
    #[serde(default)]
    pub filled_qty: Option<f64>,
    /// Entry order type. Defaults to Market for backward compat.
    #[serde(default)]
    pub entry_type: EntryType,
    /// Stop trigger price for StopLimit entries. `None` for all other types.
    /// When `entry_type == StopLimit`, `entry.price` is the limit price
    /// and this field holds the stop trigger price.
    #[serde(default)]
    pub entry_stop_price: Option<f64>,
    /// True when the entry price is on the "wrong" side of market
    /// (e.g., Limit BUY above ask). Set by the app layer; rendered
    /// as amber warning on the entry label by the chart.
    #[serde(default)]
    pub wrong_side_warning: bool,
}

/// A single leg of an order bracket.
///
/// **Slice 8a-i shape**: line geometry (price, extent, stroke) lives inside
/// `line: PriceLine`, mirroring `HorizontalLevel`. The old top-level
/// `color`/`line_width`/`style`/`label`/`timestamp` fields are gone:
///
/// - `color` / `line_width` / `style` → `line.stroke.{color,width,style}`
/// - `timestamp` → `line.extent` (`FullWidth` vs `RightFrom { timestamp }`)
/// - `label` → rebuilt at decorator-build time (Slice 8a-ii)
///
/// `role` is now stored on the leg itself so a `BracketLeg` is fully
/// self-describing for the rendering pipeline. Projected P&L stays here as
/// wire data set by the app layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BracketLeg {
    /// Line geometry: price, extent (full width or right-from-timestamp),
    /// and stroke (color/width/style).
    pub line: PriceLine,
    /// Which leg of the bracket this is (entry, TP, SL, stop trigger).
    pub role: LegRole,
    /// Projected dollar P&L at this leg's price level.
    /// Computed by the app layer using entry fill price and quantity.
    #[serde(default)]
    pub projected_pnl: Option<f64>,
    /// Projected percentage P&L.
    #[serde(default)]
    pub projected_pnl_pct: Option<f64>,
}

/// Trade direction for a bracket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BracketSide {
    /// Long position: entry below TP, above SL.
    Long,
    /// Short position: entry above TP, below SL.
    Short,
}

/// Visual status of a bracket. Drives line style and opacity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BracketStatus {
    /// Being drawn on chart, not yet actionable. Dashed lines.
    #[default]
    Draft,
    /// Submitted to broker, awaiting entry fill. Dotted lines.
    Pending,
    /// Entry partially filled.
    PartialFill,
    /// Entry filled, TP/SL orders live at broker. Solid lines.
    Active,
    /// TP or SL triggered, position closed. Dimmed solid lines.
    Closed,
    /// User or broker cancelled. Dimmed solid lines.
    Cancelled,
}

/// Chart-local leg role enum. Defined in midas-chart — NOT imported from
/// midas-broker. The chart crate must not depend on broker types.
/// The app layer maps BracketRole (broker) to LegRole (chart) when
/// creating annotations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LegRole {
    Entry,
    TakeProfit,
    StopLoss,
    /// Stop trigger price for StopLimit entries (separate from the limit
    /// execution price stored in `Entry`).
    StopTrigger,
}

impl OrderBracket {
    /// Compute risk:reward ratio. Returns None if TP or SL is missing,
    /// or if risk is effectively zero.
    pub fn risk_reward(&self) -> Option<f64> {
        let tp = self.take_profit.as_ref()?;
        let sl = self.stop_loss.as_ref()?;
        let risk = (self.entry.line.price - sl.line.price).abs();
        let reward = (tp.line.price - self.entry.line.price).abs();
        if risk < f64::EPSILON {
            return None;
        }
        Some(reward / risk)
    }

    /// Absolute dollar risk. Returns None if SL is missing or qty is zero.
    pub fn dollar_risk(&self) -> Option<f64> {
        let sl = self.stop_loss.as_ref()?;
        let qty = self.quantity?;
        Some((self.entry.line.price - sl.line.price).abs() * qty)
    }

    /// Absolute dollar reward. Returns None if TP is missing or qty is zero.
    pub fn dollar_reward(&self) -> Option<f64> {
        let tp = self.take_profit.as_ref()?;
        let qty = self.quantity?;
        Some((tp.line.price - self.entry.line.price).abs() * qty)
    }
}

// ── Default bracket colors (RGBA, linear space) ──────────────────────

/// Teal take-profit line and badge fill. Teal gives TP a distinct
/// signature against the green side-palette used by Long entries.
const BRACKET_TP_COLOR: [f32; 4] = [0.10, 0.85, 0.85, 1.0];
/// Orange stop-loss line (all brackets).
const BRACKET_SL_COLOR: [f32; 4] = [1.0, 0.60, 0.0, 1.0];
/// Green entry line for long positions (Market / Limit).
const BRACKET_LONG_ENTRY_COLOR: [f32; 4] = [0.20, 0.78, 0.35, 1.0];
/// Red entry line for short positions (Market / Limit).
const BRACKET_SHORT_ENTRY_COLOR: [f32; 4] = [0.90, 0.25, 0.25, 1.0];
/// Green entry for Long Stop orders.
const BRACKET_LONG_STOP_COLOR: [f32; 4] = [0.20, 0.78, 0.35, 1.0];
/// Lime green entry for Long StopLimit orders.
const BRACKET_LONG_STOP_LIMIT_COLOR: [f32; 4] = [0.50, 0.90, 0.20, 1.0];
/// Red entry for Short Stop orders.
const BRACKET_SHORT_STOP_COLOR: [f32; 4] = [0.90, 0.25, 0.25, 1.0];
/// Pink-red entry for Short StopLimit orders.
const BRACKET_SHORT_STOP_LIMIT_COLOR: [f32; 4] = [0.90, 0.30, 0.50, 1.0];
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

impl OrderBracket {
    /// Compute the canonical `LineStroke` for a leg based on bracket status.
    ///
    /// Returns a `LineStroke` carrying color, width, and dash style. The
    /// base color comes from the leg role and entry type: entry color
    /// depends on `(side, entry_type)`, TP = green, SL = orange. The
    /// bracket status modulates line style, width, and alpha. Draft
    /// brackets additionally distinguish saved (0.65) from unsaved (0.50).
    ///
    /// SL lines are always dotted regardless of status (user requirement
    /// for visual distinction). Other legs follow the standard
    /// status → style mapping. `compute_bracket()` recomputes this every
    /// frame and stamps the result onto each leg's `line.stroke` before
    /// emission, so per-leg stored strokes do not need to be kept fresh
    /// outside the render path.
    pub fn leg_style(&self, role: LegRole) -> LineStroke {
        // SL has a dedicated code path: always dotted, always orange.
        if role == LegRole::StopLoss {
            let (width, alpha_mult) = match self.status {
                BracketStatus::Draft => (1.0, if self.saved { 0.65 } else { 0.50 }),
                BracketStatus::Pending => (1.0, 0.80),
                BracketStatus::PartialFill => (1.5, 0.90),
                BracketStatus::Active => (1.5, 1.0),
                BracketStatus::Closed => (1.0, 0.30),
                BracketStatus::Cancelled => (1.0, 0.20),
            };
            let mut color = BRACKET_SL_COLOR;
            color[3] *= alpha_mult;
            return LineStroke {
                color,
                width,
                style: LineStyle::dotted(),
            };
        }

        let base_color = match role {
            LegRole::Entry | LegRole::StopTrigger => match (self.side, self.entry_type) {
                (BracketSide::Long, EntryType::Stop) => BRACKET_LONG_STOP_COLOR,
                (BracketSide::Long, EntryType::StopLimit) => BRACKET_LONG_STOP_LIMIT_COLOR,
                (BracketSide::Long, _) => BRACKET_LONG_ENTRY_COLOR,
                (BracketSide::Short, EntryType::Stop) => BRACKET_SHORT_STOP_COLOR,
                (BracketSide::Short, EntryType::StopLimit) => BRACKET_SHORT_STOP_LIMIT_COLOR,
                (BracketSide::Short, _) => BRACKET_SHORT_ENTRY_COLOR,
            },
            LegRole::TakeProfit => BRACKET_TP_COLOR,
            LegRole::StopLoss => unreachable!(),
        };

        let (style, width, alpha_mult) = match self.status {
            BracketStatus::Draft => {
                let alpha = if self.saved { 0.65 } else { 0.50 };
                (LineStyle::Pattern(smallvec![6.0, 4.0]), 1.0, alpha)
            }
            BracketStatus::Pending => (LineStyle::dotted(), 1.0, 0.80),
            BracketStatus::PartialFill => (LineStyle::Solid, 1.5, 0.90),
            BracketStatus::Active => (LineStyle::Solid, 1.5, 1.0),
            BracketStatus::Closed => (LineStyle::Solid, 1.0, 0.30),
            BracketStatus::Cancelled => (LineStyle::Solid, 1.0, 0.20),
        };

        let mut color = base_color;
        color[3] *= alpha_mult;
        LineStroke {
            color,
            width,
            style,
        }
    }
}

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
