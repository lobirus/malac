//! Command-line parsing and dispatch.

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use malac_core::permission::Mode;

use crate::protocol::{Launch, TabOptions, WorkspaceChoice};
use crate::{commands, daemon, repl, tui, web};

/// Multi-Agent Loop for Autonomous Coding. `malac <prompt>` starts working
/// on the prompt right away; `malac` alone opens an interactive session.
#[derive(Parser)]
#[command(name = "malac", version, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    run: RunArgs,
}

/// Fallback parser used when the prompt happens to start with a subcommand name.
#[derive(Parser)]
#[command(name = "malac")]
struct PromptOnly {
    #[command(flatten)]
    run: RunArgs,
}

#[derive(Args, Debug, Clone, Default)]
pub struct RunArgs {
    /// What to do. Quote it or not; everything after the options is the prompt.
    #[arg(trailing_var_arg = true)]
    pub prompt: Vec<String>,
    #[command(flatten)]
    pub session: SessionOptions,
    /// Exit after answering the prompt instead of staying interactive.
    #[arg(short, long)]
    pub print: bool,
    /// Stream events as JSON lines instead of rendering them.
    #[arg(long)]
    pub json: bool,
    /// Use the line-based interface instead of the full-screen one.
    #[arg(long)]
    pub plain: bool,
    /// Continue the most recent session in this directory.
    #[arg(short = 'c', long = "continue")]
    pub continue_last: bool,
    /// Resume a session by id.
    #[arg(short, long)]
    pub resume: Option<String>,
    /// Start a fresh workspace; the current one stays resumable.
    #[arg(short = 'n', long = "new", conflicts_with = "workspace")]
    pub new_workspace: bool,
    /// Open a workspace by name or id.
    #[arg(short = 'w', long)]
    pub workspace: Option<String>,
    /// Name the new tab, or with -n the new workspace.
    #[arg(long)]
    pub name: Option<String>,
    /// Also serve the web UI for this session (default: `web.enabled`).
    #[arg(long, overrides_with = "no_web")]
    pub web: bool,
    /// Do not serve the web UI, even if `web.enabled` is set.
    #[arg(long)]
    pub no_web: bool,
    /// Serve the web UI to other machines too (implies --web).
    #[arg(long)]
    pub web_remote: bool,
    /// Port for the web UI (default: `web.port`, or any free port if taken).
    #[arg(long)]
    pub web_port: Option<u16>,
}

/// Options shared by the terminal and `malac web`.
#[derive(Args, Debug, Clone, Default)]
pub struct SessionOptions {
    /// Model as provider/model, e.g. lora/smart.
    #[arg(short, long)]
    pub model: Option<String>,
    /// Apply a [profiles.<name>] section from the config.
    #[arg(long)]
    pub profile: Option<String>,
    /// Permission mode: ask, edits or autopilot.
    #[arg(long)]
    pub mode: Option<Mode>,
    /// Never ask for permission (same as --mode autopilot).
    #[arg(short = 'y', long)]
    pub autopilot: bool,
    /// Working directory.
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    /// Do not connect the computer and browser control service.
    #[arg(long)]
    pub no_control: bool,
    /// Show model reasoning as it streams.
    #[arg(short, long)]
    pub verbose: bool,
}

impl SessionOptions {
    /// The options that apply to a daemon tab.
    pub fn tab_options(&self) -> TabOptions {
        TabOptions {
            model: self.model.clone(),
            mode: if self.autopilot {
                Some(Mode::Autopilot)
            } else {
                self.mode
            },
            profile: self.profile.clone(),
            no_control: self.no_control,
        }
    }
}

impl RunArgs {
    /// What these arguments ask a daemon client to open.
    pub fn launch(&self, cwd: &std::path::Path, prompt: &str) -> Launch {
        let workspace = match (&self.workspace, self.new_workspace) {
            (Some(selector), _) => WorkspaceChoice::Open(selector.clone()),
            (None, true) => WorkspaceChoice::New,
            (None, false) => WorkspaceChoice::Last,
        };
        Launch {
            workspace,
            cwd: cwd.to_path_buf(),
            name: self.name.clone(),
            session: self.resume.clone(),
            continue_last: self.continue_last,
            prompt: (!prompt.trim().is_empty()).then(|| prompt.to_owned()),
            attachments: Vec::new(),
            options: self.session.tab_options(),
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Make sure the daemon runs and print or open its web UI.
    Web(web::WebArgs),
    /// Run, start, stop or inspect the background daemon.
    Daemon {
        #[command(subcommand)]
        action: Option<daemon::DaemonAction>,
    },
    /// List, rename or delete workspaces.
    Workspaces {
        #[command(subcommand)]
        action: Option<commands::WorkspacesAction>,
    },
    /// List, show or delete saved sessions.
    Sessions {
        #[command(subcommand)]
        action: Option<commands::SessionsAction>,
    },
    /// Add or list model providers.
    Provider {
        #[command(subcommand)]
        action: commands::ProviderAction,
    },
    /// Show configuration paths and the resolved configuration.
    Config {
        #[command(subcommand)]
        action: Option<commands::ConfigAction>,
    },
    /// List, search or reindex long-term memory.
    Memory {
        #[command(subcommand)]
        action: Option<commands::MemoryAction>,
    },
    /// List the models every configured provider offers.
    Models,
    /// Update malac to the latest release.
    Update {
        /// Only check whether an update is available.
        #[arg(long)]
        check: bool,
    },
    /// Show connected MCP servers and their tools.
    Mcp,
    /// Run the computer and browser control service (`malac-control`).
    Control {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn parse_args() -> Cli {
    let args: Vec<OsString> = std::env::args_os().collect();
    match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(err) if prompt_starts_with_subcommand(&args, &err) => Cli {
            command: None,
            run: PromptOnly::parse_from(&args).run,
        },
        Err(err) => err.exit(),
    }
}

/// `malac web scraper for the docs` is a prompt, not a malformed `web` command.
fn prompt_starts_with_subcommand(args: &[OsString], err: &clap::Error) -> bool {
    use clap::error::ErrorKind;
    if matches!(
        err.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
    ) {
        return false;
    }
    let first_positional = args
        .iter()
        .skip(1)
        .find(|a| !a.to_string_lossy().starts_with('-'));
    let names = [
        "web",
        "daemon",
        "workspaces",
        "sessions",
        "provider",
        "config",
        "memory",
        "models",
        "update",
        "mcp",
        "control",
    ];
    first_positional.is_some_and(|a| names.contains(&a.to_string_lossy().as_ref()))
}

/// Runs the command line and returns the process exit code.
pub async fn main() -> i32 {
    let cli = parse_args();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("MALAC_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(tui::log_writer)
        .init();

    let result: Result<i32> = match cli.command {
        None => repl::run(cli.run).await,
        Some(Command::Web(args)) => web::run(args).await.map(|_| 0),
        Some(Command::Daemon { action }) => daemon::command(action).await,
        Some(Command::Workspaces { action }) => commands::workspaces(action).await.map(|_| 0),
        Some(Command::Sessions { action }) => commands::sessions(action).map(|_| 0),
        Some(Command::Provider { action }) => commands::provider(action).await.map(|_| 0),
        Some(Command::Config { action }) => commands::config(action).map(|_| 0),
        Some(Command::Memory { action }) => commands::memory(action).await.map(|_| 0),
        Some(Command::Models) => commands::models().await.map(|_| 0),
        Some(Command::Update { check }) => commands::update(check).await.map(|_| 0),
        Some(Command::Mcp) => commands::mcp().await.map(|_| 0),
        Some(Command::Control { args }) => commands::control(&args),
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        match Cli::try_parse_from(&args) {
            Ok(cli) => cli,
            Err(err) if prompt_starts_with_subcommand(&args, &err) => Cli {
                command: None,
                run: PromptOnly::parse_from(&args).run,
            },
            Err(err) => panic!("{err}"),
        }
    }

    #[test]
    fn bare_words_are_a_prompt() {
        let cli = parse(&["malac", "-y", "fix", "the", "tests"]);
        assert!(cli.command.is_none());
        assert!(cli.run.session.autopilot);
        assert_eq!(cli.run.prompt.join(" "), "fix the tests");
    }

    #[test]
    fn subcommand_names_inside_prompts() {
        assert!(matches!(
            parse(&["malac", "web"]).command,
            Some(Command::Web(_))
        ));
        let cli = parse(&["malac", "web", "scraper", "for", "docs"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.run.prompt.join(" "), "web scraper for docs");
        let cli = parse(&["malac", "memory leak in the parser"]);
        assert_eq!(cli.run.prompt, vec!["memory leak in the parser"]);
        let cli = parse(&["malac", "update", "the", "changelog"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.run.prompt.join(" "), "update the changelog");
        assert!(matches!(
            parse(&["malac", "daemon", "status"]).command,
            Some(Command::Daemon {
                action: Some(daemon::DaemonAction::Status { json: false })
            })
        ));
        let cli = parse(&["malac", "daemon", "the", "build"]);
        assert_eq!(cli.run.prompt.join(" "), "daemon the build");
        assert!(matches!(
            parse(&["malac", "workspaces"]).command,
            Some(Command::Workspaces { action: None })
        ));
    }

    #[test]
    fn workspace_flags() {
        let cwd = std::path::Path::new("/src/app");
        let cli = parse(&["malac", "-n", "--name", "release", "ship", "it"]);
        assert!(cli.run.new_workspace);
        let launch = cli.run.launch(cwd, &cli.run.prompt.join(" "));
        assert_eq!(launch.workspace, WorkspaceChoice::New);
        assert_eq!(launch.name.as_deref(), Some("release"));
        assert_eq!(launch.prompt.as_deref(), Some("ship it"));

        let cli = parse(&["malac", "-w", "work", "--name", "api", "-y", "fix", "tests"]);
        let launch = cli.run.launch(cwd, &cli.run.prompt.join(" "));
        assert_eq!(launch.workspace, WorkspaceChoice::Open("work".into()));
        assert_eq!(launch.name.as_deref(), Some("api"));
        assert_eq!(launch.options.mode, Some(Mode::Autopilot));
        assert_eq!(launch.cwd, cwd);

        let cli = parse(&["malac", "--workspace=work"]);
        let launch = cli.run.launch(cwd, "");
        assert_eq!(launch.workspace, WorkspaceChoice::Open("work".into()));
        assert!(launch.prompt.is_none());

        let launch = parse(&["malac", "-r", "abc"]).run.launch(cwd, "");
        assert_eq!(launch.workspace, WorkspaceChoice::Last);
        assert_eq!(launch.session.as_deref(), Some("abc"));

        let args: Vec<OsString> = ["malac", "-n", "-w", "work"]
            .iter()
            .map(OsString::from)
            .collect();
        assert!(Cli::try_parse_from(&args).is_err(), "-n and -w conflict");
    }
}
