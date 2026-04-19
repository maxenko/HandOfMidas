//! IB-simulator child-process lifecycle for the devloop.
//!
//! Drives `midas-ib-sim-server` as a subprocess so test journeys can
//! spawn a fresh sim, run a scripted scenario, inject faults via HTTP,
//! and tear the sim down — all from devloop JSON commands.
//!
//! The module owns three responsibilities:
//! 1. Spawning the sim binary with the right CLI flags, then waiting
//!    for its `/control/health` endpoint to respond OK.
//! 2. Reading the bearer token the sim wrote to disk and caching it
//!    alongside the control-plane port for later fault-injection calls.
//! 3. SIGTERM → grace → SIGKILL teardown on [`ShutdownSim`], and
//!    best-effort reaping if the dev harness itself is dropped.
//!
//! HTTP is intentionally hand-rolled over `tokio::net::TcpStream`:
//! the control plane is local loopback HTTP/1.1 with small JSON
//! bodies, so a 30-LOC client costs far less than pulling `reqwest`
//! into the feature-gated dependency set.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Command as TokioCommand;
use tokio::time::{sleep, timeout, Instant};

use midas_devloop_proto::SimFault;

/// Handle to a running simulator child process.
///
/// Cloned (cheaply) across the harness so `ShutdownSim` /
/// `InjectSimFault` can reach the active sim without threading state
/// through every command.
#[derive(Clone, Debug)]
pub struct SimChildHandle {
    /// TWS wire-protocol port (e.g. 7497).
    pub tws_port: u16,
    /// Control-plane HTTP port (e.g. 9497).
    pub control_port: u16,
    /// Bearer token read from the sim's token file.
    pub token: String,
    /// Path of the `.pid` file written for supervisor reaping.
    pub pid_path: PathBuf,
    /// Shared child handle. `Arc<Mutex<_>>` because the child is
    /// consumed at shutdown time and the handle is cloned across
    /// command dispatch.
    child: Arc<Mutex<Option<tokio::process::Child>>>,
}

/// Errors surfaced from spawn / health-check / shutdown.
#[derive(Debug, thiserror::Error)]
pub enum SimChildError {
    #[error("spawning sim binary failed: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("sim health check timed out after {0:?}")]
    HealthTimeout(Duration),
    #[error("sim health check failed: {0}")]
    HealthFailed(String),
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

/// Resolve the path to `midas-ib-sim-server`. When the `MIDAS_IB_SIM_BIN`
/// env var is set (CI / tests) we honour it directly; otherwise we look
/// alongside the running `midas-app` binary, then fall back to `PATH`.
pub fn resolve_sim_binary() -> PathBuf {
    if let Ok(p) = std::env::var("MIDAS_IB_SIM_BIN") {
        return PathBuf::from(p);
    }
    let bin_name = if cfg!(windows) {
        "midas-ib-sim-server.exe"
    } else {
        "midas-ib-sim-server"
    };
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let sibling = dir.join(bin_name);
            if sibling.exists() {
                return sibling;
            }
        }
    }
    PathBuf::from(bin_name)
}

/// Spawn options, mirrors the fields of [`Command::SpawnSim`].
#[derive(Debug, Clone)]
pub struct SpawnOptions {
    pub tws_port: u16,
    pub control_port: u16,
    pub scenario: Option<String>,
    pub seed: Option<u64>,
}

/// Spawn the sim binary, wait for `/control/health`, write the pid
/// file, and return a handle the harness can keep alive.
pub async fn spawn(opts: SpawnOptions) -> Result<SimChildHandle, SimChildError> {
    let binary = resolve_sim_binary();

    // Put the token file where the sim + harness can both reach it
    // without needing to guess XDG paths in CI. Per-port keeps parallel
    // sims from stomping each other.
    std::fs::create_dir_all(".devloop").ok();
    let token_path = PathBuf::from(format!(".devloop/sim.{}.token", opts.tws_port));
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
        "devloop: spawning sim binary {} on tws_port={} control_port={}",
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

    // Record pid for supervisor reaping.
    let pid = child.id().unwrap_or(0);
    let pid_path = PathBuf::from(format!(".devloop/sim.{}.pid", opts.tws_port));
    std::fs::write(&pid_path, pid.to_string()).map_err(|err| SimChildError::PidWrite {
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
            use tokio::process::Command;
            if let Some(pid) = child.id() {
                // `kill -TERM` via the `kill` binary avoids pulling
                // libc into the feature-gated dep set just for SIGTERM.
                let _ = Command::new("kill")
                    .arg("-TERM")
                    .arg(pid.to_string())
                    .status()
                    .await;
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

    /// Fire a fault-injection POST to `/control/inject`.
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
    use tokio::net::TcpListener;

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
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let handle = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.ok()?;
                let (reader, mut writer) = stream.into_split();
                let mut reader = BufReader::new(reader);

                // Read headers until empty line; collect Authorization + Content-Length.
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
}
