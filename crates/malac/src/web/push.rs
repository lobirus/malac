//! Push registration over REST, for paired devices: `GET /api/push` gives
//! the daemon's VAPID key and whether this device is registered, `PUT`
//! registers the Web Push endpoint and keys its distributor gave it, and
//! `DELETE` removes them.

use std::sync::Arc;

use anyhow::Context;
use axum::extract::State;
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;
use super::settings::ApiError;
use crate::daemon::access::Caller;

type Shared = State<Arc<AppState>>;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/push", get(status).put(register).delete(unregister))
}

async fn status(State(state): Shared, Extension(caller): Extension<Caller>) -> Json<Value> {
    let push = state.hub.push();
    let config = state.hub.base_config().push;
    Json(json!({
        "enabled": config.enabled,
        "wait_seconds": config.wait_seconds,
        "vapid_key": push.public_key(),
        "registered": caller.device().is_some_and(|device| push.is_registered(device)),
    }))
}

#[derive(Deserialize)]
struct Registration {
    endpoint: String,
    p256dh: String,
    auth: String,
}

async fn register(
    State(state): Shared,
    Extension(caller): Extension<Caller>,
    Json(body): Json<Registration>,
) -> Result<Json<Value>, ApiError> {
    let device = caller
        .device()
        .context("push notifications are for paired devices")?;
    state
        .hub
        .push()
        .subscribe(device, &body.endpoint, &body.p256dh, &body.auth)?;
    tracing::info!("device {device} registered for push");
    Ok(Json(json!({ "registered": true })))
}

async fn unregister(State(state): Shared, Extension(caller): Extension<Caller>) -> Json<Value> {
    let removed = caller
        .device()
        .is_some_and(|device| state.hub.push().unsubscribe(device));
    Json(json!({ "registered": false, "removed": removed }))
}
