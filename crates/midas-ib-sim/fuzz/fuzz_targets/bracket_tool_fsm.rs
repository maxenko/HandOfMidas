//! `cargo-fuzz` target for [`midas_scene::tools::BracketTool`] (slice
//! 5b of the chart-transition plan).
//!
//! Feeds arbitrary byte sequences into the FSM as a stream of
//! `(InputEvent, preview_price)` pairs and asserts the tool never
//! panics and always leaves itself in a valid state after every event.
//!
//! Run locally (requires nightly toolchain + `cargo-fuzz`):
//!
//! ```bash
//! cargo +nightly install cargo-fuzz
//! cd crates/midas-ib-sim
//! cargo +nightly fuzz run bracket_tool_fsm -- -max_total_time=60
//! ```
//!
//! CI invocation lives in `.github/workflows/rust.yml` under the
//! `sim_fuzz_nightly` job (scheduled trigger only — per R19 this runs
//! nightly, NOT per-PR, to avoid the cargo-fuzz cold-install cost).

#![no_main]

use libfuzzer_sys::fuzz_target;
use midas_axis::PriceRange;
use midas_scene::input::{InputEvent, Key, Modifiers, MouseButton, Point};
use midas_scene::tools::{BracketTool, BracketToolMode, Side as ToolSide, ToolEffect};
use midas_scene::{InteractiveLayer, SceneError, ToolContext};

/// Build one [`InputEvent`] from a single fuzz byte. The low nibble
/// selects the event kind, the high nibble seeds the payload (button /
/// key char / cursor coordinate bucket).
fn event_from_byte(b: u8) -> InputEvent {
    let kind = b & 0x0f;
    let payload = (b >> 4) & 0x0f;
    match kind {
        0 => InputEvent::MouseDown {
            button: match payload % 3 {
                0 => MouseButton::Left,
                1 => MouseButton::Right,
                _ => MouseButton::Middle,
            },
            pt: Point::new((payload as f32) * 60.0, (payload as f32) * 25.0),
            modifiers: Modifiers::default(),
        },
        1 => InputEvent::MouseUp {
            button: MouseButton::Left,
            pt: Point::new((payload as f32) * 60.0, (payload as f32) * 25.0),
        },
        2 => InputEvent::MouseMove {
            pt: Point::new((payload as f32) * 60.0, (payload as f32) * 25.0),
        },
        3 => InputEvent::Wheel {
            dx: 0.0,
            dy: payload as f32,
            pt: Point::new(500.0, 200.0),
        },
        4 => InputEvent::KeyDown {
            key: match payload {
                0 => Key::Char('L'),
                1 => Key::Char('S'),
                2 => Key::Char('B'),
                3 => Key::Char('b'),
                4 => Key::Char('s'),
                5 => Key::Escape,
                6 => Key::ArrowUp,
                7 => Key::ArrowDown,
                _ => Key::Char((b'A' + payload) as char),
            },
            modifiers: Modifiers::default(),
        },
        5 => InputEvent::KeyUp {
            key: Key::Char((b'A' + payload) as char),
        },
        6 => InputEvent::CursorLeft,
        // Otherwise: MouseMove with a different coordinate mapping so
        // the fuzzer can walk the preview path.
        _ => InputEvent::MouseMove {
            pt: Point::new((payload as f32) * 30.0, (b as f32) * 1.5),
        },
    }
}

/// True iff the tool's mode is one of the five valid FSM states. Any
/// state beyond these is a bug — the derive-heavy enum guarantees the
/// compiler rejects novel values, but we still assert in case the FSM
/// representation grows a ghost state in future.
fn mode_is_valid(mode: BracketToolMode) -> bool {
    matches!(
        mode,
        BracketToolMode::Idle
            | BracketToolMode::AwaitingEntry { .. }
            | BracketToolMode::AwaitingTarget { .. }
            | BracketToolMode::AwaitingStop { .. }
            | BracketToolMode::Complete
    )
}

fuzz_target!(|data: &[u8]| {
    // Seed with a fresh tool in AwaitingEntry { Long }. Arbitrary byte
    // 0 toggles to Short so both side-entry paths are exercised.
    let mut tool = if data.first().copied().unwrap_or(0) & 1 == 0 {
        BracketTool::awaiting_entry(ToolSide::Long)
    } else {
        BracketTool::awaiting_entry(ToolSide::Short)
    };

    let pr = PriceRange::new(90.0, 110.0).unwrap();
    let mut last_err: Option<SceneError> = None;
    let mut effs: Vec<ToolEffect> = Vec::new();

    for &b in data.iter() {
        // Opportunistically feed a preview price once per iteration —
        // mapping the fuzz byte into a finite price so click paths
        // that check `preview_price.is_finite()` don't short-circuit.
        let price = 90.0 + ((b as f64) / 255.0) * 20.0;
        let cursor_y = (b as f32) * 1.5;
        tool.update_preview(price, cursor_y);

        let ev = event_from_byte(b);
        {
            let mut cx = ToolContext {
                price_range: &pr,
                last_error: &mut last_err,
                effects: &mut effs,
            };
            // Must never panic. Result is discarded — we only care that
            // the call returns.
            let _ = InteractiveLayer::update(&mut tool, ev, &mut cx);
        }
        // FSM must stay in a valid state after every event.
        assert!(mode_is_valid(tool.mode()));
        // Bound the effect queue so a runaway loop doesn't OOM the
        // fuzz runner.
        if effs.len() > 64 {
            effs.clear();
        }
    }

    // Final: cancel + cancel_with_effect must leave Idle regardless of
    // the FSM's last state.
    tool.cancel();
    assert_eq!(tool.mode(), BracketToolMode::Idle);
    let mut trailing = Vec::new();
    tool.cancel_with_effect(&mut trailing);
    assert_eq!(tool.mode(), BracketToolMode::Idle);
});
