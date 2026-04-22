//! Per-tab message enum for the Positions tab inside
//! [`super::AccountPanel`].
//!
//! Wrapped by [`super::AccountMsg::Positions`] when routed through the
//! app's outer `Message::Account(_, _)` dispatch.

/// Messages originating from Positions-tab widgets.
#[derive(Clone, Debug)]
#[allow(dead_code)] // `Grid` variant reserved for future row-select / resize.
pub enum PositionsMsg {
    /// The user clicked the close-X button for a position. Payload is
    /// the position symbol; the handler looks the row up in the
    /// app-wide `PositionStore` at apply time so the click captures no
    /// stale data.
    ///
    /// **Stub in v1.** The handler logs intent and sets a status
    /// message; it never dispatches to the broker. A unit test pins
    /// that guarantee so a future refactor can't accidentally wire it
    /// up.
    CloseRequested(String),
    /// Passthrough for grid chrome events (row select, future sort /
    /// resize).
    Grid(midas_grid::GridMessage),
}

/// Pure decision the close-position handler makes.
///
/// Extracted so the stub contract — "never emits a broker command" —
/// can be unit-tested without standing up a full `MidasApp`.
/// `handle_account_close_requested` wraps this in tracing +
/// status-message side-effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseDecision {
    /// Broker is offline. UI should show "Disconnected — close
    /// unavailable"; no command was or will be emitted.
    RefusedDisconnected,
    /// Broker is online. UI should show "Close intent logged for
    /// {symbol}"; still no command is emitted — v1 is stub only.
    Logged(String),
}

impl CloseDecision {
    /// Compute the decision. Pure: no I/O, no mutation, no broker
    /// command construction (the enum carries data only).
    pub fn compute(connected: bool, symbol: &str) -> Self {
        if connected {
            Self::Logged(symbol.to_owned())
        } else {
            Self::RefusedDisconnected
        }
    }

    /// The status-bar message that should accompany this decision.
    pub fn status_message(&self) -> String {
        match self {
            Self::RefusedDisconnected => "Disconnected — close unavailable".to_owned(),
            Self::Logged(sym) => format!("Close intent logged for {sym}"),
        }
    }

    /// Whether a broker call is permitted for this decision.
    ///
    /// **Always `false`** in v1 — both paths are stub only. A future
    /// slice wiring the close-position action to the broker must flip
    /// this to `true` only for the `Logged` arm, in a separate commit,
    /// accompanied by the corresponding `OrderClient::cancel_order`
    /// call.
    pub fn may_emit_command(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_refuses_with_explanatory_status() {
        let d = CloseDecision::compute(false, "AAPL");
        assert_eq!(d, CloseDecision::RefusedDisconnected);
        assert_eq!(d.status_message(), "Disconnected — close unavailable");
        assert!(
            !d.may_emit_command(),
            "v1 stub must never authorize a broker command, even when connected"
        );
    }

    #[test]
    fn connected_logs_intent_with_symbol_in_status() {
        let d = CloseDecision::compute(true, "GME");
        assert_eq!(d, CloseDecision::Logged("GME".to_owned()));
        assert_eq!(d.status_message(), "Close intent logged for GME");
        assert!(
            !d.may_emit_command(),
            "v1 stub must never authorize a broker command, even when connected"
        );
    }

    #[test]
    fn empty_symbol_is_still_logged_when_connected() {
        // The handler trims input at call sites that need it; the
        // decision function itself is permissive so the guard is
        // visible as a separate concern (and so the test fixture
        // can exercise the happy path without accidentally
        // exercising the trim path).
        let d = CloseDecision::compute(true, "");
        assert_eq!(d, CloseDecision::Logged(String::new()));
        assert!(!d.may_emit_command());
    }
}
