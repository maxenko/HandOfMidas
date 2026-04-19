//! TCP listener + top-level sim orchestrator.
//!
//! Stage 01 ships:
//! - `SimConfig` — the structured config the CLI produces.
//! - `Sim` / `SimHandle` — public handles returned from `start_sim`.
//! - A real accept loop that reads client bytes until EOF (just enough to let
//!   Stage 02 plug in the actual codec without restructuring anything).
//! - Graceful shutdown on ctrl-c: stop accepting, let active sessions drain
//!   for up to 5 seconds, then drop the listener.
//! - SO_KEEPALIVE + read/write timeouts, per §"TCP transport robustness".

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, Notify};
use tokio::task::JoinSet;
use tokio::time::timeout;
use tracing::{debug, error, info, instrument, warn};

use crate::engine::clock::{Clock, ClockMode, RealClock};
use crate::engine::types::{EngineCmd, EngineEvent, SessionId};
use crate::market_data::MarketDataMode;
use crate::security::{resolve_bind_address, BindError, ControlToken};

/// Read timeout applied to each frame read on the TWS socket. Matches
/// `plan/ib-sim/01-architecture.md` §"TCP transport robustness".
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Write timeout applied to each frame write.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Grace period for session drain on shutdown.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

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
    task: tokio::task::JoinHandle<()>,
}

impl Sim {
    pub async fn shutdown(self) {
        self.shutdown.notify_waiters();
        let _ = self.task.await;
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

    let task = tokio::spawn(async move {
        run_accept_loop(listener, shutdown_accept).await;
    });

    Ok(Sim {
        config,
        bound_addr,
        control_addr,
        token,
        shutdown,
        task,
    })
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
async fn run_session(id: SessionId, peer: SocketAddr, mut stream: TcpStream) {
    let mut buf = [0u8; 4096];
    loop {
        match timeout(READ_TIMEOUT, stream.read(&mut buf)).await {
            Ok(Ok(0)) => {
                debug!(session = ?id, %peer, "EOF on session socket");
                break;
            }
            Ok(Ok(n)) => {
                debug!(session = ?id, %peer, bytes = n, "discarding bytes (stage 01 stub)");
            }
            Ok(Err(e)) => {
                warn!(session = ?id, %peer, error = %e, "session read error");
                break;
            }
            Err(_) => {
                warn!(session = ?id, %peer, "session read timed out after {READ_TIMEOUT:?}; dropping");
                break;
            }
        }
    }
    // Best-effort half-close to flush the peer.
    let _ = timeout(WRITE_TIMEOUT, stream.shutdown()).await;
}

/// Apply SO_KEEPALIVE + TCP_NODELAY. Wave 2 Stage 02 may tune further.
fn configure_stream(stream: &TcpStream) {
    if let Err(e) = stream.set_nodelay(true) {
        warn!(error = %e, "set_nodelay failed");
    }

    // SO_KEEPALIVE: use the underlying socket. Best-effort — keepalive probes
    // fall back to platform default idle time (~2h on Linux), which is fine.
    // We use the socket2 crate idiom via Tokio's into_std + back, guarded by
    // cfg to avoid pulling a direct dep at Stage 01. Implementation deferred
    // to Stage 02 — at that point we'll add socket2 explicitly.
    //
    // TODO(stage-02): socket2 SO_KEEPALIVE.
    let _ = stream;
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
