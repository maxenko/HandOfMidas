//! Message-level synthesis for keyboard and scroll input.
//!
//! ## Design constraint
//!
//! iced 0.14 does not expose a public API to inject synthetic
//! `iced::Event::Mouse` / `iced::Event::Keyboard` events into a
//! running daemon's event loop. The event stream is OS-driven via
//! winit and flows OS → winit → iced_runtime; there is no
//! `push_event` on the runtime.
//!
//! Two ways to work around this:
//!
//! 1. **OS-level injection via `enigo`** (listed as "future growth"
//!    in the plan). Would inject real OS-level events that flow
//!    through winit. Exercises hit-testing. Steals window focus.
//! 2. **Message-level synthesis** (this module). Dispatch the
//!    `Message` that a real event would produce, bypassing
//!    hit-testing and the widget tree. Robust, deterministic, no
//!    focus steal. Does not exercise the widget-level event dispatch
//!    path.
//!
//! Chart-widget hit-testing lives in the sans-IO `midas-chart` crate
//! and is covered by its own unit tests. State transitions downstream
//! of hit-testing (which is what fixtures + assertions actually care
//! about) are fully exercisable via [`inject_ticker_msg`](super::inject)
//! and the cases handled here.
//!
//! ## Covered commands
//!
//! - [`Key`] — parses a combo string like `"Ctrl+S"` / `"Escape"` and
//!   dispatches `Message::KeyPressed` or a matching app-level message.
//! - [`Scroll`] — dispatches `Message::ChartZoom` or `Message::ChartPan`
//!   on the active chart.
//!
//! ## Not covered
//!
//! `Click` / `ClickPrice` / `Drag` return [`InputError::UseInject`] —
//! these would need either OS-level injection or per-interaction
//! Message dispatch logic that duplicates the widget tree's behaviour.
//! For state mutations (bracket creation, leg drag, level edit), use
//! `inject_ticker_msg`.

use iced::keyboard::key::Named;
use iced::keyboard::Key;
use thiserror::Error;

use crate::app::{Message, MidasApp};

#[derive(Debug, Error)]
pub enum InputError {
    #[error("unrecognised key combo: {0}")]
    UnknownCombo(String),
    #[error("no active chart to scroll against")]
    NoActiveChart,
}

/// Parse a combo string and return a `KeyPressed` Message. Supports:
///
/// - Named keys: `"Escape"`, `"Enter"`, `"Tab"`, `"Space"`, arrow keys,
///   `"F1".."F12"`.
/// - Single characters: `"n"`, `"1"`.
/// - Modifier prefixes: `"Ctrl+S"`, `"Shift+Tab"`, `"Ctrl+Shift+K"`.
///
/// Modifiers are carried through the inner `Message::KeyPressed` path
/// via the existing subscription — but since we're synthesizing
/// `Message::KeyPressed` directly we have to dispatch known-combo
/// side effects ourselves. `main.rs::subscription` maps raw events to
/// messages; the same matching logic is reproduced here.
pub fn dispatch_key(app: &mut MidasApp, combo: &str) -> Result<iced::Task<Message>, InputError> {
    let (modifiers, key_part) = split_modifiers(combo);
    let key = parse_key(key_part).ok_or_else(|| InputError::UnknownCombo(combo.to_owned()))?;

    // Reproduce the subscription-level shortcut mapping so dispatched
    // keys take the same route a real key press would. See
    // `main.rs::subscription`.
    if modifiers.ctrl {
        if let Key::Character(ref c) = key {
            if c.as_str() == "n" {
                return Ok(app.update(Message::AddChart));
            }
        }
    }

    Ok(app.update(Message::KeyPressed(key)))
}

/// Dispatch a scroll as a `ChartZoom` on the active chart. `dy` drives
/// time-axis zoom (the usual wheel scroll behaviour). `dx` drives pan.
/// Coordinates are ignored beyond picking the active chart — scroll
/// in HoM always targets the focused chart, not hover-under-cursor.
pub fn dispatch_scroll(
    app: &mut MidasApp,
    _x: f32,
    _y: f32,
    dx: f32,
    dy: f32,
) -> Result<iced::Task<Message>, InputError> {
    let chart_id = app.windows[&app.main_window_key]
        .layout
        .focused_chart_id()
        .ok_or(InputError::NoActiveChart)?;

    // Zoom factor from wheel delta. Conservative — 10% per notch.
    // Positive dy = zoom in (reduce visible range).
    let mut tasks = Vec::new();
    if dy.abs() > f32::EPSILON {
        let factor: f64 = if dy > 0.0 { 0.9 } else { 1.1 };
        let pivot = app
            .charts
            .get(&chart_id)
            .map(|c| {
                let cam = &c.chart_state.camera;
                (cam.time_start + cam.time_end) / 2.0
            })
            .unwrap_or(0.0);
        // ChartAction::Zoom takes pixel center_x; back-convert from
        // the data-space pivot we already computed so the dispatcher's
        // standard `cam.x_to_time(center_x)` round-trips to it.
        let center_x_px = app
            .charts
            .get(&chart_id)
            .map(|c| c.chart_state.camera.time_to_x(pivot))
            .unwrap_or(0.0);
        tasks.push(app.update(Message::Chart(
            chart_id,
            midas_chart::ChartAction::Zoom {
                center_x: center_x_px,
                factor,
            },
        )));
    }
    if dx.abs() > f32::EPSILON {
        // ChartPan takes a (dx, dy) in data space. Approximate with a
        // 5% shift of the visible time range.
        let (delta_time, delta_price) = app
            .charts
            .get(&chart_id)
            .map(|c| {
                let cam = &c.chart_state.camera;
                let dt = (cam.time_end - cam.time_start) * (dx as f64) * 0.05;
                (dt, 0.0)
            })
            .unwrap_or((0.0, 0.0));
        tasks.push(app.update(Message::Chart(
            chart_id,
            midas_chart::ChartAction::Pan {
                dx: delta_time,
                dy: delta_price,
            },
        )));
    }

    Ok(iced::Task::batch(tasks))
}

// ── Parsing helpers ──────────────────────────────────────────────────

#[derive(Default, Debug, Clone, Copy)]
struct ParsedMods {
    shift: bool,
    ctrl: bool,
    alt: bool,
    logo: bool,
}

fn split_modifiers(combo: &str) -> (ParsedMods, &str) {
    let mut mods = ParsedMods::default();
    let mut rest = combo;
    loop {
        let (head, tail) = match rest.find('+') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => return (mods, rest),
        };
        match head.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "shift" => mods.shift = true,
            "alt" => mods.alt = true,
            "meta" | "super" | "logo" | "win" | "cmd" => mods.logo = true,
            _ => return (mods, rest),
        }
        rest = tail;
    }
}

fn parse_key(part: &str) -> Option<Key> {
    if part.is_empty() {
        return None;
    }
    // Named keys first (case-sensitive names preferred).
    if let Some(named) = parse_named(part) {
        return Some(Key::Named(named));
    }
    // Single character fallback.
    Some(Key::Character(part.into()))
}

fn parse_named(part: &str) -> Option<Named> {
    // Match case-insensitively for ergonomics.
    let n = match part.to_ascii_lowercase().as_str() {
        "escape" | "esc" => Named::Escape,
        "enter" | "return" => Named::Enter,
        "tab" => Named::Tab,
        "space" => Named::Space,
        "backspace" => Named::Backspace,
        "delete" | "del" => Named::Delete,
        "arrowup" | "up" => Named::ArrowUp,
        "arrowdown" | "down" => Named::ArrowDown,
        "arrowleft" | "left" => Named::ArrowLeft,
        "arrowright" | "right" => Named::ArrowRight,
        "home" => Named::Home,
        "end" => Named::End,
        "pageup" => Named::PageUp,
        "pagedown" => Named::PageDown,
        "f1" => Named::F1,
        "f2" => Named::F2,
        "f3" => Named::F3,
        "f4" => Named::F4,
        "f5" => Named::F5,
        "f6" => Named::F6,
        "f7" => Named::F7,
        "f8" => Named::F8,
        "f9" => Named::F9,
        "f10" => Named::F10,
        "f11" => Named::F11,
        "f12" => Named::F12,
        _ => return None,
    };
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_modifiers_ctrl_s() {
        let (mods, rest) = split_modifiers("Ctrl+S");
        assert!(mods.ctrl);
        assert!(!mods.shift);
        assert_eq!(rest, "S");
    }

    #[test]
    fn split_modifiers_multi() {
        let (mods, rest) = split_modifiers("Ctrl+Shift+K");
        assert!(mods.ctrl);
        assert!(mods.shift);
        assert_eq!(rest, "K");
    }

    #[test]
    fn split_modifiers_none() {
        let (mods, rest) = split_modifiers("Escape");
        assert!(!mods.ctrl);
        assert!(!mods.shift);
        assert_eq!(rest, "Escape");
    }

    #[test]
    fn parse_escape() {
        let key = parse_key("Escape").unwrap();
        assert!(matches!(key, Key::Named(Named::Escape)));
    }

    #[test]
    fn parse_f11_case_insensitive() {
        let key = parse_key("f11").unwrap();
        assert!(matches!(key, Key::Named(Named::F11)));
    }

    #[test]
    fn parse_single_char() {
        let key = parse_key("n").unwrap();
        match key {
            Key::Character(c) => assert_eq!(c.as_str(), "n"),
            _ => panic!("expected Character"),
        }
    }

    #[test]
    fn parse_empty_rejected() {
        assert!(parse_key("").is_none());
    }
}
