//! Reads a plugin directory in any of the formats malac understands and
//! returns its metadata and components:
//!
//! - **Agent Plugins** (agent-plugins.org): root `plugin.json` with a
//!   `$schema`, `skills/<name>/SKILL.md`, `mcp.json`, and malac's extension
//!   namespace `io.malac` (manifest data and an `io.malac/` directory).
//! - **Claude Code**: `.claude-plugin/plugin.json`, `skills/`, `commands/`,
//!   `agents/`, `hooks/hooks.json`, `.mcp.json`, and the manifest's
//!   component paths.
//! - Neither: the same default directories, named after the directory.
//!
//! A multi-format plugin is all of them at once: components are collected
//! from every layout it uses. Paths that leave the plugin root are refused.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Map, Value};

use super::hooks::{self, HookSpec};
use crate::config::McpServerConfig;

/// Agent Plugins versions this client implements; 1.1.0 is compatible with 1.0.0.
const PLUGIN_SCHEMAS: &[&str] = &[
    "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
    "https://agent-plugins.org/schemas/1.1.0/plugin.schema.json",
];
const MCP_SCHEMAS: &[&str] = &[
    "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
    "https://agent-plugins.org/schemas/1.1.0/mcp.schema.json",
];
/// The Agent Plugins fields allowed at the top of `plugin.json`.
const AGENT_PLUGINS_FIELDS: &[&str] = &[
    "$schema",
    "name",
    "version",
    "description",
    "author",
    "homepage",
    "repository",
    "license",
    "keywords",
    "extensions",
];
/// malac's Agent Plugins extension namespace (malac.io).
pub const EXTENSION: &str = "io.malac";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    AgentPlugins,
    ClaudeCode,
    /// No manifest malac recognizes; the standard directories only.
    Plain,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Metadata {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

/// A slash command a plugin provides.
#[derive(Debug, Clone)]
pub enum CommandSource {
    /// A `.md` (Claude Code) or `.toml` (Gemini CLI) file.
    File(PathBuf),
    /// Declared in the manifest's `commands` map.
    Inline {
        name: String,
        content: String,
        description: Option<String>,
        argument_hint: Option<String>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Components {
    /// `SKILL.md` files.
    pub skills: Vec<PathBuf>,
    pub commands: Vec<CommandSource>,
    /// Agent `.md` files.
    pub agents: Vec<PathBuf>,
    pub mcp: BTreeMap<String, McpServerConfig>,
    pub hooks: Vec<HookSpec>,
    /// Directories of scripts and executables it ships (`skills/*/scripts`, `bin`, `scripts`).
    pub scripts: Vec<PathBuf>,
    /// What it declares that malac does not run, e.g. `hook event Stop`.
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Parsed {
    pub meta: Metadata,
    pub formats: Vec<Format>,
    pub components: Components,
    pub warnings: Vec<String>,
}

/// Reads the plugin at `root`. `entry` is its marketplace entry, which
/// stands in for a missing manifest and may add components. `data` is the
/// plugin's writable data directory, for `${PLUGIN_DATA}`.
pub fn parse(root: &Path, data: &Path, entry: Option<&Value>) -> Result<Parsed> {
    let root = std::fs::canonicalize(root)
        .with_context(|| format!("plugin directory {} not found", root.display()))?;
    let mut warnings = Vec::new();
    let mut formats = Vec::new();

    let agent_manifest = read_json(&root, "plugin.json")?;
    let strict = match agent_manifest.as_ref() {
        Some(manifest) => match manifest.get("$schema").and_then(Value::as_str) {
            Some(schema) if PLUGIN_SCHEMAS.contains(&schema) => {
                check_agent_manifest(manifest, &mut warnings)?;
                formats.push(Format::AgentPlugins);
                Some(schema_version(schema))
            }
            Some(schema) if schema.starts_with("https://agent-plugins.org/") => {
                bail!("unsupported Agent Plugins version: {schema}")
            }
            _ => None,
        },
        None => None,
    };
    let claude = read_json(&root, ".claude-plugin/plugin.json")?;
    if claude.is_some() {
        formats.push(Format::ClaudeCode);
    }
    if formats.is_empty() {
        formats.push(Format::Plain);
    }

    // Metadata: the portable manifest, then Claude Code's, then the entry.
    let layers: Vec<&Value> = [
        strict.and(agent_manifest.as_ref()),
        claude.as_ref(),
        agent_manifest.as_ref(),
        entry,
    ]
    .into_iter()
    .flatten()
    .collect();
    let text = |key: &str| {
        layers
            .iter()
            .find_map(|l| l.get(key)?.as_str().map(str::to_owned))
    };
    let dir_name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "plugin".into());
    let meta = Metadata {
        name: text("name").unwrap_or(dir_name),
        version: text("version"),
        description: text("description"),
        author: layers.iter().find_map(|l| match l.get("author")? {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => o.get("name")?.as_str().map(str::to_owned),
            _ => None,
        }),
        homepage: text("homepage"),
        repository: text("repository"),
        license: text("license"),
    };
    if meta.name.contains([':', '@', '/', '\\', ' ']) {
        bail!("invalid plugin name `{}`", meta.name);
    }

    let extension_dir = root.join(EXTENSION);
    let extension_dir = extension_dir.is_dir().then_some(extension_dir);
    // Claude Code component fields come from its manifest, the entry (when
    // there is no manifest of its own) and malac's extension data.
    let mut component_layers: Vec<&Value> = Vec::new();
    if let Some(claude) = &claude {
        component_layers.push(claude);
    }
    if let Some(extension) = agent_manifest
        .as_ref()
        .and_then(|m| m.get("extensions")?.get(EXTENSION))
    {
        component_layers.push(extension);
    }
    let has_manifest = claude.is_some() || agent_manifest.is_some();
    if let Some(entry) = entry {
        component_layers.push(entry);
    }

    let mut components = Components::default();
    collect_skills(&root, &component_layers, &mut components, &mut warnings);
    collect_commands(
        &root,
        &component_layers,
        extension_dir.as_deref(),
        &mut components,
        &mut warnings,
    );
    collect_agents(
        &root,
        &component_layers,
        extension_dir.as_deref(),
        &mut components,
        &mut warnings,
    );
    collect_hooks(
        &root,
        &component_layers,
        extension_dir.as_deref(),
        &mut components,
        &mut warnings,
    );
    let ctx = McpContext {
        root: &root,
        data,
        plugin_version: strict,
    };
    collect_mcp(
        &ctx,
        &component_layers,
        has_manifest,
        entry,
        &mut components,
        &mut warnings,
    );
    for dir in ["bin", "scripts"] {
        if root.join(dir).is_dir() {
            components.scripts.push(root.join(dir));
        }
    }
    for skill in &components.skills {
        if let Some(scripts) = skill
            .parent()
            .map(|d| d.join("scripts"))
            .filter(|d| d.is_dir())
        {
            components.scripts.push(scripts);
        }
    }
    for key in [
        "outputStyles",
        "lspServers",
        "channels",
        "userConfig",
        "monitors",
    ] {
        if component_layers.iter().any(|l| l.get(key).is_some()) {
            components.unsupported.push(key.to_owned());
        }
    }
    Ok(Parsed {
        meta,
        formats,
        components,
        warnings,
    })
}

fn read_json(root: &Path, rel: &str) -> Result<Option<Value>> {
    let path = root.join(rel);
    if !path.is_file() {
        return Ok(None);
    }
    contained(root, &path).with_context(|| format!("{rel} resolves outside the plugin"))?;
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    if !value.is_object() {
        bail!("{} is not a JSON object", path.display());
    }
    Ok(Some(value))
}

/// `1.0.0` from a schema URL.
fn schema_version(schema: &str) -> &'static str {
    if schema.contains("/1.1.0/") {
        "1.1.0"
    } else {
        "1.0.0"
    }
}

/// The closed Agent Plugins manifest: unknown fields are reported and
/// ignored, a wrong type or name rejects the plugin.
fn check_agent_manifest(manifest: &Value, warnings: &mut Vec<String>) -> Result<()> {
    let object = manifest
        .as_object()
        .context("plugin.json is not an object")?;
    for key in object.keys() {
        if !AGENT_PLUGINS_FIELDS.contains(&key.as_str()) {
            warnings.push(format!("plugin.json: ignored unknown field `{key}`"));
        }
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .context("plugin.json: `name` is required")?;
    if !valid_plugin_name(name) {
        bail!(
            "plugin.json: invalid name `{name}` (1-64 of a-z, 0-9, `-` and `.`, alphanumeric at both ends)"
        );
    }
    for key in [
        "version",
        "description",
        "homepage",
        "repository",
        "license",
    ] {
        if object.get(key).is_some_and(|v| !v.is_string()) {
            bail!("plugin.json: `{key}` must be a string");
        }
    }
    if let Some(author) = object.get("author") {
        let valid = author.as_object().is_some_and(|a| {
            a.iter()
                .all(|(k, v)| matches!(k.as_str(), "name" | "email" | "url") && v.is_string())
        });
        if !valid {
            bail!("plugin.json: `author` must be an object with string `name`, `email` and `url`");
        }
    }
    if let Some(keywords) = object.get("keywords") {
        if !keywords
            .as_array()
            .is_some_and(|k| k.iter().all(Value::is_string))
        {
            bail!("plugin.json: `keywords` must be an array of strings");
        }
    }
    if object.get("extensions").is_some_and(|e| !e.is_object()) {
        warnings.push("plugin.json: ignored `extensions`, which is not an object".into());
    }
    Ok(())
}

/// Agent Plugins name rules.
pub fn valid_plugin_name(name: &str) -> bool {
    let chars_ok = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.');
    let ends_ok = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    (1..=64).contains(&name.len())
        && chars_ok
        && ends_ok
        && !name.contains("--")
        && !name.contains("..")
}

/// The canonical path of `path` if it exists inside `root`.
pub fn contained(root: &Path, path: &Path) -> Option<PathBuf> {
    let resolved = std::fs::canonicalize(path).ok()?;
    resolved.starts_with(root).then_some(resolved)
}

/// A manifest path (`./x`, also accepted without `./`) inside the plugin.
fn resolve(root: &Path, rel: &str, warnings: &mut Vec<String>) -> Option<PathBuf> {
    let trimmed = rel.trim_start_matches("./");
    let trimmed = if trimmed.is_empty() || trimmed == "." {
        "."
    } else {
        trimmed
    };
    if Path::new(trimmed).is_absolute() {
        warnings.push(format!("ignored absolute path `{rel}`"));
        return None;
    }
    match contained(root, &root.join(trimmed)) {
        Some(path) => Some(path),
        None if root.join(trimmed).exists() => {
            warnings.push(format!("ignored `{rel}`: it resolves outside the plugin"));
            None
        }
        None => {
            warnings.push(format!("`{rel}` not found"));
            None
        }
    }
}

/// Strings of a path-or-array field.
fn paths(value: &Value) -> Vec<&str> {
    match value {
        Value::String(s) => vec![s.as_str()],
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn collect_skills(
    root: &Path,
    layers: &[&Value],
    components: &mut Components,
    warnings: &mut Vec<String>,
) {
    let mut dirs = vec![root.join("skills")];
    let mut declared = false;
    for layer in layers {
        if let Some(value) = layer.get("skills") {
            declared = true;
            dirs.extend(
                paths(value)
                    .iter()
                    .filter_map(|p| resolve(root, p, warnings)),
            );
        }
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let Some(dir) = contained(root, &dir) else {
            continue;
        };
        if !dir.is_dir() {
            continue;
        }
        // A directory holding SKILL.md is one skill; otherwise each child is.
        if dir != root && dir.join("SKILL.md").is_file() {
            found.extend(contained(root, &dir.join("SKILL.md")));
            continue;
        }
        let mut children: Vec<PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path().join("SKILL.md"))
            .filter(|p| p.is_file())
            .filter_map(|p| contained(root, &p))
            .collect();
        children.sort();
        found.extend(children);
    }
    // A plugin that is just a skill.
    if found.is_empty() && !declared && root.join("SKILL.md").is_file() {
        found.push(root.join("SKILL.md"));
    }
    components.skills = unique(found);
}

fn collect_commands(
    root: &Path,
    layers: &[&Value],
    extension_dir: Option<&Path>,
    components: &mut Components,
    warnings: &mut Vec<String>,
) {
    let mut sources: Vec<PathBuf> = Vec::new();
    let mut declared = false;
    for layer in layers {
        match layer.get("commands") {
            Some(Value::Object(map)) => {
                declared = true;
                for (name, spec) in map {
                    let description = spec
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let argument_hint = spec
                        .get("argumentHint")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    if let Some(source) = spec.get("source").and_then(Value::as_str) {
                        if let Some(path) = resolve(root, source, warnings) {
                            components.commands.push(CommandSource::Inline {
                                name: name.clone(),
                                content: std::fs::read_to_string(&path).unwrap_or_default(),
                                description,
                                argument_hint,
                            });
                        }
                    } else if let Some(content) = spec.get("content").and_then(Value::as_str) {
                        components.commands.push(CommandSource::Inline {
                            name: name.clone(),
                            content: content.to_owned(),
                            description,
                            argument_hint,
                        });
                    }
                }
            }
            Some(value) => {
                declared = true;
                sources.extend(
                    paths(value)
                        .iter()
                        .filter_map(|p| resolve(root, p, warnings)),
                );
            }
            None => {}
        }
    }
    if !declared {
        sources.push(root.join("commands"));
    }
    sources.extend(extension_dir.map(|d| d.join("commands")));
    for source in sources {
        if source.is_file() {
            components.commands.push(CommandSource::File(source));
        } else if source.is_dir() {
            for file in files_with(&source, &["md", "toml"], 2) {
                if let Some(file) = contained(root, &file) {
                    components.commands.push(CommandSource::File(file));
                }
            }
        }
    }
}

fn collect_agents(
    root: &Path,
    layers: &[&Value],
    extension_dir: Option<&Path>,
    components: &mut Components,
    warnings: &mut Vec<String>,
) {
    let mut agents = Vec::new();
    let mut declared = false;
    for layer in layers {
        if let Some(value) = layer.get("agents") {
            declared = true;
            agents.extend(
                paths(value)
                    .iter()
                    .filter_map(|p| resolve(root, p, warnings)),
            );
        }
    }
    let mut dirs = Vec::new();
    if !declared {
        dirs.push(root.join("agents"));
    }
    dirs.extend(extension_dir.map(|d| d.join("agents")));
    for dir in dirs.iter().filter(|d| d.is_dir()) {
        agents.extend(
            files_with(dir, &["md"], 3)
                .into_iter()
                .filter_map(|f| contained(root, &f)),
        );
    }
    agents.retain(|p| p.is_file());
    components.agents = agents;
}

fn collect_hooks(
    root: &Path,
    layers: &[&Value],
    extension_dir: Option<&Path>,
    components: &mut Components,
    warnings: &mut Vec<String>,
) {
    let mut files = vec![root.join("hooks/hooks.json")];
    files.extend(extension_dir.map(|d| d.join("hooks.json")));
    let mut inline: Vec<&Value> = Vec::new();
    for layer in layers {
        let Some(value) = layer.get("hooks") else {
            continue;
        };
        let items: Vec<&Value> = match value {
            Value::Array(items) => items.iter().collect(),
            other => vec![other],
        };
        for item in items {
            match item {
                Value::String(path) => files.extend(resolve(root, path, warnings)),
                Value::Object(_) => inline.push(item),
                _ => {}
            }
        }
    }
    let files = unique(files.iter().filter_map(|f| contained(root, f)).collect());
    for file in files.iter().filter(|f| f.is_file()) {
        let parsed = std::fs::read_to_string(file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok());
        match parsed {
            Some(value) => {
                let events = value.get("hooks").unwrap_or(&Value::Null);
                if !events.is_object() && value.get("modules").is_some() {
                    components
                        .unsupported
                        .push(format!("{} (Claude Code mods)", file.display()));
                    continue;
                }
                let (specs, unsupported) = hooks::parse(events);
                components.hooks.extend(specs);
                components.unsupported.extend(unsupported);
            }
            None => warnings.push(format!("{} is not valid JSON", file.display())),
        }
    }
    for value in inline {
        let (specs, unsupported) = hooks::parse(value);
        components.hooks.extend(specs);
        components.unsupported.extend(unsupported);
    }
    components.unsupported = unique(std::mem::take(&mut components.unsupported));
}

/// Drops repeats, keeping the first of each.
fn unique<T: Clone + Eq + std::hash::Hash>(items: Vec<T>) -> Vec<T> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|i| seen.insert(i.clone()))
        .collect()
}

struct McpContext<'a> {
    root: &'a Path,
    data: &'a Path,
    /// The Agent Plugins version of `plugin.json`, when it is a strict one.
    plugin_version: Option<&'static str>,
}

fn collect_mcp(
    ctx: &McpContext<'_>,
    layers: &[&Value],
    has_manifest: bool,
    entry: Option<&Value>,
    components: &mut Components,
    warnings: &mut Vec<String>,
) {
    // Agent Plugins `mcp.json`.
    match read_json(ctx.root, "mcp.json") {
        Ok(Some(config)) => {
            let schema = config.get("$schema").and_then(Value::as_str);
            match (schema, ctx.plugin_version) {
                (Some(schema), Some(version)) if MCP_SCHEMAS.contains(&schema) => {
                    if schema_version(schema) != version {
                        warnings.push(format!(
                            "mcp.json targets Agent Plugins {}, plugin.json {version}; MCP servers disabled",
                            schema_version(schema)
                        ));
                    } else {
                        agent_plugins_mcp(ctx, &config, components, warnings);
                    }
                }
                (Some(schema), _) if MCP_SCHEMAS.contains(&schema) => {
                    agent_plugins_mcp(ctx, &config, components, warnings);
                }
                (Some(schema), _) => warnings.push(format!(
                    "mcp.json: unsupported $schema {schema}; MCP servers disabled"
                )),
                // Without a schema it is a client-native file; read it like `.mcp.json`.
                (None, _) => claude_mcp(ctx, &config, components, warnings),
            }
        }
        Ok(None) => {}
        Err(err) => warnings.push(format!("mcp.json: {err:#}; MCP servers disabled")),
    }
    // Claude Code `.mcp.json` and manifest `mcpServers`.
    match read_json(ctx.root, ".mcp.json") {
        Ok(Some(config)) => claude_mcp(ctx, &config, components, warnings),
        Ok(None) => {}
        Err(err) => warnings.push(format!(".mcp.json: {err:#}")),
    }
    for layer in layers {
        // An entry's servers apply only to a plugin without a manifest.
        if has_manifest && entry.is_some_and(|e| std::ptr::eq(*layer, e)) {
            continue;
        }
        let Some(value) = layer.get("mcpServers") else {
            continue;
        };
        let items: Vec<&Value> = match value {
            Value::Array(items) => items.iter().collect(),
            other => vec![other],
        };
        for item in items {
            match item {
                Value::String(path) if path.ends_with(".json") => {
                    let config = resolve(ctx.root, path, warnings)
                        .and_then(|p| std::fs::read_to_string(p).ok())
                        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
                    match config {
                        Some(config) => claude_mcp(ctx, &config, components, warnings),
                        None => warnings.push(format!("cannot read MCP config {path}")),
                    }
                }
                Value::String(path) => components.unsupported.push(format!("MCP bundle {path}")),
                Value::Object(_) => claude_mcp(ctx, item, components, warnings),
                _ => {}
            }
        }
    }
}

/// Servers of an Agent Plugins `mcp.json`: closed variants, `./` commands
/// inside the plugin, `${PLUGIN_ROOT}`/`${PLUGIN_DATA}` expanded and no
/// other expansion.
fn agent_plugins_mcp(
    ctx: &McpContext<'_>,
    config: &Value,
    components: &mut Components,
    warnings: &mut Vec<String>,
) {
    let Some(object) = config.as_object() else {
        return;
    };
    if object.keys().any(|k| k != "$schema" && k != "mcpServers") {
        warnings.push(
            "mcp.json has fields besides $schema and mcpServers; MCP servers disabled".into(),
        );
        return;
    }
    let Some(servers) = object.get("mcpServers").and_then(Value::as_object) else {
        warnings.push("mcp.json: `mcpServers` must be an object; MCP servers disabled".into());
        return;
    };
    for (name, server) in servers {
        match agent_plugins_server(ctx, server) {
            Ok(Some(config)) => {
                components.mcp.insert(name.clone(), config);
            }
            Ok(None) => components
                .unsupported
                .push(format!("MCP server {name} (legacy SSE transport)")),
            Err(err) => warnings.push(format!("mcp.json: skipped server `{name}`: {err:#}")),
        }
    }
}

fn agent_plugins_server(ctx: &McpContext<'_>, server: &Value) -> Result<Option<McpServerConfig>> {
    let object = server.as_object().context("not an object")?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .context("`type` is required")?;
    let allowed: &[&str] = match kind {
        "stdio" => &["type", "command", "args", "env", "cwd"],
        "streamable-http" | "sse" => &["type", "url", "headers"],
        other => bail!("unknown type `{other}`"),
    };
    if let Some(key) = object.keys().find(|k| !allowed.contains(&k.as_str())) {
        bail!("field `{key}` is not allowed for {kind}");
    }
    let root = ctx.root.display().to_string();
    let data = ctx.data.display().to_string();
    let expand = |s: &str| {
        s.replace("${PLUGIN_ROOT}", &root)
            .replace("${PLUGIN_DATA}", &data)
    };
    match kind {
        "stdio" => {
            let command = object
                .get("command")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty());
            let command = command.context("`command` is required")?;
            let command = if let Some(rel) = command.strip_prefix("./") {
                contained(ctx.root, &ctx.root.join(rel))
                    .context("`command` must stay inside the plugin")?
                    .display()
                    .to_string()
            } else if command.contains('/') || command.contains('\\') {
                bail!("`command` must be a bare name or start with ./");
            } else {
                command.to_owned()
            };
            let args = string_list(object.get("args"))?
                .iter()
                .map(|a| expand(a))
                .collect();
            let mut env = string_map(object.get("env"))?;
            if env.contains_key("PLUGIN_ROOT") || env.contains_key("PLUGIN_DATA") {
                bail!("`env` must not set PLUGIN_ROOT or PLUGIN_DATA");
            }
            for value in env.values_mut() {
                *value = expand(value);
            }
            env.insert("PLUGIN_ROOT".into(), root.clone());
            env.insert("PLUGIN_DATA".into(), data.clone());
            let cwd = match object.get("cwd").and_then(Value::as_str) {
                None => ctx.root.to_path_buf(),
                Some(cwd) => plugin_cwd(ctx, cwd)?,
            };
            Ok(Some(McpServerConfig {
                command: Some(command),
                args,
                env,
                cwd: Some(cwd),
                literal: true,
                ..Default::default()
            }))
        }
        "sse" => Ok(None),
        _ => {
            let url = object
                .get("url")
                .and_then(Value::as_str)
                .context("`url` is required")?;
            check_remote_url(url)?;
            Ok(Some(McpServerConfig {
                url: Some(url.to_owned()),
                headers: string_map(object.get("headers"))?,
                literal: true,
                ..Default::default()
            }))
        }
    }
}

/// `./x`, `${PLUGIN_ROOT}/x` or `${PLUGIN_DATA}/x`, staying inside the root it names.
fn plugin_cwd(ctx: &McpContext<'_>, cwd: &str) -> Result<PathBuf> {
    let (base, rest) = if let Some(rest) = cwd.strip_prefix("./") {
        (ctx.root, rest)
    } else if let Some(rest) = cwd.strip_prefix("${PLUGIN_ROOT}") {
        (ctx.root, rest.trim_start_matches('/'))
    } else if let Some(rest) = cwd.strip_prefix("${PLUGIN_DATA}") {
        std::fs::create_dir_all(ctx.data)?;
        (ctx.data, rest.trim_start_matches('/'))
    } else {
        bail!("`cwd` must start with ./, ${{PLUGIN_ROOT}} or ${{PLUGIN_DATA}}");
    };
    let base = std::fs::canonicalize(base)?;
    let path = if rest.is_empty() {
        base.clone()
    } else {
        base.join(rest)
    };
    contained(&base, &path).context("`cwd` must stay inside the plugin or its data directory")
}

/// HTTPS, or HTTP to a loopback host; no credentials or fragment.
fn check_remote_url(url: &str) -> Result<()> {
    let (scheme, rest) = url.split_once("://").context("`url` must be absolute")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') || url.contains('#') {
        bail!("`url` must not contain credentials or a fragment");
    }
    let host = authority.rsplit_once(':').map_or(authority, |(h, _)| h);
    let loopback = matches!(host, "localhost" | "127.0.0.1" | "[::1]") || host.starts_with("127.");
    match scheme {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => bail!("`url` must use https (http only for loopback hosts)"),
    }
}

/// Servers of a Claude Code `.mcp.json` (or an inline `mcpServers` map):
/// `${CLAUDE_PLUGIN_ROOT}` and `${CLAUDE_PLUGIN_DATA}` are written in, other
/// `${VAR}` references come from the environment when the server starts.
fn claude_mcp(
    ctx: &McpContext<'_>,
    config: &Value,
    components: &mut Components,
    warnings: &mut Vec<String>,
) {
    let servers = config
        .get("mcpServers")
        .and_then(Value::as_object)
        .or_else(|| config.as_object().filter(|o| !o.contains_key("mcpServers")));
    let Some(servers) = servers else {
        return;
    };
    for (name, server) in servers {
        match claude_server(ctx, server) {
            Ok(Some(config)) => {
                components.mcp.insert(name.clone(), config);
            }
            Ok(None) => components
                .unsupported
                .push(format!("MCP server {name} (SSE transport)")),
            Err(err) => warnings.push(format!("skipped MCP server `{name}`: {err:#}")),
        }
    }
}

fn claude_server(ctx: &McpContext<'_>, server: &Value) -> Result<Option<McpServerConfig>> {
    let object: &Map<String, Value> = server.as_object().context("not an object")?;
    let root = ctx.root.display().to_string();
    let data = ctx.data.display().to_string();
    let expand = |s: &str| crate::catalog::substitute_plugin(s, Path::new(&root), Path::new(&data));
    let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
    if let Some(command) = object.get("command").and_then(Value::as_str) {
        let mut env = string_map(object.get("env"))?;
        for value in env.values_mut() {
            *value = expand(value);
        }
        env.insert("CLAUDE_PLUGIN_ROOT".into(), root.clone());
        env.insert("CLAUDE_PLUGIN_DATA".into(), data.clone());
        return Ok(Some(McpServerConfig {
            command: Some(expand(command)),
            args: string_list(object.get("args"))?
                .iter()
                .map(|a| expand(a))
                .collect(),
            env,
            cwd: object
                .get("cwd")
                .and_then(Value::as_str)
                .map(|c| PathBuf::from(expand(c))),
            ..Default::default()
        }));
    }
    if kind == "sse" {
        return Ok(None);
    }
    let url = object
        .get("url")
        .and_then(Value::as_str)
        .context("needs `command` or `url`")?;
    Ok(Some(McpServerConfig {
        url: Some(expand(url)),
        headers: string_map(object.get("headers"))?
            .into_iter()
            .map(|(k, v)| (k, expand(&v)))
            .collect(),
        ..Default::default()
    }))
}

fn string_list(value: Option<&Value>) -> Result<Vec<String>> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| i.as_str().map(str::to_owned).context("expected strings"))
            .collect(),
        Some(_) => bail!("expected an array of strings"),
    }
}

fn string_map(value: Option<&Value>) -> Result<BTreeMap<String, String>> {
    match value {
        None => Ok(BTreeMap::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| {
                Ok((
                    k.clone(),
                    v.as_str().context("expected string values")?.to_owned(),
                ))
            })
            .collect(),
        Some(_) => bail!("expected an object of strings"),
    }
}

/// Files with one of `extensions` under `dir`, up to `depth` levels deep.
fn files_with(dir: &Path, extensions: &[&str], depth: usize) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = ignore::WalkBuilder::new(dir)
        .max_depth(Some(depth))
        .require_git(false)
        .hidden(true)
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.contains(&e))
        })
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn reads_a_claude_code_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            ".claude-plugin/plugin.json",
            r#"{"name": "deploy-tools", "version": "1.2.0", "description": "Deploys", "author": {"name": "Ana"},
                "hooks": "./config/extra-hooks.json",
                "mcpServers": {"api": {"command": "node", "args": ["${CLAUDE_PLUGIN_ROOT}/server.js"], "env": {"TOKEN": "${API_TOKEN}"}}},
                "outputStyles": "./styles/"}"#,
        );
        write(
            root,
            "skills/deploy/SKILL.md",
            "---\nname: deploy\ndescription: Deploy it\n---\nGo.",
        );
        write(
            root,
            "commands/status.md",
            "---\ndescription: Status\n---\nShow status for $ARGUMENTS",
        );
        write(
            root,
            "agents/reviewer.md",
            "---\nname: reviewer\n---\nReview.",
        );
        write(
            root,
            "hooks/hooks.json",
            r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "./check.sh"}]}],
                          "Stop": [{"hooks": [{"type": "command", "command": "true"}]}]}}"#,
        );
        write(
            root,
            "config/extra-hooks.json",
            r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "echo hi", "timeout": 5}]}]}}"#,
        );
        write(
            root,
            ".mcp.json",
            r#"{"mcpServers": {"db": {"type": "http", "url": "https://db.example/mcp"}}}"#,
        );

        let data = root.join("data");
        let parsed = parse(root, &data, None).unwrap();
        assert_eq!(parsed.formats, vec![Format::ClaudeCode]);
        assert_eq!(parsed.meta.name, "deploy-tools");
        assert_eq!(parsed.meta.author.as_deref(), Some("Ana"));
        let c = &parsed.components;
        assert_eq!(c.skills.len(), 1);
        assert_eq!(c.commands.len(), 1);
        assert_eq!(c.agents.len(), 1);
        let events: Vec<&str> = c.hooks.iter().map(|h| h.event.as_str()).collect();
        assert_eq!(events, vec!["PreToolUse", "SessionStart"]);
        assert!(c.unsupported.contains(&"hook event Stop".to_owned()));
        assert!(c.unsupported.contains(&"outputStyles".to_owned()));
        let api = &c.mcp["api"];
        let root_text = std::fs::canonicalize(root).unwrap().display().to_string();
        assert_eq!(api.args, vec![format!("{root_text}/server.js")]);
        assert_eq!(api.env["TOKEN"], "${API_TOKEN}", "left for the environment");
        assert!(!api.literal);
        assert_eq!(c.mcp["db"].url.as_deref(), Some("https://db.example/mcp"));
    }

    #[test]
    fn reads_an_agent_plugins_package() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "plugin.json",
            r#"{"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json", "name": "reports.tools",
                "version": "1.0.0", "author": {"name": "A", "email": "a@x"}, "displayName": "Reports",
                "extensions": {"io.malac": {"commands": {"about": {"content": "Explain reports.", "description": "About"}}},
                               "com.other": {"anything": [1, 2]}}}"#,
        );
        write(
            root,
            "skills/summarize/SKILL.md",
            "---\nname: summarize\ndescription: Summarize\n---\n",
        );
        write(root, "skills/summarize/scripts/run.sh", "echo");
        write(
            root,
            "skills/nested/deeper/SKILL.md",
            "---\nname: deeper\ndescription: x\n---\n",
        );
        write(root, "bin/server", "#!/bin/sh");
        write(
            root,
            "mcp.json",
            r#"{"$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json", "mcpServers": {
                "local": {"type": "stdio", "command": "./bin/server", "args": ["${PLUGIN_DATA}/db", "${OTHER}"], "cwd": "${PLUGIN_ROOT}"},
                "remote": {"type": "streamable-http", "url": "https://deploy.example.com/mcp", "headers": {"X-Tenant": "${T}"}},
                "plain": {"type": "streamable-http", "url": "http://example.com/mcp"},
                "escape": {"type": "stdio", "command": "../bin/server"},
                "legacy": {"type": "sse", "url": "https://legacy.example.com/sse"},
                "reserved": {"type": "stdio", "command": "node", "env": {"PLUGIN_ROOT": "x"}}}}"#,
        );
        write(
            root,
            "io.malac/hooks.json",
            r#"{"hooks": {"PostToolUse": [{"matcher": "Write|Edit", "hooks": [{"type": "command", "command": "fmt"}]}]}}"#,
        );

        let data = root.join(".data");
        let parsed = parse(root, &data, None).unwrap();
        assert_eq!(parsed.formats, vec![Format::AgentPlugins]);
        assert_eq!(parsed.meta.name, "reports.tools");
        assert!(parsed.warnings.iter().any(|w| w.contains("displayName")));
        let c = &parsed.components;
        assert_eq!(c.skills.len(), 1, "only immediate children of skills/");
        assert!(matches!(&c.commands[0], CommandSource::Inline { name, .. } if name == "about"));
        assert_eq!(c.hooks.len(), 1);
        let root_text = std::fs::canonicalize(root).unwrap().display().to_string();
        let local = &c.mcp["local"];
        assert_eq!(
            local.command.as_deref(),
            Some(format!("{root_text}/bin/server").as_str())
        );
        assert_eq!(
            local.args,
            vec![format!("{}/db", data.display()), "${OTHER}".to_owned()]
        );
        assert_eq!(local.env["PLUGIN_ROOT"], root_text);
        assert!(local.literal);
        assert_eq!(c.mcp["remote"].headers["X-Tenant"], "${T}");
        for skipped in ["plain", "escape", "reserved"] {
            assert!(!c.mcp.contains_key(skipped), "{skipped}");
            assert!(
                parsed
                    .warnings
                    .iter()
                    .any(|w| w.contains(&format!("`{skipped}`")))
            );
        }
        assert!(c.unsupported.iter().any(|u| u.contains("legacy")));
        assert_eq!(c.scripts.len(), 2);
    }

    #[test]
    fn rejects_invalid_agent_plugins_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "plugin.json",
            r#"{"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json", "name": "Bad--Name"}"#,
        );
        assert!(parse(root, root, None).is_err());
        write(
            root,
            "plugin.json",
            r#"{"$schema": "https://agent-plugins.org/schemas/9.0.0/plugin.schema.json", "name": "ok"}"#,
        );
        assert!(
            parse(root, root, None)
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
        write(
            root,
            "plugin.json",
            r#"{"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json", "name": "ok", "author": {"name": 3}}"#,
        );
        assert!(parse(root, root, None).is_err());
    }

    #[test]
    fn a_marketplace_entry_stands_in_for_a_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "document-skills/xlsx/SKILL.md",
            "---\nname: xlsx\ndescription: Sheets\n---\n",
        );
        write(
            root,
            "document-skills/pdf/SKILL.md",
            "---\nname: pdf\ndescription: PDFs\n---\n",
        );
        let entry = serde_json::json!({"name": "document-skills", "description": "Docs", "strict": false,
                                       "skills": ["./document-skills/xlsx", "./document-skills/pdf"]});
        let parsed = parse(root, root, Some(&entry)).unwrap();
        assert_eq!(parsed.formats, vec![Format::Plain]);
        assert_eq!(parsed.meta.name, "document-skills");
        assert_eq!(parsed.components.skills.len(), 2);
    }
}
