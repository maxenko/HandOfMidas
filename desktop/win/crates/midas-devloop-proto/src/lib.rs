//! Wire protocol for the Hand of Midas dev harness.
//!
//! Pure serde types shared between the in-process harness listener
//! (`midas-app` behind the `dev_harness` feature) and any external driver
//! that speaks the socket protocol. Defined in their own crate so both
//! sides reference a single source of truth; no domain dependencies,
//! cheap to pull into throwaway tooling.
//!
//! Transport is newline-delimited JSON over TCP on `127.0.0.1`. One
//! request per line, one response per line. See `plan/devloop-spec.md`
//! for the full design.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Current envelope version. Bumped when [`FixtureEnvelope`]'s shape
/// changes in a way that breaks forward-reading.
pub const DEVLOOP_FIXTURE_VERSION: u32 = 1;

/// Default TCP port the harness listens on. Override with the
/// `DEVLOOP_PORT` environment variable for parallel app instances.
pub const DEFAULT_PORT: u16 = 9898;

// ── Commands ──────────────────────────────────────────────────────────

/// Command sent over the control socket to the harness.
///
/// Variant names serialise as `snake_case`; the discriminator is the
/// `cmd` field of the outer JSON object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Sanity check. Response body carries `{"pid": N}`.
    Ping,
    /// Graceful shutdown of the target app.
    Shutdown,

    // -- Fixtures --
    /// Replace current app state with the named fixture on disk.
    LoadFixture { name: String },
    /// Snapshot the current app state to a named fixture on disk.
    SnapshotFixture { name: String, note: Option<String> },

    // -- State inspection --
    /// Serialise the app's state tree. `path` is an optional jq-like
    /// dotted path into the dumped JSON (e.g. `tickers.AAPL.live_bracket`).
    DumpState { path: Option<String> },
    /// Block until an event of `event_type` appears in the log, or
    /// `timeout_ms` elapses. When `since_cursor` is set, only events
    /// appended strictly after that log cursor match — use the cursor
    /// from the previous response to defeat races with the writer.
    WaitForEvent {
        event_type: String,
        timeout_ms: u64,
        since_cursor: Option<u64>,
    },
    /// Block until no input-origin or state-mutating message has been
    /// processed for roughly three frames, or `timeout_ms` elapses.
    WaitForIdle { timeout_ms: u64 },

    // -- Output --
    /// Capture a PNG of the main window to `out_path`.
    Screenshot { out_path: PathBuf },

    // -- Input injection --
    /// Click at a logical pixel coordinate, origin top-left of the main
    /// window's client area.
    Click {
        x: f32,
        y: f32,
        button: MouseButton,
        modifiers: Modifiers,
    },
    /// Click at a chart-space coordinate on the panel bound to `symbol`.
    /// Fails with [`ErrorKind::SymbolNotBound`] if no chart shows it.
    ClickPrice {
        symbol: String,
        price: f64,
        bar_index: i64,
        button: MouseButton,
        modifiers: Modifiers,
    },
    /// Drag from `from` to `to`, emitting `interpolation_steps`
    /// intermediate `CursorMoved` events. If `pause_at_hover` is set,
    /// the sequence stops at `to` without releasing the button — caller
    /// issues a follow-up command (screenshot, etc.) and then dispatches
    /// a release by sending another `Drag` with matching endpoints.
    Drag {
        from: Point,
        to: Point,
        pause_at_hover: bool,
        interpolation_steps: u32,
    },
    /// Wheel delta at a logical pixel coordinate.
    Scroll { x: f32, y: f32, dx: f32, dy: f32 },
    /// Key combo like `"Ctrl+S"`, `"Escape"`, `"Enter"`.
    Key { combo: String },

    // -- Fast path: bypass input wiring, hit the domain directly --
    /// Apply a `TickerMsg` directly to the ticker state for `symbol`.
    /// Effects fire as in production — see the blast-radius note in
    /// `plan/devloop-spec.md` before using this on an IB-attached build.
    InjectTickerMsg {
        symbol: String,
        msg_json: serde_json::Value,
    },

    /// Synthesise a `BrokerEvent` and feed it through the desktop's
    /// event-receiving pipeline, bypassing the broker engine. Useful
    /// for scripted fill / status-change journeys that the `TestBroker`
    /// wouldn't drive on its own (e.g. a Limit leg that needs a price
    /// tick to fill, or testing `OrderRejected` handling).
    ///
    /// Hand-parsed on the harness side — internally-tagged JSON
    /// matching the [`InjectTickerMsg`] convention:
    /// `{"type": "BracketCreated", "parent_id": "...", ...}`
    InjectBrokerEvent { event_json: serde_json::Value },

    /// Open a new Orders blotter pane in the workspace. Equivalent to
    /// clicking the "Orders" toolbar button. Lets scripted journeys set
    /// up the panel without a manual click.
    OpenOrdersPanel,

    /// Advance the per-symbol thumbnail interval one step in the cycle
    /// (M1 → M5 → D1 → M1). Equivalent to clicking a thumbnail cell in
    /// the watchlist or blotter. Exposed for devloop debugging of the
    /// thumbnail-load path.
    CycleThumbnail { symbol: String },
}

// ── Responses ─────────────────────────────────────────────────────────

/// Response to a single command.
///
/// Every response carries a monotonic `log_cursor` (event-log line count
/// immediately after the command's own writes). Callers pass the cursor
/// from the previous response into `WaitForEvent { since_cursor: ... }`
/// to make event matching deterministic.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok {
        body: serde_json::Value,
        log_cursor: u64,
    },
    Error {
        kind: ErrorKind,
        message: String,
        log_cursor: u64,
    },
}

/// Failure modes surfaced in an [`Response::Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Malformed JSON on the wire.
    ParseError,
    /// Command variant not recognised.
    UnknownCommand,
    /// `click_price` / `inject_ticker_msg` referenced a symbol with no
    /// chart panel.
    SymbolNotBound,
    /// Named fixture does not exist on disk.
    FixtureNotFound,
    /// `devloop_fixture_version` or `ticker_state_version` disagrees
    /// with the current build.
    FixtureVersionMismatch,
    /// `wait_for_event` / `wait_for_idle` expired.
    Timeout,
    /// Panic hook caught a panic during command handling; process is in
    /// an unstable state, client should shut down.
    HarnessPanic,
    /// Catch-all with a message payload.
    Internal,
}

// ── Input helper types ────────────────────────────────────────────────

/// Logical pixel coordinate, top-left origin.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// Mouse button identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Modifier-key state for an input event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Windows / Super / Command key.
    pub logo: bool,
}

// ── Fixture envelope ──────────────────────────────────────────────────

/// Top-level envelope written to `.devloop/fixtures/<name>.json`.
///
/// The heavy lifting is delegated to `AppConfig` from `midas-core`,
/// which already captures window state, pane-grid topology, chart
/// cameras, watchlists, order panels, and provider selection. The
/// fixture wraps that with `TickerState` entries (which are not part
/// of `AppConfig`) and a version pair that gates compatibility.
///
/// This crate has no dependency on `midas-core` or `midas-app`, so
/// both `app_config` and `ticker_states` are opaque `serde_json::Value`s.
/// The harness side deserialises them into domain types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureEnvelope {
    /// Bumped whenever this envelope's shape changes incompatibly.
    pub devloop_fixture_version: u32,
    /// Mirror of `midas-app`'s `TickerState::CURRENT_VERSION` at capture.
    /// Mismatch with the running build errors loudly; fixtures are
    /// disposable dev artefacts, not a persistence layer.
    pub ticker_state_version: u32,
    /// ISO-8601 timestamp string at capture.
    pub captured_at: String,
    /// Free-form human note, e.g. `"SL drag bug reproduction"`.
    pub note: Option<String>,
    /// Which symbol the user was "focused on" at capture.
    pub active_ticker: Option<String>,
    /// Serialised `midas_core::config::AppConfig`. Opaque here.
    pub app_config: serde_json::Value,
    /// Serialised `Vec<TickerState>`, one per touched symbol. Opaque.
    pub ticker_states: Vec<serde_json::Value>,
}

/// Plain-data mirror of `midas_chart::Camera2D`. Used by `dump_state`
/// to project a readable camera JSON without a serde derive on
/// `Camera2D` itself.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CameraSnapshot {
    pub time_start: i64,
    pub time_end: i64,
    pub price_low: f64,
    pub price_high: f64,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub dpi_scale: f32,
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let json = serde_json::to_string(value).expect("serialise");
        serde_json::from_str(&json).expect("deserialise")
    }

    #[test]
    fn ping_command_roundtrips() {
        let cmd = Command::Ping;
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"cmd":"ping"}"#);
        let _back: Command = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn click_price_roundtrips() {
        let cmd = Command::ClickPrice {
            symbol: "AAPL".to_owned(),
            price: 184.50,
            bar_index: -10,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""cmd":"click_price""#));
        assert!(json.contains(r#""button":"left""#));
        let back: Command = serde_json::from_str(&json).unwrap();
        match back {
            Command::ClickPrice { symbol, price, .. } => {
                assert_eq!(symbol, "AAPL");
                assert!((price - 184.50).abs() < f64::EPSILON);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn wait_for_event_with_cursor() {
        let cmd = Command::WaitForEvent {
            event_type: "SetLegPrice".to_owned(),
            timeout_ms: 1_000,
            since_cursor: Some(42),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""since_cursor":42"#));
        let _back: Command = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn response_ok_roundtrips() {
        let resp = Response::Ok {
            body: serde_json::json!({"pid": 12345}),
            log_cursor: 7,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""status":"ok""#));
        assert!(json.contains(r#""log_cursor":7"#));
        let _back: Response = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn response_error_carries_kind_and_cursor() {
        let resp = Response::Error {
            kind: ErrorKind::SymbolNotBound,
            message: "no chart bound to AAPL".to_owned(),
            log_cursor: 11,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""kind":"symbol_not_bound""#));
        assert!(json.contains(r#""log_cursor":11"#));
        let _back: Response = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn fixture_envelope_roundtrips() {
        let env = FixtureEnvelope {
            devloop_fixture_version: DEVLOOP_FIXTURE_VERSION,
            ticker_state_version: 2,
            captured_at: "2026-04-17T14:22:00Z".to_owned(),
            note: Some("SL drag bug reproduction".to_owned()),
            active_ticker: Some("AAPL".to_owned()),
            app_config: serde_json::json!({"window": {"width": 2560, "height": 1440}}),
            ticker_states: vec![serde_json::json!({"symbol": "AAPL"})],
        };
        let back = roundtrip(&env);
        assert_eq!(back.ticker_state_version, 2);
        assert_eq!(back.active_ticker.as_deref(), Some("AAPL"));
        assert_eq!(back.ticker_states.len(), 1);
    }

    #[test]
    fn error_kind_wire_names_stable() {
        // Lock the JSON representations — drivers depend on these.
        let cases = [
            (ErrorKind::ParseError, "parse_error"),
            (ErrorKind::UnknownCommand, "unknown_command"),
            (ErrorKind::SymbolNotBound, "symbol_not_bound"),
            (ErrorKind::FixtureNotFound, "fixture_not_found"),
            (
                ErrorKind::FixtureVersionMismatch,
                "fixture_version_mismatch",
            ),
            (ErrorKind::Timeout, "timeout"),
            (ErrorKind::HarnessPanic, "harness_panic"),
            (ErrorKind::Internal, "internal"),
        ];
        for (kind, expected) in cases {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!(r#""{}""#, expected));
        }
    }
}
