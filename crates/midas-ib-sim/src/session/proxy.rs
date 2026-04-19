//! TCP proxy that sits between an API client and real IB TWS, teeing every
//! byte to the session [`Recorder`](crate::session::Recorder).
//!
//! # Architecture
//!
//! ```text
//! ┌──────────┐   client→sim   ┌───────┐   client→sim   ┌─────────┐
//! │ ibapi    │───────────────▶│ proxy │───────────────▶│ real IB │
//! │ client   │◀───────────────│       │◀───────────────│ TWS     │
//! └──────────┘   sim→client   └───┬───┘   sim→client   └─────────┘
//!                                 │
//!                                 ▼
//!                         [Recorder: .tws.pcap + .dbn]
//! ```
//!
//! Two async copy loops run in parallel, one per direction. Each loop reads
//! from its source socket into a buffer, writes to the destination socket,
//! and tees the slice to the recorder. The recorder is `Arc<Mutex<…>>` so
//! both loops can append without ordering beyond best-effort `Instant::now()`
//! timestamps — the pcap is append-only and single-consumer-friendly.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use crate::session::recorder::{Recorder, RecorderError};

/// Errors during a proxy session.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("recorder: {0}")]
    Recorder(#[from] RecorderError),
}

/// Configuration for a single proxy session.
#[derive(Clone, Debug)]
pub struct ProxyConfig {
    /// Local address to bind (e.g. `127.0.0.1:7497`).
    pub bind_addr: SocketAddr,
    /// Upstream IB gateway address (e.g. `127.0.0.1:7496`).
    pub upstream_addr: SocketAddr,
    /// Max bytes per copy-buffer chunk. 64 KiB is plenty for TWS frames.
    pub buf_size: usize,
    /// Accept-side TCP timeout. `None` means wait indefinitely.
    pub accept_timeout: Option<Duration>,
}

impl ProxyConfig {
    pub fn new(bind: SocketAddr, upstream: SocketAddr) -> Self {
        Self {
            bind_addr: bind,
            upstream_addr: upstream,
            buf_size: 64 * 1024,
            accept_timeout: None,
        }
    }
}

/// Run one proxy session to completion: bind, accept a single client, dial
/// upstream, tee bytes both ways into `recorder`, return when either side
/// closes.
///
/// For multi-client service, wrap this in a loop.
pub async fn run_proxy(
    config: ProxyConfig,
    recorder: Arc<Mutex<Recorder>>,
) -> Result<ProxyStats, ProxyError> {
    let listener = TcpListener::bind(config.bind_addr).await?;
    let (client, _peer) = match config.accept_timeout {
        None => listener.accept().await?,
        Some(d) => tokio::time::timeout(d, listener.accept())
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "accept timed out"))??,
    };
    let upstream = TcpStream::connect(config.upstream_addr).await?;
    bridge_streams(client, upstream, recorder, config.buf_size).await
}

/// Proxy result counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProxyStats {
    pub client_to_upstream_bytes: u64,
    pub upstream_to_client_bytes: u64,
}

/// Core tee-bidirectional logic, separated for testability: accepts any
/// duplex streams, not just `TcpStream`.
///
/// If either copy task panics or returns an error, the recorder is still
/// flushed before the error is propagated. Without that flush the last
/// few seconds of pcap+dbn data would be lost — exactly the window the
/// operator needs to understand why the session died.
pub async fn bridge_streams<C, U>(
    client: C,
    upstream: U,
    recorder: Arc<Mutex<Recorder>>,
    buf_size: usize,
) -> Result<ProxyStats, ProxyError>
where
    C: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
    U: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    let (mut cr, mut cw) = tokio::io::split(client);
    let (mut ur, mut uw) = tokio::io::split(upstream);

    let rec_a = Arc::clone(&recorder);
    let rec_b = Arc::clone(&recorder);

    let c_to_u = tokio::spawn(async move {
        copy_with_tee(
            &mut cr,
            &mut uw,
            buf_size,
            rec_a,
            CopyDirection::ClientToSim,
        )
        .await
    });
    let u_to_c = tokio::spawn(async move {
        copy_with_tee(
            &mut ur,
            &mut cw,
            buf_size,
            rec_b,
            CopyDirection::SimToClient,
        )
        .await
    });

    // We must flush the recorder before returning — even on the error
    // path — so buffered pcap/dbn data is durable. The helper below
    // turns JoinHandle results + per-direction Result<u64, ProxyError>
    // into a single `Result<u64, ProxyError>` and avoids the old
    // `.expect("...panicked")` which would bypass the flush.
    let c2u = settle(c_to_u.await, "client→upstream");
    let u2c = settle(u_to_c.await, "upstream→client");

    // Flush regardless of success/failure. Ignore flush errors when an
    // earlier failure already exists so we don't shadow the root cause.
    let flush_result = {
        let mut rec = recorder.lock().await;
        rec.flush()
    };

    match (c2u, u2c, flush_result) {
        (Ok(c), Ok(u), Ok(_)) => Ok(ProxyStats {
            client_to_upstream_bytes: c,
            upstream_to_client_bytes: u,
        }),
        (Err(e), _, _) | (_, Err(e), _) => Err(e),
        (_, _, Err(e)) => Err(ProxyError::Recorder(e)),
    }
}

/// Collapse `Result<Result<u64, ProxyError>, JoinError>` into
/// `Result<u64, ProxyError>`, logging + recovering from panics so the
/// caller can still flush the recorder.
fn settle(
    result: Result<Result<u64, ProxyError>, tokio::task::JoinError>,
    label: &str,
) -> Result<u64, ProxyError> {
    match result {
        Ok(inner) => inner,
        Err(join_err) if join_err.is_panic() => {
            tracing::error!(
                label,
                "proxy {label} task panicked: {join_err}. Recorder will still be flushed."
            );
            Err(ProxyError::Io(std::io::Error::other(format!(
                "{label} task panicked: {join_err}"
            ))))
        }
        Err(join_err) => {
            tracing::warn!(label, "proxy {label} task cancelled: {join_err}");
            Err(ProxyError::Io(std::io::Error::other(format!(
                "{label} task cancelled: {join_err}"
            ))))
        }
    }
}

/// Direction tag used by the copy loop — matches pcap `Direction`.
#[derive(Clone, Copy, Debug)]
enum CopyDirection {
    ClientToSim,
    SimToClient,
}

/// Copy bytes from `src` to `dst`, teeing each read into `recorder`.
async fn copy_with_tee<R, W>(
    src: &mut R,
    dst: &mut W,
    buf_size: usize,
    recorder: Arc<Mutex<Recorder>>,
    direction: CopyDirection,
) -> Result<u64, ProxyError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut buf = vec![0u8; buf_size];
    let mut total = 0u64;
    loop {
        let n = match src.read(&mut buf).await {
            Ok(0) => break, // clean EOF
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        dst.write_all(&buf[..n]).await?;
        {
            let mut rec = recorder.lock().await;
            match direction {
                CopyDirection::ClientToSim => rec.record_client_to_sim(&buf[..n])?,
                CopyDirection::SimToClient => rec.record_sim_to_client(&buf[..n])?,
            }
        }
        total += n as u64;
    }
    // Best-effort half-close so the other side sees EOF.
    let _ = dst.shutdown().await;
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::pcap::{Direction, TwsPcapReader};
    use crate::session::recorder::Recorder;
    use tempfile::TempDir;
    use tokio::io::duplex;

    /// Unit-test the tee logic with two in-process tokio duplex streams.
    /// No sockets, no real IB.
    #[tokio::test]
    async fn bridge_streams_tees_both_directions() {
        let dir = TempDir::new().unwrap();
        let stem = dir.path().join("proxy");
        let recorder = Arc::new(Mutex::new(
            Recorder::start(&stem, 210, false, None).unwrap(),
        ));

        // Create two duplex channels: (client_side, sim_side_of_client) and
        // (upstream_side, sim_side_of_upstream). The "proxy" sits between
        // sim_side_of_client and sim_side_of_upstream.
        let (client_ext, client_int) = duplex(4096);
        let (upstream_ext, upstream_int) = duplex(4096);

        let rec = Arc::clone(&recorder);
        let task =
            tokio::spawn(async move { bridge_streams(client_int, upstream_int, rec, 1024).await });

        // Client writes two chunks → proxy → upstream.
        let mut client = client_ext;
        let mut upstream = upstream_ext;
        client.write_all(b"HELLO").await.unwrap();
        // Upstream replies.
        upstream.write_all(b"WORLD").await.unwrap();

        // Read the "proxied" bytes on the opposite side.
        let mut buf = [0u8; 5];
        upstream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"HELLO");
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"WORLD");

        // Close both sides so the bridge task can finish.
        drop(client);
        drop(upstream);
        let stats = task.await.unwrap().unwrap();
        assert_eq!(stats.client_to_upstream_bytes, 5);
        assert_eq!(stats.upstream_to_client_bytes, 5);

        // Verify the pcap contains both directions.
        {
            let mut r = recorder.lock().await;
            r.flush().unwrap();
        }
        drop(recorder);

        let mut p = stem.clone();
        p.set_extension("tws.pcap");
        let records = TwsPcapReader::open(&p).unwrap().read_all().unwrap();
        assert!(records
            .iter()
            .any(|r| r.direction == Direction::ClientToSim && r.payload == b"HELLO"));
        assert!(records
            .iter()
            .any(|r| r.direction == Direction::SimToClient && r.payload == b"WORLD"));
    }

    /// Integration test placeholder — requires a real TWS.
    #[ignore]
    #[tokio::test]
    async fn proxy_against_real_ib_paper_gateway() {
        // Placeholder. Run manually with `cargo test -- --ignored`.
    }
}
