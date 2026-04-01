//! Order bracket annotation: entry + optional TP/SL.
//!
//! A compound annotation representing a trade idea. The chart crate
//! sees these as pure visual geometry. The app layer maps them to
//! broker orders.

use super::level::LineStyle;
use serde::{Deserialize, Serialize};

/// An order bracket: entry line + optional take-profit and stop-loss.
///
/// The chart crate uses `BracketStatus` for visual styling only.
/// The app layer maps brackets to broker order instances.
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
}

/// A single leg of an order bracket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BracketLeg {
    /// Price level for this leg.
    pub price: f64,
    /// Optional time anchor. None = full-width ray from left edge.
    pub timestamp: Option<i64>,
    /// Override color. If None, derived from BracketSide + leg role.
    pub color: Option<[f32; 4]>,
    /// Line style for this leg.
    pub style: LineStyle,
    /// Line thickness in logical pixels.
    pub line_width: f32,
    /// Text shown next to the price label.
    pub label: Option<String>,
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

impl OrderBracket {
    /// Compute risk:reward ratio. Returns None if TP or SL is missing,
    /// or if risk is effectively zero.
    pub fn risk_reward(&self) -> Option<f64> {
        let tp = self.take_profit.as_ref()?;
        let sl = self.stop_loss.as_ref()?;
        let risk = (self.entry.price - sl.price).abs();
        let reward = (tp.price - self.entry.price).abs();
        if risk < f64::EPSILON {
            return None;
        }
        Some(reward / risk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bracket(entry: f64, tp: f64, sl: f64) -> OrderBracket {
        OrderBracket {
            entry: BracketLeg {
                price: entry,
                timestamp: None,
                color: None,
                style: LineStyle::default(),
                line_width: 1.0,
                label: None,
            },
            take_profit: Some(BracketLeg {
                price: tp,
                timestamp: None,
                color: None,
                style: LineStyle::default(),
                line_width: 1.0,
                label: None,
            }),
            stop_loss: Some(BracketLeg {
                price: sl,
                timestamp: None,
                color: None,
                style: LineStyle::default(),
                line_width: 1.0,
                label: None,
            }),
            side: BracketSide::Long,
            status: BracketStatus::Draft,
            quantity: None,
        }
    }

    #[test]
    fn risk_reward_long() {
        let b = make_bracket(100.0, 110.0, 95.0);
        let rr = b.risk_reward().unwrap();
        assert!((rr - 2.0).abs() < 0.01, "expected R:R 2.0, got {}", rr);
    }

    #[test]
    fn risk_reward_short() {
        let mut b = make_bracket(100.0, 90.0, 105.0);
        b.side = BracketSide::Short;
        let rr = b.risk_reward().unwrap();
        assert!((rr - 2.0).abs() < 0.01, "expected R:R 2.0, got {}", rr);
    }

    #[test]
    fn risk_reward_missing_tp() {
        let mut b = make_bracket(100.0, 110.0, 95.0);
        b.take_profit = None;
        assert!(b.risk_reward().is_none());
    }

    #[test]
    fn risk_reward_missing_sl() {
        let mut b = make_bracket(100.0, 110.0, 95.0);
        b.stop_loss = None;
        assert!(b.risk_reward().is_none());
    }

    #[test]
    fn risk_reward_zero_risk() {
        let b = make_bracket(100.0, 110.0, 100.0);
        assert!(b.risk_reward().is_none());
    }

    #[test]
    fn serde_round_trip() {
        let b = make_bracket(185.50, 192.0, 182.0);
        let json = serde_json::to_string(&b).expect("serialize");
        let decoded: OrderBracket = serde_json::from_str(&json).expect("deserialize");
        assert!((decoded.entry.price - 185.50).abs() < f64::EPSILON);
        assert_eq!(decoded.side, BracketSide::Long);
        assert_eq!(decoded.status, BracketStatus::Draft);
    }

    #[test]
    fn bracket_status_default_is_draft() {
        assert_eq!(BracketStatus::default(), BracketStatus::Draft);
    }
}
