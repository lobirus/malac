//! Remote access over REST: pairing, paired devices and tunnels. Only
//! `GET /api/hello` and `POST /api/pair` work without a token; pairing
//! needs a code from `malac remote pair` instead.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use malac_core::config::Tunnel;
use malac_core::update;
use serde::Deserialize;
use serde_json::json;

use super::AppState;
use super::settings::ApiError;
use crate::daemon::access::{self, Caller, PairError};
use crate::protocol::{PairRequest, Paired};

type Shared = State<Arc<AppState>>;

/// Routes that need a token.
pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/pair/codes", post(create_code))
        .route("/api/devices", get(devices))
        .route("/api/devices/{id}", delete(revoke))
        .route("/api/remote", get(status))
        .route("/api/tunnel", post(start_tunnel).delete(stop_tunnel))
}

/// Routes for clients that have no token yet.
pub(super) fn public_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/hello", get(hello))
        .route("/api/pair", post(pair))
}

/// Tells a client it found a malac daemon and whether it can pair.
async fn hello(State(state): Shared) -> Json<serde_json::Value> {
    Json(json!({
        "app": "malac",
        "version": update::CURRENT,
        "remote": state.hub.options().remote,
        "pairing": true,
    }))
}

async fn pair(
    State(state): Shared,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<PairRequest>,
) -> Response {
    let address = access::display_address(peer.ip(), &headers);
    let kind = request.kind.as_deref().unwrap_or("client");
    let result = state
        .hub
        .access()
        .pair(&request.code, &request.name, kind, Some(address));
    match result {
        Ok((token, device)) => {
            tracing::info!("paired {} ({}) from {address}", device.name, device.id);
            state.hub.remote_changed();
            let paired = Paired {
                token,
                device,
                version: update::CURRENT.into(),
            };
            Json(paired).into_response()
        }
        Err(err) => {
            // Slows down guessing; the code space and the failure limit do the rest.
            tokio::time::sleep(Duration::from_millis(400)).await;
            let status = match err {
                PairError::Invalid => StatusCode::FORBIDDEN,
                PairError::TooMany => StatusCode::TOO_MANY_REQUESTS,
            };
            (status, Json(json!({ "error": err.to_string() }))).into_response()
        }
    }
}

async fn create_code(State(state): Shared) -> Json<crate::protocol::Pairing> {
    Json(state.hub.pairing())
}

async fn devices(
    State(state): Shared,
    Extension(caller): Extension<Caller>,
) -> Json<serde_json::Value> {
    let devices = state.hub.access().devices(caller.device());
    Json(json!({ "devices": devices }))
}

async fn revoke(
    State(state): Shared,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let device = state.hub.access().revoke(&id)?;
    tracing::info!("revoked {} ({})", device.name, device.id);
    state.hub.remote_changed();
    Ok(Json(json!({ "revoked": device })))
}

async fn status(State(state): Shared) -> Json<crate::protocol::RemoteStatus> {
    Json(state.hub.remote_status())
}

#[derive(Deserialize)]
struct TunnelBody {
    provider: String,
}

async fn start_tunnel(
    State(state): Shared,
    Json(body): Json<TunnelBody>,
) -> Result<Json<crate::protocol::TunnelStatus>, ApiError> {
    let provider: Tunnel = body.provider.parse()?;
    Ok(Json(state.hub.start_tunnel(provider).await?))
}

async fn stop_tunnel(State(state): Shared) -> Json<serde_json::Value> {
    Json(json!({ "stopped": state.hub.stop_tunnel().await }))
}
