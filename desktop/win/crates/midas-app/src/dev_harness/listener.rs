//! TCP accept loop + per-connection command reader.
//!
//! Runs inside the iced subscription stream, so `tokio::spawn` is
//! available. One connection = one newline-delimited JSON stream of
//! commands, each answered by a single JSON response line.

use iced::futures::channel::mpsc::Sender;
use iced::futures::SinkExt;
use midas_devloop_proto::{Command, ErrorKind, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

use super::Responder;
use crate::app::Message;

/// Bind the listener and accept-loop forever. Bind failures emit a
/// tracing error and the stream parks; iced will re-poll once the
/// subscription identity changes (e.g. port override).
pub async fn run(port: u16, output: Sender<Message>) {
    let addr = format!("127.0.0.1:{port}");
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("devloop: bind {addr} failed: {e}");
            std::future::pending::<()>().await;
            return;
        }
    };
    tracing::info!("devloop: harness listening on {addr}");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!("devloop: accept failed: {e}");
                continue;
            }
        };
        tracing::debug!("devloop: client connected from {peer}");

        let conn_output = output.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, conn_output).await {
                tracing::debug!("devloop: connection from {peer} ended: {e}");
            }
        });
    }
}

async fn handle_connection(stream: TcpStream, mut output: Sender<Message>) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Command>(&line) {
            Ok(cmd) => dispatch(cmd, &mut output).await,
            Err(e) => Response::Error {
                kind: ErrorKind::ParseError,
                message: e.to_string(),
                log_cursor: 0,
            },
        };

        let mut buf = match serde_json::to_vec(&response) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("devloop: response serialise failed: {e}");
                continue;
            }
        };
        buf.push(b'\n');
        writer.write_all(&buf).await?;
        writer.flush().await?;

        if matches!(response, Response::Ok { .. })
            && matches!(
                serde_json::from_str::<Command>(&line).ok(),
                Some(Command::Shutdown)
            )
        {
            // After a shutdown reply the app is about to exit; stop
            // draining input so the write side closes cleanly.
            break;
        }
    }

    Ok(())
}

async fn dispatch(cmd: Command, output: &mut Sender<Message>) -> Response {
    let (tx, rx) = oneshot::channel();
    let responder = Responder::new(tx);

    let send_result = output
        .send(Message::DevHarness {
            command: cmd,
            responder,
        })
        .await;

    if send_result.is_err() {
        return Response::Error {
            kind: ErrorKind::Internal,
            message: "iced subscription channel closed".to_owned(),
            log_cursor: 0,
        };
    }

    match rx.await {
        Ok(resp) => resp,
        Err(_) => Response::Error {
            kind: ErrorKind::Internal,
            message: "responder dropped before answering".to_owned(),
            log_cursor: 0,
        },
    }
}
