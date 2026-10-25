//! `malac [prompt]`: run a prompt, then keep the conversation going.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crossterm::style::Stylize;
use malac_core::config::{self, Config};
use malac_core::permission::{Approver, DenyAll, Mode};
use malac_core::{AgentHandle, AgentSpec, Block, Runtime, session};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use tokio::sync::{Mutex, mpsc};

use crate::approve::{FirstAnswer, TerminalApprover, TerminalLock};
use crate::render::{self, Renderer};
use crate::web;
use crate::{RunArgs, SessionOptions};

struct Session {
    runtime: Arc<Runtime>,
    handle: AgentHandle,
    main: Arc<RwLock<String>>,
    turn_done: mpsc::UnboundedReceiver<()>,
    terminal_approver: Option<Arc<dyn Approver>>,
    web_url: Option<String>,
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

    let runtime = Runtime::new(config, cwd.clone())?;
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
    let terminal_approver: Option<Arc<dyn Approver>> = stdin_tty
        .then(|| Arc::new(TerminalApprover::new(lock.clone(), main.clone())) as Arc<dyn Approver>);
    runtime.set_approver(
        terminal_approver
            .clone()
            .unwrap_or_else(|| Arc::new(DenyAll)),
    );
    if terminal_approver.is_none() && mode != Mode::Autopilot {
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
        runtime,
        handle,
        main,
        turn_done,
        terminal_approver,
        web_url: None,
    };
    if args.web {
        session.start_web().await?;
    }

    if interactive {
        print_banner(&session);
    }
    let mut ok = true;
    if !prompt.trim().is_empty() {
        ok = session.turn(prompt).await;
    }
    if !interactive {
        session.runtime.shutdown().await;
        return Ok(if ok { 0 } else { 1 });
    }
    let result = session.repl().await;
    session.runtime.shutdown().await;
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

fn print_banner(session: &Session) {
    let snap = session.handle.snapshot();
    let line = format!(
        "malac {} · {} · {} mode · {}",
        env!("CARGO_PKG_VERSION"),
        snap.model,
        session.runtime.policy.mode().as_str(),
        snap.info.cwd.display()
    );
    println!("{}", line.dark_grey());
    println!(
        "{}",
        "/help for commands · Ctrl-C interrupts · Ctrl-D exits".dark_grey()
    );
    if let Some(url) = &session.web_url {
        println!("{}", format!("web UI: {url}").dark_grey());
    }
}

enum Flow {
    Continue,
    Exit,
}

impl Session {
    /// Runs one prompt; Ctrl-C cancels it and every subagent it started.
    async fn turn(&mut self, text: String) -> bool {
        let runtime = self.runtime.clone();
        let id = self.handle.id.clone();
        let watcher = tokio::spawn(async move {
            while tokio::signal::ctrl_c().await.is_ok() {
                runtime.cancel_tree(&id);
            }
        });
        let result = self.handle.prompt(vec![Block::text(text)]).await;
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

    fn switch_to(&mut self, handle: AgentHandle) {
        *self.main.write().unwrap() = handle.id.clone();
        self.handle = handle;
    }

    async fn start_web(&mut self) -> Result<()> {
        if self.web_url.is_some() {
            return Ok(());
        }
        let (url, web_approver) = web::spawn_for(self.runtime.clone(), None).await?;
        let approver: Arc<dyn Approver> = match &self.terminal_approver {
            Some(terminal) => Arc::new(FirstAnswer(vec![terminal.clone(), web_approver])),
            None => web_approver,
        };
        self.runtime.set_approver(approver);
        self.web_url = Some(url);
        Ok(())
    }

    async fn command(&mut self, input: &str) -> Result<Flow> {
        let (name, arg) = input
            .split_once(' ')
            .map_or((input, ""), |(n, a)| (n, a.trim()));
        match name {
            "exit" | "quit" | "q" => return Ok(Flow::Exit),
            "help" | "?" => print_help(),
            "model" if arg.is_empty() => {
                println!("current: {}", self.handle.snapshot().model);
                for (name, provider) in &self.runtime.config.providers {
                    if !provider.models.is_empty() {
                        println!("  {name}: {}", provider.models.join(", "));
                    }
                }
            }
            "model" => self.handle.set_model(arg),
            "mode" if arg.is_empty() => println!("mode: {}", self.runtime.policy.mode().as_str()),
            "mode" => {
                let mode: Mode = arg.parse()?;
                self.runtime.policy.set_mode(mode);
                println!("mode: {}", mode.as_str());
            }
            "compact" => {
                self.handle.compact();
                self.handle.wait_idle().await;
            }
            "clear" | "new" => {
                let handle = self.runtime.spawn_agent(AgentSpec::default())?;
                self.switch_to(handle);
                println!("{}", "started a new session".dark_grey());
            }
            "resume" if arg.is_empty() => print_sessions(10)?,
            "resume" => {
                let handle = self.runtime.resume_agent(arg)?;
                self.switch_to(handle);
                println!("{}", format!("resumed {arg}").dark_grey());
            }
            "sessions" => print_sessions(20)?,
            "agents" => print_agents(&self.runtime, &self.handle.id),
            "cost" | "usage" => {
                let usage = self.handle.snapshot().usage;
                println!(
                    "input {} · cached {} · output {} · ${:.4}",
                    usage.input_tokens, usage.cache_read_tokens, usage.output_tokens, usage.cost
                );
            }
            "mcp" => {
                for server in self.runtime.servers() {
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
            "memory" => {
                for entry in self.runtime.memory.list() {
                    println!("[{:?}] {}: {}", entry.scope, entry.name, entry.description);
                }
            }
            "web" => {
                self.start_web().await?;
                if let Some(url) = &self.web_url {
                    println!("web UI: {url}");
                }
            }
            other => bail!("unknown command /{other} (try /help)"),
        }
        Ok(Flow::Continue)
    }
}

fn print_help() {
    let rows = [
        ("/model [provider/model]", "show or switch the model"),
        (
            "/mode [ask|edits|autopilot]",
            "show or change the permission mode",
        ),
        ("/agents", "show subagents and what they are doing"),
        ("/compact", "summarize the conversation to free context"),
        ("/clear", "start a new session"),
        ("/resume [id]", "list sessions or resume one"),
        ("/cost", "token usage and cost of this session"),
        ("/mcp", "connected tool servers"),
        ("/memory", "saved memories"),
        ("/web", "open the web UI for this session"),
        ("/exit", "quit"),
    ];
    for (command, help) in rows {
        println!("  {:<30} {}", command.bold(), help);
    }
}

fn print_sessions(limit: usize) -> Result<()> {
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
