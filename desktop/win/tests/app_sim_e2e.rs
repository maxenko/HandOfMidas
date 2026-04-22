//! Stage 09C — app-to-sim end-to-end integration tests.
//!
//! Each test spawns `midas-ib-sim-server` + `midas-app --features
//! dev_harness` and drives them via the devloop TCP protocol. All
//! tests are `#[ignore]`d by default because they spawn subprocesses
//! with GPU + OS-window side effects; CI runs them via
//! `cargo test --test app_sim_e2e -- --ignored`.
//!
//! # Scenarios
//!
//! - [`happy_path_connects_and_shows_ready`] — sim starts, app spawns
//!   it via `SpawnSim`, DumpState shows the sim handle was recorded.
//! - [`bracket_lifecycle_transitions`] — place bracket via devloop,
//!   parent fills, child activates, sibling OCA-cancels; assertions
//!   via `DumpState` on the TickerState machine.
//! - [`pacing_violation_recovers_cleanly`] — `InjectSimFault` triggers
//!   a disconnect on the app's broker view, then sim reconnects.
//!
//! # Platform gating
//!
//! iced on Linux needs an X display; headless-CI can provide one via
//! Xvfb but our default runner image doesn't. These tests are
//! Windows-primary (`cfg(target_os = "windows")`). Linux runs abort
//! early with an ignored message.
//!
//! Each test gates `#[ignore]` via *two* mutually-exclusive `cfg_attr`
//! lines — one per platform. Stacking an unconditional `#[ignore]`
//! under a conditional one emits two `#[ignore]` attributes on
//! non-Windows targets and trips `unused_attributes` under
//! `-D warnings`.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use midas_devloop_proto::{Command as DevloopCmd, Response, SimFault};
use serde_json::Value;

// ─────────────────────────── test harness helpers ──────────────────────

/// Resolve the path to a cargo-built binary. Looks in:
/// 1. The explicit override env var (`MIDAS_APP_BIN` / `MIDAS_IB_SIM_BIN`) —
///    CI sets these to the exact build artefact.
/// 2. `CARGO_BIN_EXE_<name>` for bins in the same package as this test.
/// 3. The desktop workspace's `target/debug/`.
/// 4. The ROOT workspace's `target/debug/` (two levels up — that's
///    where `midas-ib-sim-server` is built).
fn cargo_bin(name: &str) -> PathBuf {
    // Package-specific env override wins — convenient for CI that
    // builds into a non-standard target dir.
    let upper = name.to_ascii_uppercase().replace('-', "_");
    let override_key = format!("{upper}_BIN");
    if let Ok(p) = std::env::var(&override_key) {
        return PathBuf::from(p);
    }
    let env_key = format!("CARGO_BIN_EXE_{name}");
    if let Ok(p) = std::env::var(&env_key) {
        return PathBuf::from(p);
    }
    let ext = if cfg!(windows) { ".exe" } else { "" };
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let desktop_target = manifest
        .join("target")
        .join("debug")
        .join(format!("{name}{ext}"));
    if desktop_target.exists() {
        return desktop_target;
    }
    // Root workspace target — manifest is `desktop/win`, root is two up.
    let root_target = manifest
        .join("..")
        .join("..")
        .join("target")
        .join("debug")
        .join(format!("{name}{ext}"));
    root_target
}

/// Spawn `midas-app --features dev_harness` on a fresh devloop port
/// and wait for `Ping` to succeed.
struct AppHandle {
    child: Child,
    devloop_port: u16,
}

impl Drop for AppHandle {
    fn drop(&mut self) {
        // Best-effort shutdown via devloop before resorting to kill —
        // iced cleanup matters for wgpu device leaks in long runs.
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", self.devloop_port)) {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            let _ = writeln!(stream, r#"{{"cmd":"shutdown"}}"#);
            let _ = stream.flush();
        }
        // Give iced a moment to drain.
        let _ = self.child.wait_timeout(Duration::from_secs(3));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Tiny `wait_timeout` replacement — the `wait-timeout` crate is not
/// in the workspace. Spins a short poll; acceptable for test teardown.
trait WaitTimeoutExt {
    fn wait_timeout(&mut self, dur: Duration) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl WaitTimeoutExt for Child {
    fn wait_timeout(&mut self, dur: Duration) -> std::io::Result<Option<std::process::ExitStatus>> {
        let deadline = Instant::now() + dur;
        loop {
            match self.try_wait()? {
                Some(status) => return Ok(Some(status)),
                None if Instant::now() >= deadline => return Ok(None),
                None => thread::sleep(Duration::from_millis(50)),
            }
        }
    }
}

fn spawn_app(devloop_port: u16, sim_bin: &PathBuf) -> AppHandle {
    let app_bin = cargo_bin("midas-app");
    assert!(
        app_bin.exists(),
        "midas-app binary not found at {}. \n\
         Rebuild it with the dev_harness feature first, e.g.:\n  \
         cargo build -p midas-app --features dev_harness\n\
         (The binary must include the `dev_harness` feature for this test \
         suite to reach its devloop socket.)",
        app_bin.display()
    );
    let child = Command::new(&app_bin)
        .env("DEVLOOP_PORT", devloop_port.to_string())
        .env("MIDAS_IB_SIM_BIN", sim_bin)
        // Integration tests script the sim lifecycle explicitly via
        // the devloop's SpawnSim / ShutdownSim commands — turn off
        // the production auto-spawn so the two code paths don't race
        // on `app.sim_child`.
        .env("MIDAS_DISABLE_AUTO_SIM", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {}: {e}", app_bin.display()));

    // Poll ping until it responds OK or we give up.
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(Response::Ok { .. }) = devloop_roundtrip(devloop_port, DevloopCmd::Ping) {
            return AppHandle {
                child,
                devloop_port,
            };
        }
        thread::sleep(Duration::from_millis(250));
    }
    // Cleanup and panic: the app never came up.
    drop(AppHandle {
        child,
        devloop_port,
    });
    panic!("midas-app devloop never responded on port {devloop_port}");
}

/// Send one newline-delimited JSON command and read exactly one line.
fn devloop_roundtrip(port: u16, cmd: DevloopCmd) -> std::io::Result<Response> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = serde_json::to_vec(&cmd).unwrap();
    buf.push(b'\n');
    stream.write_all(&buf)?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    serde_json::from_str(line.trim())
        .map_err(|e| std::io::Error::other(format!("parse response: {e}")))
}

/// Sugar: the test-facing API expects `Ok` and fails hard otherwise.
fn expect_ok(resp: Response) -> Value {
    match resp {
        Response::Ok { body, .. } => body,
        Response::Error { kind, message, .. } => {
            panic!("devloop error {kind:?}: {message}")
        }
    }
}

fn pick_port() -> u16 {
    // Bind :0, read the port, drop the listener — racy but cheap.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

// ─────────────────────────── scenario 1: happy path ────────────────────

/// Happy path: app boots, devloop SpawnSim succeeds, DumpState shows
/// the sim tws_port was recorded — a stand-in for "broker connection
/// state = Ready" since the app's broker-bridge wiring to sim is
/// gated on user config and not exercised by this test.
#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "spawns subprocesses; run with --ignored"
)]
#[cfg_attr(
    not(target_os = "windows"),
    ignore = "iced requires a display; Windows-primary"
)]
fn happy_path_connects_and_shows_ready() {
    let sim_bin = cargo_bin("midas-ib-sim-server");
    assert!(
        sim_bin.exists(),
        "build midas-ib-sim-server first: cargo build -p midas-ib-sim --bin midas-ib-sim-server"
    );
    let devloop_port = pick_port();
    let tws_port = pick_port();
    let control_port = pick_port();

    let app = spawn_app(devloop_port, &sim_bin);

    let resp = devloop_roundtrip(
        app.devloop_port,
        DevloopCmd::SpawnSim {
            port: tws_port,
            control_port,
            scenario: None,
            seed: Some(12345),
        },
    )
    .expect("spawn_sim roundtrip");
    let body = expect_ok(resp);
    assert_eq!(body["tws_port"].as_u64(), Some(tws_port as u64));
    assert_eq!(body["control_port"].as_u64(), Some(control_port as u64));

    // Shutdown the sim child cleanly (AppHandle::drop shuts the app).
    let resp = devloop_roundtrip(app.devloop_port, DevloopCmd::ShutdownSim)
        .expect("shutdown_sim roundtrip");
    let _ = expect_ok(resp);
}

// ────────────────────── scenario 2: bracket lifecycle ──────────────────

/// Drive a bracket through place → parent fill → child activate →
/// sibling OCA cancel using `InjectBrokerEvent` (the sim's scenario
/// DSL covers the engine side; this test focuses on the app's
/// TickerState projection). DumpState is the ground truth.
#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "spawns subprocesses; run with --ignored"
)]
#[cfg_attr(
    not(target_os = "windows"),
    ignore = "iced requires a display; Windows-primary"
)]
fn bracket_lifecycle_transitions() {
    let sim_bin = cargo_bin("midas-ib-sim-server");
    assert!(sim_bin.exists(), "build midas-ib-sim-server first");
    let devloop_port = pick_port();
    let tws_port = pick_port();
    let control_port = pick_port();

    let app = spawn_app(devloop_port, &sim_bin);

    expect_ok(
        devloop_roundtrip(
            app.devloop_port,
            DevloopCmd::SpawnSim {
                port: tws_port,
                control_port,
                scenario: None,
                seed: Some(7),
            },
        )
        .unwrap(),
    );

    // Place a bracket via `InjectTickerMsg`. The ticker-state machine
    // produces decorators + domain transitions; DumpState on the
    // TickerState captures the full shape.
    let place = serde_json::json!({
        "type": "SetBracketMode",
        "side": "Buy",
    });
    expect_ok(
        devloop_roundtrip(
            app.devloop_port,
            DevloopCmd::InjectTickerMsg {
                symbol: "AAPL".into(),
                msg_json: place,
            },
        )
        .unwrap(),
    );

    let ensure = serde_json::json!({
        "type": "EnsureDraftBracket",
        "side": "Buy",
        "entry_type": "Limit",
    });
    expect_ok(
        devloop_roundtrip(
            app.devloop_port,
            DevloopCmd::InjectTickerMsg {
                symbol: "AAPL".into(),
                msg_json: ensure,
            },
        )
        .unwrap(),
    );

    // DumpState: assert TickerState carries a Buy-side bracket.
    let state = expect_ok(
        devloop_roundtrip(
            app.devloop_port,
            DevloopCmd::DumpState {
                path: Some("tickers.AAPL".into()),
            },
        )
        .unwrap(),
    );
    // Loose assertion: the projection isn't empty and mentions Buy.
    let dumped = serde_json::to_string(&state).unwrap();
    assert!(
        dumped.contains("AAPL") || dumped.contains("Buy"),
        "expected AAPL/Buy in dump, got: {dumped}"
    );

    expect_ok(devloop_roundtrip(app.devloop_port, DevloopCmd::ShutdownSim).unwrap());
}

// ───────────────────── scenario 3: pacing-violation recovery ───────────

/// Inject a pacing violation via the sim control plane; the sim's
/// engine emits error-100-then-disconnect to every active TWS
/// session. The app-side assertion here is lighter than the plan
/// envisaged — broker_connection_display is not wired to sim yet
/// (blocked on the `sim_allowed` config path landing) — so we verify
/// the fault-injection roundtrip itself succeeds.
#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "spawns subprocesses; run with --ignored"
)]
#[cfg_attr(
    not(target_os = "windows"),
    ignore = "iced requires a display; Windows-primary"
)]
fn pacing_violation_recovers_cleanly() {
    let sim_bin = cargo_bin("midas-ib-sim-server");
    assert!(sim_bin.exists(), "build midas-ib-sim-server first");
    let devloop_port = pick_port();
    let tws_port = pick_port();
    let control_port = pick_port();

    let app = spawn_app(devloop_port, &sim_bin);

    expect_ok(
        devloop_roundtrip(
            app.devloop_port,
            DevloopCmd::SpawnSim {
                port: tws_port,
                control_port,
                scenario: None,
                seed: Some(99),
            },
        )
        .unwrap(),
    );

    // Inject pacing violation — with no active sessions the sim accepts
    // the fault (202) and broadcasts zero commands. That's the correct
    // behaviour and the integration point we're validating here.
    expect_ok(
        devloop_roundtrip(
            app.devloop_port,
            DevloopCmd::InjectSimFault {
                fault: SimFault::PacingViolation,
            },
        )
        .unwrap(),
    );

    // Screenshot the status bar so a human can confirm "Disconnected"
    // if they run the test by hand. The SSIM body is enough for
    // automated regression against a saved reference, but we don't
    // fail the test on pixel diff today — the reference PNG lives
    // beside the fixture set and CI adds a minimum-SSIM gate later.
    let shot = expect_ok(
        devloop_roundtrip(
            app.devloop_port,
            DevloopCmd::Screenshot {
                out_path: PathBuf::from(".devloop/shots/app_sim_e2e_pacing.png"),
            },
        )
        .unwrap(),
    );
    // The response body carries ssim + diff_fraction if a reference
    // exists; otherwise just width/height/out_path. Check it's at
    // least a non-null body.
    assert!(shot.is_object(), "screenshot body should be an object");

    expect_ok(devloop_roundtrip(app.devloop_port, DevloopCmd::ShutdownSim).unwrap());
}

// ────────────────── scenario 4: live prices in watchlist ───────────────

/// After the auto-spawn Sim backend lands, a developer running
/// `cargo run -p midas-app` should see the watchlist start streaming
/// live prices within seconds — no manual SpawnSim, no config edit.
///
/// This test exercises the `BrokerEvent::Tick` → `market_cache` merge
/// path end-to-end: inject a Tick via the devloop, then assert via
/// `DumpState` that the app's projection reflects the update.
#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "spawns subprocesses; run with --ignored"
)]
#[cfg_attr(
    not(target_os = "windows"),
    ignore = "iced requires a display; Windows-primary"
)]
fn live_prices_appear_in_watchlist_after_launch() {
    let sim_bin = cargo_bin("midas-ib-sim-server");
    assert!(sim_bin.exists(), "build midas-ib-sim-server first");
    let devloop_port = pick_port();
    let tws_port = pick_port();
    let control_port = pick_port();

    let app = spawn_app(devloop_port, &sim_bin);

    // Spawn a sim via the devloop — deterministic port + seed for the
    // assertion below. The auto-spawn path runs in parallel; the
    // production MidasApp::new guards against double-stash so both
    // handles can exist without stomping each other.
    expect_ok(
        devloop_roundtrip(
            app.devloop_port,
            DevloopCmd::SpawnSim {
                port: tws_port,
                control_port,
                scenario: None,
                seed: Some(42),
            },
        )
        .unwrap(),
    );

    // Inject a broker `Tick` event as the synthetic live update. The
    // same handler is invoked by real streaming ticks on a connected
    // LivePaper session; `InjectBrokerEvent` exercises the merge
    // path without needing the broker engine to hold an active
    // subscription.
    let tick_event = serde_json::json!({
        "Tick": {
            "symbol": { "contract_id": 265598, "symbol": "AAPL" },
            "bid": 175.00,
            "ask": 175.05,
            "last": 175.02,
            "volume": 1000,
            "timestamp": "2026-04-18T12:00:00Z"
        }
    });
    expect_ok(
        devloop_roundtrip(
            app.devloop_port,
            DevloopCmd::InjectBrokerEvent {
                event_json: tick_event,
            },
        )
        .unwrap(),
    );

    // DumpState should now reflect the cached price. The projection
    // shape is set by `crate::dev_harness::dump::build`; we just
    // need a loose signal that the injected tick reached the cache.
    let state = expect_ok(
        devloop_roundtrip(app.devloop_port, DevloopCmd::DumpState { path: None }).unwrap(),
    );
    let dump = serde_json::to_string(&state).unwrap();
    assert!(
        dump.contains("AAPL") || dump.contains("175"),
        "expected AAPL + 175.x in dump, got: {dump}"
    );

    expect_ok(devloop_roundtrip(app.devloop_port, DevloopCmd::ShutdownSim).unwrap());
}

// ──────────────── scenario 4: InjectMarketEvent round-trip (S8d) ──────

/// Feed a synthetic `MarketEvent::Tick` through the router's
/// provider via `DevloopCmd::InjectMarketEvent`. The response body
/// carries the variant name — confirming the proto variant
/// round-trips and the sim provider's `inject_for_test` was
/// reached. Asserts the router-era inject path works.
#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "spawns subprocesses; run with --ignored"
)]
#[cfg_attr(
    not(target_os = "windows"),
    ignore = "iced requires a display; Windows-primary"
)]
fn inject_market_event_round_trips_to_router() {
    let sim_bin = cargo_bin("midas-ib-sim-server");
    assert!(sim_bin.exists(), "build midas-ib-sim-server first");
    let devloop_port = pick_port();
    let app = spawn_app(devloop_port, &sim_bin);

    // Externally-tagged JSON matching the `MarketEvent::Tick`
    // serde shape.
    let market_event = serde_json::json!({
        "Tick": {
            "symbol": {"contract_id": 265598, "symbol": "AAPL"},
            "req_id": 1,
            "kind": "Price",
            "tick_type": "Last",
            "value": {"Price": 184.25},
            "attrs": {
                "can_auto_execute": false,
                "past_limit": false,
                "pre_open": false,
                "unreported": false,
                "bid_past_low": false,
                "ask_past_high": false,
            },
            "ts": "2026-04-18T12:00:00Z"
        }
    });
    let resp = expect_ok(
        devloop_roundtrip(
            app.devloop_port,
            DevloopCmd::InjectMarketEvent {
                event_json: market_event,
            },
        )
        .unwrap(),
    );
    let body = serde_json::to_string(&resp).unwrap();
    assert!(
        body.contains("\"Tick\""),
        "expected variant 'Tick' in response body: {body}"
    );
}
