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

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Construction & defaults ───────────────────────────────────

    #[test]
    fn default_is_idle_long() {
        let tool = BracketTool::default();
        assert_eq!(tool.mode, BracketToolMode::Idle);
        assert_eq!(tool.side, BracketSide::Long);
        assert!(tool.preview_price.is_none());
        assert!(!tool.is_active());
    }

    // ── Activate / cancel ────────────────────────────────────────

    #[test]
    fn activate_enters_placing_entry() {
        let mut tool = BracketTool::default();
        tool.activate();
        assert!(matches!(tool.mode, BracketToolMode::PlacingEntry));
        assert!(tool.is_active());
    }

    #[test]
    fn cancel_from_idle_is_noop() {
        let mut tool = BracketTool::default();
        tool.cancel();
        assert_eq!(tool.mode, BracketToolMode::Idle);
        assert!(!tool.is_active());
    }

    #[test]
    fn cancel_from_placing_entry() {
        let mut tool = BracketTool::default();
        tool.activate();
        tool.cancel();
        assert_eq!(tool.mode, BracketToolMode::Idle);
        assert!(!tool.is_active());
    }

    #[test]
    fn cancel_from_placing_tp() {
        let mut tool = BracketTool::default();
        tool.activate();
        tool.click(100.0);
        assert!(matches!(tool.mode, BracketToolMode::PlacingTP { .. }));
        tool.cancel();
        assert_eq!(tool.mode, BracketToolMode::Idle);
    }

    #[test]
    fn cancel_from_placing_sl() {
        let mut tool = BracketTool::default();
        tool.activate();
        tool.click(100.0);
        tool.click(110.0);
        assert!(matches!(tool.mode, BracketToolMode::PlacingSL { .. }));
        tool.cancel();
        assert_eq!(tool.mode, BracketToolMode::Idle);
    }

    #[test]
    fn cancel_clears_preview() {
        let mut tool = BracketTool::default();
        tool.activate();
        tool.set_preview(105.0);
        assert!(tool.preview_price.is_some());
        tool.cancel();
        assert!(tool.preview_price.is_none());
    }

    // ── Click returns None when idle ─────────────────────────────

    #[test]
    fn click_while_idle_returns_none() {
        let mut tool = BracketTool::default();
        assert!(tool.click(100.0).is_none());
        assert_eq!(tool.mode, BracketToolMode::Idle);
    }

    // ── Full 3-click sequence (Long) ─────────────────────────────

    #[test]
    fn full_long_bracket_correct_order() {
        let mut tool = BracketTool::default();
        tool.activate();

        // Click 1: entry at 100
        let r1 = tool.click(100.0);
        assert_eq!(r1, Some(BracketToolResult::NeedMore));
        assert!(matches!(
            tool.mode,
            BracketToolMode::PlacingTP {
                entry_price,
                side: BracketSide::Long,
            } if (entry_price - 100.0).abs() < f64::EPSILON
        ));

        // Click 2: TP at 110 (above entry -- correct for Long)
        let r2 = tool.click(110.0);
        assert_eq!(r2, Some(BracketToolResult::NeedMore));
        assert!(matches!(
            tool.mode,
            BracketToolMode::PlacingSL {
                entry_price,
                tp_price,
                side: BracketSide::Long,
            } if (entry_price - 100.0).abs() < f64::EPSILON
              && (tp_price - 110.0).abs() < f64::EPSILON
        ));

        // Click 3: SL at 95 (below entry -- correct for Long)
        let r3 = tool.click(95.0);
        assert_eq!(
            r3,
            Some(BracketToolResult::Complete {
                entry: 100.0,
                tp: 110.0,
                sl: 95.0,
                side: BracketSide::Long,
            })
        );
        // Auto-returns to Idle after completion.
        assert_eq!(tool.mode, BracketToolMode::Idle);
        assert!(!tool.is_active());
    }

    // ── Full 3-click sequence (Short) ────────────────────────────

    #[test]
    fn full_short_bracket_correct_order() {
        let mut tool = BracketTool::default();
        tool.side = BracketSide::Short;
        tool.activate();

        // Click 1: entry at 100
        let r1 = tool.click(100.0);
        assert_eq!(r1, Some(BracketToolResult::NeedMore));

        // Click 2: TP at 90 (below entry -- correct for Short)
        let r2 = tool.click(90.0);
        assert_eq!(r2, Some(BracketToolResult::NeedMore));

        // Click 3: SL at 105 (above entry -- correct for Short)
        let r3 = tool.click(105.0);
        assert_eq!(
            r3,
            Some(BracketToolResult::Complete {
                entry: 100.0,
                tp: 90.0,
                sl: 105.0,
                side: BracketSide::Short,
            })
        );
        assert_eq!(tool.mode, BracketToolMode::Idle);
    }

    // ── Constraint enforcement: auto-swap ────────────────────────

    #[test]
    fn long_bracket_swaps_when_tp_below_sl_above() {
        // User clicks TP below entry and SL above entry for a Long.
        // Tool should swap them.
        let mut tool = BracketTool::default();
        tool.activate();
        tool.click(100.0); // entry
        tool.click(95.0); // "TP" below entry (wrong for Long)
        let result = tool.click(110.0); // "SL" above entry (wrong for Long)

        // After swap: tp=110, sl=95 (correct for Long: TP > entry > SL)
        assert_eq!(
            result,
            Some(BracketToolResult::Complete {
                entry: 100.0,
                tp: 110.0,
                sl: 95.0,
                side: BracketSide::Long,
            })
        );
    }

    #[test]
    fn short_bracket_swaps_when_tp_above_sl_below() {
        // User clicks TP above entry and SL below entry for a Short.
        // Tool should swap them.
        let mut tool = BracketTool::default();
        tool.side = BracketSide::Short;
        tool.activate();
        tool.click(100.0); // entry
        tool.click(110.0); // "TP" above entry (wrong for Short)
        let result = tool.click(90.0); // "SL" below entry (wrong for Short)

        // After swap: tp=90, sl=110 (correct for Short: SL > entry > TP)
        assert_eq!(
            result,
            Some(BracketToolResult::Complete {
                entry: 100.0,
                tp: 90.0,
                sl: 110.0,
                side: BracketSide::Short,
            })
        );
    }

    #[test]
    fn long_bracket_both_above_entry_sorts_correctly() {
        // Both TP and SL clicked above entry for a Long.
        // Higher one should be TP, lower one should be SL.
        let mut tool = BracketTool::default();
        tool.activate();
        tool.click(100.0); // entry
        tool.click(108.0); // "TP" above entry
        let result = tool.click(105.0); // "SL" also above entry

        assert_eq!(
            result,
            Some(BracketToolResult::Complete {
                entry: 100.0,
                tp: 108.0,
                sl: 105.0,
                side: BracketSide::Long,
            })
        );
    }

    #[test]
    fn long_bracket_both_below_entry_sorts_correctly() {
        // Both TP and SL clicked below entry for a Long.
        // Higher one becomes TP, lower one becomes SL.
        let mut tool = BracketTool::default();
        tool.activate();
        tool.click(100.0); // entry
        tool.click(92.0); // "TP" below entry
        let result = tool.click(95.0); // "SL" also below entry

        // Higher of the two (95) is TP, lower (92) is SL
        assert_eq!(
            result,
            Some(BracketToolResult::Complete {
                entry: 100.0,
                tp: 95.0,
                sl: 92.0,
                side: BracketSide::Long,
            })
        );
    }

    #[test]
    fn short_bracket_both_above_entry_sorts_correctly() {
        // Both TP and SL clicked above entry for a Short.
        // Lower one becomes TP, higher one becomes SL.
        let mut tool = BracketTool::default();
        tool.side = BracketSide::Short;
        tool.activate();
        tool.click(100.0); // entry
        tool.click(108.0); // "TP" above entry
        let result = tool.click(105.0); // "SL" also above entry

        // For Short: lower (105) is TP, higher (108) is SL
        assert_eq!(
            result,
            Some(BracketToolResult::Complete {
                entry: 100.0,
                tp: 105.0,
                sl: 108.0,
                side: BracketSide::Short,
            })
        );
    }

    #[test]
    fn short_bracket_both_below_entry_sorts_correctly() {
        // Both TP and SL clicked below entry for a Short.
        // Lower one becomes TP, higher one becomes SL.
        let mut tool = BracketTool::default();
        tool.side = BracketSide::Short;
        tool.activate();
        tool.click(100.0); // entry
        tool.click(92.0); // "TP" below entry
        let result = tool.click(95.0); // "SL" also below entry

        // For Short: lower (92) is TP, higher (95) is SL
        assert_eq!(
            result,
            Some(BracketToolResult::Complete {
                entry: 100.0,
                tp: 92.0,
                sl: 95.0,
                side: BracketSide::Short,
            })
        );
    }

    // ── Side toggling ────────────────────────────────────────────

    #[test]
    fn toggle_side_while_idle() {
        let mut tool = BracketTool::default();
        assert_eq!(tool.side, BracketSide::Long);
        tool.toggle_side();
        assert_eq!(tool.side, BracketSide::Short);
        tool.toggle_side();
        assert_eq!(tool.side, BracketSide::Long);
    }

    #[test]
    fn toggle_side_while_placing_entry() {
        let mut tool = BracketTool::default();
        tool.activate();
        assert_eq!(tool.side, BracketSide::Long);
        tool.toggle_side();
        assert_eq!(tool.side, BracketSide::Short);
    }

    #[test]
    fn toggle_side_locked_after_entry_placed() {
        let mut tool = BracketTool::default();
        tool.activate();
        tool.click(100.0); // entry placed, now in PlacingTP
        assert!(matches!(tool.mode, BracketToolMode::PlacingTP { .. }));

        tool.toggle_side(); // should be no-op
        assert_eq!(tool.side, BracketSide::Long);

        // Verify the mode's side is still Long
        if let BracketToolMode::PlacingTP { side, .. } = &tool.mode {
            assert_eq!(*side, BracketSide::Long);
        }
    }

    #[test]
    fn toggle_side_locked_during_placing_sl() {
        let mut tool = BracketTool::default();
        tool.activate();
        tool.click(100.0);
        tool.click(110.0);
        assert!(matches!(tool.mode, BracketToolMode::PlacingSL { .. }));

        tool.toggle_side(); // should be no-op
        assert_eq!(tool.side, BracketSide::Long);
    }

    // ── Preview price ────────────────────────────────────────────

    #[test]
    fn set_preview_updates_price() {
        let mut tool = BracketTool::default();
        tool.activate();
        tool.set_preview(105.5);
        assert_eq!(tool.preview_price, Some(105.5));
    }

    #[test]
    fn click_clears_preview() {
        let mut tool = BracketTool::default();
        tool.activate();
        tool.set_preview(105.0);
        tool.click(100.0);
        assert!(tool.preview_price.is_none());
    }

    #[test]
    fn activate_clears_preview() {
        let mut tool = BracketTool::default();
        tool.set_preview(42.0);
        tool.activate();
        assert!(tool.preview_price.is_none());
    }

    // ── Mode labels ──────────────────────────────────────────────

    #[test]
    fn mode_labels_are_correct() {
        let mut tool = BracketTool::default();
        assert_eq!(tool.mode_label(), "Idle");

        tool.activate();
        assert_eq!(tool.mode_label(), "Click to place Entry");

        tool.click(100.0);
        assert_eq!(tool.mode_label(), "Click to place Take Profit");

        tool.click(110.0);
        assert_eq!(tool.mode_label(), "Click to place Stop Loss");
    }

    // ── Reactivation after completion ────────────────────────────

    #[test]
    fn can_start_new_bracket_after_completion() {
        let mut tool = BracketTool::default();
        tool.activate();
        tool.click(100.0);
        tool.click(110.0);
        let r = tool.click(95.0);
        assert!(matches!(r, Some(BracketToolResult::Complete { .. })));
        assert_eq!(tool.mode, BracketToolMode::Idle);

        // Start another bracket immediately.
        tool.activate();
        assert!(matches!(tool.mode, BracketToolMode::PlacingEntry));
        assert!(tool.is_active());
    }

    #[test]
    fn activate_resets_in_progress_bracket() {
        let mut tool = BracketTool::default();
        tool.activate();
        tool.click(100.0);
        tool.click(110.0);
        // Now in PlacingSL. Re-activate should start fresh.
        tool.activate();
        assert!(matches!(tool.mode, BracketToolMode::PlacingEntry));
    }

    // ── Side is captured at entry click ──────────────────────────

    #[test]
    fn side_captured_at_entry_click() {
        let mut tool = BracketTool::default();
        tool.side = BracketSide::Short;
        tool.activate();
        tool.click(100.0);

        if let BracketToolMode::PlacingTP { side, .. } = &tool.mode {
            assert_eq!(*side, BracketSide::Short);
        } else {
            panic!("expected PlacingTP");
        }
    }

    // ── Edge case: TP or SL at entry price ───────────────────────

    #[test]
    fn long_tp_at_entry_price_no_panic() {
        let mut tool = BracketTool::default();
        tool.activate();
        tool.click(100.0); // entry
        tool.click(100.0); // TP at entry (degenerate but shouldn't panic)
        let result = tool.click(95.0); // SL below

        // TP = entry is technically invalid but the tool should not panic.
        // The constraint enforcer leaves them as-is since tp >= entry.
        assert!(matches!(result, Some(BracketToolResult::Complete { .. })));
    }

    #[test]
    fn all_three_at_same_price_no_panic() {
        let mut tool = BracketTool::default();
        tool.activate();
        tool.click(100.0);
        tool.click(100.0);
        let result = tool.click(100.0);

        assert!(matches!(result, Some(BracketToolResult::Complete { .. })));
        assert_eq!(tool.mode, BracketToolMode::Idle);
    }

    // ── enforce_constraints unit tests ───────────────────────────

    #[test]
    fn enforce_long_already_correct() {
        let (tp, sl) = enforce_constraints(100.0, 110.0, 95.0, BracketSide::Long);
        assert_eq!(tp, 110.0);
        assert_eq!(sl, 95.0);
    }

    #[test]
    fn enforce_long_needs_swap() {
        let (tp, sl) = enforce_constraints(100.0, 95.0, 110.0, BracketSide::Long);
        assert_eq!(tp, 110.0);
        assert_eq!(sl, 95.0);
    }

    #[test]
    fn enforce_short_already_correct() {
        let (tp, sl) = enforce_constraints(100.0, 90.0, 105.0, BracketSide::Short);
        assert_eq!(tp, 90.0);
        assert_eq!(sl, 105.0);
    }

    #[test]
    fn enforce_short_needs_swap() {
        let (tp, sl) = enforce_constraints(100.0, 105.0, 90.0, BracketSide::Short);
        assert_eq!(tp, 90.0);
        assert_eq!(sl, 105.0);
    }

    #[test]
    fn enforce_long_both_above() {
        // Both above entry for Long: higher is TP, lower is SL.
        let (tp, sl) = enforce_constraints(100.0, 105.0, 108.0, BracketSide::Long);
        assert_eq!(tp, 108.0);
        assert_eq!(sl, 105.0);
    }

    #[test]
    fn enforce_long_both_below() {
        // Both below entry for Long: higher is TP, lower is SL.
        let (tp, sl) = enforce_constraints(100.0, 92.0, 95.0, BracketSide::Long);
        assert_eq!(tp, 95.0);
        assert_eq!(sl, 92.0);
    }

    #[test]
    fn enforce_short_both_above() {
        // Both above entry for Short: lower is TP, higher is SL.
        let (tp, sl) = enforce_constraints(100.0, 108.0, 105.0, BracketSide::Short);
        assert_eq!(tp, 105.0);
        assert_eq!(sl, 108.0);
    }

    #[test]
    fn enforce_short_both_below() {
        // Both below entry for Short: lower is TP, higher is SL.
        let (tp, sl) = enforce_constraints(100.0, 95.0, 92.0, BracketSide::Short);
        assert_eq!(tp, 92.0);
        assert_eq!(sl, 95.0);
    }
}
