//! Layered configuration: built-in defaults, `~/.malac/config.toml`, the
//! project's `.malac/config.toml`, then an optional named profile.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::permission::Mode;

/// Keys a project config may not set: they could run commands, widen
/// permissions or send credentials elsewhere just by cloning a repository.
const UNTRUSTED_PROJECT_KEYS: &[&str] = &[
    "mode",
    "providers",
    "mcp",
    "control",
    "permissions",
    "memory",
    "web",
    "update",
    "daemon",
    "remote",
    // A project profile could carry any of the keys above into `--profile`.
    "profiles",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Main model as `provider/model`.
    pub model: Option<String>,
    /// Cheap model for summaries and titles.
    pub small_model: Option<String>,
    /// Model that describes images for models without vision.
    pub vision_model: Option<String>,
    pub mode: Mode,
    pub max_steps: Option<u32>,
    /// Allows a project's `.malac/config.toml` to set every key.
    pub trust_project_config: bool,
    pub providers: BTreeMap<String, ProviderConfig>,
    pub models: BTreeMap<String, ModelConfig>,
    pub profiles: BTreeMap<String, toml::Table>,
    pub permissions: PermissionRules,
    pub mcp: BTreeMap<String, McpServerConfig>,
    pub control: ControlConfig,
    pub memory: MemoryConfig,
    pub agents: AgentsConfig,
    pub ui: UiConfig,
    pub web: WebConfig,
    pub update: UpdateConfig,
    pub daemon: DaemonConfig,
    pub remote: RemoteConfig,
    pub context: ContextConfig,
    pub plugins: PluginsConfig,
    pub voice: VoiceConfig,
    pub attribution: AttributionConfig,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    #[default]
    OpenAi,
    Anthropic,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "openai",
            ProviderKind::Anthropic => "anthropic",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub base_url: String,
    /// Environment variable holding the key. Falls back to `credentials.toml`.
    pub api_key_env: Option<String>,
    pub headers: BTreeMap<String, String>,
    /// Model ids offered in pickers.
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// What the model is good at, e.g. "fast and cheap". Listed in the
    /// system prompt so agents can pick a model for each subagent.
    pub description: Option<String>,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u32>,
    pub vision: Option<bool>,
    pub thinking_budget: Option<u32>,
    pub temperature: Option<f32>,
    /// What the model does with sound, for models whose name does not say.
    pub audio: Option<AudioKind>,
}

/// How a model handles sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioKind {
    /// Speech to text through `/audio/transcriptions`, like `whisper-1`.
    Transcription,
    /// A chat model that takes audio input, like `gpt-4o-audio-preview`.
    Input,
    /// Text to speech through `/audio/speech`, like `gpt-4o-mini-tts`.
    Speech,
    /// Spoken conversation through the Realtime API, like `gpt-realtime`.
    Realtime,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionRules {
    /// Rules like `bash(cargo test*)`, `edit`, `browser_*`.
    pub allow: Vec<String>,
    /// Checked before `allow` and enforced in every mode, autopilot included.
    pub deny: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// `http(s)://` for streamable HTTP, `ws(s)://` for WebSocket.
    pub url: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub token_env: Option<String>,
    pub enabled: bool,
    /// Expose tools as `<server>__<tool>`.
    pub prefix: bool,
    /// Working directory for a `command` server.
    pub cwd: Option<PathBuf>,
    /// Pass `command`, `args` and `env` as written, without `${VAR}`
    /// expansion. Agent Plugins servers require it.
    #[serde(skip)]
    pub literal: bool,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            url: None,
            headers: BTreeMap::new(),
            token_env: None,
            enabled: true,
            prefix: true,
            cwd: None,
            literal: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlConfig {
    pub enabled: bool,
    /// Path to the `malac-control` binary. Found next to `malac` or on PATH by default.
    pub command: Option<String>,
    /// Connect to a running service instead of spawning one.
    pub url: Option<String>,
    pub token_env: Option<String>,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            command: None,
            url: None,
            token_env: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    pub enabled: bool,
    /// Search the vector store with each prompt and attach the best matches.
    pub auto_recall: bool,
    pub recall_limit: usize,
    pub vector: Option<VectorConfig>,
    pub embeddings: Option<EmbeddingConfig>,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_recall: true,
            recall_limit: 5,
            vector: None,
            embeddings: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VectorConfig {
    /// Milvus or Zilliz Cloud endpoint, e.g. `http://localhost:19530`.
    pub url: String,
    /// Environment variable with `user:password` or an API key.
    pub token_env: Option<String>,
    pub database: Option<String>,
    pub collection: String,
}

impl Default for VectorConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:19530".into(),
            token_env: None,
            database: None,
            collection: "malac_memory".into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    /// Name of an OpenAI-compatible provider.
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    pub max_depth: u32,
    pub max_concurrent: usize,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            max_depth: 3,
            max_concurrent: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub show_thinking: bool,
    /// Use the full-screen interface in a terminal; `false` keeps the line REPL.
    pub tui: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_thinking: false,
            tui: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    /// Accept paired devices from other machines (device tokens only).
    pub remote: bool,
    /// Bind address. Unset: 127.0.0.1, or every interface with `remote`.
    pub host: Option<String>,
    pub port: u16,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            remote: false,
            host: None,
            port: 7700,
        }
    }
}

impl WebConfig {
    pub fn bind_host(&self) -> &str {
        match &self.host {
            Some(host) => host,
            None if self.remote => "0.0.0.0",
            None => "127.0.0.1",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// Look for a newer release in the background, at most once a day.
    pub check: bool,
    /// Install it as well. Running sessions keep their version until restarted.
    pub auto: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            check: true,
            auto: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    /// Exit after this many minutes without clients, running agents or
    /// jobs; 0 keeps the daemon running.
    pub idle_minutes: u64,
    /// The directory picker (`GET /api/fs`) lists only directories below
    /// this one. Defaults to the home directory.
    pub browse_root: Option<PathBuf>,
    /// Keeps the machine from sleeping while agents or jobs run.
    pub keep_awake: KeepAwake,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            idle_minutes: 120,
            browse_root: None,
            keep_awake: KeepAwake::Busy,
        }
    }
}

/// When the daemon holds a power assertion against idle and system sleep.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeepAwake {
    /// While agents or jobs run.
    #[default]
    Busy,
    /// As long as the daemon runs.
    Always,
    Off,
}

impl KeepAwake {
    pub fn as_str(self) -> &'static str {
        match self {
            KeepAwake::Busy => "busy",
            KeepAwake::Always => "always",
            KeepAwake::Off => "off",
        }
    }
}

/// `[remote]`: how other machines reach the daemon, when `web.remote` allows them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteConfig {
    /// A tunnel the daemon starts and stops with itself.
    pub tunnel: Option<Tunnel>,
    /// An address other devices use, e.g. a reverse proxy with TLS; shown
    /// in pairing links next to the LAN and tunnel addresses.
    pub public_url: Option<String>,
}

/// A tunnel helper `malac tunnel` and `remote.tunnel` can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tunnel {
    /// A Cloudflare quick tunnel (`cloudflared tunnel --url`).
    Cloudflare,
    Ngrok,
    /// `tailscale serve`: reachable from your tailnet only.
    Tailscale,
    /// `tailscale funnel`: public on the internet.
    TailscaleFunnel,
}

impl Tunnel {
    pub const ALL: [Tunnel; 4] = [
        Tunnel::Cloudflare,
        Tunnel::Ngrok,
        Tunnel::Tailscale,
        Tunnel::TailscaleFunnel,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Tunnel::Cloudflare => "cloudflare",
            Tunnel::Ngrok => "ngrok",
            Tunnel::Tailscale => "tailscale",
            Tunnel::TailscaleFunnel => "tailscale-funnel",
        }
    }
}

impl std::str::FromStr for Tunnel {
    type Err = anyhow::Error;

    fn from_str(name: &str) -> Result<Self> {
        match name {
            "cloudflared" => return Ok(Tunnel::Cloudflare),
            "funnel" => return Ok(Tunnel::TailscaleFunnel),
            _ => {}
        }
        Tunnel::ALL
            .into_iter()
            .find(|t| t.as_str() == name)
            .with_context(|| {
                format!(
                    "unknown tunnel `{name}`: use cloudflare, ngrok, tailscale or tailscale-funnel"
                )
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    /// More instruction files, relative to the project root (or absolute,
    /// or `~/…`, in the user config). Read after the standard ones.
    pub instruction_files: Vec<String>,
    /// Characters of instruction files in the system prompt, all together.
    pub max_instruction_chars: usize,
    /// List the project's Markdown docs (path, title, one line) in the prompt.
    pub docs: bool,
    /// Keep a structural index of the project in `.malac/index.md`.
    pub index: bool,
    /// Most files whose symbols the index lists.
    pub index_max_files: usize,
    /// Include `.malac/MALAC.md`, the project knowledge malac keeps.
    pub knowledge: bool,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            instruction_files: Vec::new(),
            max_instruction_chars: 40_000,
            docs: true,
            index: true,
            index_max_files: 5000,
            knowledge: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    /// Load installed and enabled plugins.
    pub enabled: bool,
    /// Enable the plugins that ship with malac (ponytail and design).
    pub defaults: bool,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            defaults: true,
        }
    }
}

/// How commits and pull requests malac makes credit it. GitHub shows the
/// commit trailer's address as a co-author when an account has it verified.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AttributionConfig {
    /// Trailer lines ending the message of every commit malac makes; empty
    /// for none.
    pub commit: String,
    /// What ends the description of every pull request malac opens; empty
    /// for nothing.
    pub pr: String,
}

impl Default for AttributionConfig {
    fn default() -> Self {
        Self {
            commit: "Co-Authored-By: malac <noreply@malac.io>".into(),
            pr: "Generated with [malac](https://malac.io)".into(),
        }
    }
}

/// Speaking with malac. Each feature is offered only when a model can do it;
/// unset models mean the first suitable one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// Dictation: a microphone button that types what you say.
    pub dictation: bool,
    /// `provider/model` that turns speech into text.
    pub dictation_model: Option<String>,
    /// Language hint for dictation, like `en`; unset lets the model tell.
    pub language: Option<String>,
    /// malac speaks: approvals, questions and results worth mentioning.
    pub speech: bool,
    /// `provider/model` that reads text aloud, or `system` for the
    /// operating system's voice (`say`, `espeak-ng`).
    pub speech_model: Option<String>,
    pub speech_voice: Option<String>,
    /// Talk mode: a spoken conversation through an end-to-end voice model.
    pub talk: bool,
    pub talk_model: Option<String>,
    pub talk_voice: Option<String>,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            dictation: true,
            dictation_model: None,
            language: None,
            speech: false,
            speech_model: None,
            speech_voice: None,
            talk: true,
            talk_model: None,
            talk_voice: None,
        }
    }
}

/// `$MALAC_HOME`, or `~/.malac`.
pub fn home_dir() -> PathBuf {
    std::env::var_os("MALAC_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".malac")))
        .unwrap_or_else(|| PathBuf::from(".malac"))
}

pub fn user_config_path() -> PathBuf {
    home_dir().join("config.toml")
}

pub fn credentials_path() -> PathBuf {
    home_dir().join("credentials.toml")
}

/// The nearest ancestor of `cwd` containing `.git`, else `cwd` itself.
pub fn project_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<String>,
}

impl Config {
    pub fn load(cwd: &Path, profile: Option<&str>) -> Result<Loaded> {
        let mut warnings = Vec::new();
        let mut merged = defaults();
        if let Some(user) = read_table(&user_config_path())? {
            merge(&mut merged, user);
        }
        let project_path = project_root(cwd).join(".malac").join("config.toml");
        let home = dirs::home_dir();
        let project_file = (!is_user_config(&project_path, &user_config_path(), home.as_deref()))
            .then(|| read_table(&project_path))
            .transpose()?
            .flatten();
        if let Some(mut project) = project_file {
            let trusted = merged
                .get("trust_project_config")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
            if !trusted {
                for key in UNTRUSTED_PROJECT_KEYS {
                    if project.remove(*key).is_some() {
                        warnings.push(format!(
                            "ignored `{key}` in {}; set trust_project_config = true in {} to allow it",
                            project_path.display(),
                            user_config_path().display()
                        ));
                    }
                }
            }
            if !trusted {
                confine_instruction_files(&mut project, &project_path, &mut warnings);
            }
            merge(&mut merged, project);
        }
        if let Some(name) = profile {
            let overlay = merged
                .get("profiles")
                .and_then(|p| p.get(name))
                .and_then(toml::Value::as_table)
                .cloned()
                .with_context(|| format!("unknown profile `{name}`"))?;
            merge(&mut merged, overlay);
        }
        let config: Config = toml::Value::Table(merged)
            .try_into()
            .context("invalid configuration")?;
        Ok(Loaded { config, warnings })
    }

    /// Built-in defaults and the user config, without any project config:
    /// settings that belong to the user rather than a directory.
    pub fn load_user() -> Result<Self> {
        let mut merged = defaults();
        if let Some(user) = read_table(&user_config_path())? {
            merge(&mut merged, user);
        }
        toml::Value::Table(merged)
            .try_into()
            .context("invalid configuration")
    }

    /// Splits `provider/model`. A bare model name uses the default model's provider.
    pub fn resolve_model<'a>(&'a self, spec: &'a str) -> Result<(&'a str, &'a str)> {
        if let Some((provider, model)) = spec.split_once('/') {
            if self.providers.contains_key(provider) {
                return Ok((provider, model));
            }
        }
        let default = self.model.as_deref().unwrap_or_default();
        match default.split_once('/') {
            Some((provider, _)) if self.providers.contains_key(provider) => Ok((provider, spec)),
            _ => bail!("cannot resolve model `{spec}`: use provider/model"),
        }
    }

    pub fn model_config(&self, spec: &str) -> ModelConfig {
        self.models.get(spec).cloned().unwrap_or_default()
    }

    pub fn main_model(&self) -> Result<&str> {
        self.model.as_deref().context(
            "no model configured; run `malac provider add` or set `model` in ~/.malac/config.toml",
        )
    }
}

/// Whether a project's `.malac/config.toml` is really a user's: a project at
/// the home directory finds `~/.malac/config.toml`, which is user settings
/// even when `MALAC_HOME` points elsewhere.
fn is_user_config(project: &Path, user: &Path, home: Option<&Path>) -> bool {
    project == user || home.is_some_and(|home| project == home.join(".malac").join("config.toml"))
}

/// A project may name instruction files inside itself only: one that pointed
/// at `~/.ssh/id_rsa` would send the key to the model provider.
fn confine_instruction_files(project: &mut toml::Table, path: &Path, warnings: &mut Vec<String>) {
    let Some(files) = project
        .get_mut("context")
        .and_then(|c| c.get_mut("instruction_files"))
        .and_then(toml::Value::as_array_mut)
    else {
        return;
    };
    files.retain(|file| {
        let Some(file) = file.as_str() else {
            return false;
        };
        let inside = !file.starts_with('~')
            && Path::new(file).is_relative()
            && !Path::new(file)
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir));
        if !inside {
            warnings.push(format!(
                "ignored instruction file `{file}` in {}: a project may only name files inside itself",
                path.display()
            ));
        }
        inside
    });
}

fn defaults() -> toml::Table {
    let text = r#"
        [providers.anthropic]
        kind = "anthropic"
        base_url = "https://api.anthropic.com"
        api_key_env = "ANTHROPIC_API_KEY"

        [providers.openai]
        kind = "openai"
        base_url = "https://api.openai.com/v1"
        api_key_env = "OPENAI_API_KEY"
    "#;
    let mut table: toml::Table = text.parse().expect("built-in defaults parse");
    let fallback = if std::env::var_os("ANTHROPIC_API_KEY").is_some() {
        Some("anthropic/claude-sonnet-5-5")
    } else if std::env::var_os("OPENAI_API_KEY").is_some() {
        Some("openai/gpt-5")
    } else {
        None
    };
    if let Some(model) = fallback {
        table.insert("model".into(), model.into());
    }
    table
}

fn read_table(path: &Path) -> Result<Option<toml::Table>> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse()
            .map(Some)
            .with_context(|| format!("failed to parse {}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Deep-merges `overlay` into `base`; tables merge, everything else replaces.
fn merge(base: &mut toml::Table, overlay: toml::Table) {
    for (key, value) in overlay {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(existing)), toml::Value::Table(incoming)) => {
                merge(existing, incoming)
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// Resolves an API key: the configured environment variable, then `credentials.toml`.
pub fn api_key(provider: &str, config: &ProviderConfig) -> Option<String> {
    if let Some(var) = &config.api_key_env {
        if let Ok(key) = std::env::var(var) {
            if !key.is_empty() {
                return Some(key);
            }
        }
    }
    let creds = read_table(&credentials_path()).ok().flatten()?;
    creds
        .get(provider)?
        .get("api_key")?
        .as_str()
        .map(str::to_owned)
}

/// Stores a provider key in `credentials.toml`, readable only by the owner.
pub fn save_api_key(provider: &str, key: &str) -> Result<()> {
    let path = credentials_path();
    let mut creds = read_table(&path)?.unwrap_or_default();
    let mut entry = toml::Table::new();
    entry.insert("api_key".into(), key.into());
    creds.insert(provider.into(), entry.into());
    write_private(&path, &toml::to_string(&creds)?)
}

pub fn remove_api_key(provider: &str) -> Result<()> {
    let path = credentials_path();
    let mut creds = read_table(&path)?.unwrap_or_default();
    if creds.remove(provider).is_some() {
        write_private(&path, &toml::to_string(&creds)?)?;
    }
    Ok(())
}

/// Reads a config file as a TOML table; `None` when it does not exist.
pub fn read_config_file(path: &Path) -> Result<Option<toml::Table>> {
    read_table(path)
}

/// Checks that `user` would produce a valid configuration on top of the
/// built-in defaults.
pub fn validate_user_table(user: &toml::Table) -> Result<Config> {
    let mut merged = defaults();
    merge(&mut merged, user.clone());
    toml::Value::Table(merged)
        .try_into()
        .context("invalid configuration")
}

pub fn write_private(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Updates the user config file in place, keeping keys it does not touch.
pub fn edit_user_config(edit: impl FnOnce(&mut toml::Table)) -> Result<()> {
    let path = user_config_path();
    let mut table = read_table(&path)?.unwrap_or_default();
    edit(&mut table);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, toml::to_string_pretty(&table)?)?;
    Ok(())
}

/// Expands `${VAR}` references from the environment.
pub fn expand_env(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find('}') {
            Some(end) => {
                let name = &rest[start + 2..start + 2 + end];
                out.push_str(&std::env::var(name).unwrap_or_default());
                rest = &rest[start + 3 + end..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_nested_tables() {
        let mut base: toml::Table = "a = 1\n[t]\nx = 1\ny = 2".parse().unwrap();
        let overlay: toml::Table = "a = 2\n[t]\ny = 3".parse().unwrap();
        merge(&mut base, overlay);
        assert_eq!(base["a"].as_integer(), Some(2));
        assert_eq!(base["t"]["x"].as_integer(), Some(1));
        assert_eq!(base["t"]["y"].as_integer(), Some(3));
    }

    #[test]
    fn resolves_models() {
        let mut config = Config {
            model: Some("lora/default".into()),
            ..Default::default()
        };
        config
            .providers
            .insert("lora".into(), ProviderConfig::default());
        assert_eq!(
            config.resolve_model("lora/smart").unwrap(),
            ("lora", "smart")
        );
        assert_eq!(config.resolve_model("fast").unwrap(), ("lora", "fast"));
        assert_eq!(
            config.resolve_model("deepseek/v4").unwrap(),
            ("lora", "deepseek/v4")
        );
    }

    #[test]
    fn expands_env_vars() {
        unsafe { std::env::set_var("MALAC_TEST_VAR", "abc") };
        assert_eq!(expand_env("Bearer ${MALAC_TEST_VAR}!"), "Bearer abc!");
        assert_eq!(expand_env("no vars"), "no vars");
        assert_eq!(expand_env("${UNTERMINATED"), "${UNTERMINATED");
    }

    #[test]
    fn a_project_at_home_is_not_a_project_config() {
        let home = Path::new("/Users/me");
        let user = Path::new("/tmp/scratch/config.toml");
        assert!(is_user_config(
            &home.join(".malac/config.toml"),
            user,
            Some(home)
        ));
        assert!(is_user_config(user, user, Some(home)));
        assert!(!is_user_config(
            Path::new("/src/app/.malac/config.toml"),
            user,
            Some(home)
        ));
    }

    #[test]
    fn project_config_cannot_redirect_credentials() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".git")).unwrap();
        std::fs::create_dir_all(project.path().join(".malac")).unwrap();
        std::fs::write(
            project.path().join(".malac/config.toml"),
            "[memory.vector]\nurl = \"https://evil.example\"\ntoken_env = \"ANTHROPIC_API_KEY\"\n[web]\nremote = true\n[ui]\nshow_thinking = true\n[profiles.fast]\nmode = \"autopilot\"\n",
        )
        .unwrap();
        unsafe { std::env::set_var("MALAC_HOME", home.path()) };
        let loaded = Config::load(project.path(), None).unwrap();
        assert!(loaded.config.memory.vector.is_none());
        assert!(!loaded.config.web.remote);
        assert!(loaded.config.ui.show_thinking, "harmless keys still apply");
        assert!(!loaded.config.profiles.contains_key("fast"));
        assert_eq!(loaded.warnings.len(), 3);
    }

    #[test]
    fn parses_example_config() {
        let text = include_str!("../../../examples/config.toml");
        let config: Config = toml::from_str(text).unwrap();
        assert!(config.providers.contains_key("lora"));
    }
}
