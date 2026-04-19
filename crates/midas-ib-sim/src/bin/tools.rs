//! `midas-ib-sim` — tools CLI (not the daemon).
//!
//! Subcommands: `recordings list/show`, `anonymize`, `calibrate`, `replay`.
//!
//! The long-running TCP sim is `midas-ib-sim-server`. This binary is the
//! batch-mode counterpart — it operates on `.tws.pcap` and `.dbn` artifacts
//! without binding any sockets.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use midas_ib_sim::session::{
    calibrate::calibrate_to_file, AnonymizeConfig, Anonymizer, ReplayMode, Replayer, TwsPcapReader,
};

#[derive(Debug, Parser)]
#[command(
    name = "midas-ib-sim",
    version,
    about = "Batch tools for IB session recordings"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// List / inspect recorded sessions.
    Recordings(RecordingsArgs),
    /// Anonymise a raw `.tws.pcap` before it can be committed to git.
    Anonymize(AnonymizeArgs),
    /// Fit synthetic-model parameters from a captured `.dbn`.
    Calibrate(CalibrateArgs),
    /// Replay a `.tws.pcap` and validate client→sim bytes.
    Replay(ReplayArgs),
}

#[derive(Debug, Parser)]
struct RecordingsArgs {
    #[command(subcommand)]
    cmd: RecordingsCmd,
}

#[derive(Debug, Subcommand)]
enum RecordingsCmd {
    /// List every `.tws.pcap` in a directory with per-file metadata.
    List {
        /// Directory to scan.
        dir: PathBuf,
    },
    /// Print a summary or frame-dump of a single recording.
    Show {
        /// Path to the `.tws.pcap` (raw or zstd).
        pcap: PathBuf,
        /// Print just the header + record count, no per-frame info.
        #[arg(long)]
        summary: bool,
        /// Print the first N frames. Default: no per-frame output.
        #[arg(long)]
        frames: Option<usize>,
    },
}

#[derive(Debug, Parser)]
struct AnonymizeArgs {
    /// Input raw `.tws.pcap`.
    input: PathBuf,
    /// Output anonymised `.tws.pcap`.
    #[arg(long)]
    out: PathBuf,
    /// Path to `anonymize.config.yaml`. Uses the built-in default salt when
    /// omitted — the default is fine for unit tests, not for production.
    #[arg(long)]
    config: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct CalibrateArgs {
    /// Input `.dbn` file.
    input: PathBuf,
    /// Symbol to calibrate (used purely as a label in the emitted preset).
    #[arg(long)]
    symbol: String,
    /// Output preset YAML.
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Parser)]
struct ReplayArgs {
    /// Input `.tws.pcap`.
    input: PathBuf,
    /// Replay mode: `strict`, `best-effort`, `ignore-client`.
    #[arg(long, default_value = "strict")]
    mode: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.cmd {
        Cmd::Recordings(args) => run_recordings(args.cmd),
        Cmd::Anonymize(args) => run_anonymize(args),
        Cmd::Calibrate(args) => run_calibrate(args),
        Cmd::Replay(args) => run_replay(args),
    }
}

fn run_recordings(cmd: RecordingsCmd) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        RecordingsCmd::List { dir } => {
            let mut total = 0usize;
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                if !is_pcap_path(&path) {
                    continue;
                }
                match TwsPcapReader::open(&path) {
                    Ok(reader) => {
                        let hdr = *reader.header();
                        let n = reader.read_all().map(|v| v.len()).unwrap_or(0);
                        println!(
                            "{}\tserver_version={}\tstart_ns={}\tframes={}",
                            path.display(),
                            hdr.server_version_neg,
                            hdr.start_ts_nanos,
                            n
                        );
                    }
                    Err(e) => {
                        eprintln!("  [skip] {}: {}", path.display(), e);
                    }
                }
                total += 1;
            }
            eprintln!("{total} file(s) scanned in {}", dir.display());
        }
        RecordingsCmd::Show {
            pcap,
            summary,
            frames,
        } => {
            let reader = TwsPcapReader::open(&pcap)?;
            let hdr = *reader.header();
            let records = reader.read_all()?;
            println!("file: {}", pcap.display());
            println!(
                "  server_version_neg: {}\n  start_ts_nanos:     {}\n  version:            {}\n  records:            {}",
                hdr.server_version_neg,
                hdr.start_ts_nanos,
                hdr.version,
                records.len()
            );
            if summary {
                return Ok(());
            }
            let n = frames.unwrap_or(records.len().min(50));
            for (i, r) in records.iter().take(n).enumerate() {
                println!(
                    "  #{i:4}  +{:>10} ns  {:?}  len={}",
                    r.ts_nanos_since_start,
                    r.direction,
                    r.payload.len()
                );
            }
        }
    }
    Ok(())
}

fn is_pcap_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.ends_with(".tws.pcap") || s.ends_with(".tws.pcap.zst")
}

fn run_anonymize(args: AnonymizeArgs) -> Result<(), Box<dyn std::error::Error>> {
    let config = if let Some(cfg_path) = args.config {
        AnonymizeConfig::load(cfg_path)?
    } else {
        AnonymizeConfig::default()
    };
    let mut anon = Anonymizer::new(config);
    let n = anon.process_files(&args.input, &args.out)?;
    println!(
        "anonymized {} record(s): {} → {}",
        n,
        args.input.display(),
        args.out.display()
    );
    Ok(())
}

fn run_calibrate(args: CalibrateArgs) -> Result<(), Box<dyn std::error::Error>> {
    let preset = calibrate_to_file(&args.input, &args.symbol, &args.out)?;
    println!(
        "calibrated {} from {} ({} samples) → {}",
        preset.symbol,
        args.input.display(),
        preset.sample_count,
        args.out.display()
    );
    Ok(())
}

fn run_replay(args: ReplayArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mode = match args.mode.as_str() {
        // Strict mode requires a live client and is driven by the sim
        // server, not by this offline tool. Reject it here with a helpful
        // message so users don't hit the auto-confirm no-op below.
        "strict" => {
            return Err(
                "`strict` replay requires a live client — use `midas-ib-sim-server --replay` instead"
                    .into(),
            );
        }
        "best-effort" | "best_effort" | "besteffort" => ReplayMode::BestEffort,
        "ignore-client" | "ignore_client" | "ignoreclient" => ReplayMode::IgnoreClient,
        other => return Err(format!("unknown --mode value: {other}").into()),
    };
    let file = std::fs::File::open(&args.input)?;
    let mut replayer = Replayer::with_reader(file, mode)?;
    let mut server_bytes = 0u64;
    let mut expect_client = 0u64;
    loop {
        use midas_ib_sim::session::replayer::ReplayEmission;
        match replayer.step()? {
            ReplayEmission::ServerBytes { bytes, .. } => {
                server_bytes += bytes.len() as u64;
            }
            ReplayEmission::ExpectClient { expected_len, .. } => {
                expect_client += expected_len as u64;
                // best-effort only cares about presence, ignore-client never
                // reaches this arm — so a zero-filled dummy is safe.
                let dummy = vec![0u8; expected_len];
                replayer.submit_client_bytes(&dummy)?;
            }
            ReplayEmission::Done => break,
        }
    }
    println!(
        "replay complete: {} bytes server→client, {} bytes expected client→sim",
        server_bytes, expect_client
    );
    Ok(())
}
