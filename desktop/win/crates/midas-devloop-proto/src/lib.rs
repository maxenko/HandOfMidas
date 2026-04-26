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
///
/// ## Version history
///
/// - `1` (retired) — original shape; no `schema` field on the envelope.
///   Deserialisation still accepts this via `#[serde(default)]` on the
///   new field.
/// - `2` (current) — adds the per-envelope `schema: u32` stamp so future
///   schema bumps don't require another top-level version churn. Slice
///   8c of the chart-transition plan.
pub const DEVLOOP_FIXTURE_VERSION: u32 = 2;

/// The oldest `devloop_fixture_version` this build accepts on load.
/// Setting this to `1` preserves the slice-8c backward-compat story —
/// a v1 fixture loads, upgrades forward in memory, and saves as v2.
pub const MIN_SUPPORTED_FIXTURE_VERSION: u32 = 1;

/// Default schema for [`FixtureEnvelope::schema`] when the field is
/// missing on a v1 envelope. Mirrors the legacy "implicit v1" shape.
pub const FIXTURE_SCHEMA_V1: u32 = 1;

/// Current schema version stamped by new-stack writers. Slice 8c.
pub const CURRENT_FIXTURE_SCHEMA: u32 = 2;

/// Returns the default fixture schema for new envelopes. Used as a
/// serde `default = ...` callback on [`FixtureEnvelope::schema`] so
/// older on-disk fixtures that lack the field round-trip as v1.
fn default_fixture_schema() -> u32 {
    FIXTURE_SCHEMA_V1
}

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
    /// event-receiving pipeline, bypassing the router. Useful for
    /// scripted fill / status-change journeys the sim wouldn't drive
    /// on its own (e.g. a Limit leg that needs a price tick to fill,
    /// or testing `OrderRejected` handling).
    ///
    /// Hand-parsed on the harness side — internally-tagged JSON
    /// matching the [`InjectTickerMsg`] convention:
    /// `{"type": "BracketCreated", "parent_id": "...", ...}`
    InjectBrokerEvent { event_json: serde_json::Value },

    /// Set Volume Profile settings for a chart. Used by devloop scripts
    /// (see `tools/devloop-vp-anchored-*.sh`) to drive Slice 2/3 render
    /// paths without going through the gear popup UI.
    ///
    /// Payload is opaque JSON — the harness deserialises it into
    /// `midas_core::VolumeProfileSettings` on receipt — to preserve
    /// the proto crate's "no domain dependencies" rule (matches the
    /// [`InjectTickerMsg`] / [`InjectBrokerEvent`] / [`InjectMarketEvent`]
    /// pattern). The expected shape is the TOML-serialised settings
    /// table, e.g. `{"anchor": "Daily", "width_fraction": 0.7,
    /// "show_value_area": false, "value_area_pct": 0.7}`.
    SetVpSettings {
        chart_id: u64,
        settings_json: serde_json::Value,
    },

    /// Synthesise a `MarketEvent` and push it through the router's
    /// underlying provider (S8d).
    ///
    /// Wire shape is the externally-tagged JSON that
    /// `midas_broker_core::market_data::MarketEvent` serialises to
    /// — `{"Tick": {...}}`, `{"Bar": {...}}`, `{"FarmStatus": {...}}`,
    /// etc. The harness deserialises the value into a `MarketEvent`
    /// and calls `router.source_for_test().inject_for_test(event)`.
    ///
    /// Only the sim provider implements `inject_for_test`; the real
    /// IB provider is a no-op. Requires the `test_inject` feature on
    /// `midas-market-data`, which `dev_harness` enables transitively.
    InjectMarketEvent { event_json: serde_json::Value },

    /// Open a new Orders blotter pane in the workspace. Equivalent to
    /// clicking the "Orders" toolbar button. Lets scripted journeys set
    /// up the panel without a manual click.
    OpenOrdersPanel,

    /// Advance the per-symbol thumbnail interval one step in the cycle
    /// (M1 → M5 → D1 → M1). Equivalent to clicking a thumbnail cell in
    /// the watchlist or blotter. Exposed for devloop debugging of the
    /// thumbnail-load path.
    CycleThumbnail { symbol: String },
    /// Switch the active tab of the first (or sole) Account panel.
    /// Useful for scripted visual verification — the devloop otherwise
    /// has no way to click a tab.
    ///
    /// `tab` is one of `"positions" | "orders" | "trade-history" |
    /// "recents"` (kebab-case, matching `AccountTab` serde).
    SetAccountTab { tab: String },

    // -- IB simulator child-process lifecycle (Stage 09B) --
    /// Spawn `midas-ib-sim-server` as a child process bound to `port`
    /// (TWS wire-protocol) and `control_port` (HTTP control plane).
    ///
    /// The harness blocks until the sim's `/control/health` endpoint
    /// responds `200 OK`, then records the child's PID under
    /// `.devloop/sim.<port>.pid` and reads the bearer token the sim
    /// wrote to disk. Subsequent [`InjectSimFault`] calls reuse that
    /// token against the same control port.
    ///
    /// `scenario` is forwarded as `--scenario <path>` when set.
    /// `seed` is forwarded as `--seed <n>` when set; otherwise the sim
    /// uses its own default (12345).
    SpawnSim {
        port: u16,
        control_port: u16,
        scenario: Option<String>,
        seed: Option<u64>,
    },
    /// SIGTERM the running sim child process, with a 5s grace period
    /// before falling back to SIGKILL. No-op if no sim was spawned.
    ShutdownSim,
    /// Forward a fault-injection request to the running sim's control
    /// plane (`POST /control/inject`). The harness supplies the bearer
    /// token read at [`SpawnSim`] time. Body is serialised to the same
    /// `{"type": "...", ...}` shape the sim accepts.
    InjectSimFault { fault: SimFault },

    // -- Chart-transition parity harness (Slice 0) --
    /// Compare two PNGs on disk. Mirrors the same
    /// `image_compare::rgba_hybrid_compare` + pixel-diff path the
    /// screenshot handler uses, without requiring a
    /// `.devloop/refs/<stem>.png` convention. The response body is a
    /// [`CompareResult`]. Written to drive chart-parity-harness
    /// scripts that need to diff two arbitrary paths (e.g.
    /// legacy-backend.png vs new-backend.png of the same fixture).
    CompareImages {
        path_a: PathBuf,
        path_b: PathBuf,
        /// Optional: write the similarity map to this path. Skipped if
        /// `None` so CI scripts can keep the harness read-only.
        diff_out: Option<PathBuf>,
    },
}

/// Fault-injection payloads forwarded to the sim control plane. Wire
/// format is internally-tagged JSON with the variant name in `type`.
///
/// Variants are intentionally shape-compatible with the engine's
/// `InjectDisconnect` / `InjectPacingViolation` / `InjectFarmOutage` /
/// `InjectFarmRestore` / `InjectPriceJump` / `InjectGap` / `InjectHalt` /
/// `InjectBurst` commands — the sim deserialises, builds the
/// corresponding `EngineCmd`, and pushes it onto the engine inbox.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SimFault {
    /// Force a disconnect of every active TWS session. Mirrors the
    /// scenario DSL `inject_disconnect` verb with no session filter.
    Disconnect,
    /// Trigger a pacing-violation (error 100 + disconnect) on the
    /// active session.
    PacingViolation,
    /// Emit a farm-outage bulletin for the named farms. Names match the
    /// IB convention (e.g. `usfarm`, `usfuture`, `cashfarm`).
    FarmOutage { farms: Vec<String> },
    /// Emit a farm-restore bulletin. `data_lost` mirrors real IB's
    /// 1101/1102 distinction — `true` means subscribers must re-request
    /// market data (code 1101), `false` means no data was lost (1102).
    FarmRestore { farms: Vec<String>, data_lost: bool },
    /// Jump the quoted mid-price of `symbol` by `magnitude_pct` over a
    /// single tick. Positive values move the price up.
    PriceJump { symbol: String, magnitude_pct: f64 },
    /// Splice a price gap ending at `to` into the quote stream for
    /// `symbol`. The from-price is whatever the engine last quoted.
    Gap { symbol: String, to: f64 },
    /// Halt quoting on `symbol` for `duration_ms` milliseconds.
    Halt { symbol: String, duration_ms: u64 },
    /// Scale emission rate for every listed symbol by `multiplier` for
    /// the next `duration_ms` milliseconds. Used to stress the UI's
    /// tick-coalescing path.
    Burst {
        symbols: Vec<String>,
        multiplier: f64,
        duration_ms: u64,
    },
}

/// Body payload for a successful [`Command::CompareImages`]. Serialises
/// into the `Response::Ok { body }` envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompareResult {
    /// SSIM ∈ `[0.0, 1.0]`; `1.0` = identical. Mirrors
    /// `image_compare::rgba_hybrid_compare(..).score`.
    pub ssim: f64,
    /// Fraction of pixels that differ by more than the perceptual
    /// threshold used in the screenshot diff path.
    pub diff_fraction: f64,
    pub width: u32,
    pub height: u32,
    /// Set iff the command supplied `diff_out` AND the similarity map
    /// was written successfully.
    pub diff_path: Option<PathBuf>,
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
    /// [`Command::InjectSimFault`] / [`Command::ShutdownSim`] invoked
    /// before a successful [`Command::SpawnSim`].
    SimNotRunning,
    /// [`Command::SpawnSim`] failed to launch the sim binary, bind the
    /// control plane, or observe a healthy `/control/health` response.
    SimSpawnFailed,
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
    /// Per-envelope schema stamp — orthogonal to
    /// [`Self::devloop_fixture_version`]. Slice 8c of the
    /// chart-transition plan. Carries the chart-view-store schema the
    /// capturing binary wrote; the loader forwards-migrates v1 → v2
    /// on next snapshot.
    ///
    /// `default = 1` so a pre-slice-8c fixture file (no `schema` key)
    /// deserialises as v1. Writers always emit
    /// [`CURRENT_FIXTURE_SCHEMA`].
    #[serde(default = "default_fixture_schema")]
    pub schema: u32,
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
            schema: CURRENT_FIXTURE_SCHEMA,
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
        assert_eq!(back.schema, CURRENT_FIXTURE_SCHEMA);
    }

    /// Slice 8c: a v1 envelope on disk lacks the `schema` field.
    /// Deserialisation must succeed and default the field to
    /// [`FIXTURE_SCHEMA_V1`] so the app-side loader can detect "this
    /// came from an older build" and translate forward on next
    /// snapshot.
    #[test]
    fn v1_envelope_missing_schema_defaults_to_v1() {
        let raw = r#"{
            "devloop_fixture_version": 1,
            "ticker_state_version": 2,
            "captured_at": "2026-04-17T00:00:00Z",
            "note": null,
            "active_ticker": null,
            "app_config": {},
            "ticker_states": []
        }"#;
        let env: FixtureEnvelope = serde_json::from_str(raw).expect("v1 parse");
        assert_eq!(env.schema, FIXTURE_SCHEMA_V1);
        assert_eq!(env.devloop_fixture_version, 1);
    }

    /// Slice 8c: v2 envelopes carry the `schema` field explicitly.
    #[test]
    fn v2_envelope_carries_explicit_schema() {
        let raw = r#"{
            "devloop_fixture_version": 2,
            "schema": 2,
            "ticker_state_version": 2,
            "captured_at": "2026-04-17T00:00:00Z",
            "note": null,
            "active_ticker": null,
            "app_config": {},
            "ticker_states": []
        }"#;
        let env: FixtureEnvelope = serde_json::from_str(raw).expect("v2 parse");
        assert_eq!(env.schema, 2);
        assert_eq!(env.devloop_fixture_version, 2);
    }

    #[test]
    fn spawn_sim_roundtrips() {
        let cmd = Command::SpawnSim {
            port: 7497,
            control_port: 9497,
            scenario: Some("fixtures/bracket_happy.yaml".to_owned()),
            seed: Some(42),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""cmd":"spawn_sim""#));
        assert!(json.contains(r#""port":7497"#));
        assert!(json.contains(r#""control_port":9497"#));
        assert!(json.contains(r#""seed":42"#));
        let back: Command = serde_json::from_str(&json).unwrap();
        match back {
            Command::SpawnSim {
                port,
                control_port,
                scenario,
                seed,
            } => {
                assert_eq!(port, 7497);
                assert_eq!(control_port, 9497);
                assert_eq!(scenario.as_deref(), Some("fixtures/bracket_happy.yaml"));
                assert_eq!(seed, Some(42));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn spawn_sim_optional_fields_omit() {
        let cmd = Command::SpawnSim {
            port: 7497,
            control_port: 9497,
            scenario: None,
            seed: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        // None fields serialise as JSON null (serde default); ensure
        // deserialisation accepts both absent and null forms.
        let _back: Command = serde_json::from_str(&json).unwrap();
        let _back2: Command =
            serde_json::from_str(r#"{"cmd":"spawn_sim","port":7497,"control_port":9497}"#).unwrap();
    }

    #[test]
    fn shutdown_sim_roundtrips() {
        let cmd = Command::ShutdownSim;
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"cmd":"shutdown_sim"}"#);
        let _back: Command = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn sim_fault_disconnect_roundtrips() {
        let fault = SimFault::Disconnect;
        let json = serde_json::to_string(&fault).unwrap();
        assert_eq!(json, r#"{"type":"disconnect"}"#);
        let back: SimFault = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SimFault::Disconnect);
    }

    #[test]
    fn sim_fault_pacing_violation_roundtrips() {
        let fault = SimFault::PacingViolation;
        let json = serde_json::to_string(&fault).unwrap();
        assert_eq!(json, r#"{"type":"pacing_violation"}"#);
        let _back: SimFault = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn sim_fault_farm_outage_roundtrips() {
        let fault = SimFault::FarmOutage {
            farms: vec!["usfarm".to_owned(), "cashfarm".to_owned()],
        };
        let json = serde_json::to_string(&fault).unwrap();
        assert!(json.contains(r#""type":"farm_outage""#));
        assert!(json.contains(r#""farms":["usfarm","cashfarm"]"#));
        let back: SimFault = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back,
            SimFault::FarmOutage {
                farms: vec!["usfarm".to_owned(), "cashfarm".to_owned()]
            }
        );
    }

    #[test]
    fn sim_fault_farm_restore_tracks_data_lost() {
        let fault = SimFault::FarmRestore {
            farms: vec!["usfarm".to_owned()],
            data_lost: true,
        };
        let json = serde_json::to_string(&fault).unwrap();
        assert!(json.contains(r#""type":"farm_restore""#));
        assert!(json.contains(r#""data_lost":true"#));
        let _back: SimFault = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn sim_fault_price_jump_roundtrips() {
        let fault = SimFault::PriceJump {
            symbol: "AAPL".to_owned(),
            magnitude_pct: -5.0,
        };
        let json = serde_json::to_string(&fault).unwrap();
        assert!(json.contains(r#""type":"price_jump""#));
        assert!(json.contains(r#""symbol":"AAPL""#));
        let _back: SimFault = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn sim_fault_gap_roundtrips() {
        let fault = SimFault::Gap {
            symbol: "AAPL".to_owned(),
            to: 150.25,
        };
        let json = serde_json::to_string(&fault).unwrap();
        assert!(json.contains(r#""type":"gap""#));
        let _back: SimFault = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn sim_fault_halt_roundtrips() {
        let fault = SimFault::Halt {
            symbol: "AAPL".to_owned(),
            duration_ms: 60_000,
        };
        let json = serde_json::to_string(&fault).unwrap();
        assert!(json.contains(r#""type":"halt""#));
        assert!(json.contains(r#""duration_ms":60000"#));
        let _back: SimFault = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn sim_fault_burst_roundtrips() {
        let fault = SimFault::Burst {
            symbols: vec!["AAPL".to_owned(), "MSFT".to_owned()],
            multiplier: 10.0,
            duration_ms: 1_000,
        };
        let json = serde_json::to_string(&fault).unwrap();
        assert!(json.contains(r#""type":"burst""#));
        let _back: SimFault = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn inject_sim_fault_command_roundtrips() {
        let cmd = Command::InjectSimFault {
            fault: SimFault::PacingViolation,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""cmd":"inject_sim_fault""#));
        assert!(json.contains(r#""type":"pacing_violation""#));
        let _back: Command = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn inject_market_event_command_roundtrips() {
        // Mirrors the `MarketEvent::Tick(..)` wire shape produced by
        // `midas-broker-core`: externally-tagged JSON with the
        // variant name as the outer key.
        let cmd = Command::InjectMarketEvent {
            event_json: serde_json::json!({
                "Tick": {
                    "symbol": {"contract_id": 265598, "symbol": "AAPL"},
                    "req_id": 1,
                    "kind": "Price",
                    "tick_type": "Last",
                    "value": {"Price": 184.5},
                    "attrs": {},
                    "ts": "2026-04-18T12:00:00Z"
                }
            }),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""cmd":"inject_market_event""#));
        assert!(json.contains(r#""Tick""#));
        let back: Command = serde_json::from_str(&json).unwrap();
        match back {
            Command::InjectMarketEvent { event_json } => {
                assert!(event_json.get("Tick").is_some());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn set_vp_settings_command_roundtrips() {
        // Mirrors the `VolumeProfileSettings` shape but kept opaque
        // here — the proto crate doesn't depend on `midas-core`. The
        // harness side does the typed deserialise.
        let cmd = Command::SetVpSettings {
            chart_id: 7,
            settings_json: serde_json::json!({
                "anchor": "Daily",
                "width_fraction": 0.7,
                "show_value_area": false,
                "value_area_pct": 0.7,
            }),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""cmd":"set_vp_settings""#));
        assert!(json.contains(r#""chart_id":7"#));
        let back: Command = serde_json::from_str(&json).unwrap();
        match back {
            Command::SetVpSettings {
                chart_id,
                settings_json,
            } => {
                assert_eq!(chart_id, 7);
                assert_eq!(settings_json["anchor"], "Daily");
            }
            _ => panic!("wrong variant"),
        }
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
            (ErrorKind::SimNotRunning, "sim_not_running"),
            (ErrorKind::SimSpawnFailed, "sim_spawn_failed"),
            (ErrorKind::Internal, "internal"),
        ];
        for (kind, expected) in cases {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!(r#""{}""#, expected));
        }
    }
}
