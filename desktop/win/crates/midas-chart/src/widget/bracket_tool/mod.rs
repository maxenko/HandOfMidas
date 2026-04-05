//! Bracket drawing tool: 3-click state machine for placing order brackets.
//!
//! Click 1: entry price, Click 2: take-profit, Click 3: stop-loss.
//! The tool enforces directional constraints automatically:
//! - Long brackets: TP > entry > SL
//! - Short brackets: SL > entry > TP
//!
//! If the user clicks in the wrong order, the tool swaps TP/SL
//! rather than rejecting the click. Escape cancels at any step.

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
    /// # Constraint enforcement
    ///
    /// - **Long**: TP must be above entry, SL must be below entry.
    /// - **Short**: TP must be below entry, SL must be above entry.
    ///
    /// If the user clicks on the wrong side, the tool swaps TP/SL
    /// automatically rather than rejecting the input.
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

/// Enforce directional constraints on TP and SL relative to entry.
///
/// For Long: TP > entry > SL. For Short: SL > entry > TP.
/// If both TP and SL are on the same side, they are swapped.
fn enforce_constraints(entry: f64, tp: f64, sl: f64, side: BracketSide) -> (f64, f64) {
    match side {
        BracketSide::Long => {
            // Long: TP should be above entry, SL below.
            // If TP is below entry and SL is above, swap them.
            if tp < entry && sl > entry {
                (sl, tp)
            } else if tp < entry && sl < entry {
                // Both below entry: the higher one is TP, lower is SL.
                if tp > sl {
                    (tp, sl)
                } else {
                    (sl, tp)
                }
            } else if tp > entry && sl > entry {
                // Both above entry: the lower one is SL (closer), higher is TP.
                // Actually for Long, SL must be below entry. Swap so that
                // the one closer to entry becomes SL and the farther becomes TP.
                if tp > sl {
                    (tp, sl)
                } else {
                    (sl, tp)
                }
            } else {
                // Already correct: tp >= entry, sl <= entry.
                (tp, sl)
            }
        }
        BracketSide::Short => {
            // Short: TP should be below entry, SL above.
            if tp > entry && sl < entry {
                (sl, tp)
            } else if tp > entry && sl > entry {
                // Both above entry: the lower one is TP, higher is SL.
                if tp < sl {
                    (tp, sl)
                } else {
                    (sl, tp)
                }
            } else if tp < entry && sl < entry {
                // Both below entry: the higher one is SL (closer), lower is TP.
                if tp < sl {
                    (tp, sl)
                } else {
                    (sl, tp)
                }
            } else {
                // Already correct: tp <= entry, sl >= entry.
                (tp, sl)
            }
        }
    }
}

#[cfg(test)]
mod tests;
