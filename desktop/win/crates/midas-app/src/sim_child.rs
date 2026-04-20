//! IB-simulator child-process lifecycle.
//!
//! Owns the spawn-and-reap lifecycle for `midas-ib-sim-server`. Used by
//! two call-sites:
//!
//! 1. **Production startup** — `MidasApp::new` auto-spawns a sim when
//!    `config.broker.backend == Sim` (the default for fresh installs)
//!    so developers get a working broker connection out of the box
//!    with zero config.
//! 2. **Dev harness** — the feature-gated devloop re-exports this
//!    module so the `SpawnSim` / `ShutdownSim` / `InjectSimFault`
//!    commands drive the same code path as production.
//!
//! Responsibilities:
//! - Spawn the sim binary with CLI flags, then wait for its
//!   `/control/health` endpoint to respond OK.
//! - Cache the bearer token the sim wrote to disk so fault-injection
//!   POSTs can authenticate.
//! - SIGTERM → grace → SIGKILL teardown on [`SimChildHandle::shutdown`],
//!   and best-effort reaping via `kill_on_drop` if the parent process
//!   itself is dropped.
//!
//! HTTP is intentionally hand-rolled over `tokio::net::TcpStream`:
//! the control plane is local loopback HTTP/1.1 with small JSON
//! bodies, so a ~30-LOC client costs far less than pulling `reqwest`
//! into the base dependency set.

use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Command as TokioCommand;
use tokio::time::{sleep, timeout, Instant};

#[cfg(feature = "dev_harness")]
use midas_devloop_proto::SimFault;

/// Handle to a running simulator child process.
///
/// Cloned (cheaply) across callers so shutdown / fault-injection can
/// reach the active sim without threading state through every command.
#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "dev_harness"), allow(dead_code))]
pub struct SimChildHandle {
    /// TWS wire-protocol port (e.g. 7498).
    pub tws_port: u16,
    /// Control-plane HTTP port (e.g. 9498).
    pub control_port: u16,
    /// Bearer token read from the sim's token file.
    ///
    /// Read only by the dev-harness fault-injection path; the
    /// production auto-spawn never needs to POST to the control
    /// plane. Kept on the handle so devloop tests that bind to an
    /// auto-spawned sim can still inject faults.
    pub token: String,
    /// Path of the `.pid` file written for supervisor reaping.
    pub pid_path: PathBuf,
    /// Shared child handle. `Arc<Mutex<_>>` because the child is
    /// consumed at shutdown time and the handle is cloned across
    /// command dispatch. `kill_on_drop` makes the Drop impl a best-
    /// effort reaper even if callers forget to call `shutdown`.
    child: Arc<Mutex<Option<tokio::process::Child>>>,
}

/// Errors surfaced from spawn / health-check / shutdown.
#[derive(Debug, thiserror::Error)]
#[cfg_attr(not(feature = "dev_harness"), allow(dead_code))]
pub enum SimChildError {
    #[error("could not locate midas-ib-sim-server binary: {0}")]
    BinaryNotFound(String),
    #[error("spawning sim binary failed: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("sim health check timed out after {0:?}")]
    HealthTimeout(Duration),
    #[error("sim health check failed: {0}")]
    HealthFailed(String),
    #[error("no free port available in range {start}..{end}")]
    NoFreePort { start: u16, end: u16 },
    #[error("reading sim token file {path}: {err}")]
    TokenRead {
        path: PathBuf,
        #[source]
        err: std::io::Error,
    },
    #[error("writing sim pid file {path}: {err}")]
    PidWrite {
        path: PathBuf,
        #[source]
        err: std::io::Error,
    },
    #[error("HTTP call to /control/inject failed: {0}")]
    HttpFailed(String),
    #[error("/control/inject returned {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("shutdown failed: {0}")]
    Shutdown(String),
}

/// How long we wait for `/control/health` to respond after spawn. The
/// sim's cold-start on a dev machine is typically under 250ms; 10s
/// is well above the noise floor without being so generous that a
/// truly broken binary stalls the dev loop forever.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);
/// Poll interval between successive `/control/health` attempts.
const HEALTH_POLL: Duration = Duration::from_millis(50);
/// Grace period between SIGTERM and SIGKILL on shutdown.
#[cfg(unix)]
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Resolve the path to `midas-ib-sim-server`. Priority:
///
/// 1. `MIDAS_IB_SIM_BIN` env var — explicit override (CI, tests).
/// 2. Alongside the running `midas-app` binary (release/debug installs).
/// 3. Desktop workspace's `target/debug/` next to the app crate —
///    `cargo run -p midas-app` from `desktop/win/` leaves the app
///    there but the sim lives in the root workspace.
/// 4. Root workspace's `target/{release,debug}/` — the sim is built
///    under the repo root at `HandOfMidas/target/<profile>/`.
/// 5. `PATH` lookup as a last resort.
pub fn resolve_sim_binary() -> Result<PathBuf, SimChildError> {
    if let Ok(p) = std::env::var("MIDAS_IB_SIM_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
        return Err(SimChildError::BinaryNotFound(format!(
            "MIDAS_IB_SIM_BIN={} does not exist",
            p.display()
        )));
    }
    let bin_name = if cfg!(windows) {
        "midas-ib-sim-server.exe"
    } else {
        "midas-ib-sim-server"
    };

    // (2) Alongside the running app binary.
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let sibling = dir.join(bin_name);
            if sibling.exists() {
                return Ok(sibling);
            }
        }
    }

    // (3 + 4) Cargo workspace layout. `CARGO_MANIFEST_DIR` at compile
    // time is `.../desktop/win/crates/midas-app`; the desktop
    // workspace target is `.../desktop/win/target`, and the root
    // workspace target is `.../target`.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest.join("../../target/release").join(bin_name),
        manifest.join("../../target/debug").join(bin_name),
        manifest.join("../../../../target/release").join(bin_name),
        manifest.join("../../../../target/debug").join(bin_name),
    ];
    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    // (5) PATH — fall through to the name and let the OS resolve.
    // We can't verify existence without searching PATH ourselves, so
    // we trust the OS loader and surface spawn failure downstream
    // with a clearer error.
    Err(SimChildError::BinaryNotFound(format!(
        "tried MIDAS_IB_SIM_BIN, current_exe sibling, and workspace \
         target dirs; none contained {bin_name}. Build it first: \
         `cargo build -p midas-ib-sim --bin midas-ib-sim-server` from \
         the repo root."
    )))
}

/// Spawn options, mirrors the fields of `Command::SpawnSim`.
#[derive(Debug, Clone)]
pub struct SpawnOptions {
    pub tws_port: u16,
    pub control_port: u16,
    pub scenario: Option<String>,
    pub seed: Option<u64>,
}

/// Resolve the directory used for sim-child runtime artifacts (PID
/// files, token files, etc.).
///
/// Priority:
/// 1. `MIDAS_DEVLOOP_DIR` env var — explicit override for CI.
/// 2. `dirs::data_local_dir()/midas-app/.devloop/` — stable per-user
///    location independent of the process's current working directory,
///    so two `midas-app` instances launched from different cwds don't
///    stomp each other's pid/token files.
/// 3. Cwd-relative `.devloop/` — last-resort fallback for environments
///    without XDG/LocalAppData.
///
/// The returned directory is created on demand.
pub fn devloop_runtime_dir() -> PathBuf {
    let dir = if let Ok(p) = std::env::var("MIDAS_DEVLOOP_DIR") {
        PathBuf::from(p)
    } else if let Some(base) = dirs::data_local_dir() {
        base.join("midas-app").join(".devloop")
    } else {
        PathBuf::from(".devloop")
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(
            "sim_child: could not create runtime dir {}: {}",
            dir.display(),
            e
        );
    }
    dir
}

/// Write `contents` to `path` atomically: write to `path.tmp`, fsync,
/// then rename over `path`. A concurrent reader never observes a
/// truncated-then-partial file.
fn atomic_write(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut tmp = path.to_path_buf();
    let mut tmp_name = match path.file_name() {
        Some(f) => f.to_owned(),
        None => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "atomic_write: path has no file name",
            ));
        }
    };
    tmp_name.push(".tmp");
    tmp.set_file_name(tmp_name);
    std::fs::create_dir_all(parent).ok();
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Try to bind `port` on 127.0.0.1. Returns `true` iff the port is free.
fn port_is_free(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

/// Allocate a free TWS port for the sim, preferring `preferred`. If
/// the preferred port is taken, scans `7498..7600` for the first free
/// slot. Returns the chosen port.
///
/// There is an inherent TOCTOU race between this check and the sim's
/// own bind: another process could claim the port in between. The
/// sim will fail fast with a clear error if that happens; picking any
/// port in a wide range makes the collision window vanishingly small
/// for a single-user dev machine.
pub fn allocate_sim_port(preferred: u16) -> Result<u16, SimChildError> {
    if preferred != 0 && port_is_free(preferred) {
        return Ok(preferred);
    }
    const START: u16 = 7498;
    const END: u16 = 7600;
    for p in START..END {
        if port_is_free(p) {
            return Ok(p);
        }
    }
    Err(SimChildError::NoFreePort {
        start: START,
        end: END,
    })
}

/// Spawn the sim binary, wait for `/control/health`, write the pid
/// file, and return a handle the caller can keep alive.
pub async fn spawn(opts: SpawnOptions) -> Result<SimChildHandle, SimChildError> {
    let binary = resolve_sim_binary()?;

    // Put the token file in the resolved runtime dir so two apps
    // launched from different cwds don't stomp each other's tokens.
    // Per-port keeps parallel sims from colliding on the same user
    // account.
    let runtime_dir = devloop_runtime_dir();
    let token_path = runtime_dir.join(format!("sim.{}.token", opts.tws_port));
    // Best-effort cleanup: the sim refuses to overwrite an existing
    // token file with `create_new`, so stale tokens break re-spawn.
    let _ = std::fs::remove_file(&token_path);

    let mut cmd = TokioCommand::new(&binary);
    cmd.arg("--port")
        .arg(opts.tws_port.to_string())
        .arg("--control-port")
        .arg(opts.control_port.to_string())
        .arg("--token-path")
        .arg(&token_path);
    if let Some(path) = opts.scenario.as_deref() {
        cmd.arg("--scenario").arg(path);
    }
    if let Some(seed) = opts.seed {
        cmd.arg("--seed").arg(seed.to_string());
    }
    // Inherit stdio in dev to surface sim tracing to the same terminal;
    // callers that want silence can redirect via the parent.
    cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    cmd.kill_on_drop(true);

    tracing::info!(
        "sim_child: spawning {} on tws_port={} control_port={}",
        binary.display(),
        opts.tws_port,
        opts.control_port
    );
    let mut child = cmd.spawn().map_err(SimChildError::Spawn)?;

    // Block until /control/health responds OK, or the child exits
    // early (binary broken, port in use, etc).
    let started = Instant::now();
    loop {
        if started.elapsed() >= HEALTH_TIMEOUT {
            let _ = child.start_kill();
            return Err(SimChildError::HealthTimeout(HEALTH_TIMEOUT));
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(SimChildError::HealthFailed(format!(
                "sim exited during health-check with status {status:?}"
            )));
        }
        match http_get_health(opts.control_port).await {
            Ok(()) => break,
            Err(_) => {
                sleep(HEALTH_POLL).await;
            }
        }
    }

    // Read the bearer token the sim just wrote.
    let token = std::fs::read_to_string(&token_path).map_err(|err| SimChildError::TokenRead {
        path: token_path.clone(),
        err,
    })?;
    let token = token.trim().to_owned();

    // Record pid for supervisor reaping. Atomic-write so a reaper
    // that opens the file between `File::create(truncate)` and
    // `write_all` can't observe an empty/partial file.
    let pid = child.id().unwrap_or(0);
    let pid_path = runtime_dir.join(format!("sim.{}.pid", opts.tws_port));
    atomic_write(&pid_path, pid.to_string().as_bytes()).map_err(|err| SimChildError::PidWrite {
        path: pid_path.clone(),
        err,
    })?;

    Ok(SimChildHandle {
        tws_port: opts.tws_port,
        control_port: opts.control_port,
        token,
        pid_path,
        child: Arc::new(Mutex::new(Some(child))),
    })
}

impl SimChildHandle {
    /// Best-effort graceful shutdown: SIGTERM (Unix) or `kill`
    /// (Windows — `tokio::process::Child::kill` maps to
    /// `TerminateProcess`), wait up to [`SHUTDOWN_GRACE`], then force.
    ///
    /// The production auto-spawn path never calls this explicitly —
    /// `kill_on_drop(true)` runs the equivalent reaper from the
    /// Child's Drop impl when `SimChildHandle` goes out of scope. It
    /// stays on the handle for the dev-harness `ShutdownSim` command
    /// which needs a deterministic await point.
    #[cfg_attr(not(feature = "dev_harness"), allow(dead_code))]
    pub async fn shutdown(&self) -> Result<(), SimChildError> {
        let mut child = match self.child.lock().take() {
            Some(c) => c,
            None => return Ok(()),
        };

        // On Unix send SIGTERM so the sim's tokio::signal::ctrl_c path
        // runs a clean shutdown (flushes recordings, notifies
        // subscribers). Windows has no SIGTERM; `kill()` delivers the
        // closest equivalent (`TerminateProcess`) which is synchronous
        // — no grace period on that path.
        #[cfg(unix)]
        {
            if let Some(pid) = child.id() {
                // Call libc::kill directly rather than shelling out to
                // /usr/bin/kill: minimal containers may lack the
                // binary, forking a process per teardown inherits the
                // parent env unnecessarily, and a direct syscall is
                // one fewer moving part at shutdown.
                //
                // SAFETY: `pid` comes from `tokio::process::Child::id`
                // which returns the child we just spawned; `libc::kill`
                // is thread-safe and has no undefined-behaviour hazards
                // for the `SIGTERM` signal.
                let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
                if rc != 0 {
                    let err = std::io::Error::last_os_error();
                    // ESRCH (no such process) is expected when the
                    // child already exited — don't surface as an
                    // error.
                    if err.raw_os_error() != Some(libc::ESRCH) {
                        tracing::warn!("sim_child: libc::kill(SIGTERM) on pid {pid} failed: {err}");
                    }
                }
            }
            match timeout(SHUTDOWN_GRACE, child.wait()).await {
                Ok(Ok(_)) => {}
                _ => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
            }
        }
        #[cfg(not(unix))]
        {
            // Windows: TerminateProcess equivalent, no grace period.
            if let Err(e) = child.kill().await {
                return Err(SimChildError::Shutdown(e.to_string()));
            }
        }

        let _ = std::fs::remove_file(&self.pid_path);
        Ok(())
    }

    /// Fire a fault-injection POST to `/control/inject`. Only compiled
    /// when `dev_harness` is enabled — production callers never need
    /// it and pulling in `SimFault` would leak the proto crate into
    /// the base build.
    #[cfg(feature = "dev_harness")]
    pub async fn inject_fault(&self, fault: &SimFault) -> Result<(), SimChildError> {
        let body = serde_json::to_vec(fault)
            .map_err(|e| SimChildError::HttpFailed(format!("fault json: {e}")))?;
        http_post_inject(self.control_port, &self.token, &body).await
    }
}

// ────────────────────────────── HTTP plumbing ──────────────────────────

/// Minimal GET /control/health — 200 OK returns `Ok(())`, anything else
/// returns an error so the spawn loop re-polls.
async fn http_get_health(port: u16) -> Result<(), SimChildError> {
    let mut stream = timeout(
        Duration::from_millis(200),
        TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
    )
    .await
    .map_err(|_| SimChildError::HttpFailed("connect timeout".into()))?
    .map_err(|e| SimChildError::HttpFailed(format!("connect: {e}")))?;

    let req = format!(
        "GET /control/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| SimChildError::HttpFailed(format!("write: {e}")))?;
    stream.shutdown().await.ok(); // best-effort half-close

    let mut resp = Vec::with_capacity(128);
    let read_res = timeout(Duration::from_millis(500), async {
        stream.read_to_end(&mut resp).await
    })
    .await;
    match read_res {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(SimChildError::HttpFailed(format!("read: {e}"))),
        Err(_) => return Err(SimChildError::HttpFailed("read timeout".into())),
    }

    let status = parse_status_line(&resp)
        .ok_or_else(|| SimChildError::HttpFailed("no status line".into()))?;
    if status == 200 {
        Ok(())
    } else {
        Err(SimChildError::HttpStatus {
            status,
            body: String::new(),
        })
    }
}

/// POST /control/inject with bearer auth. 200 OK returns `Ok(())`,
/// non-2xx surfaces `HttpStatus`, I/O errors surface `HttpFailed`.
#[cfg(feature = "dev_harness")]
async fn http_post_inject(port: u16, token: &str, body: &[u8]) -> Result<(), SimChildError> {
    let mut stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|e| SimChildError::HttpFailed(format!("connect: {e}")))?;

    let header = format!(
        "POST /control/inject HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .await
        .map_err(|e| SimChildError::HttpFailed(format!("write header: {e}")))?;
    stream
        .write_all(body)
        .await
        .map_err(|e| SimChildError::HttpFailed(format!("write body: {e}")))?;
    stream.shutdown().await.ok();

    let mut resp = Vec::with_capacity(256);
    timeout(Duration::from_secs(3), stream.read_to_end(&mut resp))
        .await
        .map_err(|_| SimChildError::HttpFailed("response timeout".into()))?
        .map_err(|e| SimChildError::HttpFailed(format!("read: {e}")))?;

    let status = parse_status_line(&resp)
        .ok_or_else(|| SimChildError::HttpFailed("no status line".into()))?;
    if (200..300).contains(&status) {
        Ok(())
    } else {
        let body_text = response_body(&resp).unwrap_or_default();
        Err(SimChildError::HttpStatus {
            status,
            body: body_text,
        })
    }
}

/// Parse `HTTP/1.1 200 OK\r\n` → 200. Returns None on malformed input.
fn parse_status_line(resp: &[u8]) -> Option<u16> {
    let text = std::str::from_utf8(resp).ok()?;
    let line = text.split("\r\n").next()?;
    let mut parts = line.split_whitespace();
    parts.next()?; // HTTP/1.1
    let code = parts.next()?;
    code.parse().ok()
}

/// Extract the body after the `\r\n\r\n` header terminator.
#[cfg(any(test, feature = "dev_harness"))]
fn response_body(resp: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(resp).ok()?;
    let idx = text.find("\r\n\r\n")?;
    Some(text[idx + 4..].trim().to_owned())
}

// ────────────────────────────── Tests ──────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::TcpListener as TokioTcpListener;

    /// Fake control-plane server: reads one HTTP request, optionally
    /// verifies path + auth, returns the configured status + body.
    struct FakeControl {
        port: u16,
        handle: tokio::task::JoinHandle<Option<Vec<u8>>>,
    }

    impl FakeControl {
        async fn start(
            status: u16,
            body: &'static str,
            require_auth: Option<&'static str>,
        ) -> Self {
            let listener = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let handle = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.ok()?;
                let (reader, mut writer) = stream.into_split();
                let mut reader = BufReader::new(reader);

                let mut headers = Vec::new();
                let mut method_line = String::new();
                reader.read_line(&mut method_line).await.ok()?;
                let mut auth: Option<String> = None;
                let mut content_length: usize = 0;
                loop {
                    let mut line = String::new();
                    let n = reader.read_line(&mut line).await.ok()?;
                    if n == 0 || line == "\r\n" || line == "\n" {
                        break;
                    }
                    if let Some(rest) = line.to_ascii_lowercase().strip_prefix("authorization: ") {
                        auth = Some(rest.trim().to_owned());
                    }
                    if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length: ") {
                        content_length = rest.trim().parse().unwrap_or(0);
                    }
                    headers.push(line);
                }

                let mut body_buf = vec![0u8; content_length];
                if content_length > 0 {
                    reader.read_exact(&mut body_buf).await.ok()?;
                }

                if let Some(expected) = require_auth {
                    let ok = auth
                        .as_deref()
                        .is_some_and(|v| v.eq_ignore_ascii_case(expected));
                    if !ok {
                        let _ = writer
                            .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
                            .await;
                        return Some(body_buf);
                    }
                }

                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = writer.write_all(resp.as_bytes()).await;
                Some(body_buf)
            });
            FakeControl { port, handle }
        }

        async fn await_body(self) -> Option<Vec<u8>> {
            self.handle.await.ok().flatten()
        }
    }

    #[tokio::test]
    async fn health_ok_returns_ok() {
        let fake = FakeControl::start(200, "ok", None).await;
        let port = fake.port;
        http_get_health(port).await.expect("200 should succeed");
        let _ = fake.await_body().await;
    }

    #[tokio::test]
    async fn health_5xx_returns_err() {
        let fake = FakeControl::start(503, "broken", None).await;
        let port = fake.port;
        let err = http_get_health(port).await.unwrap_err();
        match err {
            SimChildError::HttpStatus { status, .. } => assert_eq!(status, 503),
            other => panic!("unexpected err: {other:?}"),
        }
        let _ = fake.await_body().await;
    }

    #[cfg(feature = "dev_harness")]
    #[tokio::test]
    async fn inject_sends_bearer_and_body() {
        let fake = FakeControl::start(200, "{}", Some("bearer deadbeef")).await;
        let port = fake.port;
        let fault = SimFault::PacingViolation;
        let body = serde_json::to_vec(&fault).unwrap();
        http_post_inject(port, "deadbeef", &body)
            .await
            .expect("200");
        let captured = fake.await_body().await.expect("body captured");
        let captured: SimFault = serde_json::from_slice(&captured).unwrap();
        assert_eq!(captured, SimFault::PacingViolation);
    }

    #[cfg(feature = "dev_harness")]
    #[tokio::test]
    async fn inject_missing_auth_surfaces_401() {
        let fake = FakeControl::start(200, "ok", Some("bearer good")).await;
        let port = fake.port;
        let body = serde_json::to_vec(&SimFault::Disconnect).unwrap();
        let err = http_post_inject(port, "wrong", &body).await.unwrap_err();
        match err {
            SimChildError::HttpStatus { status, .. } => assert_eq!(status, 401),
            other => panic!("unexpected err: {other:?}"),
        }
        let _ = fake.await_body().await;
    }

    #[test]
    fn parse_status_line_happy() {
        assert_eq!(parse_status_line(b"HTTP/1.1 200 OK\r\n"), Some(200));
        assert_eq!(parse_status_line(b"HTTP/1.1 404 Not Found\r\n"), Some(404));
    }

    #[test]
    fn parse_status_line_bad() {
        assert_eq!(parse_status_line(b""), None);
        assert_eq!(parse_status_line(b"garbage"), None);
    }

    #[test]
    fn response_body_extracts_after_headers() {
        let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        assert_eq!(response_body(resp).as_deref(), Some("hello"));
    }

    #[test]
    fn resolve_sim_binary_honours_env_override() {
        // Use a guaranteed-non-existent path so the Err branch fires
        // cleanly regardless of what's in target dirs. Uses a
        // unique suffix to dodge test parallelism (other tests may
        // set MIDAS_IB_SIM_BIN).
        let saved = std::env::var("MIDAS_IB_SIM_BIN").ok();
        let fake = PathBuf::from("/definitely/not/a/real/path/midas-ib-sim-xyz");
        // SAFETY: tests run serially for env-var fiddling. Set → assert → restore.
        std::env::set_var("MIDAS_IB_SIM_BIN", &fake);
        let err = resolve_sim_binary().unwrap_err();
        match err {
            SimChildError::BinaryNotFound(msg) => {
                assert!(msg.contains("MIDAS_IB_SIM_BIN"), "msg: {msg}");
            }
            other => panic!("expected BinaryNotFound, got: {other:?}"),
        }
        match saved {
            Some(v) => std::env::set_var("MIDAS_IB_SIM_BIN", v),
            None => std::env::remove_var("MIDAS_IB_SIM_BIN"),
        }
    }

    #[test]
    fn allocate_sim_port_returns_preferred_when_free() {
        // Bind :0 to get any free port, drop the listener to free it,
        // then assert `allocate_sim_port` hands back that same port.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0u16)).unwrap();
        let free_port = listener.local_addr().unwrap().port();
        drop(listener);
        let got = allocate_sim_port(free_port).expect("free port available");
        assert_eq!(got, free_port);
    }

    #[test]
    fn allocate_sim_port_falls_back_when_preferred_taken() {
        // Hold a listener on the preferred port, then ask allocate_sim_port
        // for that port. It should return *some* port in the fallback
        // range (or the same port if the fallback scan starts at 7498
        // and our held port happens to also be 7498, but that's
        // extremely unlikely for a random :0-assigned port).
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0u16)).unwrap();
        let taken = held.local_addr().unwrap().port();
        let got = allocate_sim_port(taken).expect("scan should find one");
        // Either the fallback range kicks in, or we got a different
        // port (possible if `taken` happens to be 0). Either way it
        // must not equal the held port.
        assert_ne!(got, taken);
    }

    #[test]
    fn port_is_free_detects_bound_port() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0u16)).unwrap();
        let taken = listener.local_addr().unwrap().port();
        assert!(!port_is_free(taken), "bound port must not be free");
        drop(listener);
        // After drop the port should be free again (SO_REUSEADDR
        // semantics vary by OS — on Windows it's typically immediate).
        // Skip the post-drop check to keep the test deterministic.
    }
}
