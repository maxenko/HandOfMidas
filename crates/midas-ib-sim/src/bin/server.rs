//! `midas-ib-sim-server` — binary entry point.
//!
//! Parses CLI args, initialises tracing with the span hierarchy documented in
//! `plan/ib-sim/01-architecture.md` §Observability, constructs a `SimConfig`,
//! and boots the sim via `start_sim`. Blocks on ctrl-c and shuts down cleanly.
//!
//! Stage-07 operating modes:
//!
//! - `--proxy-to HOST:PORT --record STEM` — record live traffic between the
//!   API client and a real IB gateway into `STEM.tws.pcap` (+ `.dbn`).
//! - `--replay SESSION.tws.pcap` — serve the client side of a recorded
//!   session, per `--replay-mode`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use midas_ib_sim::session::{run_proxy, ProxyConfig, Recorder};
use midas_ib_sim::{start_sim, ClockMode, MarketDataMode, SimConfig};
use tokio::sync::Mutex;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

/// CLI surface per `plan/ib-sim/01-architecture.md` §"CLI surface".
#[derive(Debug, Parser)]
#[command(
    name = "midas-ib-sim-server",
    version,
    about = "TWS gateway simulator for Hand of Midas"
)]
struct Cli {
    /// TWS wire-protocol port.
    #[arg(long, default_value_t = 7497)]
    port: u16,

    /// Control-plane HTTP port.
    #[arg(long, default_value_t = 9497)]
    control_port: u16,

    /// Clock mode: `real`, `virtual`, or `accelerated=<multiplier>`.
    #[arg(long, default_value = "real")]
    clock: String,

    /// Market-data mode: `synthetic`, `replay=<path>`, or `hybrid=<path>`.
    #[arg(long, default_value = "synthetic")]
    mode: String,

    /// Scenario YAML to load at startup.
    #[arg(long)]
    scenario: Option<PathBuf>,

    /// Deterministic RNG seed.
    #[arg(long, default_value_t = 12345)]
    seed: u64,

    /// Bind on all interfaces. Requires `--external-bind-acknowledged`.
    #[arg(long)]
    listen_external: bool,

    /// Second flag gating `--listen-external` (same pattern as `allow_live`).
    #[arg(long)]
    external_bind_acknowledged: bool,

    /// Override location of the control-plane token file.
    #[arg(long)]
    token_path: Option<PathBuf>,

    /// Log level (falls back to `RUST_LOG` env var when unset).
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Proxy mode: forward to this upstream IB gateway (e.g. `127.0.0.1:7496`).
    /// When set the server runs as a recording proxy instead of a sim.
    #[arg(long)]
    proxy_to: Option<String>,

    /// Output stem for the `.tws.pcap` + `.dbn` pair written in proxy mode.
    /// Required when `--proxy-to` is set.
    #[arg(long)]
    record: Option<PathBuf>,

    /// Compress the recorded `.tws.pcap` with zstd at rest.
    #[arg(long)]
    record_zstd: bool,

    /// Replay a previously recorded `.tws.pcap`. Mutually exclusive with
    /// `--proxy-to`.
    #[arg(long)]
    replay: Option<PathBuf>,

    /// Client-side validation strictness during replay.
    /// `strict` (default) | `best-effort` | `ignore-client`.
    #[arg(long, default_value = "strict")]
    replay_mode: String,
}

impl Cli {
    fn into_config(self) -> Result<SimConfig, String> {
        let clock_mode = parse_clock(&self.clock)?;
        let market_data = parse_mode(&self.mode)?;
        Ok(SimConfig {
            port: self.port,
            control_port: self.control_port,
            clock_mode,
            market_data,
            scenario_path: self.scenario,
            seed: self.seed,
            listen_external: self.listen_external,
            external_bind_acknowledged: self.external_bind_acknowledged,
            token_path: self.token_path,
        })
    }
}

fn parse_clock(s: &str) -> Result<ClockMode, String> {
    match s {
        "real" => Ok(ClockMode::Real),
        "virtual" => Ok(ClockMode::Virtual),
        rest if rest.starts_with("accelerated=") => {
            let mult: f64 = rest["accelerated=".len()..]
                .parse()
                .map_err(|e| format!("invalid accelerator multiplier: {e}"))?;
            Ok(ClockMode::Accelerated(mult))
        }
        other => Err(format!(
            "unknown --clock value `{other}` (expected real|virtual|accelerated=<n>)"
        )),
    }
}

fn parse_mode(s: &str) -> Result<MarketDataMode, String> {
    match s {
        "synthetic" => Ok(MarketDataMode::Synthetic),
        rest if rest.starts_with("replay=") => Ok(MarketDataMode::Replay {
            path: PathBuf::from(&rest["replay=".len()..]),
        }),
        rest if rest.starts_with("hybrid=") => Ok(MarketDataMode::Hybrid {
            replay: PathBuf::from(&rest["hybrid=".len()..]),
            perturbation: String::new(),
        }),
        other => Err(format!(
            "unknown --mode value `{other}` (expected synthetic|replay=<path>|hybrid=<path>)"
        )),
    }
}

fn init_tracing(default_level: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(&cli.log_level);

    // Stage-07 mode dispatch runs BEFORE we build a `SimConfig` so the
    // proxy/replay paths don't depend on the sim core (which is still
    // scaffolded in Stage 01).
    if let Some(proxy_to) = cli.proxy_to.as_ref() {
        let Some(record_stem) = cli.record.as_ref() else {
            error!("--proxy-to requires --record STEM");
            return ExitCode::from(2);
        };
        return run_proxy_mode(proxy_to, cli.port, record_stem, cli.record_zstd).await;
    }
    if let Some(replay_path) = cli.replay.as_ref() {
        return run_replay_mode(replay_path, &cli.replay_mode).await;
    }

    let config = match cli.into_config() {
        Ok(c) => c,
        Err(e) => {
            error!("invalid CLI args: {e}");
            return ExitCode::from(2);
        }
    };

    info!(
        port = config.port,
        control_port = config.control_port,
        "midas-ib-sim-server starting"
    );

    let sim = match start_sim(config).await {
        Ok(s) => s,
        Err(e) => {
            error!("start_sim failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Block until ctrl-c, then ask the sim to shut down.
    match tokio::signal::ctrl_c().await {
        Ok(()) => info!("ctrl-c received — shutting down"),
        Err(e) => error!("ctrl-c handler failed: {e}"),
    }
    sim.shutdown().await;
    ExitCode::SUCCESS
}

async fn run_proxy_mode(
    proxy_to: &str,
    bind_port: u16,
    record_stem: &PathBuf,
    zstd: bool,
) -> ExitCode {
    let upstream: SocketAddr = match proxy_to.parse() {
        Ok(a) => a,
        Err(e) => {
            error!("invalid --proxy-to address `{proxy_to}`: {e}");
            return ExitCode::from(2);
        }
    };
    let bind: SocketAddr = format!("127.0.0.1:{bind_port}")
        .parse()
        .expect("loopback+u16 is always a valid socket address");
    let recorder = match Recorder::start(record_stem, 0, zstd, Some("IB.LIVE")) {
        Ok(r) => Arc::new(Mutex::new(r)),
        Err(e) => {
            error!("recorder init failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let cfg = ProxyConfig::new(bind, upstream);
    info!(%bind, %upstream, "proxy mode — waiting for client");

    // Race the proxy session against ctrl-c so a SIGINT/SIGTERM delivered
    // mid-session drives us through `Recorder::finalize`, which closes
    // the trailing zstd frame so the captured pcap is decodable. A
    // SIGKILL still leaves the file unfinalised — there's nothing the
    // process can do about that — but SIGTERM is the common case.
    let proxy_fut = run_proxy(cfg, Arc::clone(&recorder));
    let shutdown_fut = tokio::signal::ctrl_c();
    let (exit_code, result) = tokio::select! {
        r = proxy_fut => {
            match &r {
                Ok(stats) => info!(
                    client_to_upstream_bytes = stats.client_to_upstream_bytes,
                    upstream_to_client_bytes = stats.upstream_to_client_bytes,
                    "proxy session ended"
                ),
                Err(e) => error!("proxy failed: {e}"),
            }
            (if r.is_ok() { ExitCode::SUCCESS } else { ExitCode::FAILURE }, Some(r))
        }
        _ = shutdown_fut => {
            info!("ctrl-c received — finalising recorder");
            (ExitCode::SUCCESS, None)
        }
    };

    // Extract the recorder out of the Arc<Mutex<_>> and finalise.
    // `Arc::try_unwrap` fails if copies leak; in that case we drop the
    // Arc and rely on `AutoFinishEncoder`'s drop to run finish() when
    // the last reference goes away. That still completes the zstd
    // frame; the explicit finalize path is preferred because it can
    // surface I/O errors instead of silently dropping them.
    drop(result); // release any leftover Recorder refs held by the Result
    match Arc::try_unwrap(recorder) {
        Ok(mutex) => {
            let rec = mutex.into_inner();
            if let Err(e) = rec.finalize() {
                error!("recorder finalize failed: {e}");
                return ExitCode::FAILURE;
            }
        }
        Err(arc) => {
            tracing::warn!("recorder still shared — relying on Drop to finalise zstd frame");
            drop(arc);
        }
    }
    exit_code
}

async fn run_replay_mode(replay_path: &PathBuf, mode_str: &str) -> ExitCode {
    use midas_ib_sim::session::{ReplayMode, Replayer};

    let mode = match mode_str {
        "strict" => ReplayMode::Strict,
        "best-effort" | "best_effort" | "besteffort" => ReplayMode::BestEffort,
        "ignore-client" | "ignore_client" | "ignoreclient" => ReplayMode::IgnoreClient,
        other => {
            error!("unknown --replay-mode `{other}`");
            return ExitCode::from(2);
        }
    };
    let file = match std::fs::File::open(replay_path) {
        Ok(f) => f,
        Err(e) => {
            error!("opening {}: {}", replay_path.display(), e);
            return ExitCode::FAILURE;
        }
    };
    let replayer = match Replayer::with_reader(file, mode) {
        Ok(r) => r,
        Err(e) => {
            error!("replay init failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    info!(
        path = %replay_path.display(),
        mode = ?mode,
        server_version_neg = replayer.header().server_version_neg,
        "replay mode — Stage-07 standalone replay server is not yet wired to a TCP listener; \
         use the library API or `midas-ib-sim replay` for now"
    );
    ExitCode::SUCCESS
}
