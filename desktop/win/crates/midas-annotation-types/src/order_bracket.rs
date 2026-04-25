//! Order bracket annotation: entry + optional TP/SL.
//!
//! Moved from `midas-chart/src/widget/order_bracket/mod.rs` (Slice A1).
//! Carries the data types (`OrderBracket`, `BracketLeg`, `BracketSide`,
//! `BracketStatus`, `LegRole`, `EntryType`) plus the pure-data helpers
//! (`risk_reward`, `dollar_risk`, `dollar_reward`, `leg_style`,
//! `is_leg_on_wrong_side`).
//!
//! The chart-rendering helpers (`bracket_zone_rects`,
//! `compute_bracket`, `format_*_label`, etc.) stay in `midas-chart` —
//! they depend on chart-only `GridLineInstance`/`ComputeContext`/
//! `WidgetOutput` types.
//!
//! A compound annotation representing a trade idea. The chart crate
//! sees these as pure visual geometry. The app layer maps them to
//! broker orders.

use crate::line_style::LineStyle;
use crate::price_line::{LineStroke, PriceLine};
use serde::{Deserialize, Serialize};
use smallvec::smallvec;

/// Entry order type for a bracket. Defined in midas-annotation-types
/// (not broker) to maintain sans-IO boundary. The app layer maps
/// `EntryType` → broker `OrderKind` at the bridge.
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

/// Chart-local leg role enum. Defined in midas-annotation-types — NOT
/// imported from midas-broker. The chart-data layer must not depend on
/// broker types. The app layer maps BracketRole (broker) to LegRole
/// (chart) when creating annotations.
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
pub const BRACKET_TP_COLOR: [f32; 4] = [0.10, 0.85, 0.85, 1.0];
/// Orange stop-loss line (all brackets).
pub const BRACKET_SL_COLOR: [f32; 4] = [1.0, 0.60, 0.0, 1.0];
/// Green entry line for long positions (Market / Limit).
pub const BRACKET_LONG_ENTRY_COLOR: [f32; 4] = [0.20, 0.78, 0.35, 1.0];
/// Red entry line for short positions (Market / Limit).
pub const BRACKET_SHORT_ENTRY_COLOR: [f32; 4] = [0.90, 0.25, 0.25, 1.0];
/// Green entry for Long Stop orders.
pub const BRACKET_LONG_STOP_COLOR: [f32; 4] = [0.20, 0.78, 0.35, 1.0];
/// Lime green entry for Long StopLimit orders.
pub const BRACKET_LONG_STOP_LIMIT_COLOR: [f32; 4] = [0.50, 0.90, 0.20, 1.0];
/// Red entry for Short Stop orders.
pub const BRACKET_SHORT_STOP_COLOR: [f32; 4] = [0.90, 0.25, 0.25, 1.0];
/// Pink-red entry for Short StopLimit orders.
pub const BRACKET_SHORT_STOP_LIMIT_COLOR: [f32; 4] = [0.90, 0.30, 0.50, 1.0];

/// Amber fill used to warn when a bracket leg is on the "wrong" side
/// of entry given the bracket's side. The leg is still rendered at its
/// user-chosen price; the amber tint signals that it does not match
/// the bracket's direction.
pub const BRACKET_WARNING_COLOR: [f32; 4] = [0.95, 0.70, 0.18, 1.0];

// ── Wrong-side classification ────────────────────────────────────────

/// Returns true when `leg_role` is on the directional side of
/// `entry_price` that contradicts `bracket_side` — i.e., a Long TP
/// priced at or below entry, a Long SL priced at or above entry, a
/// Short TP priced at or above entry, or a Short SL priced at or below
/// entry.
///
/// Entry and StopTrigger legs always return false — they are defined
/// relative to the bracket's own mechanics, not TP/SL direction.
///
/// This is a pure render-time classifier: the underlying leg price is
/// never modified. Callers (decorators, line-stroke helpers) tint the
/// leg amber when this returns true; see
/// [`BRACKET_WARNING_COLOR`].
pub fn is_leg_on_wrong_side(
    bracket_side: BracketSide,
    leg_role: LegRole,
    leg_price: f64,
    entry_price: f64,
) -> bool {
    match (bracket_side, leg_role) {
        (BracketSide::Long, LegRole::TakeProfit) => leg_price <= entry_price,
        (BracketSide::Long, LegRole::StopLoss) => leg_price >= entry_price,
        (BracketSide::Short, LegRole::TakeProfit) => leg_price >= entry_price,
        (BracketSide::Short, LegRole::StopLoss) => leg_price <= entry_price,
        (_, LegRole::Entry | LegRole::StopTrigger) => false,
    }
}

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
