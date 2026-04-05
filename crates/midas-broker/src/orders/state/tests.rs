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
fn from_ib_status_partially_filled() {
    assert_eq!(
        OrderStatus::from_ib_status("PartiallyFilled"),
        OrderStatus::PartiallyFilled
    );
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
    assert!(OrderStatus::PartiallyFilled.can_modify_at_ib());
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

#[test]
fn valid_pre_submitted_to_cancelled_oca() {
    // IB OCA: server-side cancel when bracket sibling fills.
    OrderStatus::validate_transition(OrderStatus::PreSubmitted, OrderStatus::Cancelled)
        .unwrap();
}

#[test]
fn valid_submitted_to_cancelled_oca() {
    // IB OCA: server-side cancel when bracket sibling fills.
    OrderStatus::validate_transition(OrderStatus::Submitted, OrderStatus::Cancelled).unwrap();
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
fn valid_partially_filled_to_cancelled_oca() {
    // IB OCA: server-side cancel of a partially filled child when bracket
    // sibling fills. IB sends Cancelled directly without PendingCancel.
    OrderStatus::validate_transition(OrderStatus::PartiallyFilled, OrderStatus::Cancelled)
        .unwrap();
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
