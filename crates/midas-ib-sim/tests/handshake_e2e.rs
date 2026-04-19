//! Stage 01 smoke — boot the sim in-process, connect a TCP client, verify the
//! listener accepts the connection, shut down cleanly.
//!
//! Wave 2 Stage 02 replaces this with a real TWS handshake round-trip.

use std::time::Duration;

use midas_ib_sim::{start_sim, SimConfig};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connects_to_listener_and_shuts_down() {
    let config = SimConfig {
        port: 0,
        control_port: 0,
        token_path: Some(tempfile::tempdir().unwrap().keep().join("control.token")),
        ..Default::default()
    };

    let sim = start_sim(config).await.expect("start_sim");
    let addr = sim.bound_addr;

    // Connect + write a greeting + close. The sim's Stage 01 session task
    // reads bytes until EOF, so this is the full dance it currently supports.
    let mut client = TcpStream::connect(addr).await.expect("connect to sim");
    client
        .write_all(b"handshake-placeholder")
        .await
        .expect("write greeting");
    client.shutdown().await.ok();

    // Let the session task observe the EOF before shutting down the sim.
    tokio::time::sleep(Duration::from_millis(50)).await;

    sim.shutdown().await;
}
