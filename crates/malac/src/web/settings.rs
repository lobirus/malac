//! Settings API: read and write the configuration files, provider keys and
//! memory, and apply changes by rebuilding the runtime.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, bail};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use malac_core::config::{self, home_dir};
use malac_core::memory::Scope;
use malac_core::{Config, Runtime, session};
use serde::Deserialize;
use serde_json::{Value, json};

use super::AppState;

type Shared = State<Arc<AppState>>;
type ApiResult = Result<Json<Value>, ApiError>;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/settings/raw/{file}", get(get_raw).put(put_raw))
        .route("/api/settings/apply", post(apply))
        .route("/api/providers/{name}/key", put(put_key).delete(delete_key))
        .route("/api/models", get(models))
        .route("/api/tools", get(tools))
        .route("/api/memory", get(memory_list))
        .route("/api/memory/search", post(memory_search))
        .route("/api/memory/reindex", post(memory_reindex))
        .route("/api/memory/{scope}/{name}", delete(memory_delete))
        .route("/api/update", get(update_status))
        .route("/api/update/check", post(update_check))
        .route("/api/update/install", post(update_install))
}

pub struct ApiError(anyhow::Error);

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({ "error": format!("{:#}", self.0) }));
        (StatusCode::BAD_REQUEST, body).into_response()
    }
}

/// Which file a raw edit targets.
fn config_file(runtime: &Runtime, file: &str) -> anyhow::Result<PathBuf> {
    Ok(match file {
        "user" => config::user_config_path(),
        "project" => runtime.project_root.join(".malac").join("config.toml"),
        "control" => home_dir().join("control.toml"),
        other => bail!("unknown settings file `{other}` (expected user, project or control)"),
    })
}

fn read_json(path: &std::path::Path) -> anyhow::Result<Value> {
    Ok(match config::read_config_file(path)? {
        Some(table) => serde_json::to_value(table)?,
        None => Value::Null,
    })
}

async fn get_settings(State(state): Shared) -> ApiResult {
    let runtime = state.hub.settings_runtime().await?;
    let project_path = config_file(&runtime, "project")?;
    let control_path = config_file(&runtime, "control")?;
    let keys: BTreeMap<&String, bool> = runtime
        .config
        .providers
        .iter()
        .map(|(name, p)| (name, config::api_key(name, p).is_some()))
        .collect();
    let agent_types: Vec<Value> = runtime
        .agent_types
        .iter()
        .map(|t| json!({ "name": t.name, "description": t.description, "model": t.model, "tools": t.tools }))
        .collect();
    let skills: Vec<Value> = runtime
        .skills
        .iter()
        .map(|s| json!({ "name": s.name, "description": s.description, "path": s.path }))
        .collect();
    let instructions: Vec<&PathBuf> = runtime.instructions.iter().map(|i| &i.path).collect();
    Ok(Json(json!({
        "paths": {
            "home": home_dir(),
            "user": config::user_config_path(),
            "project": project_path,
            "control": control_path,
            "credentials": config::credentials_path(),
        },
        "user": read_json(&config::user_config_path())?,
        "project": read_json(&project_path)?,
        "control": read_json(&control_path)?,
        "effective": runtime.config,
        "keys": keys,
        "agent_types": agent_types,
        "skills": skills,
        "instructions": instructions,
        "context": runtime.context_report(),
        "reloadable": !state.hub.is_attached(),
    })))
}

#[derive(Deserialize)]
struct UserSettings {
    /// The whole user config file as JSON. `null` values remove keys.
    user: Value,
}

/// Replaces `~/.malac/config.toml` with a structured edit. Comments in the
/// file are not preserved; use the raw editor to keep them.
async fn put_settings(State(state): Shared, Json(body): Json<UserSettings>) -> ApiResult {
    let table = json_to_table(body.user)?;
    config::validate_user_table(&table)?;
    let path = config::user_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("cannot create the config directory")?;
    }
    std::fs::write(
        &path,
        toml::to_string_pretty(&table).context("cannot encode TOML")?,
    )
    .with_context(|| format!("cannot write {}", path.display()))?;
    applied(&state).await
}

async fn get_raw(State(state): Shared, Path(file): Path<String>) -> ApiResult {
    let runtime = state.hub.settings_runtime().await?;
    let path = config_file(&runtime, &file)?;
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    Ok(Json(json!({ "path": path, "text": text })))
}

#[derive(Deserialize)]
struct RawSettings {
    text: String,
}

async fn put_raw(
    State(state): Shared,
    Path(file): Path<String>,
    Json(body): Json<RawSettings>,
) -> ApiResult {
    let runtime = state.hub.settings_runtime().await?;
    let path = config_file(&runtime, &file)?;
    let table: toml::Table = body.text.parse().context("invalid TOML")?;
    if file != "control" {
        config::validate_user_table(&table)?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("cannot create the config directory")?;
    }
    std::fs::write(&path, &body.text)
        .with_context(|| format!("cannot write {}", path.display()))?;
    applied(&state).await
}

async fn apply(State(state): Shared) -> ApiResult {
    applied(&state).await
}

/// Reports whether saved changes took effect; saving itself already succeeded.
async fn applied(state: &AppState) -> ApiResult {
    Ok(Json(match state.hub.reload().await {
        Ok(()) => json!({ "saved": true, "applied": true }),
        Err(err) => json!({ "saved": true, "applied": false, "message": format!("{err:#}") }),
    }))
}

#[derive(Deserialize)]
struct KeyBody {
    api_key: String,
}

async fn put_key(
    State(state): Shared,
    Path(name): Path<String>,
    Json(body): Json<KeyBody>,
) -> ApiResult {
    let key = body.api_key.trim();
    if key.is_empty() {
        return Err(anyhow::anyhow!("the key is empty").into());
    }
    config::save_api_key(&name, key)?;
    state.hub.forget_provider(&name);
    Ok(Json(json!({ "saved": true })))
}

async fn delete_key(State(state): Shared, Path(name): Path<String>) -> ApiResult {
    config::remove_api_key(&name)?;
    state.hub.forget_provider(&name);
    Ok(Json(json!({ "saved": true })))
}

async fn models(State(state): Shared) -> ApiResult {
    let runtime = state.hub.settings_runtime().await?;
    // The files as they are now: providers apply without a reload, so the
    // runtime's own copy may be older.
    let config = Config::load(&runtime.cwd, None)
        .map(|loaded| loaded.config)
        .unwrap_or_else(|_| runtime.config.clone());
    let providers = malac_core::provider::discover_models(&config).await;
    Ok(Json(json!({
        "default": config.model,
        "small": config.small_model,
        "vision": config.vision_model,
        "providers": providers,
    })))
}

async fn tools(State(state): Shared) -> ApiResult {
    let runtime = state.hub.settings_runtime().await?;
    let tools: Vec<Value> = runtime
        .tools()
        .iter()
        .map(|t| {
            let description = t.description();
            let kind = match t.kind() {
                malac_core::tools::ToolKind::Safe => "safe",
                malac_core::tools::ToolKind::Edit => "edit",
                malac_core::tools::ToolKind::Execute => "execute",
            };
            json!({
                "name": t.name(),
                "kind": kind,
                "description": description.lines().next().unwrap_or_default(),
            })
        })
        .collect();
    Ok(Json(
        json!({ "tools": tools, "servers": runtime.servers() }),
    ))
}

async fn memory_list(State(state): Shared) -> ApiResult {
    let runtime = state.hub.settings_runtime().await?;
    Ok(Json(json!({
        "entries": runtime.memory.list(),
        "vector": runtime.memory.has_vector_store(),
    })))
}

#[derive(Deserialize)]
struct SearchBody {
    query: String,
}

async fn memory_search(State(state): Shared, Json(body): Json<SearchBody>) -> ApiResult {
    let hits = state
        .hub
        .settings_runtime()
        .await?
        .memory
        .search(&body.query, 20)
        .await?;
    Ok(Json(json!({ "hits": hits })))
}

async fn memory_reindex(State(state): Shared) -> ApiResult {
    let count = state.hub.settings_runtime().await?.memory.reindex().await?;
    Ok(Json(json!({ "indexed": count })))
}

async fn memory_delete(
    State(state): Shared,
    Path((scope, name)): Path<(String, String)>,
) -> ApiResult {
    state
        .hub
        .settings_runtime()
        .await?
        .memory
        .forget(Scope::parse(&scope)?, &name)
        .await?;
    Ok(Json(json!({ "deleted": true })))
}

pub(super) async fn delete_session(State(state): Shared, Path(id): Path<String>) -> ApiResult {
    if state.hub.agent_busy(&id) {
        return Err(anyhow::anyhow!("the session is running; stop it first").into());
    }
    let path = session::sessions_dir().join(format!("{id}.jsonl"));
    std::fs::remove_file(&path).with_context(|| format!("no session {id}"))?;
    state.hub.forget_agent(&id);
    Ok(Json(json!({ "deleted": true })))
}

/// JSON object to TOML table; `null` removes the key, since TOML has no null.
fn json_to_table(value: Value) -> anyhow::Result<toml::Table> {
    match strip_nulls(value) {
        Some(Value::Object(map)) => {
            let value =
                toml::Value::try_from(Value::Object(map)).context("cannot convert to TOML")?;
            match value {
                toml::Value::Table(table) => Ok(table),
                _ => bail!("settings must be an object"),
            }
        }
        _ => bail!("settings must be an object"),
    }
}

fn strip_nulls(value: Value) -> Option<Value> {
    match value {
        Value::Null => None,
        Value::Object(map) => Some(Value::Object(
            map.into_iter()
                .filter_map(|(k, v)| strip_nulls(v).map(|v| (k, v)))
                .collect(),
        )),
        Value::Array(items) => Some(Value::Array(
            items.into_iter().filter_map(strip_nulls).collect(),
        )),
        other => Some(other),
    }
}

async fn update_status() -> ApiResult {
    Ok(Json(
        serde_json::to_value(malac_core::update::status()).context("encode")?,
    ))
}

async fn update_check() -> ApiResult {
    let status = malac_core::update::check(true).await?;
    Ok(Json(serde_json::to_value(status).context("encode")?))
}

/// Replaces the binaries on disk. This server and its agents keep running the
/// old version; the response says to restart.
async fn update_install(State(state): Shared) -> ApiResult {
    let status = state.hub.install_update().await?;
    Ok(Json(serde_json::to_value(status).context("encode")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nulls_remove_keys() {
        let table = json_to_table(json!({
            "model": "lora/default",
            "small_model": null,
            "providers": { "lora": { "kind": "openai", "headers": null } },
        }))
        .unwrap();
        assert_eq!(table["model"].as_str(), Some("lora/default"));
        assert!(!table.contains_key("small_model"));
        assert!(
            !table["providers"]["lora"]
                .as_table()
                .unwrap()
                .contains_key("headers")
        );
        assert!(json_to_table(json!([1, 2])).is_err());
    }
}
