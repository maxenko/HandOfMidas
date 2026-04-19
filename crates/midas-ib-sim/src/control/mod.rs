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

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::sync::{mpsc, oneshot};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tracing::{info, warn};

use crate::engine::types::{EngineCmd, EngineSnapshot, SessionId};
use crate::quirks::error_codes;
use crate::security::ControlToken;

use self::api::FaultInject;

/// Max body size accepted on the control plane. 64 KiB is an order of
/// magnitude above the largest legitimate `FaultInject` payload.
pub const CONTROL_BODY_LIMIT_BYTES: usize = 64 * 1024;
/// Global per-request timeout for control-plane handlers. Past this the
/// layer returns 408 Request Timeout instead of leaking a blocked task.
pub const CONTROL_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounded window for the mpsc send in `inject_fault` — if the engine is
/// wedged we 503 rather than leaving the HTTP task hanging forever.
pub const INJECT_ENGINE_SEND_TIMEOUT: Duration = Duration::from_secs(1);

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
    ///
    /// Layer order (applied outermost → innermost):
    /// 1. `TimeoutLayer` — per-request wall-clock bound.
    /// 2. `RequestBodyLimitLayer` — caps body size BEFORE extraction, so
    ///    a 1 GiB body on an unauth'd request is rejected without being
    ///    fully consumed.
    /// 3. `from_fn(require_bearer)` — validates `Authorization: Bearer
    ///    <token>` BEFORE any handler's body extractor runs. `/control/health`
    ///    is whitelisted (no auth needed) inside the middleware so health
    ///    probes don't need the token.
    pub fn router(&self) -> Router {
        let state = self.state.clone();
        let auth_state = state.clone();
        Router::new()
            .route("/control/health", get(health))
            .route("/control/dump", post(dump_state))
            .route("/control/inject", post(inject_fault))
            .layer(middleware::from_fn_with_state(auth_state, require_bearer))
            .layer(RequestBodyLimitLayer::new(CONTROL_BODY_LIMIT_BYTES))
            .layer(TimeoutLayer::new(CONTROL_REQUEST_TIMEOUT))
            .with_state(state)
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

/// Middleware: reject any request to an auth-guarded route before the
/// handler's body extractor runs.
///
/// `/control/health` is a whitelist — unauthenticated health probes are
/// a legitimate monitoring pattern and carry no PII. Everything else
/// must present a valid bearer token or get 401'd before `axum::Json`
/// (or any other extractor) touches the body. That means a 1 GiB POST
/// with a bogus token exits the middleware without being read off the
/// wire past the `RequestBodyLimitLayer` cap, not after being fully
/// deserialised.
async fn require_bearer(
    State(state): State<ControlState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let path = req.uri().path();
    if path == "/control/health" {
        return Ok(next.run(req).await);
    }
    if !authorized(&state.token, req.headers()) {
        warn!(%path, "control plane: rejected — missing/invalid token");
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(req).await)
}

async fn dump_state(
    State(state): State<ControlState>,
) -> Result<axum::Json<EngineSnapshot>, StatusCode> {
    // Auth is enforced upstream by `require_bearer`; by the time the
    // handler runs the caller is known-good.

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
    Json(fault): Json<FaultInject>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Auth is enforced by the `require_bearer` middleware; by the time
    // the handler runs we're certain the caller presented the token.
    let cmds = fault_to_cmds(&state, fault).await?;
    for cmd in cmds {
        // A wedged engine must not pin the HTTP task forever — bound
        // the send with a short timeout and translate to 503 so the
        // devloop caller can retry rather than hang on this request.
        match tokio::time::timeout(INJECT_ENGINE_SEND_TIMEOUT, state.engine_cmd_tx.send(cmd)).await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("engine closed: {e}"),
                ));
            }
            Err(_) => {
                warn!("control plane: engine send timed out after {INJECT_ENGINE_SEND_TIMEOUT:?}");
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("engine send timed out after {INJECT_ENGINE_SEND_TIMEOUT:?}"),
                ));
            }
        }
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
    use axum::body::Body;
    use axum::http::{Method, Request};
    use tower::util::ServiceExt;

    fn make_api() -> (ControlApi, mpsc::Receiver<EngineCmd>, String) {
        let (tx, rx) = mpsc::channel(4);
        let token = ControlToken::generate();
        let token_str = token.as_str().to_owned();
        let api = ControlApi::new(Arc::new(token), tx);
        (api, rx, token_str)
    }

    /// Smoke: the router builds without panicking under realistic state.
    #[test]
    fn router_builds() {
        let (tx, _rx) = mpsc::channel(1);
        let api = ControlApi::new(Arc::new(ControlToken::generate()), tx);
        let _router = api.router();
    }

    /// Auth runs BEFORE the JSON body extractor. A wrong-token POST with a
    /// gigantic body must get 401 without the server reading past the
    /// `RequestBodyLimitLayer` cap.
    #[tokio::test]
    async fn inject_rejects_wrong_token_before_body_extraction() {
        let (api, _rx, _token) = make_api();
        let router = api.router();
        // 1 MiB body of junk — far larger than any real FaultInject payload
        // and beyond the body-limit cap. Should 401 without OOM / timeout.
        let huge = vec![b'x'; 1024 * 1024];
        let req = Request::builder()
            .method(Method::POST)
            .uri("/control/inject")
            .header("authorization", "Bearer wrong-token")
            .header("content-type", "application/json")
            .body(Body::from(huge))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// With a valid token, a legitimate body fits under the limit and
    /// reaches the handler.
    #[tokio::test]
    async fn inject_accepts_valid_token_within_limit() {
        let (api, mut rx, token) = make_api();
        let router = api.router();
        let fault = FaultInject::FarmOutage {
            farms: vec!["usfarm".into()],
        };
        let body = serde_json::to_vec(&fault).unwrap();
        assert!(body.len() < CONTROL_BODY_LIMIT_BYTES);
        let req = Request::builder()
            .method(Method::POST)
            .uri("/control/inject")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let cmd = rx.recv().await.expect("cmd delivered");
        match cmd {
            EngineCmd::InjectFarmOutage { code, farms } => {
                assert_eq!(code, error_codes::MD_FARM_BROKEN);
                assert_eq!(farms, vec!["usfarm".to_owned()]);
            }
            other => panic!("unexpected cmd: {other:?}"),
        }
    }

    /// A body above the limit is rejected by `RequestBodyLimitLayer`
    /// even for an authenticated caller — defence-in-depth.
    #[tokio::test]
    async fn inject_rejects_oversized_body_with_valid_token() {
        let (api, _rx, token) = make_api();
        let router = api.router();
        let huge = vec![b'x'; CONTROL_BODY_LIMIT_BYTES + 1];
        let req = Request::builder()
            .method(Method::POST)
            .uri("/control/inject")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(huge))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        // tower-http returns 413 Payload Too Large when the limit fires.
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    /// `/control/health` is intentionally unauth'd so external probes
    /// (devloop health-check, k8s liveness) don't need the token.
    #[tokio::test]
    async fn health_needs_no_auth() {
        let (api, _rx, _token) = make_api();
        let router = api.router();
        let req = Request::builder()
            .method(Method::GET)
            .uri("/control/health")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn inject_farm_restore_picks_code_from_data_lost() {
        let (api, mut rx, token) = make_api();
        let router = api.router();
        let fault = FaultInject::FarmRestore {
            farms: vec!["usfarm".into()],
            data_lost: true,
        };
        let body = serde_json::to_vec(&fault).unwrap();
        let req = Request::builder()
            .method(Method::POST)
            .uri("/control/inject")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        match rx.recv().await.unwrap() {
            EngineCmd::InjectFarmRestore { code, .. } => {
                assert_eq!(code, error_codes::FARM_RESTORED_NO_DATA);
            }
            other => panic!("unexpected cmd: {other:?}"),
        }
    }
}
