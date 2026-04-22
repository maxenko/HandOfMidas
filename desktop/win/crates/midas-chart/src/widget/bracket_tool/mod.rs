//! Bracket drawing tool: 3-click state machine for placing order brackets.
//!
//! Click 1: entry price, Click 2: take-profit, Click 3: stop-loss.
//!
//! The tool does **not** enforce directional constraints. Legs are
//! placed wherever the trader clicks; brackets whose TP/SL land on the
//! "wrong" side of entry are classified visually by the decorator
//! layer at render time (see
//! `plan/live-sim-and-free-brackets.md`). Escape cancels at any step.

use super::order_bracket::BracketSide;

// ── Result type ──────────────────────────────────────────────────

/// Outcome of a click in the bracket tool.
#[derive(Clone, Debug, PartialEq)]
pub enum BracketToolResult {
    /// More clicks are needed to complete the bracket.
    NeedMore,
    /// All three clicks received. The bracket is ready to create.
    Complete {
        /// Entry price (first click).
        entry: f64,
        /// Take-profit price.
        tp: f64,
        /// Stop-loss price.
        sl: f64,
        /// Trade direction.
        side: BracketSide,
    },
}

// ── Mode enum ────────────────────────────────────────────────────

/// The bracket tool's internal state machine.
#[derive(Clone, Debug, PartialEq)]
pub enum BracketToolMode {
    /// Tool is not active. No preview, no pending clicks.
    Idle,
    /// First click places the entry price.
    PlacingEntry,
    /// Entry placed; second click places take-profit.
    PlacingTP {
        /// The entry price from the first click.
        entry_price: f64,
        /// Trade direction (determines constraint logic).
        side: BracketSide,
    },
    /// Entry and TP placed; third click places stop-loss.
    PlacingSL {
        /// The entry price from the first click.
        entry_price: f64,
        /// The take-profit price from the second click.
        tp_price: f64,
        /// Trade direction.
        side: BracketSide,
    },
}

// ── BracketTool ──────────────────────────────────────────────────

/// Self-contained bracket drawing tool.
///
/// Owns all state for the 3-click bracket placement flow.
/// Lives as a field on [`ChartState`](crate::state::ChartState).
/// The interaction layer delegates bracket-related event handling
/// to this struct.
///
/// # State Machine
///
/// ```text
/// Idle ──activate()──> PlacingEntry
///   │                      │
///   │<──cancel()──────────│
///   │                 click(entry)
///   │                      │
///   │                      v
///   │<──cancel()──── PlacingTP { entry }
///   │                      │
///   │                 click(tp)
///   │                      │
///   │                      v
///   │<──cancel()──── PlacingSL { entry, tp }
///   │                      │
///   │                 click(sl)
///   │                      │
///   │<─── Complete ────────┘
/// ```
#[derive(Clone, Debug)]
pub struct BracketTool {
    /// Current tool mode.
    pub mode: BracketToolMode,
    /// Default trade direction for new brackets.
    pub side: BracketSide,
    /// Current mouse position as price, for live preview rendering.
    /// `None` when cursor is out of chart bounds.
    pub preview_price: Option<f64>,
}

impl Default for BracketTool {
    fn default() -> Self {
        Self {
            mode: BracketToolMode::Idle,
            side: BracketSide::Long,
            preview_price: None,
        }
    }
}

impl BracketTool {
    /// Enter `PlacingEntry` mode, ready for the first click.
    ///
    /// Clears any in-progress bracket and starts fresh.
    pub fn activate(&mut self) {
        self.mode = BracketToolMode::PlacingEntry;
        self.preview_price = None;
    }

    /// Cancel the current bracket and return to `Idle`.
    ///
    /// Safe to call from any state, including `Idle` (no-op).
    pub fn cancel(&mut self) {
        self.mode = BracketToolMode::Idle;
        self.preview_price = None;
    }

    /// Whether the tool is in any active state (not `Idle`).
    pub fn is_active(&self) -> bool {
        !matches!(self.mode, BracketToolMode::Idle)
    }

    /// Toggle trade direction between Long and Short.
    ///
    /// Only takes effect when the tool is `Idle` or in `PlacingEntry`
    /// (before any prices are committed). Once entry is placed, the
    /// side is locked for that bracket to avoid confusion.
    pub fn toggle_side(&mut self) {
        match self.mode {
            BracketToolMode::Idle | BracketToolMode::PlacingEntry => {
                self.side = match self.side {
                    BracketSide::Long => BracketSide::Short,
                    BracketSide::Short => BracketSide::Long,
                };
            }
            // Side is locked once entry price is committed.
            BracketToolMode::PlacingTP { .. } | BracketToolMode::PlacingSL { .. } => {}
        }
    }

    /// Update the preview price (current mouse Y as price).
    ///
    /// The compute/render layer reads this to draw a preview line
    /// at the next leg's position.
    pub fn set_preview(&mut self, price: f64) {
        self.preview_price = Some(price);
    }

    /// Advance the state machine on a click at the given price.
    ///
    /// Returns `NeedMore` if the bracket is still incomplete, or
    /// `Complete` with all three prices when the final click lands.
    /// On `Complete`, the tool automatically returns to `Idle`.
    ///
    /// No directional enforcement: every click price flows through
    /// verbatim into the resulting bracket. Wrong-side placements are
    /// flagged visually at render time (see
    /// [`crate::widget::order_bracket::is_leg_on_wrong_side`]).
    pub fn click(&mut self, price: f64) -> Option<BracketToolResult> {
        match self.mode.clone() {
            BracketToolMode::Idle => None,

            BracketToolMode::PlacingEntry => {
                self.mode = BracketToolMode::PlacingTP {
                    entry_price: price,
                    side: self.side,
                };
                self.preview_price = None;
                Some(BracketToolResult::NeedMore)
            }

            BracketToolMode::PlacingTP { entry_price, side } => {
                self.mode = BracketToolMode::PlacingSL {
                    entry_price,
                    tp_price: price,
                    side,
                };
                self.preview_price = None;
                Some(BracketToolResult::NeedMore)
            }

            BracketToolMode::PlacingSL {
                entry_price,
                tp_price,
                side,
            } => {
                let sl_price = price;
                let (tp, sl) = enforce_constraints(entry_price, tp_price, sl_price, side);

                self.mode = BracketToolMode::Idle;
                self.preview_price = None;

                Some(BracketToolResult::Complete {
                    entry: entry_price,
                    tp,
                    sl,
                    side,
                })
            }
        }
    }

    /// The current mode label, useful for status bar display.
    pub fn mode_label(&self) -> &'static str {
        match &self.mode {
            BracketToolMode::Idle => "Idle",
            BracketToolMode::PlacingEntry => "Click to place Entry",
            BracketToolMode::PlacingTP { .. } => "Click to place Take Profit",
            BracketToolMode::PlacingSL { .. } => "Click to place Stop Loss",
        }
    }
}

/// Identity pass-through for the three-click tool's TP/SL assignment.
///
/// No longer enforces direction; placements that cross entry are
/// classified visually by the decorator layer (see
/// `plan/live-sim-and-free-brackets.md`). Kept as a function so the
/// single call site in [`BracketTool::click`] stays explicit about
/// "the second click becomes TP, the third becomes SL" and so the
/// associated unit tests have a fixed target.
fn enforce_constraints(_entry: f64, tp: f64, sl: f64, _side: BracketSide) -> (f64, f64) {
    (tp, sl)
}

#[cfg(test)]
mod tests;
