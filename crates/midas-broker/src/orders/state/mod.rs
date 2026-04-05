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
            "PartiallyFilled" => Self::PartiallyFilled,
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
    ///
    /// Includes `PartiallyFilled` because IB accepts modifications to the
    /// remaining unfilled quantity. This is important for bracket children
    /// (TP/SL) on large or illiquid orders where partial fills are possible.
    pub fn can_modify_at_ib(&self) -> bool {
        matches!(
            self,
            Self::PreSubmitted | Self::Submitted | Self::PartiallyFilled
        )
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
            // Cancelled is allowed directly from PreSubmitted/Submitted for
            // IB server-side cancellations (OCA auto-cancel on bracket sibling
            // fill, parent cancel propagation). IB sends "Cancelled" without
            // going through PendingCancel for these cases.
            Self::PreSubmitted => matches!(
                to,
                Self::Submitted | Self::PendingCancel | Self::Cancelled | Self::Filled | Self::Rejected
            ),
            Self::Submitted => matches!(
                to,
                Self::PartiallyFilled | Self::Filled | Self::PendingCancel | Self::Cancelled | Self::Rejected
            ),
            Self::PartiallyFilled => matches!(to, Self::Filled | Self::PendingCancel | Self::Cancelled),
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

#[cfg(test)]
mod tests;
