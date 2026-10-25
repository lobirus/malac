//! The line-based interface (`--plain`, or `ui.tui = false`) as a client of
//! the daemon: the conversation lives in a daemon tab, so it keeps running
//! when the terminal closes and every other client sees it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result, bail};
use crossterm::style::Stylize;
use malac_core::config;
use malac_core::event::{AgentId, Envelope, Event};
use malac_core::permission::{Approver, Mode, PermissionRequest};
use malac_core::update;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use serde_json::Value;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::RunArgs;
use crate::approve::{TerminalApprover, TerminalLock};
use crate::client::{Client, Events};
use crate::protocol::{Command, ServerMessage, Snapshot, TabId, TabView};
use crate::render::Renderer;
use crate::repl::{PIG, Slash, TAGLINE, print_help, print_sessions, update_notice};

struct Remote {
    client: Arc<Client>,
    tab: TabId,
    main: Arc<RwLock<AgentId>>,
    turn_done: mpsc::UnboundedReceiver<()>,
    /// The last `/model` listing, so `/model 3` can pick from it.
    model_choices: Vec<String>,
}

/// Runs the session in a daemon tab over an open connection.
pub async fn run(
    (client, events, snapshot): (Client, Events, Snapshot),
    args: &RunArgs,
    cwd: &Path,
    prompt: String,
    show_thinking: bool,
) -> Result<i32> {
    let client = Arc::new(client);
    let launched = client.launch(args.launch(cwd, &prompt)).await?;
    let tab = launched.tab.context("the daemon opened no tab")?;
    let main = match launched.agent {
        Some(agent) => agent,
        None => start_agent(&client, &tab).await?,
    };
    let main = Arc::new(RwLock::new(main));

    let lock: TerminalLock = Arc::new(Mutex::new(()));
    let (bus, _) = broadcast::channel(8192);
    let (done_tx, turn_done) = mpsc::unbounded_channel();
    Renderer {
        lock: lock.clone(),
        main: main.clone(),
        show_thinking,
        turn_done: done_tx,
    }
    .spawn(bus.subscribe());
    let relay = relay(events, bus, client.clone(), tab.clone(), lock, main.clone());

    let view = launched.workspace.tab(&tab).cloned();
    print_banner(&snapshot, &launched.workspace.name, view.as_ref(), &client);
    let mut remote = Remote {
        client,
        tab,
        main,
        turn_done,
        model_choices: Vec::new(),
    };
    if !prompt.trim().is_empty() {
        // The daemon already sent it with the launch.
        remote.wait_for_turn().await;
        println!();
    }
    let result = remote.repl().await;
    relay.abort();
    if result.is_ok() {
        println!(
            "{}",
            "the session keeps running in the daemon; `malac --plain` returns to it".dark_grey()
        );
    }
    result.map(|_| 0)
}

async fn start_agent(client: &Client, tab: &str) -> Result<AgentId> {
    let reply = client
        .request(Command::AgentStart {
            tab: tab.to_owned(),
            model: None,
            label: None,
        })
        .await?;
    match reply {
        ServerMessage::Ack {
            agent: Some(agent), ..
        } => Ok(agent),
        other => bail!("unexpected reply: {other:?}"),
    }
}

/// Feeds this tab's events to the renderer and asks in the terminal when
/// one of its agents needs permission, until another client answers.
fn relay(
    mut events: Events,
    bus: broadcast::Sender<Arc<Envelope>>,
    client: Arc<Client>,
    tab: TabId,
    lock: TerminalLock,
    main: Arc<RwLock<AgentId>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let approver = Arc::new(TerminalApprover::new(lock, main));
        let mut labels: HashMap<AgentId, String> = HashMap::new();
        let mut asking: HashMap<String, JoinHandle<()>> = HashMap::new();
        while let Some(message) = events.recv().await {
            match message {
                ServerMessage::Event {
                    tab: Some(from),
                    envelope,
                    ..
                } if from == tab => {
                    match &envelope.event {
                        Event::AgentStarted { label, .. } => {
                            labels.insert(envelope.agent.clone(), label.clone());
                        }
                        Event::PermissionRequested {
                            request_id,
                            call_id,
                            tool,
                            summary,
                            input,
                        } => {
                            let request = PermissionRequest {
                                id: request_id.clone(),
                                call_id: call_id.clone(),
                                agent: envelope.agent.clone(),
                                agent_label: labels
                                    .get(&envelope.agent)
                                    .cloned()
                                    .unwrap_or_default(),
                                tool: tool.clone(),
                                summary: summary.clone(),
                                input: input.clone(),
                            };
                            let (approver, client) = (approver.clone(), client.clone());
                            let ask = tokio::spawn(async move {
                                let decision = approver.approve(&request).await;
                                let _ = client.send(Command::Approve {
                                    request_id: request.id,
                                    decision,
                                });
                            });
                            asking.insert(request_id.clone(), ask);
                        }
                        Event::PermissionResolved { request_id, .. } => {
                            if let Some(ask) = asking.remove(request_id) {
                                ask.abort();
                            }
                        }
                        _ => {}
                    }
                    let _ = bus.send(Arc::from(envelope));
                }
                ServerMessage::Event {
                    tab: None,
                    envelope,
                    ..
                } if matches!(envelope.event, Event::Update { .. }) => {
                    if let Event::Update { status } = &envelope.event {
                        if let Some(notice) = update_notice(status) {
                            println!("{}", notice.green());
                        }
                    }
                }
                ServerMessage::Notice {
                    message, tab: from, ..
                } if from.as_deref().is_none_or(|t| t == tab) => {
                    println!("{}", format!("! {message}").yellow());
                }
                ServerMessage::TabClosed { tab: closed, .. } if closed == tab => {
                    println!("{}", "this tab was closed by another client".yellow());
                }
                ServerMessage::Shutdown { reason } => {
                    println!("{}", format!("the daemon stopped ({reason})").yellow());
                    break;
                }
                _ => {}
            }
        }
    })
}

fn print_banner(snapshot: &Snapshot, workspace: &str, tab: Option<&TabView>, client: &Client) {
    let main = tab.and_then(|t| t.active_agent.as_ref());
    let model = tab
        .and_then(|t| {
            t.snapshots
                .iter()
                .find(|s| Some(&s.snapshot.info.id) == main)
        })
        .map(|s| s.snapshot.model.clone())
        .or_else(|| snapshot.default_model.clone())
        .unwrap_or_default();
    let mode = tab.map_or(Mode::Ask, |t| t.mode);
    let info = [
        format!("malac {} · {TAGLINE}", update::CURRENT).bold(),
        format!("{model} · {} mode", mode.as_str()).reset(),
        match tab {
            Some(tab) => format!("{workspace} › {} · {}", tab.name, tab.cwd.display()),
            None => workspace.to_owned(),
        }
        .dark_grey(),
        "/help for commands · Ctrl-C interrupts · Ctrl-D detaches"
            .to_owned()
            .dark_grey(),
        format!("web UI: {}", client.file().web_url()).dark_grey(),
    ];
    for (art, text) in PIG.iter().zip(info) {
        println!("{}{text}", art.bold());
    }
    let indent = " ".repeat(PIG[0].len());
    if snapshot.daemon.version_mismatch {
        println!(
            "{indent}{}",
            format!(
                "the daemon runs malac {}; `malac daemon restart` switches it to {}",
                snapshot.daemon.version,
                update::CURRENT
            )
            .yellow()
        );
    } else if let Some(notice) = update_notice(&snapshot.update) {
        println!("{indent}{}", notice.green());
    }
    println!();
}

enum Flow {
    Continue,
    Exit,
}

impl Remote {
    fn main(&self) -> AgentId {
        self.main.read().unwrap().clone()
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
                    } else if let Err(err) = self.turn(line).await {
                        println!("{}", format!("✗ {err:#}").red());
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

    async fn turn(&mut self, text: String) -> Result<()> {
        while self.turn_done.try_recv().is_ok() {}
        self.client
            .request(Command::Prompt {
                tab: Some(self.tab.clone()),
                agent: Some(self.main()),
                text,
                images: Vec::new(),
                attachments: Vec::new(),
                model: None,
            })
            .await?;
        self.wait_for_turn().await;
        Ok(())
    }

    /// Waits until the main agent's turn is over; Ctrl-C cancels it and
    /// every subagent it started.
    async fn wait_for_turn(&mut self) {
        loop {
            tokio::select! {
                _ = self.turn_done.recv() => return,
                interrupted = tokio::signal::ctrl_c() => {
                    if interrupted.is_err() {
                        return;
                    }
                    let _ = self.client.send(Command::Cancel { agent: self.main() });
                }
            }
        }
    }

    async fn snapshot(&self) -> Result<Snapshot> {
        match self
            .client
            .request(Command::Snapshot { workspace: None })
            .await?
        {
            ServerMessage::Snapshot(snapshot) => Ok(*snapshot),
            other => bail!("unexpected reply: {other:?}"),
        }
    }

    async fn tab_view(&self) -> Result<TabView> {
        let snapshot = self.snapshot().await?;
        snapshot
            .workspace
            .and_then(|w| w.tab(&self.tab).cloned())
            .context("this tab is gone")
    }

    async fn command(&mut self, input: &str) -> Result<Flow> {
        match Slash::parse(input)? {
            Slash::Exit => return Ok(Flow::Exit),
            Slash::Help => print_help(),
            Slash::Model("") => {
                let snapshot = self.snapshot().await?;
                let current = snapshot
                    .workspace
                    .as_ref()
                    .and_then(|w| w.tab(&self.tab))
                    .and_then(|t| {
                        t.snapshots
                            .iter()
                            .find(|s| s.snapshot.info.id == self.main())
                    })
                    .map(|s| s.snapshot.model.clone())
                    .unwrap_or_default();
                self.model_choices = snapshot.models;
                for (i, spec) in self.model_choices.iter().enumerate() {
                    let marker = if *spec == current { "*" } else { " " };
                    println!("  {marker} {:>3}  {spec}", i + 1);
                }
                println!(
                    "{}",
                    "/model <number> or /model provider/model to switch".dark_grey()
                );
            }
            Slash::Model(arg) => {
                let model = match arg.parse::<usize>() {
                    Ok(n) => self
                        .model_choices
                        .get(n.wrapping_sub(1))
                        .cloned()
                        .with_context(|| format!("no model #{n}; run /model to list them"))?,
                    Err(_) => arg.to_owned(),
                };
                self.client
                    .request(Command::Model {
                        agent: self.main(),
                        model,
                    })
                    .await?;
            }
            Slash::Mode("") => println!("mode: {}", self.tab_view().await?.mode.as_str()),
            Slash::Mode(arg) => {
                let mode: Mode = arg.parse()?;
                self.client
                    .request(Command::Mode {
                        mode,
                        tab: Some(self.tab.clone()),
                    })
                    .await?;
                println!("mode: {}", mode.as_str());
            }
            Slash::Compact => {
                self.client
                    .request(Command::Compact { agent: self.main() })
                    .await?;
                println!("{}", "compacting the conversation".dark_grey());
            }
            Slash::Clear => {
                let agent = start_agent(&self.client, &self.tab).await?;
                *self.main.write().unwrap() = agent;
                println!("{}", "started a new agent in this tab".dark_grey());
            }
            Slash::Resume("") => print_sessions(10)?,
            Slash::Resume(id) => {
                let reply = self
                    .client
                    .request(Command::AgentOpen {
                        agent: id.to_owned(),
                        tab: Some(self.tab.clone()),
                    })
                    .await?;
                if let ServerMessage::Ack {
                    agent: Some(agent), ..
                } = reply
                {
                    *self.main.write().unwrap() = agent;
                }
                println!("{}", format!("resumed {id}").dark_grey());
            }
            Slash::Sessions => print_sessions(20)?,
            Slash::Agents => {
                let view = self.tab_view().await?;
                let main = self.main();
                let children: Vec<_> = view
                    .snapshots
                    .iter()
                    .filter(|s| s.snapshot.info.parent.is_some())
                    .collect();
                if children.is_empty() {
                    println!("{}", "no subagents yet".dark_grey());
                }
                fn walk(all: &[&crate::protocol::AgentView], parent: &str, depth: usize) {
                    for agent in all
                        .iter()
                        .filter(|a| a.snapshot.info.parent.as_deref() == Some(parent))
                    {
                        let snap = &agent.snapshot;
                        let activity = if snap.activity.is_empty() {
                            String::new()
                        } else {
                            format!(" — {}", snap.activity)
                        };
                        println!(
                            "{}{} {} {:?}{activity}",
                            "  ".repeat(depth),
                            snap.info.label.as_str().bold(),
                            snap.info.id.as_str().dark_grey(),
                            snap.status,
                        );
                        walk(all, &snap.info.id, depth + 1);
                    }
                }
                walk(&children, &main, 0);
            }
            Slash::Cost => {
                let view = self.tab_view().await?;
                let usage = view
                    .snapshots
                    .iter()
                    .find(|s| s.snapshot.info.id == self.main())
                    .map(|s| s.snapshot.usage)
                    .unwrap_or_default();
                println!(
                    "input {} · cached {} · output {} · ${:.4}",
                    usage.input_tokens, usage.cache_read_tokens, usage.output_tokens, usage.cost
                );
            }
            Slash::Mcp => {
                let tools: Value = self.client.get("/api/tools").await?;
                for server in tools["servers"].as_array().into_iter().flatten() {
                    let name = server["name"].as_str().unwrap_or_default();
                    match server["error"].as_str() {
                        Some(err) => println!(
                            "{} {name}: {}",
                            "✗".red(),
                            err.lines().next().unwrap_or_default()
                        ),
                        None => println!("{} {name}: {} tools", "●".green(), server["tools"]),
                    }
                }
            }
            Slash::Memory => {
                let memory: Value = self.client.get("/api/memory").await?;
                for entry in memory["entries"].as_array().into_iter().flatten() {
                    println!(
                        "[{}] {}: {}",
                        entry["scope"].as_str().unwrap_or_default(),
                        entry["name"].as_str().unwrap_or_default(),
                        entry["description"].as_str().unwrap_or_default()
                    );
                }
            }
            Slash::Web(_) => {
                for url in &self.client.file().urls {
                    println!("web UI: {url}");
                }
            }
        }
        Ok(Flow::Continue)
    }
}
