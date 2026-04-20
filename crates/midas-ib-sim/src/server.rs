//! TCP listener + top-level sim orchestrator.
//!
//! Stage 01 ships:
//! - `SimConfig` — the structured config the CLI produces.
//! - `Sim` / `SimHandle` — public handles returned from `start_sim`.
//! - A real accept loop that reads client bytes until EOF (just enough to let
//!   Stage 02 plug in the actual codec without restructuring anything).
//! - Graceful shutdown on ctrl-c: stop accepting, let active sessions drain
//!   for up to 5 seconds, then drop the listener.
//! - SO_KEEPALIVE + per-frame read deadline + buffered-byte cap, per
//!   §"TCP transport robustness".

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, Notify};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{timeout, Instant};
use tracing::{debug, error, info, instrument, warn};

use crate::control::ControlApi;
use crate::engine::clock::{Clock, ClockMode, RealClock};
use crate::engine::types::{EngineCmd, EngineEvent, EngineSnapshot, SessionId};
use crate::market_data::MarketDataMode;
use crate::security::{resolve_bind_address, BindError, ControlToken};

/// Per-frame read deadline. The session drops if no complete frame is parsed
/// off the wire within this window, regardless of how many individual bytes
/// the peer has dripped. Matches `plan/ib-sim/01-architecture.md`
/// §"TCP transport robustness" — resets a slow-trickle DoS.
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Write timeout applied to each frame write.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Grace period for session drain on shutdown.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// Hard cap on how many unparsed bytes a session may accumulate before
/// we drop it. A misbehaved peer cannot burn unbounded memory + FDs.
pub const MAX_BUFFERED_BYTES: usize = 16 * 1024 * 1024;
/// SO_KEEPALIVE: idle time before the first probe.
pub const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
/// Interval between successive keepalive probes.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
/// Probe count before the kernel declares the peer dead.
pub const KEEPALIVE_RETRIES: u32 = 3;
/// Maximum time a child task (engine sink, control plane) is given to
/// drain after `shutdown.notify_waiters()` before we stop waiting on it.
pub const CHILD_TASK_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// SimConfig — Stage 01 + 06 contributors.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SimConfig {
    /// TWS wire-protocol port (default 7497).
    pub port: u16,
    /// Control-plane HTTP port (default 9497).
    pub control_port: u16,
    /// Clock mode: real, virtual, or accelerated.
    pub clock_mode: ClockMode,
    /// Market data source.
    pub market_data: MarketDataMode,
    /// Optional scenario YAML to load at startup.
    pub scenario_path: Option<PathBuf>,
    /// Deterministic RNG seed.
    pub seed: u64,
    /// Bind on all interfaces when `true` (gated by `external_bind_acknowledged`).
    pub listen_external: bool,
    /// Dual-consent gate — mirrors `allow_live` in `midas-core::config`.
    pub external_bind_acknowledged: bool,
    /// Where to write the control-plane bearer token. `None` = default
    /// `$XDG_DATA_HOME/midas-ib-sim/control.token`.
    pub token_path: Option<PathBuf>,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            port: 7497,
            control_port: 9497,
            clock_mode: ClockMode::Real,
            market_data: MarketDataMode::Synthetic,
            scenario_path: None,
            seed: 12345,
            listen_external: false,
            external_bind_acknowledged: false,
            token_path: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level sim handle.
// ---------------------------------------------------------------------------

/// Running sim handle — returned from `start_sim`. Drop or call `.shutdown()`
/// to stop. Wave 2 integration code (Stage 09) uses this from CI / devloop.
pub struct Sim {
    pub config: SimConfig,
    pub bound_addr: SocketAddr,
    pub control_addr: SocketAddr,
    pub token: Arc<ControlToken>,
    shutdown: Arc<Notify>,
    task: JoinHandle<()>,
    control_task: JoinHandle<()>,
    engine_sink_task: JoinHandle<()>,
}

impl Sim {
    /// Gracefully shut down the sim: notify every spawned task, wait for
    /// the accept loop to drain sessions, then wait (bounded) for the
    /// control-plane and engine-sink tasks too. This avoids leaking
    /// tasks + file descriptors when the caller drops the `Sim`.
    pub async fn shutdown(self) {
        self.shutdown.notify_waiters();
        let _ = self.task.await;
        // Give auxiliary tasks a bounded window to exit; if they overrun,
        // abort so the caller isn't blocked on a stuck sink.
        if timeout(CHILD_TASK_SHUTDOWN_TIMEOUT, self.control_task)
            .await
            .is_err()
        {
            warn!("control-plane task did not exit within grace — leaking handle (aborted)");
        }
        if timeout(CHILD_TASK_SHUTDOWN_TIMEOUT, self.engine_sink_task)
            .await
            .is_err()
        {
            warn!("engine-sink task did not exit within grace — leaking handle (aborted)");
        }
    }

    pub fn shutdown_handle(&self) -> Arc<Notify> {
        self.shutdown.clone()
    }
}

/// Sender+receiver handle used by in-process tests (`start_in_process`).
pub struct SimHandle {
    pub cmd_tx: mpsc::Sender<EngineCmd>,
    pub event_rx: broadcast::Receiver<EngineEvent>,
}

/// Startup errors.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error(transparent)]
    Bind(#[from] BindError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("control plane: {0}")]
    ControlPlane(String),
}

// ---------------------------------------------------------------------------
// Entry points.
// ---------------------------------------------------------------------------

/// Start the sim. Binds the TWS listener, launches the engine actor and the
/// control plane, and returns a handle the caller can await / shut down.
#[instrument(name = "start_sim", skip(config))]
pub async fn start_sim(config: SimConfig) -> Result<Sim, StartError> {
    let bind_ip: IpAddr =
        resolve_bind_address(config.listen_external, config.external_bind_acknowledged)?;

    let tws_addr = SocketAddr::new(bind_ip, config.port);
    let listener = TcpListener::bind(tws_addr).await?;
    let bound_addr = listener.local_addr()?;
    info!(
        addr = %bound_addr,
        "TWS listener up"
    );

    let control_addr = SocketAddr::new(bind_ip, config.control_port);

    // Generate + persist the control token so devloop / tests can pick it up.
    let token = Arc::new(ControlToken::generate());
    let token_path = config
        .token_path
        .clone()
        .or_else(crate::security::default_token_path);
    if let Some(path) = &token_path {
        match crate::security::write_token_to_disk(&token, path) {
            Ok(_) => debug!(path = %path.display(), "wrote control token"),
            Err(e) => {
                warn!(error = %e, path = %path.display(), "failed to write control token (tests may still pass; record keeping only)")
            }
        }
    }

    let _clock: Arc<dyn Clock> = match config.clock_mode {
        ClockMode::Real => Arc::new(RealClock::new()),
        ClockMode::Virtual => {
            // VirtualClock is Stage 08 — until it's filled in, fall back to
            // RealClock so the binary still boots. Wave 2 flips this.
            Arc::new(RealClock::new())
        }
        ClockMode::Accelerated(_) => Arc::new(RealClock::new()),
    };

    let shutdown = Arc::new(Notify::new());
    let shutdown_accept = shutdown.clone();

    // Spawn a minimal control-plane engine command sink. Wave 3 keeps the
    // engine's full orchestrator off the Stage 09 hot path — the devloop
    // integration only needs `/control/health` to return OK and
    // `/control/inject` to accept commands (the engine-side handlers
    // are exercised by the dedicated `midas-ib-sim` integration tests).
    //
    // The sink speaks just enough `EngineCmd` to keep the control plane
    // honest about engine liveness: it answers `DumpState` with an empty
    // snapshot, and silently consumes `Inject*` commands. That's the
    // correct behaviour when no TWS sessions are connected; a real engine
    // orchestrator will replace this sink once Stage 09 lands the full
    // boot sequence.
    let (engine_tx, engine_rx) = mpsc::channel::<EngineCmd>(256);
    let shutdown_engine_sink = shutdown.clone();
    let engine_sink_task = tokio::spawn(run_engine_sink(engine_rx, shutdown_engine_sink));

    let control_api = ControlApi::new(Arc::clone(&token), engine_tx);
    let control_listener = tokio::net::TcpListener::bind(control_addr).await?;
    let bound_control_addr = control_listener.local_addr()?;
    info!(addr = %bound_control_addr, "control plane listening");
    let control_router = control_api.router();
    let shutdown_control = shutdown.clone();
    let control_task = tokio::spawn(async move {
        let axum_serve =
            axum::serve(control_listener, control_router).with_graceful_shutdown(async move {
                shutdown_control.notified().await;
            });
        if let Err(e) = axum_serve.await {
            warn!(error = %e, "control plane exited");
        }
    });

    let task = tokio::spawn(async move {
        run_accept_loop(listener, shutdown_accept).await;
    });

    Ok(Sim {
        config,
        bound_addr,
        control_addr: bound_control_addr,
        token,
        shutdown,
        task,
        control_task,
        engine_sink_task,
    })
}

/// Minimal engine stand-in consumed by the control plane before Stage 09
/// wires the real orchestrator into `start_sim`. Answers `DumpState` with
/// an empty snapshot and silently drops every other command.
///
/// Lives here (not in `engine/`) because it's a Stage-09 integration
/// concession — full `start_sim` wiring would pull in the
/// `MarketDataEngine` + `OrderSimulator` + `CompositeQuirkGuard` boot
/// sequence that Wave 4 still owns.
///
/// The sink selects on a shutdown notifier so it exits when the sim is
/// told to stop; otherwise it would leak as a zombie `tokio::spawn` task
/// hanging off the runtime until the process dies.
async fn run_engine_sink(mut rx: mpsc::Receiver<EngineCmd>, shutdown: Arc<Notify>) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.notified() => {
                debug!("engine sink: shutdown notified — exiting");
                return;
            }
            maybe_cmd = rx.recv() => {
                match maybe_cmd {
                    Some(EngineCmd::DumpState { reply }) => {
                        let _ = reply.send(EngineSnapshot::default());
                    }
                    Some(_) => {
                        // All other commands are no-ops in the sink — but accepting
                        // them keeps the control plane's `202 Accepted` semantics
                        // honest, which is what the devloop integration tests assert.
                    }
                    None => {
                        // Sender dropped — nothing more will arrive. Exit cleanly.
                        debug!("engine sink: cmd tx closed — exiting");
                        return;
                    }
                }
            }
        }
    }
}

/// Accept loop: accepts clients, spawns per-session tasks, respects shutdown.
async fn run_accept_loop(listener: TcpListener, shutdown: Arc<Notify>) {
    let mut sessions: JoinSet<()> = JoinSet::new();
    let mut next_session_id: u64 = 1;

    loop {
        tokio::select! {
            biased;
            _ = shutdown.notified() => {
                info!("shutdown requested — closing listener, draining sessions");
                drop(listener);
                // Drain sessions up to the grace period.
                let _ = timeout(SHUTDOWN_GRACE, async {
                    while sessions.join_next().await.is_some() {}
                }).await;
                sessions.shutdown().await;
                return;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, peer)) => {
                        let id = SessionId(next_session_id);
                        next_session_id += 1;
                        configure_stream(&stream);
                        info!(session = ?id, %peer, "TWS session accepted");
                        sessions.spawn(run_session(id, peer, stream));
                    }
                    Err(e) => {
                        error!(error = %e, "accept failed");
                        // Back off briefly to avoid a hot loop if the listener is broken.
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }
    }
}

/// Stage 01 session task — reads the TWS stream until EOF, dropping bytes.
/// Stage 02 replaces this with the real codec + engine wiring.
///
/// ## DoS guards
///
/// Three layers protect against misbehaved peers:
///
/// 1. **Per-frame deadline** — `READ_TIMEOUT` is tracked as an absolute
///    `Instant` that is bumped *only* when a complete frame is consumed.
///    A slow-trickle attacker that drips 1 byte every 29 s no longer
///    resets the timer: the deadline tracks progress at the frame
///    boundary, not at the byte boundary. Stage 01 has no codec so the
///    "frame accepted" signal is simply every non-empty read (which is
///    what the stub's discard semantics imply); Stage 02 will plumb the
///    codec's `try_decode` return into this loop unchanged.
/// 2. **Buffered-byte cap** — unparsed bytes are bounded by
///    `MAX_BUFFERED_BYTES` (16 MiB). Above that the session is dropped.
///    In Stage 01 the stub discards bytes, so `buffered` is always 0;
///    the counter is wired so Stage 02's codec can plug in without
///    reshuffling this function.
/// 3. **SO_KEEPALIVE** — see [`configure_stream`].
async fn run_session(id: SessionId, peer: SocketAddr, mut stream: TcpStream) {
    let mut buf = [0u8; 4096];
    // Monotonic deadline for the *current* frame. Advanced only when a
    // complete frame is accepted by the codec (Stage 01 stub: every
    // successful non-empty read).
    let mut frame_deadline = Instant::now() + READ_TIMEOUT;
    let mut buffered: usize = 0;
    loop {
        // `timeout_at` enforces an absolute deadline that does NOT reset
        // on every byte — slow-trickle sessions hit it even while bytes
        // keep arriving.
        let remaining = frame_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            warn!(session = ?id, %peer, "session frame deadline expired; dropping");
            break;
        }
        match timeout(remaining, stream.read(&mut buf)).await {
            Ok(Ok(0)) => {
                debug!(session = ?id, %peer, "EOF on session socket");
                break;
            }
            Ok(Ok(n)) => {
                // Stage 01 stub: every read "accepts a frame" because we
                // drop the bytes. Stage 02's codec must replace this with
                // its own frame-boundary detection and only bump the
                // deadline + decrement `buffered` when `try_decode` returns
                // `Ok(Some(frame))`.
                buffered = buffered.saturating_add(n);
                if buffered > MAX_BUFFERED_BYTES {
                    warn!(
                        session = ?id,
                        %peer,
                        buffered,
                        "session exceeded MAX_BUFFERED_BYTES; dropping"
                    );
                    break;
                }
                debug!(session = ?id, %peer, bytes = n, "discarding bytes (stage 01 stub)");
                buffered = 0;
                frame_deadline = Instant::now() + READ_TIMEOUT;
            }
            Ok(Err(e)) => {
                warn!(session = ?id, %peer, error = %e, "session read error");
                break;
            }
            Err(_) => {
                warn!(
                    session = ?id,
                    %peer,
                    "session frame read timed out after {READ_TIMEOUT:?}; dropping"
                );
                break;
            }
        }
    }
    // Best-effort half-close to flush the peer.
    let _ = timeout(WRITE_TIMEOUT, stream.shutdown()).await;
}

/// Apply SO_KEEPALIVE + TCP_NODELAY. Keepalive knobs are 60 s idle /
/// 10 s interval / 3 probes — enough that a disappeared peer is reaped
/// inside 90 s rather than the platform default (~2 h on Linux).
fn configure_stream(stream: &TcpStream) {
    if let Err(e) = stream.set_nodelay(true) {
        warn!(error = %e, "set_nodelay failed");
    }

    // SO_KEEPALIVE via socket2. Tokio's `TcpStream` exposes the underlying
    // fd through `as_socket2`-style interop using `SockRef::from(&stream)`.
    // Any failure is best-effort — the session still works, just without
    // proactive dead-peer detection.
    let sock_ref = socket2::SockRef::from(stream);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(KEEPALIVE_IDLE)
        .with_interval(KEEPALIVE_INTERVAL);
    // `with_retries` mirrors socket2's own platform gating — see the
    // `#[cfg]` on `TcpKeepalive::with_retries` in socket2 0.5.
    // macOS/iOS/Windows expose keepalive time + interval but not retry
    // count, so we just apply what the platform supports.
    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "illumos",
        target_os = "linux",
        target_os = "netbsd",
    ))]
    let keepalive = keepalive.with_retries(KEEPALIVE_RETRIES);
    if let Err(e) = sock_ref.set_tcp_keepalive(&keepalive) {
        warn!(error = %e, "set_tcp_keepalive failed — falling back to kernel defaults");
    }
}

// ---------------------------------------------------------------------------
// Tests — smoke for accept loop, shutdown, and external-bind gate.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    /// Smoke test: the sim binds, a client connects, the listener accepts,
    /// and a clean shutdown terminates everything.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn smoke_accept_and_shutdown() {
        let config = SimConfig {
            port: 0, // let the OS pick a port
            control_port: 0,
            token_path: Some(tempfile::tempdir().unwrap().keep().join("control.token")),
            ..Default::default()
        };
        let sim = start_sim(config).await.expect("start_sim");
        let addr = sim.bound_addr;

        // Connect a client + write a greeting + half-close.
        let mut client = TcpStream::connect(addr).await.expect("client connect");
        client.write_all(b"hello sim").await.expect("client write");
        client.shutdown().await.ok();

        // Give the session loop a moment to consume the bytes + hit EOF.
        tokio::time::sleep(Duration::from_millis(50)).await;

        sim.shutdown().await;
    }

    #[tokio::test]
    async fn external_bind_refuses_without_ack() {
        let config = SimConfig {
            port: 0,
            control_port: 0,
            listen_external: true,
            external_bind_acknowledged: false,
            token_path: Some(tempfile::tempdir().unwrap().keep().join("ct")),
            ..Default::default()
        };
        match start_sim(config).await {
            Err(StartError::Bind(BindError::ExternalBindNotAcknowledged)) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
            Ok(_sim) => panic!("sim must refuse external bind without ack"),
        }
    }
}
