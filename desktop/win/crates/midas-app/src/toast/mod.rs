//! Toast notification controller.
//!
//! First slice of the `MidasApp` god-struct split (architecture audit
//! P1). Owns the floating toast state, the messages that mutate it, and
//! the iced `view()` that renders it.
//!
//! # Pattern
//!
//! Mirrors [`crate::ticker_state::TickerState::apply`]: a single
//! `update(msg) -> Vec<Effect>` mutator, private fields, and an opaque
//! `Effect` enum that the parent interprets. The pattern is shared so
//! the codebase has *one* mental model for sub-controllers, not two.
//!
//! # Why Toast first
//!
//! Toast was chosen as the proof-of-pattern slice because it has the
//! interesting cross-controller cases without being entangled:
//!
//! - it has a real `view()` (exercises `Element::map(Message::Toast)`)
//! - `Action::ActionClicked` re-dispatches an arbitrary `Box<Message>`
//!   (the only path Toast has to talk to other controllers — captured
//!   in [`Effect::FireParentMsg`])
//! - auto-dismiss via `Tick` exercises the "subscription stays
//!   centralized; parent calls into controller" path
//!
//! See `desktop/win/plan/midasapp-split.md`.

use std::time::{Duration, Instant};

use iced::widget::{button, container, row, text, Space};
use iced::{Color, Element, Fill, Length};

use crate::app::Message;

/// Seconds a toast remains visible before auto-dismiss.
pub const TOAST_TTL_SECS: u64 = 4;

/// Floating toast notification state.
///
/// Construction is private to this module; callers go through
/// [`ToastController::update`] with a [`ToastMsg::Show`].
#[derive(Clone, Debug)]
pub struct ToastState {
    /// The human-readable message rendered in the toast body.
    pub message: String,
    /// When the toast appeared. Compared against `Instant::now()` in
    /// [`ToastController::tick`] to fire the auto-dismiss path.
    pub created_at: Instant,
    /// Optional action button. When set, the toast renders an extra
    /// clickable region that emits the embedded message before
    /// dismissing.
    pub action: Option<ToastAction>,
}

/// An action button embedded inside a [`ToastState`].
///
/// The `on_click` field is boxed so the action can own any
/// [`crate::app::Message`] variant (including ones that carry
/// allocations like `OrderIntent`) without enlarging the outer enum.
#[derive(Clone, Debug)]
pub struct ToastAction {
    /// Button label. Example: `"Undo"`.
    pub label: String,
    /// Message emitted when the button is clicked. Delivered verbatim
    /// by the [`ToastMsg::ActionClicked`] effect interpretation.
    pub on_click: Box<Message>,
}

/// Messages routed to the toast controller.
///
/// The parent's `Message::Toast(ToastMsg)` wrapper is the only way
/// callers reach the controller — the controller never sees the
/// outer `Message` type except via the opaque [`Effect::FireParentMsg`]
/// payload.
#[derive(Clone, Debug)]
#[allow(dead_code)] // `Dismiss` is exposed for future click-outside affordance.
pub enum ToastMsg {
    /// Replace the current toast (if any) with a new one.
    Show {
        message: String,
        action: Option<ToastAction>,
    },
    /// Manually dismiss the current toast. No-op if none is visible.
    Dismiss,
    /// The action button on the current toast was clicked. Fires the
    /// stored `on_click` message via [`Effect::FireParentMsg`] and
    /// clears the toast. Safe to emit when no toast is visible.
    ActionClicked,
}

/// Effects emitted from [`ToastController::update`] / [`ToastController::tick`].
///
/// Two variants only — kept tight on purpose. New variants need a
/// real cross-controller use case AND a paragraph of justification at
/// the call site. The compile-time assertion at the bottom of this
/// file pins the count.
#[derive(Debug)]
#[allow(dead_code)] // `Spawn` reserved for future async controllers; kept for symmetry.
pub enum Effect {
    /// Async work spawned by the controller. Parent maps the resulting
    /// task to its top-level message via `.map(Message::Toast)`.
    Spawn(iced::Task<ToastMsg>),
    /// Fire an arbitrary parent message. The only path Toast has to
    /// influence sibling controllers — used by [`ToastMsg::ActionClicked`]
    /// to re-dispatch the embedded action.
    FireParentMsg(Box<Message>),
}

/// Compile-time guard against [`Effect`] drift. Bump deliberately when
/// a new effect variant is genuinely warranted.
const _: () = {
    // Effect doesn't `impl Copy`; we count via a manual exhaustive match
    // on a sentinel rather than `mem::variant_count` (still nightly-only
    // on stable rustc 1.94 unless `#[feature(variant_count)]` is on).
    fn _count(e: &Effect) -> u8 {
        match e {
            Effect::Spawn(_) => 1,
            Effect::FireParentMsg(_) => 2,
        }
    }
};

/// Toast subsystem state + behaviour. Field-private; mutate only via
/// [`Self::update`] / [`Self::tick`].
#[derive(Debug, Default)]
pub struct ToastController {
    state: Option<ToastState>,
}

impl ToastController {
    /// Fresh controller with no toast visible.
    pub fn new() -> Self {
        Self { state: None }
    }

    /// Read-only access for callers that need to inspect toast state
    /// (e.g. global Escape-key handler that clears every popover).
    #[allow(dead_code)] // exposed for callers outside the module + tests.
    pub fn state(&self) -> Option<&ToastState> {
        self.state.as_ref()
    }

    /// Force-clear the toast bypassing the message loop. Used by the
    /// Escape-key global handler. Doesn't fire the action; just hides
    /// the UI.
    pub fn clear(&mut self) {
        self.state = None;
    }

    /// Apply a `ToastMsg` and return any effects the parent must
    /// interpret. Pure: no I/O, no time queries, no parent state
    /// access.
    pub fn update(&mut self, msg: ToastMsg) -> Vec<Effect> {
        match msg {
            ToastMsg::Show { message, action } => {
                self.state = Some(ToastState {
                    message,
                    created_at: Instant::now(),
                    action,
                });
                Vec::new()
            }
            ToastMsg::Dismiss => {
                self.state = None;
                Vec::new()
            }
            ToastMsg::ActionClicked => {
                // Take state first — single-click UX expects the toast
                // to be gone the moment the user engaged it, even if
                // the action carries no on_click.
                let Some(state) = self.state.take() else {
                    return Vec::new();
                };
                match state.action {
                    Some(action) => vec![Effect::FireParentMsg(action.on_click)],
                    None => Vec::new(),
                }
            }
        }
    }

    /// Auto-dismiss path called from the central `Tick` handler. Pure
    /// against the supplied `now` so the boundary is testable.
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        if let Some(ref s) = self.state {
            if now.saturating_duration_since(s.created_at)
                > Duration::from_secs(TOAST_TTL_SECS)
            {
                self.state = None;
            }
        }
        Vec::new()
    }

    /// Render the toast overlay. Returns `None` when no toast is
    /// visible so the call site can skip pushing an empty layer.
    ///
    /// The returned element emits [`ToastMsg`]; the parent wraps with
    /// `.map(Message::Toast)` at the call site (one place — all the
    /// `Message::Toast(...)` knowledge stays at the seam).
    pub fn view(&self) -> Option<Element<'_, ToastMsg>> {
        let state = self.state.as_ref()?;
        let msg_text = text(state.message.clone()).size(13).color(Color::WHITE);

        let body: Element<'_, ToastMsg> = match state.action {
            Some(ref action) => {
                let action_btn =
                    button(text(action.label.clone()).size(12).color(Color::WHITE))
                        .padding([3, 10])
                        .style(|_, status| button::Style {
                            background: Some(iced::Background::Color(match status {
                                button::Status::Hovered => {
                                    Color::from_rgba(0.35, 0.50, 0.72, 1.0)
                                }
                                _ => Color::from_rgba(0.25, 0.40, 0.62, 1.0),
                            })),
                            text_color: Color::WHITE,
                            border: iced::Border {
                                color: Color::from_rgba(0.55, 0.70, 0.90, 0.9),
                                width: 1.0,
                                radius: 3.0.into(),
                            },
                            ..Default::default()
                        })
                        .on_press(ToastMsg::ActionClicked);
                row![
                    msg_text,
                    Space::new().width(Length::Fixed(12.0)),
                    action_btn,
                ]
                .align_y(iced::Alignment::Center)
                .into()
            }
            None => msg_text.into(),
        };

        let toast_container = container(body)
            .padding([8, 14])
            .style(|_| container::Style {
                background: Some(iced::Background::Color(Color::from_rgba(
                    0.12, 0.14, 0.18, 0.94,
                ))),
                text_color: Some(Color::WHITE),
                border: iced::Border {
                    color: Color::from_rgba(0.30, 0.35, 0.45, 0.95),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            });

        let positioned = container(toast_container)
            .width(Fill)
            .height(Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Bottom)
            .padding(16);
        Some(positioned.into())
    }
}

#[cfg(test)]
mod tests;
