//! Control-plane HTTP API (side channel for fault injection + introspection).
//!
//! Stage 01 wires the bearer-token auth middleware + `/control/dump` endpoint
//! skeleton so the security story is real from day one. Stage 06 adds the
//! inject endpoints; Stage 05 adds `/control/metrics`.

pub mod api;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::engine::types::{EngineCmd, EngineSnapshot};
use crate::security::ControlToken;

/// Axum state passed into control-plane handlers.
#[derive(Clone)]
pub struct ControlState {
    pub token: Arc<ControlToken>,
    pub engine_cmd_tx: mpsc::Sender<EngineCmd>,
}

/// Public façade for the control plane.
pub struct ControlApi {
    state: ControlState,
}

impl ControlApi {
    pub fn new(token: Arc<ControlToken>, engine_cmd_tx: mpsc::Sender<EngineCmd>) -> Self {
        Self {
            state: ControlState {
                token,
                engine_cmd_tx,
            },
        }
    }

    /// Build the `axum::Router` for the control plane. Wave 2 (Stage 06 + 05)
    /// attaches the `inject`/`metrics` routes.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/control/health", get(health))
            .route("/control/dump", post(dump_state))
            .with_state(self.state.clone())
    }

    /// Start the control-plane listener. Binds `127.0.0.1` unless the caller
    /// already checked `--listen-external` (server.rs enforces this).
    pub async fn serve(self, addr: SocketAddr) -> std::io::Result<()> {
        let router = self.router();
        let listener = tokio::net::TcpListener::bind(addr).await?;
        info!(?addr, "control plane listening");
        axum::serve(listener, router)
            .await
            .map_err(std::io::Error::other)?;
        Ok(())
    }
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn dump_state(
    State(state): State<ControlState>,
    headers: HeaderMap,
) -> Result<axum::Json<EngineSnapshot>, StatusCode> {
    if !authorized(&state.token, &headers) {
        warn!("control plane: rejected /control/dump — missing/invalid token");
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Wave 2 fills in the real plumbing: send a `DumpState { reply }` command
    // and await the oneshot. Stage 01 returns a default snapshot so the route
    // and auth gate are exercised end-to-end.
    let _ = state; // silence unused warning until Wave 2 wires the cmd channel.
    Ok(axum::Json(EngineSnapshot::default()))
}

fn authorized(token: &ControlToken, headers: &HeaderMap) -> bool {
    let Some(h) = headers.get(axum::http::header::AUTHORIZATION) else {
        return false;
    };
    let Ok(s) = h.to_str() else { return false };
    let Some(presented) = s.strip_prefix("Bearer ") else {
        return false;
    };
    token.matches(presented)
}
