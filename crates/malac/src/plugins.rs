//! `malac plugin`: marketplaces and plugins.

use std::io::{BufRead, IsTerminal, Write};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use crossterm::style::Stylize;
use malac_core::config::{self, Config};
use malac_core::plugins::{Manager, NeedsApproval, PluginInfo, Scope};

#[derive(Subcommand)]
pub enum PluginAction {
    /// Add, list, update or remove marketplaces.
    Marketplace {
        #[command(subcommand)]
        action: MarketplaceAction,
    },
    /// Install a plugin (name, name@marketplace, or a folder or repository
    /// holding one) and enable it. Without a name, installs what the
    /// project's .malac/plugins.toml enables.
    Install {
        plugin: Option<String>,
        /// Enable it for this project (.malac/plugins.toml) instead of for you.
        #[arg(long)]
        project: bool,
        /// Approve hooks, MCP servers and scripts without asking.
        #[arg(short, long)]
        yes: bool,
        /// Install without enabling.
        #[arg(long)]
        no_enable: bool,
    },
    /// Remove an installed plugin.
    Uninstall { plugin: String },
    /// Turn a plugin on, after showing what it runs.
    Enable {
        plugin: String,
        #[arg(long)]
        project: bool,
        #[arg(short, long)]
        yes: bool,
    },
    /// Turn a plugin off (built-in ones too).
    Disable {
        plugin: String,
        #[arg(long)]
        project: bool,
    },
    /// List installed, built-in and project plugins.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show what a plugin contains and what of it runs code.
    Show { plugin: String },
    /// Fetch the latest marketplaces and reinstall plugins (all, or one).
    Update { plugin: Option<String> },
}

#[derive(Subcommand)]
pub enum MarketplaceAction {
    /// Add a marketplace: a folder, owner/repo (GitHub), a git URL (GitLab,
    /// SSH, private repositories through your git credentials), or the URL
    /// of a marketplace.json. `#ref` pins a branch or tag.
    Add {
        source: String,
        /// Record it in the project's .malac/plugins.toml for the team.
        #[arg(long)]
        project: bool,
    },
    /// List marketplaces and their plugins.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Fetch the latest catalogs (all, or one).
    Update { name: Option<String> },
    /// Forget a marketplace. Plugins installed from it stay installed.
    Remove {
        name: String,
        #[arg(long)]
        project: bool,
    },
}

fn scope(project: bool) -> Scope {
    if project { Scope::Project } else { Scope::User }
}

fn manager() -> Result<Manager> {
    let cwd = std::env::current_dir()?
        .canonicalize()
        .context("cannot open the current directory")?;
    let root = config::project_root(&cwd);
    let defaults = Config::load(&cwd, None).map_or(true, |l| l.config.plugins.defaults);
    Ok(Manager::new(
        &config::home_dir(),
        Some(&root),
        &cwd,
        defaults,
    ))
}

pub async fn run(action: PluginAction) -> Result<()> {
    let manager = manager()?;
    match action {
        PluginAction::Marketplace { action } => marketplace(&manager, action).await?,
        PluginAction::Install {
            plugin: Some(plugin),
            project,
            yes,
            no_enable,
        } => {
            let installed = manager.install(&plugin).await?;
            println!("Installed {}", installed.id.as_str().bold());
            println!("{}", installed.describe().dark_grey());
            if !no_enable {
                enable(&manager, &installed.id, scope(project), yes)?;
            }
        }
        PluginAction::Install {
            plugin: None,
            yes,
            no_enable,
            ..
        } => {
            install_project(&manager, yes, no_enable).await?;
        }
        PluginAction::Uninstall { plugin } => {
            let id = manager.uninstall(&plugin)?;
            println!("Uninstalled {id}");
        }
        PluginAction::Enable {
            plugin,
            project,
            yes,
        } => enable(&manager, &plugin, scope(project), yes)?,
        PluginAction::Disable { plugin, project } => {
            let id = manager.disable(&plugin, scope(project))?;
            println!(
                "Disabled {id} in {}",
                manager.file(scope(project))?.display()
            );
        }
        PluginAction::List { json } => {
            let list = manager.list()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else {
                print_list(&list);
            }
        }
        PluginAction::Show { plugin } => {
            let id = manager.resolve(&plugin)?;
            let (plugin, approved) = manager.load_installed(&id)?;
            println!("{}", plugin.describe());
            println!("  path: {}", plugin.root.display());
            if !approved.is_empty() {
                println!("  approved: {}", approved.join(", "));
            }
            for warning in &plugin.warnings {
                println!("  {}", format!("! {warning}").yellow());
            }
        }
        PluginAction::Update { plugin } => {
            for line in manager.update(plugin.as_deref()).await? {
                println!("{line}");
            }
        }
    }
    Ok(())
}

async fn marketplace(manager: &Manager, action: MarketplaceAction) -> Result<()> {
    match action {
        MarketplaceAction::Add { source, project } => {
            let catalog = manager.add_marketplace(&source, scope(project)).await?;
            println!(
                "Added marketplace {} with {}",
                catalog.name.as_str().bold(),
                count(catalog.plugins.len(), "plugin")
            );
            for plugin in &catalog.plugins {
                println!(
                    "  {}@{}  {}",
                    plugin.name,
                    catalog.name,
                    plugin
                        .description
                        .as_deref()
                        .unwrap_or_default()
                        .dark_grey()
                );
            }
            for warning in &catalog.warnings {
                println!("  {}", format!("! {warning}").yellow());
            }
        }
        MarketplaceAction::List { json } => {
            let list = manager.list_marketplaces()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
                return Ok(());
            }
            if list.is_empty() {
                println!("No marketplaces. Add one with `malac plugin marketplace add <source>`.");
            }
            for market in list {
                println!(
                    "{} {}",
                    market.name.as_str().bold(),
                    format!("({}, {})", market.scope.as_str(), market.source).dark_grey()
                );
                if let Some(err) = market.error {
                    println!("  {}", format!("! {err}").red());
                }
                for plugin in market.plugins {
                    println!(
                        "  {}  {}",
                        plugin.name,
                        plugin.description.unwrap_or_default().dark_grey()
                    );
                }
            }
        }
        MarketplaceAction::Update { name } => {
            for line in manager.update_marketplaces(name.as_deref()).await? {
                println!("{line}");
            }
        }
        MarketplaceAction::Remove { name, project } => {
            manager.remove_marketplace(&name, scope(project))?;
            println!("Removed marketplace {name}");
        }
    }
    Ok(())
}

/// Enables a plugin, asking before one that runs code is turned on.
fn enable(manager: &Manager, plugin: &str, scope: Scope, yes: bool) -> Result<()> {
    let id = match manager.enable(plugin, scope, false) {
        Ok(id) => id,
        Err(err) => {
            let Some(needs) = err.downcast_ref::<NeedsApproval>() else {
                return Err(err);
            };
            println!(
                "{} runs code on this machine: {}.",
                needs.id.as_str().bold(),
                needs.parts.join(", ")
            );
            if !yes && !confirm("Enable it?", &needs.id)? {
                println!(
                    "Left it installed but off; `malac plugin enable {}` asks again.",
                    needs.id
                );
                return Ok(());
            }
            manager.enable(&needs.id, scope, true)?
        }
    };
    println!(
        "Enabled {} in {}; new sessions use it",
        id.as_str().bold(),
        manager.file(scope)?.display()
    );
    Ok(())
}

fn confirm(question: &str, id: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!(
            "{id} is installed but off: `malac plugin enable {id} --yes` approves and enables it"
        );
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Installs the plugins the project's file enables that are missing here.
async fn install_project(manager: &Manager, yes: bool, no_enable: bool) -> Result<()> {
    let file = manager.file(Scope::Project)?;
    let missing: Vec<PluginInfo> = manager
        .list()?
        .into_iter()
        .filter(|p| p.enabled && !p.installed && p.scope == Some(Scope::Project))
        .collect();
    if missing.is_empty() {
        println!("Everything {} enables is installed.", file.display());
        return Ok(());
    }
    for plugin in missing {
        match manager.install(&plugin.id).await {
            Ok(installed) => {
                println!("Installed {}", installed.id.as_str().bold());
                println!("{}", installed.describe().dark_grey());
                if !no_enable {
                    enable(manager, &installed.id, Scope::Project, yes)?;
                }
            }
            Err(err) => println!("{}", format!("✗ {}: {err:#}", plugin.id).red()),
        }
    }
    Ok(())
}

fn print_list(list: &[PluginInfo]) {
    if list.is_empty() {
        println!("No plugins. Add a marketplace with `malac plugin marketplace add <source>`.");
    }
    for plugin in list {
        let state = match (&plugin.error, plugin.enabled) {
            (Some(_), _) if !plugin.installed => "not installed".yellow(),
            (Some(_), _) => "error".red(),
            (None, true) => "enabled".green(),
            (None, false) => "disabled".dark_grey(),
        };
        let scope = plugin
            .scope
            .map(|s| format!(", {}", s.as_str()))
            .unwrap_or_default();
        println!(
            "{} {} {}",
            plugin.id.as_str().bold(),
            plugin.version.as_deref().unwrap_or_default(),
            format!("({state}{scope})").dark_grey()
        );
        if let Some(description) = &plugin.description {
            println!("  {}", description.as_str().dark_grey());
        }
        let parts: Vec<String> = [
            ("skill", plugin.skills.len()),
            ("command", plugin.commands.len()),
            ("agent", plugin.agents.len()),
            ("MCP server", plugin.mcp.len()),
            ("hook event", plugin.hooks.len()),
        ]
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(label, n)| count(*n, label))
        .collect();
        if !parts.is_empty() {
            println!("  {}", parts.join(", "));
        }
        if !plugin.pending.is_empty() {
            println!(
                "  {}",
                format!(
                    "! {} not approved; `malac plugin enable {}` reviews them",
                    plugin.pending.join(" and "),
                    plugin.id
                )
                .yellow()
            );
        }
        if let Some(err) = &plugin.error {
            println!("  {}", err.as_str().dark_grey());
        }
    }
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}
