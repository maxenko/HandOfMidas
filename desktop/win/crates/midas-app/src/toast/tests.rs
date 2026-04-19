//! Pure tests for [`super::ToastController`]. No `MidasApp` needed.

use std::time::{Duration, Instant};

use super::{Effect, ToastAction, ToastController, ToastMsg, TOAST_TTL_SECS};
use crate::app::Message;

fn make_action() -> ToastAction {
    ToastAction {
        label: "Undo".to_owned(),
        on_click: Box::new(Message::Tick),
    }
}

#[test]
fn new_starts_empty() {
    let c = ToastController::new();
    assert!(c.state().is_none());
    assert!(c.view().is_none());
}

#[test]
fn show_sets_state_no_effects() {
    let mut c = ToastController::new();
    let effects = c.update(ToastMsg::Show {
        message: "Hello".into(),
        action: None,
    });
    assert!(effects.is_empty());
    let s = c.state().expect("state set");
    assert_eq!(s.message, "Hello");
    assert!(s.action.is_none());
    assert!(c.view().is_some());
}

#[test]
fn show_replaces_existing_toast() {
    let mut c = ToastController::new();
    c.update(ToastMsg::Show {
        message: "first".into(),
        action: None,
    });
    c.update(ToastMsg::Show {
        message: "second".into(),
        action: Some(make_action()),
    });
    let s = c.state().expect("state set");
    assert_eq!(s.message, "second");
    assert!(s.action.is_some());
}

#[test]
fn dismiss_clears_state() {
    let mut c = ToastController::new();
    c.update(ToastMsg::Show {
        message: "x".into(),
        action: None,
    });
    let effects = c.update(ToastMsg::Dismiss);
    assert!(effects.is_empty());
    assert!(c.state().is_none());
}

#[test]
fn dismiss_when_empty_is_noop() {
    let mut c = ToastController::new();
    let effects = c.update(ToastMsg::Dismiss);
    assert!(effects.is_empty());
    assert!(c.state().is_none());
}

#[test]
fn action_clicked_with_action_emits_fire_parent_msg_and_clears() {
    let mut c = ToastController::new();
    c.update(ToastMsg::Show {
        message: "x".into(),
        action: Some(make_action()),
    });

    let effects = c.update(ToastMsg::ActionClicked);
    assert_eq!(effects.len(), 1);
    match effects.into_iter().next().unwrap() {
        Effect::FireParentMsg(boxed) => {
            assert!(matches!(*boxed, Message::Tick));
        }
        _ => panic!("expected FireParentMsg"),
    }
    assert!(
        c.state().is_none(),
        "state must clear regardless of action presence"
    );
}

#[test]
fn action_clicked_without_action_clears_no_effects() {
    let mut c = ToastController::new();
    c.update(ToastMsg::Show {
        message: "x".into(),
        action: None,
    });
    let effects = c.update(ToastMsg::ActionClicked);
    assert!(effects.is_empty());
    assert!(c.state().is_none());
}

#[test]
fn action_clicked_when_empty_is_noop() {
    let mut c = ToastController::new();
    let effects = c.update(ToastMsg::ActionClicked);
    assert!(effects.is_empty());
    assert!(c.state().is_none());
}

#[test]
fn tick_before_ttl_preserves_state() {
    let mut c = ToastController::new();
    c.update(ToastMsg::Show {
        message: "x".into(),
        action: None,
    });
    let created = c.state().unwrap().created_at;
    // Half the TTL — must remain.
    let now = created + Duration::from_secs(TOAST_TTL_SECS / 2);
    let effects = c.tick(now);
    assert!(effects.is_empty());
    assert!(c.state().is_some());
}

#[test]
fn tick_past_ttl_clears_state() {
    let mut c = ToastController::new();
    c.update(ToastMsg::Show {
        message: "x".into(),
        action: None,
    });
    let created = c.state().unwrap().created_at;
    let now = created + Duration::from_secs(TOAST_TTL_SECS + 1);
    let effects = c.tick(now);
    assert!(effects.is_empty());
    assert!(c.state().is_none());
}

#[test]
fn tick_when_empty_is_noop() {
    let mut c = ToastController::new();
    let effects = c.tick(Instant::now());
    assert!(effects.is_empty());
}

#[test]
fn clear_force_drops_state_without_firing_action() {
    // Escape-key path: hide the toast immediately, never fire its
    // action (the user is dismissing, not engaging).
    let mut c = ToastController::new();
    c.update(ToastMsg::Show {
        message: "x".into(),
        action: Some(make_action()),
    });
    c.clear();
    assert!(c.state().is_none());
}

#[test]
fn view_renders_when_state_present() {
    let mut c = ToastController::new();
    c.update(ToastMsg::Show {
        message: "Visible".into(),
        action: None,
    });
    // Smoke check — `view()` returns `Some(_)` and doesn't panic.
    // Concrete shape is exercised by integration tests; the goal here
    // is just to prove the controller's view path doesn't depend on
    // any external state.
    assert!(c.view().is_some());
}
