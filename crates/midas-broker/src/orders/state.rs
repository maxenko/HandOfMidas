use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::BrokerError;

/// Canonical lifecycle status of a [`LocalOrder`](super::types::LocalOrder).
///
/// The state machine enforces which transitions are legal.  See
/// [`validate_transition`](OrderStatus::validate_transition) for the full
/// adjacency list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Order exists locally but has not been persisted yet.
    Draft,
    /// Persisted locally; not submitted to IB.
    Inactive,
    /// Submitted to IB; awaiting acknowledgement.
    PendingSubmit,
    /// IB accepted the order but it has not yet hit the exchange.
    PreSubmitted,
    /// Working at the exchange.
    Submitted,
    /// Some shares filled; remainder still working.
    PartiallyFilled,
    /// Completely filled (terminal).
    Filled,
    /// Cancel request sent to IB; awaiting confirmation.
    PendingCancel,
    /// Fully cancelled (terminal).
    Cancelled,
    /// IB rejected the order (terminal).
    Rejected,
    /// A local error occurred (e.g. serialization failure before submit).
    Error,
}

// ---------------------------------------------------------------------------
// Display / FromStr
// ---------------------------------------------------------------------------

impl fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Draft => "Draft",
            Self::Inactive => "Inactive",
            Self::PendingSubmit => "PendingSubmit",
            Self::PreSubmitted => "PreSubmitted",
            Self::Submitted => "Submitted",
            Self::PartiallyFilled => "PartiallyFilled",
            Self::Filled => "Filled",
            Self::PendingCancel => "PendingCancel",
            Self::Cancelled => "Cancelled",
            Self::Rejected => "Rejected",
            Self::Error => "Error",
        };
        f.write_str(s)
    }
}

impl FromStr for OrderStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Draft" => Ok(Self::Draft),
            "Inactive" => Ok(Self::Inactive),
            "PendingSubmit" => Ok(Self::PendingSubmit),
            "PreSubmitted" => Ok(Self::PreSubmitted),
            "Submitted" => Ok(Self::Submitted),
            "PartiallyFilled" => Ok(Self::PartiallyFilled),
            "Filled" => Ok(Self::Filled),
            "PendingCancel" => Ok(Self::PendingCancel),
            "Cancelled" => Ok(Self::Cancelled),
            "Rejected" => Ok(Self::Rejected),
            "Error" => Ok(Self::Error),
            other => Err(format!("unknown OrderStatus: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// IB status mapping
// ---------------------------------------------------------------------------

impl OrderStatus {
    /// Map an IB TWS/Gateway status string to our canonical status.
    ///
    /// **CRITICAL**: IB's `"Inactive"` means the order was *rejected* by IB
    /// (e.g. margin violation).  It does **not** correspond to our `Inactive`
    /// status which means "persisted locally, not submitted".
    pub fn from_ib_status(ib_status: &str) -> Self {
        match ib_status {
            "ApiPending" | "PendingSubmit" => Self::PendingSubmit,
            "PreSubmitted" => Self::PreSubmitted,
            "Submitted" => Self::Submitted,
            "Filled" => Self::Filled,
            "PendingCancel" => Self::PendingCancel,
            "Cancelled" | "ApiCancelled" => Self::Cancelled,
            // IB "Inactive" = order rejected (e.g. margin, permissions).
            "Inactive" => Self::Rejected,
            _ => Self::Rejected,
        }
    }
}

// ---------------------------------------------------------------------------
// Predicate helpers
// ---------------------------------------------------------------------------

impl OrderStatus {
    /// A terminal order will never change status again.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Filled | Self::Cancelled | Self::Rejected)
    }

    /// The order is currently live (tracked) at the IB gateway.
    pub fn is_live_at_ib(&self) -> bool {
        matches!(
            self,
            Self::PendingSubmit
                | Self::PreSubmitted
                | Self::Submitted
                | Self::PartiallyFilled
                | Self::PendingCancel
        )
    }

    /// The order can be activated (submitted to IB).
    pub fn can_activate(&self) -> bool {
        matches!(self, Self::Inactive | Self::Error)
    }

    /// The order can be deactivated (pulled back from IB without cancelling).
    /// NOT allowed for PartiallyFilled because shares are already filled.
    pub fn can_deactivate(&self) -> bool {
        matches!(self, Self::PreSubmitted | Self::Submitted)
    }

    /// The order can be cancelled at IB.
    pub fn can_cancel(&self) -> bool {
        matches!(
            self,
            Self::PreSubmitted | Self::Submitted | Self::PartiallyFilled
        )
    }

    /// The order's parameters can be edited locally (not yet at IB).
    pub fn can_modify_locally(&self) -> bool {
        matches!(self, Self::Inactive | Self::Error)
    }

    /// The order can be modified in-flight at IB.
    pub fn can_modify_at_ib(&self) -> bool {
        matches!(self, Self::PreSubmitted | Self::Submitted)
    }
}

// ---------------------------------------------------------------------------
// State machine transition validation
// ---------------------------------------------------------------------------

impl OrderStatus {
    /// Returns `Ok(())` if the transition `from -> to` is legal according to
    /// the broker state machine, or `Err(BrokerError::InvalidTransition)`
    /// otherwise.
    pub fn validate_transition(from: Self, to: Self) -> Result<(), BrokerError> {
        let valid = match from {
            Self::Draft => matches!(to, Self::Inactive),
            Self::Inactive => matches!(to, Self::PendingSubmit),
            Self::Error => matches!(to, Self::Inactive | Self::PendingSubmit),
            Self::PendingSubmit => matches!(
                to,
                Self::PreSubmitted
                    | Self::Submitted
                    | Self::Rejected
                    | Self::Error
                    | Self::PendingCancel
            ),
            Self::PreSubmitted => matches!(
                to,
                Self::Submitted | Self::PendingCancel | Self::Filled | Self::Rejected
            ),
            Self::Submitted => matches!(
                to,
                Self::PartiallyFilled | Self::Filled | Self::PendingCancel | Self::Rejected
            ),
            Self::PartiallyFilled => matches!(to, Self::Filled | Self::PendingCancel),
            Self::PendingCancel => matches!(
                to,
                Self::Cancelled | Self::Inactive | Self::Filled
            ),
            // Terminal states: no transitions out.
            Self::Filled | Self::Cancelled | Self::Rejected => false,
        };

        if valid {
            Ok(())
        } else {
            Err(BrokerError::InvalidTransition { from, to })
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Display / FromStr round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn display_fromstr_round_trip() {
        let all = [
            OrderStatus::Draft,
            OrderStatus::Inactive,
            OrderStatus::PendingSubmit,
            OrderStatus::PreSubmitted,
            OrderStatus::Submitted,
            OrderStatus::PartiallyFilled,
            OrderStatus::Filled,
            OrderStatus::PendingCancel,
            OrderStatus::Cancelled,
            OrderStatus::Rejected,
            OrderStatus::Error,
        ];
        for status in all {
            let s = status.to_string();
            let parsed: OrderStatus = s.parse().unwrap();
            assert_eq!(parsed, status, "round-trip failed for {status}");
        }
    }

    #[test]
    fn fromstr_unknown_returns_error() {
        let result = "Bogus".parse::<OrderStatus>();
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // from_ib_status
    // -----------------------------------------------------------------------

    #[test]
    fn from_ib_status_api_pending() {
        assert_eq!(OrderStatus::from_ib_status("ApiPending"), OrderStatus::PendingSubmit);
    }

    #[test]
    fn from_ib_status_pending_submit() {
        assert_eq!(OrderStatus::from_ib_status("PendingSubmit"), OrderStatus::PendingSubmit);
    }

    #[test]
    fn from_ib_status_pre_submitted() {
        assert_eq!(OrderStatus::from_ib_status("PreSubmitted"), OrderStatus::PreSubmitted);
    }

    #[test]
    fn from_ib_status_submitted() {
        assert_eq!(OrderStatus::from_ib_status("Submitted"), OrderStatus::Submitted);
    }

    #[test]
    fn from_ib_status_filled() {
        assert_eq!(OrderStatus::from_ib_status("Filled"), OrderStatus::Filled);
    }

    #[test]
    fn from_ib_status_pending_cancel() {
        assert_eq!(OrderStatus::from_ib_status("PendingCancel"), OrderStatus::PendingCancel);
    }

    #[test]
    fn from_ib_status_cancelled() {
        assert_eq!(OrderStatus::from_ib_status("Cancelled"), OrderStatus::Cancelled);
    }

    #[test]
    fn from_ib_status_api_cancelled() {
        assert_eq!(OrderStatus::from_ib_status("ApiCancelled"), OrderStatus::Cancelled);
    }

    /// CRITICAL: IB "Inactive" means the order was rejected, not our Inactive.
    #[test]
    fn from_ib_status_inactive_maps_to_rejected() {
        assert_eq!(OrderStatus::from_ib_status("Inactive"), OrderStatus::Rejected);
    }

    #[test]
    fn from_ib_status_unknown_maps_to_rejected() {
        assert_eq!(OrderStatus::from_ib_status("SomethingNew"), OrderStatus::Rejected);
    }

    // -----------------------------------------------------------------------
    // Predicate helpers
    // -----------------------------------------------------------------------

    #[test]
    fn is_terminal() {
        assert!(OrderStatus::Filled.is_terminal());
        assert!(OrderStatus::Cancelled.is_terminal());
        assert!(OrderStatus::Rejected.is_terminal());
        // Non-terminal
        assert!(!OrderStatus::Draft.is_terminal());
        assert!(!OrderStatus::Inactive.is_terminal());
        assert!(!OrderStatus::PendingSubmit.is_terminal());
        assert!(!OrderStatus::Submitted.is_terminal());
        assert!(!OrderStatus::PartiallyFilled.is_terminal());
        assert!(!OrderStatus::Error.is_terminal());
    }

    #[test]
    fn is_live_at_ib() {
        assert!(OrderStatus::PendingSubmit.is_live_at_ib());
        assert!(OrderStatus::PreSubmitted.is_live_at_ib());
        assert!(OrderStatus::Submitted.is_live_at_ib());
        assert!(OrderStatus::PartiallyFilled.is_live_at_ib());
        assert!(OrderStatus::PendingCancel.is_live_at_ib());
        // Not live
        assert!(!OrderStatus::Draft.is_live_at_ib());
        assert!(!OrderStatus::Inactive.is_live_at_ib());
        assert!(!OrderStatus::Filled.is_live_at_ib());
        assert!(!OrderStatus::Cancelled.is_live_at_ib());
        assert!(!OrderStatus::Rejected.is_live_at_ib());
        assert!(!OrderStatus::Error.is_live_at_ib());
    }

    #[test]
    fn can_activate() {
        assert!(OrderStatus::Inactive.can_activate());
        assert!(OrderStatus::Error.can_activate());
        assert!(!OrderStatus::Draft.can_activate());
        assert!(!OrderStatus::Submitted.can_activate());
        assert!(!OrderStatus::Filled.can_activate());
    }

    #[test]
    fn can_deactivate() {
        assert!(OrderStatus::PreSubmitted.can_deactivate());
        assert!(OrderStatus::Submitted.can_deactivate());
        // PartiallyFilled must NOT be deactivatable.
        assert!(!OrderStatus::PartiallyFilled.can_deactivate());
        assert!(!OrderStatus::Inactive.can_deactivate());
        assert!(!OrderStatus::PendingCancel.can_deactivate());
    }

    #[test]
    fn can_cancel() {
        assert!(OrderStatus::PreSubmitted.can_cancel());
        assert!(OrderStatus::Submitted.can_cancel());
        assert!(OrderStatus::PartiallyFilled.can_cancel());
        assert!(!OrderStatus::Inactive.can_cancel());
        assert!(!OrderStatus::Draft.can_cancel());
        assert!(!OrderStatus::PendingCancel.can_cancel());
    }

    #[test]
    fn can_modify_locally() {
        assert!(OrderStatus::Inactive.can_modify_locally());
        assert!(OrderStatus::Error.can_modify_locally());
        assert!(!OrderStatus::Submitted.can_modify_locally());
        assert!(!OrderStatus::Draft.can_modify_locally());
    }

    #[test]
    fn can_modify_at_ib() {
        assert!(OrderStatus::PreSubmitted.can_modify_at_ib());
        assert!(OrderStatus::Submitted.can_modify_at_ib());
        assert!(!OrderStatus::PartiallyFilled.can_modify_at_ib());
        assert!(!OrderStatus::Inactive.can_modify_at_ib());
    }

    // -----------------------------------------------------------------------
    // Valid transitions
    // -----------------------------------------------------------------------

    #[test]
    fn valid_draft_to_inactive() {
        OrderStatus::validate_transition(OrderStatus::Draft, OrderStatus::Inactive).unwrap();
    }

    #[test]
    fn valid_inactive_to_pending_submit() {
        OrderStatus::validate_transition(OrderStatus::Inactive, OrderStatus::PendingSubmit)
            .unwrap();
    }

    #[test]
    fn valid_error_to_inactive() {
        OrderStatus::validate_transition(OrderStatus::Error, OrderStatus::Inactive).unwrap();
    }

    #[test]
    fn valid_error_to_pending_submit() {
        OrderStatus::validate_transition(OrderStatus::Error, OrderStatus::PendingSubmit).unwrap();
    }

    #[test]
    fn valid_pending_submit_to_pre_submitted() {
        OrderStatus::validate_transition(OrderStatus::PendingSubmit, OrderStatus::PreSubmitted)
            .unwrap();
    }

    #[test]
    fn valid_pending_submit_to_submitted() {
        OrderStatus::validate_transition(OrderStatus::PendingSubmit, OrderStatus::Submitted)
            .unwrap();
    }

    #[test]
    fn valid_pending_submit_to_rejected() {
        OrderStatus::validate_transition(OrderStatus::PendingSubmit, OrderStatus::Rejected)
            .unwrap();
    }

    #[test]
    fn valid_pending_submit_to_error() {
        OrderStatus::validate_transition(OrderStatus::PendingSubmit, OrderStatus::Error).unwrap();
    }

    #[test]
    fn valid_pending_submit_to_pending_cancel() {
        OrderStatus::validate_transition(OrderStatus::PendingSubmit, OrderStatus::PendingCancel)
            .unwrap();
    }

    #[test]
    fn valid_pre_submitted_to_submitted() {
        OrderStatus::validate_transition(OrderStatus::PreSubmitted, OrderStatus::Submitted)
            .unwrap();
    }

    #[test]
    fn valid_pre_submitted_to_pending_cancel() {
        OrderStatus::validate_transition(OrderStatus::PreSubmitted, OrderStatus::PendingCancel)
            .unwrap();
    }

    #[test]
    fn valid_pre_submitted_to_filled() {
        OrderStatus::validate_transition(OrderStatus::PreSubmitted, OrderStatus::Filled).unwrap();
    }

    #[test]
    fn valid_pre_submitted_to_rejected() {
        OrderStatus::validate_transition(OrderStatus::PreSubmitted, OrderStatus::Rejected)
            .unwrap();
    }

    #[test]
    fn valid_submitted_to_partially_filled() {
        OrderStatus::validate_transition(OrderStatus::Submitted, OrderStatus::PartiallyFilled)
            .unwrap();
    }

    #[test]
    fn valid_submitted_to_filled() {
        OrderStatus::validate_transition(OrderStatus::Submitted, OrderStatus::Filled).unwrap();
    }

    #[test]
    fn valid_submitted_to_pending_cancel() {
        OrderStatus::validate_transition(OrderStatus::Submitted, OrderStatus::PendingCancel)
            .unwrap();
    }

    #[test]
    fn valid_submitted_to_rejected() {
        OrderStatus::validate_transition(OrderStatus::Submitted, OrderStatus::Rejected).unwrap();
    }

    #[test]
    fn valid_partially_filled_to_filled() {
        OrderStatus::validate_transition(OrderStatus::PartiallyFilled, OrderStatus::Filled)
            .unwrap();
    }

    #[test]
    fn valid_partially_filled_to_pending_cancel() {
        OrderStatus::validate_transition(OrderStatus::PartiallyFilled, OrderStatus::PendingCancel)
            .unwrap();
    }

    #[test]
    fn valid_pending_cancel_to_cancelled() {
        OrderStatus::validate_transition(OrderStatus::PendingCancel, OrderStatus::Cancelled)
            .unwrap();
    }

    #[test]
    fn valid_pending_cancel_to_inactive() {
        OrderStatus::validate_transition(OrderStatus::PendingCancel, OrderStatus::Inactive)
            .unwrap();
    }

    #[test]
    fn valid_pending_cancel_to_filled_race() {
        // Race condition: fill arrives after cancel request.
        OrderStatus::validate_transition(OrderStatus::PendingCancel, OrderStatus::Filled).unwrap();
    }

    // -----------------------------------------------------------------------
    // Invalid transitions
    // -----------------------------------------------------------------------

    #[test]
    fn invalid_draft_to_submitted() {
        let err =
            OrderStatus::validate_transition(OrderStatus::Draft, OrderStatus::Submitted)
                .unwrap_err();
        match err {
            BrokerError::InvalidTransition { from, to } => {
                assert_eq!(from, OrderStatus::Draft);
                assert_eq!(to, OrderStatus::Submitted);
            }
            other => panic!("expected InvalidTransition, got: {other}"),
        }
    }

    #[test]
    fn invalid_filled_to_anything() {
        let targets = [
            OrderStatus::Draft,
            OrderStatus::Inactive,
            OrderStatus::PendingSubmit,
            OrderStatus::Submitted,
            OrderStatus::Cancelled,
        ];
        for to in targets {
            assert!(
                OrderStatus::validate_transition(OrderStatus::Filled, to).is_err(),
                "Filled -> {to} should be invalid"
            );
        }
    }

    #[test]
    fn invalid_cancelled_to_anything() {
        assert!(
            OrderStatus::validate_transition(OrderStatus::Cancelled, OrderStatus::Inactive)
                .is_err()
        );
    }

    #[test]
    fn invalid_rejected_to_anything() {
        assert!(
            OrderStatus::validate_transition(OrderStatus::Rejected, OrderStatus::Error).is_err()
        );
    }

    #[test]
    fn invalid_inactive_to_filled() {
        assert!(
            OrderStatus::validate_transition(OrderStatus::Inactive, OrderStatus::Filled).is_err()
        );
    }

    #[test]
    fn invalid_submitted_to_inactive() {
        assert!(
            OrderStatus::validate_transition(OrderStatus::Submitted, OrderStatus::Inactive)
                .is_err()
        );
    }

    #[test]
    fn invalid_partially_filled_to_cancelled() {
        // Must go through PendingCancel first.
        assert!(
            OrderStatus::validate_transition(
                OrderStatus::PartiallyFilled,
                OrderStatus::Cancelled
            )
            .is_err()
        );
    }

    #[test]
    fn invalid_pending_cancel_to_submitted() {
        assert!(
            OrderStatus::validate_transition(OrderStatus::PendingCancel, OrderStatus::Submitted)
                .is_err()
        );
    }

    #[test]
    fn invalid_self_transition() {
        // No status should transition to itself.
        let all = [
            OrderStatus::Draft,
            OrderStatus::Inactive,
            OrderStatus::PendingSubmit,
            OrderStatus::PreSubmitted,
            OrderStatus::Submitted,
            OrderStatus::PartiallyFilled,
            OrderStatus::Filled,
            OrderStatus::PendingCancel,
            OrderStatus::Cancelled,
            OrderStatus::Rejected,
            OrderStatus::Error,
        ];
        for status in all {
            assert!(
                OrderStatus::validate_transition(status, status).is_err(),
                "{status} -> {status} should be invalid"
            );
        }
    }
}
