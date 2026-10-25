//! Non-interactive subcommands.

use std::io::Read;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use crossterm::style::Stylize;
use malac_core::config::{self, Config, ProviderKind};
use malac_core::message::Role;
use malac_core::{Block, Runtime, session};

use crate::repl::working_dir;

#[derive(Subcommand)]
pub enum SessionsAction {
    /// List recent sessions (default).
    List {
        /// Include subagent sessions.
        #[arg(long)]
        all: bool,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Print a session's transcript and its subagents.
    Show { id: String },
    /// Delete a session file.
    Delete { id: String },
}

#[derive(Subcommand)]
pub enum ProviderAction {
    /// Add or replace a provider in ~/.malac/config.toml.
    Add {
        name: String,
        #[arg(long)]
        base_url: String,
        /// openai (chat completions) or anthropic (messages API).
        #[arg(long, default_value = "openai")]
        kind: String,
        /// Model to use with --set-default.
        #[arg(long)]
        model: Option<String>,
        /// Read the API key from this environment variable.
        #[arg(long)]
        api_key_env: Option<String>,
        /// Read the API key from stdin and store it in ~/.malac/credentials.toml.
        #[arg(long)]
        api_key_stdin: bool,
        /// Make provider/model the default model.
        #[arg(long)]
        set_default: bool,
    },
    /// List configured providers.
    List,
    /// List the models a provider serves.
    Models { name: String },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Print the configuration file locations.
    Path,
    /// Print the merged configuration for this directory.
    Show {
        #[arg(long)]
        profile: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum MemoryAction {
    /// List saved memories (default).
    List,
    /// Search memories.
    Search { query: Vec<String> },
    /// Re-embed every memory file into the vector store.
    Reindex,
}

pub fn sessions(action: Option<SessionsAction>) -> Result<()> {
    match action.unwrap_or(SessionsAction::List {
        all: false,
        limit: 20,
    }) {
        SessionsAction::List { all, limit } => {
            for s in session::list(all, limit)? {
                let parent = if s.meta.parent.is_some() {
                    format!(" [{}]", s.meta.label)
                } else {
                    String::new()
                };
                println!(
                    "{}  {}  {}{}  {}",
                    s.meta.id.as_str().dark_grey(),
                    s.updated
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M"),
                    s.meta.cwd.display().to_string().dark_grey(),
                    parent,
                    s.title
                );
            }
        }
        SessionsAction::Show { id } => show_session(&id, 0)?,
        SessionsAction::Delete { id } => {
            let path = session::sessions_dir().join(format!("{id}.jsonl"));
            std::fs::remove_file(&path).with_context(|| format!("no session {id}"))?;
            println!("deleted {id}");
        }
    }
    Ok(())
}

fn show_session(id: &str, depth: usize) -> Result<()> {
    let transcript = session::load(id)?;
    let indent = "  ".repeat(depth);
    let meta = &transcript.meta;
    println!(
        "{indent}{} {} · {} · {}",
        meta.label.as_str().bold(),
        meta.id.as_str().dark_grey(),
        meta.model,
        meta.created
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
    );
    for message in &transcript.history {
        for block in &message.content {
            match (message.role, block) {
                (Role::User, Block::Text { text }) => {
                    println!("{indent}{} {}", "›".cyan(), text.trim())
                }
                (Role::Assistant, Block::Text { text }) => println!("{indent}{}", text.trim()),
                (_, Block::ToolUse { name, input, .. }) => println!(
                    "{indent}{} {} {}",
                    "●".cyan(),
                    name.as_str().bold(),
                    malac_core::tools::truncate_chars(&input.to_string(), 140).dark_grey()
                ),
                (
                    _,
                    Block::ToolResult {
                        content, is_error, ..
                    },
                ) => {
                    let first = content
                        .iter()
                        .find_map(|c| match c {
                            Block::Text { text } => text.lines().next(),
                            _ => Some("[image]"),
                        })
                        .unwrap_or_default();
                    let first = malac_core::tools::truncate_chars(first, 140);
                    if *is_error {
                        println!("{indent}  ⎿ {}", first.red());
                    } else {
                        println!("{indent}  ⎿ {}", first.dark_grey());
                    }
                }
                _ => {}
            }
        }
    }
    println!(
        "{indent}{}",
        format!(
            "─ {} in · {} out · ${:.4}",
            transcript.usage.input_tokens + transcript.usage.cache_read_tokens,
            transcript.usage.output_tokens,
            transcript.usage.cost
        )
        .dark_grey()
    );
    for child in session::list(true, usize::MAX)?
        .into_iter()
        .filter(|s| s.meta.parent.as_deref() == Some(id))
    {
        println!();
        show_session(&child.meta.id, depth + 1)?;
    }
    Ok(())
}

pub async fn provider(action: ProviderAction) -> Result<()> {
    match action {
        ProviderAction::Add {
            name,
            base_url,
            kind,
            model,
            api_key_env,
            api_key_stdin,
            set_default,
        } => {
            let kind = match kind.as_str() {
                "openai" => "openai",
                "anthropic" => "anthropic",
                other => bail!("unknown provider kind `{other}` (expected openai or anthropic)"),
            };
            if api_key_stdin {
                let mut key = String::new();
                std::io::stdin().read_to_string(&mut key)?;
                let key = key.trim();
                if key.is_empty() {
                    bail!("no API key on stdin");
                }
                config::save_api_key(&name, key)?;
            }
            if set_default && model.is_none() {
                bail!("--set-default needs --model");
            }
            config::edit_user_config(|table| {
                let providers = table
                    .entry("providers")
                    .or_insert_with(|| toml::Value::Table(Default::default()));
                let mut entry = toml::Table::new();
                entry.insert("kind".into(), kind.into());
                entry.insert("base_url".into(), base_url.clone().into());
                if let Some(var) = &api_key_env {
                    entry.insert("api_key_env".into(), var.clone().into());
                }
                if let Some(model) = &model {
                    entry.insert(
                        "models".into(),
                        toml::Value::Array(vec![model.clone().into()]),
                    );
                }
                if let Some(providers) = providers.as_table_mut() {
                    providers.insert(name.clone(), entry.into());
                }
                if let (true, Some(model)) = (set_default, &model) {
                    table.insert("model".into(), format!("{name}/{model}").into());
                }
            })?;
            println!(
                "saved provider `{name}` to {}",
                config::user_config_path().display()
            );
            if api_key_stdin {
                println!(
                    "stored its API key in {}",
                    config::credentials_path().display()
                );
            }
        }
        ProviderAction::List => {
            let config = Config::load(&working_dir(None)?, None)?.config;
            for (name, provider) in &config.providers {
                let key = if config::api_key(name, provider).is_some() {
                    "key set".green()
                } else {
                    "no key".dark_grey()
                };
                println!(
                    "{} ({}) {} · {}",
                    name.as_str().bold(),
                    provider.kind.as_str(),
                    provider.base_url,
                    key
                );
            }
            if let Some(model) = &config.model {
                println!("default model: {model}");
            }
        }
        ProviderAction::Models { name } => {
            let config = Config::load(&working_dir(None)?, None)?.config;
            let provider = config
                .providers
                .get(&name)
                .with_context(|| format!("unknown provider `{name}`"))?;
            let key = config::api_key(&name, provider);
            let models = malac_core::provider::list_models(provider, key.as_deref()).await?;
            for model in models {
                println!("{name}/{model}");
            }
            if provider.kind == ProviderKind::Anthropic {
                println!(
                    "{}",
                    "(listed through the Anthropic models endpoint)".dark_grey()
                );
            }
        }
    }
    Ok(())
}

pub fn config(action: Option<ConfigAction>) -> Result<()> {
    match action.unwrap_or(ConfigAction::Path) {
        ConfigAction::Path => {
            let cwd = working_dir(None)?;
            println!("home:        {}", config::home_dir().display());
            println!("user:        {}", config::user_config_path().display());
            println!(
                "project:     {}",
                config::project_root(&cwd)
                    .join(".malac/config.toml")
                    .display()
            );
            println!("credentials: {}", config::credentials_path().display());
            println!("sessions:    {}", session::sessions_dir().display());
        }
        ConfigAction::Show { profile } => {
            let loaded = Config::load(&working_dir(None)?, profile.as_deref())?;
            for warning in loaded.warnings {
                eprintln!("! {warning}");
            }
            print!("{}", toml::to_string_pretty(&loaded.config)?);
        }
    }
    Ok(())
}

pub async fn memory(action: Option<MemoryAction>) -> Result<()> {
    let cwd = working_dir(None)?;
    let config = Config::load(&cwd, None)?.config;
    let runtime = Runtime::new(config, cwd)?;
    match action.unwrap_or(MemoryAction::List) {
        MemoryAction::List => {
            for entry in runtime.memory.list() {
                println!(
                    "[{:?}] {}: {}  {}",
                    entry.scope,
                    entry.name.as_str().bold(),
                    entry.description,
                    entry.path.display().to_string().dark_grey()
                );
            }
        }
        MemoryAction::Search { query } => {
            for hit in runtime.memory.search(&query.join(" "), 10).await? {
                println!(
                    "{:.2}  {}: {}",
                    hit.score,
                    hit.entry.name.as_str().bold(),
                    hit.entry.description
                );
            }
        }
        MemoryAction::Reindex => {
            let count = runtime.memory.reindex().await?;
            println!("indexed {count} memories");
        }
    }
    Ok(())
}

pub async fn mcp() -> Result<()> {
    let cwd = working_dir(None)?;
    let config = Config::load(&cwd, None)?.config;
    let runtime = Runtime::new(config, cwd)?;
    let statuses = runtime.connect_mcp().await;
    if statuses.is_empty() {
        println!("no MCP servers configured");
    }
    let tools = runtime.tools();
    for status in statuses {
        match status.error {
            Some(err) => println!("{} {}\n  {}", "✗".red(), status.name.as_str().bold(), err),
            None => println!(
                "{} {} ({} tools)",
                "●".green(),
                status.name.as_str().bold(),
                status.tools
            ),
        }
    }
    let builtin = malac_core::tools::builtin().len();
    let names: Vec<&str> = tools.iter().skip(builtin).map(|t| t.name()).collect();
    if !names.is_empty() {
        println!("\n{}", names.join(", ").dark_grey());
    }
    runtime.shutdown().await;
    Ok(())
}

pub fn control(args: &[String]) -> Result<i32> {
    let binary = malac_core::mcp::find_control_binary()
        .context("malac-control not found next to malac or on PATH")?;
    let status = std::process::Command::new(binary).args(args).status()?;
    Ok(status.code().unwrap_or(1))
}
