//! `midas-ib-sim-server` — binary entry point.
//!
//! Parses CLI args, initialises tracing with the span hierarchy documented in
//! `plan/ib-sim/01-architecture.md` §Observability, constructs a `SimConfig`,
//! and boots the sim via `start_sim`. Blocks on ctrl-c and shuts down cleanly.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use midas_ib_sim::{start_sim, ClockMode, MarketDataMode, SimConfig};
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
