//! Voice and shared files over REST: dictation, speech audio, talk mode's
//! WebSocket and the files agents send with `send_to_user`.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use malac_core::{shared, voice};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::AppState;
use super::settings::ApiError;
use crate::daemon::talk;
use crate::protocol::VoiceStatus;

type Shared = State<Arc<AppState>>;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/voice", get(status).put(update))
        .route(
            "/api/voice/transcribe",
            post(transcribe).layer(DefaultBodyLimit::max(voice::MAX_AUDIO_BYTES)),
        )
        .route("/api/voice/speak", post(speak))
        .route("/api/voice/say/{id}", get(said))
        .route("/api/voice/talk", get(talk_socket))
        .route("/api/shared/{id}", get(shared_file))
}

#[derive(Deserialize)]
struct StatusQuery {
    #[serde(default)]
    refresh: bool,
}

async fn status(State(state): Shared, Query(query): Query<StatusQuery>) -> Json<VoiceStatus> {
    if query.refresh {
        state.hub.refresh_voice().await;
    }
    Json(state.hub.voice().status())
}

async fn update(
    State(state): Shared,
    Json(changes): Json<Map<String, Value>>,
) -> Result<Json<VoiceStatus>, ApiError> {
    Ok(Json(state.hub.set_voice(&changes).await?))
}

#[derive(Deserialize)]
struct TranscribeQuery {
    language: Option<String>,
}

/// The recording is the body, with its type in `Content-Type`.
async fn transcribe(
    State(state): Shared,
    Query(query): Query<TranscribeQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let status = state.hub.voice().status();
    let model = status
        .dictation
        .model
        .filter(|_| status.dictation.enabled)
        .ok_or_else(|| anyhow::anyhow!("dictation is off or no model can transcribe"))?;
    let media_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("audio/wav")
        .split(';')
        .next()
        .unwrap_or("audio/wav")
        .trim()
        .to_owned();
    let language = query.language.or(status.language);
    let config = state.hub.base_config();
    let text = voice::transcribe(
        &config,
        &model,
        body.to_vec(),
        &media_type,
        language.as_deref(),
    )
    .await?;
    Ok(Json(json!({ "text": text, "model": model })))
}

#[derive(Deserialize)]
struct SpeakBody {
    text: String,
}

/// Reads any text aloud with the speech model, for "read aloud" buttons.
async fn speak(State(state): Shared, Json(body): Json<SpeakBody>) -> Result<Response, ApiError> {
    let status = state.hub.voice().status();
    let model = status
        .speech
        .model
        .ok_or_else(|| anyhow::anyhow!("no model can speak"))?;
    let config = state.hub.base_config();
    let (audio, media_type) =
        voice::synthesize(&config, &model, status.speech.voice.as_deref(), &body.text).await?;
    Ok(([(header::CONTENT_TYPE, media_type)], audio).into_response())
}

async fn said(State(state): Shared, Path(id): Path<String>) -> Result<Response, ApiError> {
    let config = state.hub.base_config();
    let (audio, media_type) = state.hub.voice().clip(&id, &config).await?;
    Ok((
        [
            (header::CONTENT_TYPE, media_type),
            (header::CACHE_CONTROL, "private, max-age=600"),
        ],
        audio.as_ref().clone(),
    )
        .into_response())
}

#[derive(Deserialize)]
struct SharedQuery {
    #[serde(default)]
    download: bool,
}

async fn shared_file(Path(id): Path<String>, Query(query): Query<SharedQuery>) -> Response {
    let Ok((path, file)) = shared::open(&id) else {
        return (StatusCode::NOT_FOUND, "no such shared file").into_response();
    };
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return (StatusCode::NOT_FOUND, "no such shared file").into_response();
    };
    let disposition = format!(
        "{}; filename=\"{}\"",
        if query.download {
            "attachment"
        } else {
            "inline"
        },
        file.name.replace(['"', '\\'], "_")
    );
    // Shared HTML or SVG runs no script on the daemon's origin.
    let media_type = match file.media_type.as_str() {
        "text/html" | "image/svg+xml" if !query.download => "text/plain; charset=utf-8".to_owned(),
        other => other.to_owned(),
    };
    (
        [
            (header::CONTENT_TYPE, media_type),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, "private, max-age=86400".to_owned()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        ],
        bytes,
    )
        .into_response()
}

#[derive(Deserialize)]
struct TalkQuery {
    tab: Option<String>,
    agent: Option<String>,
}

async fn talk_socket(
    State(state): Shared,
    Query(query): Query<TalkQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if !origin.is_none_or(|origin| super::origin_allowed(origin, &headers)) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    let hub = state.hub.clone();
    ws.on_upgrade(move |socket| talk::run(hub, socket, query.tab, query.agent))
}
