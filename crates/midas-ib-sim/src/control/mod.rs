//! Control-plane HTTP API (side channel for fault injection + introspection).
//!
//! Stage 01 wired the bearer-token auth middleware + `/control/dump` endpoint
//! skeleton. Stage 09 adds `/control/inject` — the unified fault-injection
//! surface the `midas-app` devloop drives. Stage 05 later adds
//! `/control/metrics`.

pub mod api;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

use crate::engine::types::{EngineCmd, EngineSnapshot, SessionId};
use crate::quirks::error_codes;
use crate::security::ControlToken;

use self::api::FaultInject;

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

    /// Build the `axum::Router` for the control plane.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/control/health", get(health))
            .route("/control/dump", post(dump_state))
            .route("/control/inject", post(inject_fault))
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

    // Ask the engine for its current snapshot. A disconnected engine
    // shouldn't 200 OK with stale data — surface 503 instead.
    match engine_dump(&state).await {
        Ok(snap) => Ok(axum::Json(snap)),
        Err(e) => {
            warn!(error = %e, "engine dump failed");
            Err(StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

/// `POST /control/inject` — consumes a [`FaultInject`] body and routes
/// it to one or more `EngineCmd`s.
///
/// Broadcast semantics: `Disconnect` and `PacingViolation` fan out to
/// every active session discovered via [`engine_dump`]. Farm /
/// price-perturbation variants target the engine directly.
///
/// Why not one-shot responses? The engine's `Inject*` commands are
/// fire-and-forget — the engine emits `EngineEvent`s on its broadcast
/// channel but there's no per-command ACK. The control plane returns
/// 202 Accepted to reflect that.
async fn inject_fault(
    State(state): State<ControlState>,
    headers: HeaderMap,
    Json(fault): Json<FaultInject>,
) -> Result<StatusCode, (StatusCode, String)> {
    if !authorized(&state.token, &headers) {
        warn!("control plane: rejected /control/inject — missing/invalid token");
        return Err((StatusCode::UNAUTHORIZED, "unauthorized".into()));
    }

    let cmds = fault_to_cmds(&state, fault).await?;
    for cmd in cmds {
        state.engine_cmd_tx.send(cmd).await.map_err(|e| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("engine closed: {e}"),
            )
        })?;
    }
    Ok(StatusCode::ACCEPTED)
}

/// Fetch the engine's live snapshot via a oneshot-reply command.
async fn engine_dump(state: &ControlState) -> Result<EngineSnapshot, String> {
    let (tx, rx) = oneshot::channel();
    state
        .engine_cmd_tx
        .send(EngineCmd::DumpState { reply: tx })
        .await
        .map_err(|e| format!("send DumpState: {e}"))?;
    tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .map_err(|_| "engine dump timed out".to_owned())?
        .map_err(|e| format!("engine dropped reply: {e}"))
}

/// Translate a fault-injection request into one-or-more engine commands.
async fn fault_to_cmds(
    state: &ControlState,
    fault: FaultInject,
) -> Result<Vec<EngineCmd>, (StatusCode, String)> {
    fn sym(s: String) -> midas_broker_core::SymbolKey {
        // The simulator's `SymbolKey` carries both a contract id and
        // the symbol string; injection by symbol alone means we hash
        // with the same djb2 the injector uses.
        let mut hash = 5381i32;
        for b in s.bytes() {
            hash = hash.wrapping_mul(33).wrapping_add(b as i32);
        }
        let contract_id = (hash ^ 0x5f5f5f5f).unsigned_abs() as i32;
        midas_broker_core::SymbolKey {
            contract_id,
            symbol: s,
        }
    }

    Ok(match fault {
        FaultInject::Disconnect => {
            let sessions = active_sessions(state).await?;
            if sessions.is_empty() {
                // Nothing to disconnect. That's fine — report 202 but
                // push no commands, which keeps the endpoint idempotent
                // when the devloop pre-emptively disconnects before the
                // first client has connected.
                Vec::new()
            } else {
                sessions
                    .into_iter()
                    .map(|session| EngineCmd::InjectDisconnect {
                        session,
                        reason: "control-plane fault inject".into(),
                    })
                    .collect()
            }
        }
        FaultInject::PacingViolation => {
            let sessions = active_sessions(state).await?;
            sessions
                .into_iter()
                .map(|session| EngineCmd::InjectPacingViolation { session })
                .collect()
        }
        FaultInject::FarmOutage { farms } => vec![EngineCmd::InjectFarmOutage {
            code: error_codes::MD_FARM_BROKEN,
            farms,
        }],
        FaultInject::FarmRestore { farms, data_lost } => {
            let code = if data_lost {
                error_codes::FARM_RESTORED_NO_DATA
            } else {
                error_codes::FARM_RESTORED_DATA
            };
            vec![EngineCmd::InjectFarmRestore { code, farms }]
        }
        FaultInject::PriceJump {
            symbol,
            magnitude_pct,
        } => vec![EngineCmd::InjectPriceJump {
            symbol: sym(symbol),
            magnitude_pct,
        }],
        FaultInject::Gap { symbol, to } => vec![EngineCmd::InjectGap {
            symbol: sym(symbol),
            from: 0.0, // engine picks from last-known quote
            to,
        }],
        FaultInject::Halt {
            symbol,
            duration_ms,
        } => vec![EngineCmd::InjectHalt {
            symbol: sym(symbol),
            duration: Duration::from_millis(duration_ms),
        }],
        FaultInject::Burst {
            symbols,
            multiplier,
            duration_ms,
        } => vec![EngineCmd::InjectBurst {
            symbols: symbols.into_iter().map(sym).collect(),
            multiplier,
            duration: Duration::from_millis(duration_ms),
        }],
    })
}

async fn active_sessions(state: &ControlState) -> Result<Vec<SessionId>, (StatusCode, String)> {
    let snap = engine_dump(state)
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e))?;
    Ok(snap.sessions.into_iter().map(|s| s.session).collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke: the router builds without panicking under realistic state.
    /// Exercised against the default `ControlState` with a dummy cmd tx.
    #[test]
    fn router_builds() {
        let (tx, _rx) = mpsc::channel(1);
        let api = ControlApi::new(Arc::new(ControlToken::generate()), tx);
        let _router = api.router();
    }

    #[tokio::test]
    async fn inject_rejects_without_auth() {
        let (tx, _rx) = mpsc::channel(1);
        let state = ControlState {
            token: Arc::new(ControlToken::generate()),
            engine_cmd_tx: tx,
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer wrong-token".parse().unwrap(),
        );
        let result = inject_fault(State(state), headers, Json(FaultInject::PacingViolation)).await;
        match result {
            Err((status, _)) => assert_eq!(status, StatusCode::UNAUTHORIZED),
            Ok(_) => panic!("expected unauthorized"),
        }
    }

    #[tokio::test]
    async fn inject_farm_outage_routes_to_cmd() {
        let (tx, mut rx) = mpsc::channel(4);
        let state = ControlState {
            token: Arc::new(ControlToken::generate()),
            engine_cmd_tx: tx,
        };
        let token = state.token.as_str().to_owned();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let fault = FaultInject::FarmOutage {
            farms: vec!["usfarm".into()],
        };
        let result = inject_fault(State(state), headers, Json(fault)).await;
        assert!(matches!(result, Ok(StatusCode::ACCEPTED)));
        let cmd = rx.recv().await.expect("cmd delivered");
        match cmd {
            EngineCmd::InjectFarmOutage { code, farms } => {
                assert_eq!(code, error_codes::MD_FARM_BROKEN);
                assert_eq!(farms, vec!["usfarm".to_owned()]);
            }
            other => panic!("unexpected cmd: {other:?}"),
        }
    }

    #[tokio::test]
    async fn inject_farm_restore_picks_code_from_data_lost() {
        let (tx, mut rx) = mpsc::channel(4);
        let state = ControlState {
            token: Arc::new(ControlToken::generate()),
            engine_cmd_tx: tx,
        };
        let token = state.token.as_str().to_owned();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let fault = FaultInject::FarmRestore {
            farms: vec!["usfarm".into()],
            data_lost: true,
        };
        let result = inject_fault(State(state), headers, Json(fault)).await;
        assert!(matches!(result, Ok(StatusCode::ACCEPTED)));
        match rx.recv().await.unwrap() {
            EngineCmd::InjectFarmRestore { code, .. } => {
                assert_eq!(code, error_codes::FARM_RESTORED_NO_DATA);
            }
            other => panic!("unexpected cmd: {other:?}"),
        }
    }
}
