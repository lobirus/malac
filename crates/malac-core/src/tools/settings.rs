//! Lets the agent look at and change malac's own configuration, so users can
//! configure malac by asking it.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use toml_edit::{DocumentMut, Item, TableLike};

use super::{Tool, ToolContext, ToolKind, ToolOutput, str_arg};
use crate::config;
use crate::event::Event;
use crate::permission::{Decision, PermissionRequest};
use crate::plugins::{Manager, NeedsApproval, Scope};
use crate::update;

const REFERENCE: &str = include_str!("../../../../docs/configuration.md");

pub struct Settings;

#[async_trait]
impl Tool for Settings {
    fn name(&self) -> &str {
        "malac_settings"
    }

    fn description(&self) -> String {
        "Inspect malac itself: `config` shows the user and project config files and the merged result, \
         `reference` documents every setting, `models` lists the models each provider offers, `status` \
         shows the version, available updates and tool servers, `context` the instruction files, docs, \
         project index, skills, commands and plugins this session uses, `plugins` the installed plugins and \
         known marketplaces. `use_model` switches this session's model from the next message on. Use \
         `malac_configure` to change settings and manage plugins."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "show": { "type": "string", "enum": ["config", "reference", "models", "status", "context", "plugins"] },
                "use_model": { "type": "string", "description": "provider/model to switch this session to" }
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        inspect(&input, ctx).await.into()
    }
}

async fn inspect(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let runtime = &ctx.runtime;
    if let Some(spec) = input["use_model"].as_str() {
        let (provider, model) = runtime.config.resolve_model(spec)?;
        let spec = format!("{provider}/{model}");
        let agent = runtime
            .agent(&ctx.agent)
            .context("this agent is not registered")?;
        agent.set_model(&spec);
        return Ok(ToolOutput::text(format!(
            "This session switches to {spec} from the next message."
        )));
    }
    let text = match input["show"].as_str().unwrap_or("config") {
        "config" => {
            let keys: Vec<&String> = runtime
                .config
                .providers
                .iter()
                .filter(|(name, p)| config::api_key(name, p).is_some())
                .map(|(name, _)| name)
                .collect();
            let file = |path: PathBuf| {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let body = if text.trim().is_empty() {
                    "(empty)".to_owned()
                } else {
                    text
                };
                format!("{}:\n```toml\n{}\n```", path.display(), body.trim_end())
            };
            format!(
                "User config {}\n\nProject config {}\n\nMerged configuration, defaults included:\n```toml\n{}```\n\n\
                 API keys are stored for: {} (values are never shown).",
                file(config::user_config_path()),
                file(project_config(ctx)),
                toml::to_string_pretty(&runtime.config)
                    .context("cannot encode the configuration")?,
                if keys.is_empty() {
                    "none".to_owned()
                } else {
                    keys.iter()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            )
        }
        "reference" => REFERENCE.to_owned(),
        "models" => {
            let mut out = Vec::new();
            for provider in crate::provider::discover_models(&runtime.config).await {
                let models = provider.all();
                let note = provider
                    .error
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default();
                out.push(format!(
                    "{}{note}: {}",
                    provider.provider,
                    models.join(", ")
                ));
            }
            format!(
                "Default model: {}\n{}",
                runtime.config.model.as_deref().unwrap_or("none"),
                out.join("\n")
            )
        }
        "status" => {
            let status = update::status();
            let servers: Vec<String> = runtime
                .servers()
                .iter()
                .map(|s| match &s.error {
                    Some(err) => format!(
                        "{}: unavailable ({})",
                        s.name,
                        err.lines().next().unwrap_or_default()
                    ),
                    None => format!("{}: {} tools", s.name, s.tools),
                })
                .collect();
            format!(
                "malac {}\nUpdate: {}\nMode: {}\nTool servers: {}\nLive agents: {}",
                status.current,
                match (&status.installed, &status.latest, status.available) {
                    (Some(installed), _, false) =>
                        format!("{installed} installed; restart to use it"),
                    (_, Some(latest), true) => format!("{latest} available"),
                    _ => "up to date".into(),
                },
                runtime.policy.mode().as_str(),
                if servers.is_empty() {
                    "none".into()
                } else {
                    servers.join("; ")
                },
                runtime.agents().len(),
            )
        }
        "context" => runtime.context_report().render(),
        "plugins" => plugins_text(ctx)?,
        other => bail!("unknown `show` value `{other}`"),
    };
    Ok(ToolOutput::text(text))
}

fn manager(ctx: &ToolContext) -> Manager {
    Manager::new(
        &config::home_dir(),
        Some(&ctx.runtime.project_root),
        &ctx.cwd,
        ctx.runtime.config.plugins.defaults,
    )
}

fn plugins_text(ctx: &ToolContext) -> Result<String> {
    let manager = manager(ctx);
    let mut out = vec!["Plugins:".to_owned()];
    for plugin in manager.list()? {
        let state = if plugin.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let error = plugin.error.map(|e| format!(" ({e})")).unwrap_or_default();
        out.push(format!(
            "- {} {} [{state}]{error}: {}",
            plugin.id,
            plugin.version.unwrap_or_default(),
            plugin.description.unwrap_or_default()
        ));
    }
    out.push("\nMarketplaces:".into());
    for market in manager.list_marketplaces()? {
        let names: Vec<&str> = market.plugins.iter().map(|p| p.name.as_str()).collect();
        out.push(format!(
            "- {} ({}, {}): {}",
            market.name,
            market.scope.as_str(),
            market.source,
            market.error.unwrap_or_else(|| names.join(", "))
        ));
    }
    Ok(out.join("\n"))
}

pub struct Configure;

#[async_trait]
impl Tool for Configure {
    fn name(&self) -> &str {
        "malac_configure"
    }

    fn description(&self) -> String {
        "Change malac's configuration when the user asks. `set`/`unset` edit one key of the user (or project) \
         config file by dotted path, e.g. `providers.openrouter.base_url` or `models.\"lora/smart\".context_window`; \
         comments in the file are kept and the result is validated before saving. Check `malac_settings` \
         (`reference`) for valid keys. `set_api_key` stores a provider key securely (prefer `api_key_env` when \
         the user keeps keys in the environment). `install_update` installs the latest release. Plugins: \
         `marketplace_add` (a `source`: folder, owner/repo, git URL or marketplace.json URL), `marketplace_remove`, \
         `marketplace_update`, `plugin_install`, `plugin_enable`, `plugin_disable`, `plugin_uninstall` and \
         `plugin_update` (a `plugin`: name or name@marketplace); `file` picks the user or project scope. \
         Enabling a plugin that runs code (hooks, MCP servers, scripts) asks the user. Saved settings and \
         plugins apply to new sessions."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": [
                    "set", "unset", "set_api_key", "remove_api_key", "install_update",
                    "marketplace_add", "marketplace_remove", "marketplace_update",
                    "plugin_install", "plugin_enable", "plugin_disable", "plugin_uninstall", "plugin_update"
                ] },
                "file": { "type": "string", "enum": ["user", "project"], "default": "user" },
                "key": { "type": "string", "description": "Dotted path; quote segments containing dots or slashes" },
                "value": { "description": "Any JSON value for `set`" },
                "provider": { "type": "string", "description": "Provider name for the API key actions" },
                "source": { "type": "string", "description": "Marketplace source for marketplace_add" },
                "plugin": { "type": "string", "description": "Plugin or marketplace name for the plugin actions" },
                "api_key": { "type": "string" }
            },
            "required": ["action"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Execute
    }

    fn summary(&self, input: &Value) -> String {
        let action = input["action"].as_str().unwrap_or_default();
        let file = input["file"].as_str().unwrap_or("user");
        let key = input["key"].as_str().unwrap_or_default();
        let provider = input["provider"].as_str().unwrap_or_default();
        match action {
            "set" => format!("{file} config: {key} = {}", input["value"]),
            "unset" => format!("{file} config: remove {key}"),
            "set_api_key" => format!("store the API key for {provider}"),
            "remove_api_key" => format!("remove the API key for {provider}"),
            "marketplace_add" => format!(
                "{file}: add marketplace {}",
                input["source"].as_str().unwrap_or_default()
            ),
            action if action.starts_with("plugin_") || action.starts_with("marketplace_") => {
                format!(
                    "{} {}",
                    action.replace('_', " "),
                    input["plugin"].as_str().unwrap_or("(all)")
                )
            }
            other => other.to_owned(),
        }
    }

    /// `action:key`, so rules like `malac_configure(set:ui.*)` can pre-approve edits.
    fn permission_keys(&self, input: &Value) -> Vec<String> {
        let action = input["action"].as_str().unwrap_or_default();
        let target = input["key"]
            .as_str()
            .or(input["provider"].as_str())
            .or(input["plugin"].as_str())
            .or(input["source"].as_str())
            .unwrap_or_default();
        vec![format!("{action}:{target}")]
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        configure(&input, ctx).await.into()
    }
}

async fn configure(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    match str_arg(input, "action")? {
        action @ ("set" | "unset") => {
            let path = match input["file"].as_str().unwrap_or("user") {
                "user" => config::user_config_path(),
                "project" => project_config(ctx),
                other => bail!("unknown file `{other}` (expected user or project)"),
            };
            let key = parse_key(str_arg(input, "key")?)?;
            let value = (action == "set")
                .then(|| input.get("value").cloned().context("`set` needs a value"))
                .transpose()?;
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let updated = edit(&text, &key, value)?;
            let table: toml::Table = updated.parse().context("the edit produced invalid TOML")?;
            config::validate_user_table(&table)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, updated)
                .with_context(|| format!("cannot write {}", path.display()))?;
            Ok(ToolOutput::text(format!(
                "Saved {}. New sessions use it; running sessions keep their settings until restarted \
                 (the web UI can apply them right away).",
                path.display()
            )))
        }
        "set_api_key" => {
            let provider = str_arg(input, "provider")?;
            let key = str_arg(input, "api_key")?.trim();
            if key.is_empty() {
                bail!("the key is empty");
            }
            config::save_api_key(provider, key)?;
            ctx.runtime.forget_provider(provider);
            Ok(ToolOutput::text(format!(
                "Stored the key for {provider} in {} (readable only by you). It is used from the next request on.",
                config::credentials_path().display()
            )))
        }
        "remove_api_key" => {
            let provider = str_arg(input, "provider")?;
            config::remove_api_key(provider)?;
            ctx.runtime.forget_provider(provider);
            Ok(ToolOutput::text(format!(
                "Removed the stored key for {provider}."
            )))
        }
        "install_update" => {
            let status = ctx.runtime.install_update().await?;
            Ok(ToolOutput::text(format!(
                "Installed malac {}. Running sessions keep version {} until restarted.",
                status.installed.unwrap_or_default(),
                status.current
            )))
        }
        action if action.starts_with("plugin_") || action.starts_with("marketplace_") => {
            manage_plugins(action, input, ctx).await
        }
        other => bail!("unknown action `{other}`"),
    }
}

async fn manage_plugins(action: &str, input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let manager = manager(ctx);
    let scope = match input["file"].as_str().unwrap_or("user") {
        "user" => Scope::User,
        "project" => Scope::Project,
        other => bail!("unknown file `{other}` (expected user or project)"),
    };
    let plugin = input["plugin"].as_str();
    let named = || plugin.context("this action needs `plugin`");
    let applies = "It applies to new sessions; the web UI can apply it now.";
    let text = match action {
        "marketplace_add" => {
            let catalog = manager
                .add_marketplace(str_arg(input, "source")?, scope)
                .await?;
            let names: Vec<&str> = catalog.plugins.iter().map(|p| p.name.as_str()).collect();
            format!(
                "Added the marketplace `{}` with {} plugins: {}. Install one with plugin_install `<plugin>@{}`.",
                catalog.name,
                names.len(),
                names.join(", "),
                catalog.name
            )
        }
        "marketplace_remove" => {
            manager.remove_marketplace(named()?, scope)?;
            format!("Removed the marketplace `{}`.", named()?)
        }
        "marketplace_update" => manager.update_marketplaces(plugin).await?.join("\n"),
        "plugin_install" => {
            let installed = manager.install(named()?).await?;
            let id = installed.id.clone();
            let enabled = enable(&manager, &id, scope, ctx).await?;
            format!(
                "Installed {}.\n{}\n{enabled} {applies}",
                id,
                installed.describe()
            )
        }
        "plugin_enable" => {
            let message = enable(&manager, named()?, scope, ctx).await?;
            format!("{message} {applies}")
        }
        "plugin_disable" => format!("Disabled {}. {applies}", manager.disable(named()?, scope)?),
        "plugin_uninstall" => format!("Uninstalled {}.", manager.uninstall(named()?)?),
        "plugin_update" => manager.update(plugin).await?.join("\n"),
        other => bail!("unknown action `{other}`"),
    };
    Ok(ToolOutput::text(text))
}

/// Enables a plugin, asking the user first, whatever the permission mode,
/// when it runs code that was not approved before.
async fn enable(manager: &Manager, spec: &str, scope: Scope, ctx: &ToolContext) -> Result<String> {
    let needs = match manager.enable(spec, scope, false) {
        Ok(id) => {
            return Ok(format!(
                "Enabled {id} in the {} plugins file.",
                scope.as_str()
            ));
        }
        Err(err) => match err.downcast::<NeedsApproval>() {
            Ok(needs) => needs,
            Err(err) => return Err(err),
        },
    };
    let request = PermissionRequest {
        id: uuid::Uuid::now_v7().to_string(),
        call_id: String::new(),
        agent: ctx.agent.clone(),
        agent_label: ctx
            .runtime
            .agent(&ctx.agent)
            .map(|a| a.info().label.clone())
            .unwrap_or_default(),
        tool: "malac_configure".into(),
        summary: format!(
            "enable plugin {}, which runs {}",
            needs.id,
            needs.parts.join(", ")
        ),
        input: json!({ "plugin": needs.id, "contents": needs.summary }),
    };
    ctx.emit(Event::PermissionRequested {
        request_id: request.id.clone(),
        call_id: request.call_id.clone(),
        tool: request.tool.clone(),
        summary: request.summary.clone(),
        input: request.input.clone(),
    });
    let decision = ctx.runtime.approver().approve(&request).await;
    ctx.emit(Event::PermissionResolved {
        request_id: request.id,
        allowed: decision != Decision::Deny,
    });
    if decision == Decision::Deny {
        bail!(
            "the user did not approve enabling {}; it stays installed but off. They can run `malac plugin enable {}` themselves.\n{}",
            needs.id,
            needs.id,
            needs.summary
        );
    }
    let id = manager.enable(&needs.id, scope, true)?;
    Ok(format!(
        "Enabled {id} in the {} plugins file; the user approved its {}.",
        scope.as_str(),
        needs.parts.join(", ")
    ))
}

fn project_config(ctx: &ToolContext) -> PathBuf {
    ctx.runtime.project_root.join(".malac").join("config.toml")
}

/// Splits `a."b.c".d` into `["a", "b.c", "d"]`.
fn parse_key(key: &str) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in key.chars() {
        match c {
            '"' => quoted = !quoted,
            '.' if !quoted => parts.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    if quoted {
        bail!("unterminated quote in key `{key}`");
    }
    parts.push(current);
    if parts.iter().any(|p| p.trim().is_empty()) {
        bail!("empty segment in key `{key}`");
    }
    Ok(parts.into_iter().map(|p| p.trim().to_owned()).collect())
}

/// Sets (or with `None`, removes) a key, keeping the rest of the document intact.
pub fn edit(text: &str, key: &[String], value: Option<Value>) -> Result<String> {
    let mut doc: DocumentMut = text
        .parse()
        .context("the current config file is not valid TOML")?;
    let (last, parents) = key.split_last().context("empty key")?;
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    for segment in parents {
        if !table.contains_key(segment) {
            if value.is_none() {
                return Ok(doc.to_string());
            }
            let mut created = toml_edit::Table::new();
            created.set_implicit(true);
            table.insert(segment, Item::Table(created));
        }
        table = table
            .get_mut(segment)
            .and_then(Item::as_table_like_mut)
            .with_context(|| format!("`{segment}` is not a table"))?;
    }
    match value {
        Some(value) => {
            let toml_value =
                toml::Value::try_from(value).context("the value cannot be written as TOML")?;
            let mut item: toml_edit::Value = toml_value
                .to_string()
                .parse()
                .context("the value cannot be written as TOML")?;
            // Replace the value in place so the key keeps its comments.
            match table.get_mut(last).and_then(Item::as_value_mut) {
                Some(existing) => {
                    *item.decor_mut() = existing.decor().clone();
                    *existing = item;
                }
                None => {
                    table.insert(last, Item::Value(item));
                }
            }
        }
        None => {
            table.remove(last);
        }
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_keys() {
        assert_eq!(
            parse_key(r#"models."lora/smart".context_window"#).unwrap(),
            vec!["models", "lora/smart", "context_window"]
        );
        assert!(parse_key("a..b").is_err());
        assert!(parse_key(r#"a."b"#).is_err());
    }

    #[test]
    fn edits_keep_comments_and_create_tables() {
        let text = "# my settings\nmodel = \"lora/default\" # main\n\n[providers.lora]\nkind = \"openai\"\n";
        let key = |k: &str| parse_key(k).unwrap();
        let out = edit(text, &key("model"), Some(json!("lora/smart"))).unwrap();
        assert!(out.contains("# my settings"));
        assert!(out.contains("model = \"lora/smart\""));
        let out = edit(
            &out,
            &key(r#"models."lora/smart".context_window"#),
            Some(json!(128000)),
        )
        .unwrap();
        let parsed: toml::Table = out.parse().unwrap();
        assert_eq!(
            parsed["models"]["lora/smart"]["context_window"].as_integer(),
            Some(128000)
        );
        let out = edit(
            &out,
            &key("providers.lora.models"),
            Some(json!(["default", "fast"])),
        )
        .unwrap();
        assert!(out.contains("kind = \"openai\""));
        let out = edit(&out, &key("model"), None).unwrap();
        assert!(!out.contains("lora/smart\" # main"));
        assert!(edit(&out, &key("providers.lora.kind.x"), Some(json!(1))).is_err());
    }
}
