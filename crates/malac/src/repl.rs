//! `malac [prompt]`: run a prompt, then keep the conversation going.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crossterm::style::Stylize;
use malac_core::config::{self, Config};
use malac_core::permission::{Approver, DenyAll, Mode};
use malac_core::{AgentHandle, AgentSpec, Block, Runtime, session, update};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use tokio::sync::{Mutex, mpsc};

use crate::approve::{FirstAnswer, TerminalApprover, TerminalLock};
use crate::client::Client;
use crate::daemon::ServeArgs;
use crate::render::{self, Renderer};
use crate::{RunArgs, SessionOptions, remote, tui, web};

/// What the line REPL and the TUI share: the runtime, the agent being
/// talked to and the web UI serving it.
pub struct Session {
    pub runtime: Arc<Runtime>,
    pub handle: AgentHandle,
    pub main: Arc<RwLock<String>>,
    /// Asks in this terminal; raced against the web UI once that serves.
    pub approver: Option<Arc<dyn Approver>>,
    pub web_urls: Vec<String>,
}

/// The line REPL's own state.
struct Repl {
    session: Session,
    turn_done: mpsc::UnboundedReceiver<()>,
    /// The last `/model` listing, so `/model 3` can pick from it.
    model_choices: Vec<String>,
}

pub fn load_config(args: &SessionOptions, cwd: &std::path::Path) -> Result<Config> {
    let loaded = Config::load(cwd, args.profile.as_deref())?;
    for warning in &loaded.warnings {
        eprintln!("{}", format!("! {warning}").yellow());
    }
    let mut config = loaded.config;
    if let Some(model) = &args.model {
        config.model = Some(model.clone());
    }
    if args.autopilot {
        config.mode = Mode::Autopilot;
    } else if let Some(mode) = args.mode {
        config.mode = mode;
    }
    if args.no_control {
        config.control.enabled = false;
    }
    if args.verbose {
        config.ui.show_thinking = true;
    }
    Ok(config)
}

pub fn working_dir(cwd: Option<&PathBuf>) -> Result<PathBuf> {
    let dir = match cwd {
        Some(dir) => dir.clone(),
        None => std::env::current_dir()?,
    };
    dir.canonicalize()
        .with_context(|| format!("cannot open {}", dir.display()))
}

pub async fn run(args: RunArgs) -> Result<i32> {
    let cwd = working_dir(args.session.cwd.as_ref())?;
    let config = load_config(&args.session, &cwd)?;
    config.main_model()?;
    let show_thinking = config.ui.show_thinking;
    let mode = config.mode;

    let stdin_tty = std::io::stdin().is_terminal();
    let interactive = stdin_tty && std::io::stdout().is_terminal() && !args.print && !args.json;
    let use_tui = interactive
        && !args.plain
        && config.ui.tui
        && std::env::var("TERM").is_ok_and(|term| term != "dumb");
    let mut prompt = args.prompt.join(" ");
    if !stdin_tty {
        let piped = read_piped_stdin(!prompt.trim().is_empty());
        if !piped.trim().is_empty() {
            prompt = format!("{prompt}\n\n<stdin>\n{}\n</stdin>", piped.trim_end());
        }
    }
    if !interactive && prompt.trim().is_empty() {
        bail!("no prompt given");
    }
    // Both interactive interfaces are daemon clients, so agents outlive the
    // terminal. Scripts (-p, --json) and pipes run here, in this process, and
    // so does the line interface when the daemon cannot be reached.
    if use_tui {
        let connection = Client::connect_or_start(&ServeArgs::default())
            .await
            .context(
                "the full-screen interface needs the malac daemon, which could not start; \
                 `malac --plain` and `malac -p <prompt>` work without it",
            )?;
        tui::run(connection, &args, config, &cwd, prompt).await?;
        return Ok(0);
    }
    if interactive {
        match Client::connect_or_start(&ServeArgs::default()).await {
            Ok(connection) => {
                return remote::run(connection, &args, &cwd, prompt, show_thinking).await;
            }
            Err(err) => eprintln!(
                "{}",
                format!("! cannot reach the malac daemon ({err:#}); this session runs here only")
                    .yellow()
            ),
        }
    }

    let runtime = Runtime::new(config, cwd.clone())?;
    if interactive {
        runtime.start_update_check();
    }
    for server in runtime.connect_mcp().await {
        if let Some(err) = server.error {
            let first = err.lines().next().unwrap_or_default();
            eprintln!(
                "{}",
                format!("! MCP server `{}` unavailable: {first}", server.name).yellow()
            );
        }
    }

    let lock: TerminalLock = Arc::new(Mutex::new(()));
    let main = Arc::new(RwLock::new(String::new()));
    let approver = stdin_tty
        .then(|| Arc::new(TerminalApprover::new(lock.clone(), main.clone())) as Arc<dyn Approver>);
    runtime.set_approver(approver.clone().unwrap_or_else(|| Arc::new(DenyAll)));
    if approver.is_none() && mode != Mode::Autopilot {
        eprintln!(
            "{}",
            "! no terminal to ask for permission: calls that need approval will be denied (use -y)"
                .yellow()
        );
    }

    let (done_tx, turn_done) = mpsc::unbounded_channel();
    if args.json {
        render::spawn_json(runtime.bus.subscribe(), main.clone(), done_tx);
    } else {
        Renderer {
            lock: lock.clone(),
            main: main.clone(),
            show_thinking,
            turn_done: done_tx,
        }
        .spawn(runtime.bus.subscribe());
    }

    let handle = open_agent(&runtime, &args, &cwd)?;
    *main.write().unwrap() = handle.id.clone();
    let mut session = Session {
        runtime: runtime.clone(),
        handle,
        main,
        approver,
        web_urls: Vec::new(),
    };
    let web = &runtime.config.web;
    if (args.web || args.web_remote || web.enabled) && !args.no_web {
        session
            .serve_web(args.web_remote || web.remote, args.web_port)
            .await?;
    }

    let mut repl = Repl {
        session,
        turn_done,
        model_choices: Vec::new(),
    };
    if interactive {
        print_banner(&repl.session);
    }
    let mut ok = true;
    if !prompt.trim().is_empty() {
        ok = repl.turn(prompt).await;
    }
    if !interactive {
        runtime.shutdown().await;
        return Ok(if ok { 0 } else { 1 });
    }
    let result = repl.repl().await;
    runtime.shutdown().await;
    result.map(|_| 0)
}

/// Piped input becomes part of the prompt. When a prompt was also given on
/// the command line, a stdin that stays open (as under some IDEs and CI
/// runners) is ignored after a short wait instead of blocking forever.
fn read_piped_stdin(have_prompt: bool) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut input = String::new();
        let _ = std::io::stdin().read_to_string(&mut input);
        let _ = tx.send(input);
    });
    if have_prompt {
        rx.recv_timeout(Duration::from_secs(3)).unwrap_or_default()
    } else {
        rx.recv().unwrap_or_default()
    }
}

fn open_agent(
    runtime: &Arc<Runtime>,
    args: &RunArgs,
    cwd: &std::path::Path,
) -> Result<AgentHandle> {
    if let Some(id) = &args.resume {
        return runtime.resume_agent(id);
    }
    if args.continue_last {
        let latest = session::latest_in(cwd)?.context("no previous session in this directory")?;
        return runtime.resume_agent(&latest.meta.id);
    }
    runtime.spawn_agent(AgentSpec::default())
}

/// What the name stands for.
pub const TAGLINE: &str = "Multi-Agent Loop for Autonomous Coding";

/// Shown when an interactive session starts.
pub const PIG: [&str; 5] = [
    r"   ^    ^    ",
    r"  / \__/ \   ",
    r" (  o  o  )  ",
    r" (  (oo)  )  ",
    r"  \______/~  ",
];

fn print_banner(session: &Session) {
    let snap = session.handle.snapshot();
    let info = [
        format!("malac {} · {TAGLINE}", env!("CARGO_PKG_VERSION")).bold(),
        format!(
            "{} · {} mode",
            snap.model,
            session.runtime.policy.mode().as_str()
        )
        .reset(),
        snap.info.cwd.display().to_string().dark_grey(),
        "/help for commands · Ctrl-C interrupts · Ctrl-D exits"
            .to_owned()
            .dark_grey(),
        session
            .web_urls
            .first()
            .map(|url| format!("web UI: {url}"))
            .unwrap_or_default()
            .dark_grey(),
    ];
    for (art, text) in PIG.iter().zip(info) {
        println!("{}{text}", art.bold());
    }
    let indent = " ".repeat(PIG[0].len());
    for url in session.web_urls.iter().skip(1) {
        println!("{indent}{}", format!("remote: {url}").dark_grey());
    }
    if let Some(notice) = update_notice(&update::status()) {
        println!("{indent}{}", notice.green());
    }
    println!();
}

/// One line about a newer malac, if there is one.
pub fn update_notice(status: &update::Status) -> Option<String> {
    if let Some(version) = &status.installed {
        return Some(format!("malac {version} installed · restart to use it"));
    }
    match (&status.latest, status.available) {
        (Some(version), true) => Some(format!("malac {version} is available · run `malac update`")),
        _ => None,
    }
}

/// A slash command, parsed the same way by the line REPL and the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slash<'a> {
    Exit,
    Help,
    Model(&'a str),
    Mode(&'a str),
    Agents,
    Compact,
    Clear,
    Resume(&'a str),
    Sessions,
    Cost,
    Mcp,
    Memory,
    Web(&'a str),
}

/// Every slash command: name, arguments and what it does.
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("model", "[provider/model]", "show or switch the model"),
    (
        "mode",
        "[ask|edits|autopilot]",
        "show or change the permission mode",
    ),
    ("agents", "", "show subagents and what they are doing"),
    ("compact", "", "summarize the conversation to free context"),
    ("clear", "", "start a new session"),
    ("resume", "[id]", "list sessions or resume one"),
    ("sessions", "", "list recent sessions"),
    ("cost", "", "token usage and cost of this session"),
    ("mcp", "", "connected tool servers"),
    ("memory", "", "saved memories"),
    ("web", "[remote]", "serve the web UI for this session"),
    ("help", "", "list commands"),
    ("exit", "", "quit"),
];

impl<'a> Slash<'a> {
    /// Parses a command line without its leading `/`.
    pub fn parse(input: &'a str) -> Result<Self> {
        let (name, arg) = input
            .trim()
            .split_once(' ')
            .map_or((input.trim(), ""), |(n, a)| (n, a.trim()));
        Ok(match name {
            "exit" | "quit" | "q" => Slash::Exit,
            "help" | "?" => Slash::Help,
            "model" => Slash::Model(arg),
            "mode" => Slash::Mode(arg),
            "agents" => Slash::Agents,
            "compact" => Slash::Compact,
            "clear" | "new" => Slash::Clear,
            "resume" => Slash::Resume(arg),
            "sessions" => Slash::Sessions,
            "cost" | "usage" => Slash::Cost,
            "mcp" => Slash::Mcp,
            "memory" => Slash::Memory,
            "web" => Slash::Web(arg),
            other => bail!("unknown command /{other} (try /help)"),
        })
    }
}

impl Session {
    pub fn switch_to(&mut self, handle: AgentHandle) {
        *self.main.write().unwrap() = handle.id.clone();
        self.handle = handle;
    }

    async fn start_web(&mut self, host: &str, port: u16) -> Result<()> {
        if !self.web_urls.is_empty() {
            return Ok(());
        }
        let (urls, web_approver) = web::spawn_for(self.runtime.clone(), host, port).await?;
        let approver: Arc<dyn Approver> = match &self.approver {
            Some(terminal) => Arc::new(FirstAnswer(vec![terminal.clone(), web_approver])),
            None => web_approver,
        };
        self.runtime.set_approver(approver);
        self.web_urls = urls;
        Ok(())
    }

    /// Serves the web UI on the configured host, or on every interface when
    /// `remote`; does nothing if it already serves.
    pub async fn serve_web(&mut self, remote: bool, port: Option<u16>) -> Result<()> {
        let web = &self.runtime.config.web;
        let host = if remote {
            "0.0.0.0".to_owned()
        } else {
            web.host.clone()
        };
        let port = port.unwrap_or(web.port);
        self.start_web(&host, port).await
    }
}

enum Flow {
    Continue,
    Exit,
}

impl Repl {
    /// Runs one prompt; Ctrl-C cancels it and every subagent it started.
    async fn turn(&mut self, text: String) -> bool {
        let runtime = self.session.runtime.clone();
        let id = self.session.handle.id.clone();
        let watcher = tokio::spawn(async move {
            while tokio::signal::ctrl_c().await.is_ok() {
                runtime.cancel_tree(&id);
            }
        });
        let result = self.session.handle.prompt(vec![Block::text(text)]).await;
        watcher.abort();
        let _ = tokio::time::timeout(Duration::from_secs(2), self.turn_done.recv()).await;
        result.is_ok()
    }

    async fn repl(&mut self) -> Result<()> {
        let history = config::home_dir().join("history.txt");
        let mut editor = DefaultEditor::new()?;
        let _ = editor.load_history(&history);
        loop {
            let (returned, line) = tokio::task::spawn_blocking(move || {
                let line = editor.readline("› ");
                (editor, line)
            })
            .await?;
            editor = returned;
            match line {
                Ok(line) => {
                    let line = line.trim().to_owned();
                    if line.is_empty() {
                        continue;
                    }
                    let _ = editor.add_history_entry(&line);
                    if let Some(command) = line.strip_prefix('/') {
                        match self.command(command).await {
                            Ok(Flow::Exit) => break,
                            Ok(Flow::Continue) => {}
                            Err(err) => println!("{}", format!("✗ {err:#}").red()),
                        }
                    } else {
                        self.turn(line).await;
                    }
                    println!();
                }
                Err(ReadlineError::Interrupted) => continue,
                Err(ReadlineError::Eof) => break,
                Err(err) => return Err(err.into()),
            }
        }
        let _ = editor.save_history(&history);
        Ok(())
    }

    /// Lists every model the providers offer, numbered for `/model <n>`.
    async fn list_models(&mut self) {
        let current = self.session.handle.snapshot().model;
        self.model_choices.clear();
        for provider in malac_core::provider::discover_models(&self.session.runtime.config).await {
            let models = provider.all();
            if models.is_empty() {
                continue;
            }
            println!("{}", provider.provider.as_str().bold());
            for model in models {
                let spec = format!("{}/{model}", provider.provider);
                self.model_choices.push(spec.clone());
                let marker = if spec == current { "*" } else { " " };
                println!("  {marker} {:>3}  {spec}", self.model_choices.len());
            }
        }
        println!(
            "{}",
            "/model <number> or /model provider/model to switch".dark_grey()
        );
    }

    async fn command(&mut self, input: &str) -> Result<Flow> {
        let session = &mut self.session;
        match Slash::parse(input)? {
            Slash::Exit => return Ok(Flow::Exit),
            Slash::Help => print_help(),
            Slash::Model("") => self.list_models().await,
            Slash::Model(arg) => match arg.parse::<usize>() {
                Ok(n) => match self.model_choices.get(n.wrapping_sub(1)) {
                    Some(spec) => session.handle.set_model(&spec.clone()),
                    None => bail!("no model #{n}; run /model to list them"),
                },
                Err(_) => session.handle.set_model(arg),
            },
            Slash::Mode("") => println!("mode: {}", session.runtime.policy.mode().as_str()),
            Slash::Mode(arg) => {
                let mode: Mode = arg.parse()?;
                session.runtime.policy.set_mode(mode);
                println!("mode: {}", mode.as_str());
            }
            Slash::Compact => {
                session.handle.compact();
                session.handle.wait_idle().await;
            }
            Slash::Clear => {
                let handle = session.runtime.spawn_agent(AgentSpec::default())?;
                session.switch_to(handle);
                println!("{}", "started a new session".dark_grey());
            }
            Slash::Resume("") => print_sessions(10)?,
            Slash::Resume(id) => {
                let handle = session.runtime.resume_agent(id)?;
                session.switch_to(handle);
                println!("{}", format!("resumed {id}").dark_grey());
            }
            Slash::Sessions => print_sessions(20)?,
            Slash::Agents => print_agents(&session.runtime, &session.handle.id),
            Slash::Cost => {
                let usage = session.handle.snapshot().usage;
                println!(
                    "input {} · cached {} · output {} · ${:.4}",
                    usage.input_tokens, usage.cache_read_tokens, usage.output_tokens, usage.cost
                );
            }
            Slash::Mcp => {
                for server in session.runtime.servers() {
                    match server.error {
                        Some(err) => println!(
                            "{} {}: {}",
                            "✗".red(),
                            server.name,
                            err.lines().next().unwrap_or_default()
                        ),
                        None => println!("{} {}: {} tools", "●".green(), server.name, server.tools),
                    }
                }
            }
            Slash::Memory => {
                for entry in session.runtime.memory.list() {
                    println!("[{:?}] {}: {}", entry.scope, entry.name, entry.description);
                }
            }
            Slash::Web(arg) => {
                let remote = arg == "remote" || session.runtime.config.web.remote;
                session.serve_web(remote, None).await?;
                for url in &session.web_urls {
                    println!("web UI: {url}");
                }
            }
        }
        Ok(Flow::Continue)
    }
}

pub(crate) fn print_help() {
    for (name, args, help) in COMMANDS {
        let command = format!("/{name} {args}");
        println!("  {:<30} {}", command.trim_end().bold(), help);
    }
}

pub(crate) fn print_sessions(limit: usize) -> Result<()> {
    for summary in session::list(false, limit)? {
        println!(
            "{}  {}  {}",
            summary.meta.id.as_str().dark_grey(),
            summary
                .updated
                .with_timezone(&chrono::Local)
                .format("%m-%d %H:%M"),
            summary.title
        );
    }
    Ok(())
}

fn print_agents(runtime: &Runtime, root: &str) {
    fn walk(runtime: &Runtime, id: &str, depth: usize) {
        for child in runtime.children(id) {
            let snap = child.snapshot();
            let activity = if snap.activity.is_empty() {
                String::new()
            } else {
                format!(" — {}", snap.activity)
            };
            println!(
                "{}{} {} {:?}{}",
                "  ".repeat(depth),
                snap.info.label.bold(),
                child.id.as_str().dark_grey(),
                snap.status,
                activity
            );
            walk(runtime, &child.id, depth + 1);
        }
    }
    if runtime.children(root).is_empty() {
        println!("{}", "no subagents yet".dark_grey());
    }
    walk(runtime, root, 0);
}
