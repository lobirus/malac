//! Renders the event stream in the terminal: the main agent in full,
//! subagents as one indented line per step.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use crossterm::style::Stylize;
use malac_core::event::{AgentId, Envelope, Event, Level, TodoStatus, TurnStatus};
use malac_core::message::Usage;
use malac_core::tools::truncate_chars;
use tokio::sync::{broadcast, mpsc};

use crate::approve::TerminalLock;

pub struct Renderer {
    pub lock: TerminalLock,
    pub main: Arc<RwLock<AgentId>>,
    pub show_thinking: bool,
    /// Signalled after the main agent's turn summary has been printed.
    pub turn_done: mpsc::UnboundedSender<()>,
}

#[derive(Default)]
struct View {
    at_line_start: bool,
    in_thinking: bool,
    agents: HashMap<AgentId, AgentView>,
    turn_started: Option<Instant>,
    turn_usage: Usage,
    context: u64,
}

#[derive(Clone)]
struct AgentView {
    label: String,
    depth: usize,
    model: String,
    cwd: String,
}

impl Renderer {
    pub fn spawn(self, mut rx: broadcast::Receiver<Arc<Envelope>>) {
        tokio::spawn(async move {
            let mut view = View {
                at_line_start: true,
                ..Default::default()
            };
            loop {
                let envelope = match rx.recv().await {
                    Ok(envelope) => envelope,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        println!("{}", format!("({n} events skipped)").dark_grey());
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let _guard = self.lock.lock().await;
                if let Event::AgentStarted {
                    label,
                    parent,
                    model,
                    cwd,
                } = &envelope.event
                {
                    let depth = parent
                        .as_ref()
                        .and_then(|p| view.agents.get(p))
                        .map_or(0, |a| a.depth + 1);
                    let agent = AgentView {
                        label: label.clone(),
                        depth,
                        model: model.clone(),
                        cwd: format!("{cwd}/"),
                    };
                    if parent.is_some() {
                        view.break_line();
                        let indent = "  ".repeat(depth);
                        println!(
                            "{}",
                            format!("{indent}↳ {label} started ({model})").dark_cyan()
                        );
                    }
                    view.agents.insert(envelope.agent.clone(), agent);
                    continue;
                }
                if let Event::ModelChanged { model } = &envelope.event {
                    if let Some(agent) = view.agents.get_mut(&envelope.agent) {
                        agent.model = model.clone();
                    }
                }
                let is_main = *self.main.read().unwrap() == envelope.agent;
                if is_main {
                    self.main_event(&mut view, &envelope);
                } else {
                    self.sub_event(&mut view, &envelope);
                }
                let _ = std::io::stdout().flush();
            }
        });
    }

    fn main_event(&self, view: &mut View, envelope: &Envelope) {
        let cwd = view
            .agents
            .get(&envelope.agent)
            .map(|a| a.cwd.clone())
            .unwrap_or_default();
        match &envelope.event {
            Event::ModelChanged { model } => {
                view.break_line();
                println!("{}", format!("model: {model}").dark_grey());
            }
            Event::UserMessage { .. } => {
                view.turn_started = Some(Instant::now());
                view.turn_usage = Usage::default();
                view.in_thinking = false;
            }
            Event::ThinkingDelta { text } => {
                if !view.in_thinking {
                    view.break_line();
                    view.in_thinking = true;
                    if !self.show_thinking {
                        println!("{}", "∴ thinking".dark_grey().italic());
                    }
                }
                if self.show_thinking {
                    print!("{}", text.as_str().dark_grey().italic());
                    view.at_line_start = text.ends_with('\n');
                }
            }
            Event::TextDelta { text } => {
                if view.in_thinking {
                    view.in_thinking = false;
                    view.break_line();
                }
                print!("{text}");
                view.at_line_start = text.ends_with('\n');
            }
            Event::ToolStarted { tool, summary, .. } => {
                view.in_thinking = false;
                view.break_line();
                println!(
                    "{} {} {}",
                    "●".cyan(),
                    tool.as_str().bold(),
                    summary.replace(&cwd, "")
                );
            }
            Event::ToolFinished {
                output,
                is_error,
                millis,
                ..
            } => {
                let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
                let first =
                    truncate_chars(lines.first().copied().unwrap_or("(no output)").trim(), 140);
                let more = match lines.len() {
                    0 | 1 => String::new(),
                    n => format!(" (+{} lines)", n - 1),
                };
                let timing = if *millis >= 1000 {
                    format!(" {:.1}s", *millis as f64 / 1000.0)
                } else {
                    String::new()
                };
                if *is_error {
                    println!("  {} {}", "⎿".dark_grey(), first.red());
                    for line in lines.iter().skip(1).take(4) {
                        println!("    {}", truncate_chars(line, 140).red());
                    }
                } else {
                    println!(
                        "  {} {}",
                        "⎿".dark_grey(),
                        format!("{first}{more}{timing}").dark_grey()
                    );
                }
            }
            Event::Todos { items } => {
                view.break_line();
                for item in items {
                    let line = match item.status {
                        TodoStatus::Completed => format!("  [x] {}", item.content).dark_grey(),
                        TodoStatus::InProgress => format!("  [~] {}", item.content).bold(),
                        TodoStatus::Pending => format!("  [ ] {}", item.content).reset(),
                    };
                    println!("{line}");
                }
            }
            Event::Usage {
                turn,
                context_tokens,
                ..
            } => {
                view.turn_usage.add(turn);
                view.context = *context_tokens;
            }
            Event::Compacted { .. } => {
                view.break_line();
                println!("{}", "↻ conversation compacted".dark_grey());
            }
            Event::Notice { level, message } => {
                view.break_line();
                print_notice(*level, message, "");
            }
            Event::TurnFinished { status, .. } => {
                view.in_thinking = false;
                view.break_line();
                let model = view
                    .agents
                    .get(&envelope.agent)
                    .map(|a| a.model.clone())
                    .unwrap_or_default();
                println!("{}", footer(view, &model, *status).dark_grey());
                let _ = self.turn_done.send(());
            }
            _ => {}
        }
    }

    fn sub_event(&self, view: &mut View, envelope: &Envelope) {
        let Some(agent) = view.agents.get(&envelope.agent).cloned() else {
            return;
        };
        let AgentView {
            label, depth, cwd, ..
        } = agent;
        let indent = "  ".repeat(depth);
        let line = match &envelope.event {
            Event::ToolStarted { tool, summary, .. } => {
                let summary = summary.replace(&cwd, "");
                format!(
                    "{indent}↳ {label} · {tool} {}",
                    truncate_chars(&summary, 100)
                )
                .dark_grey()
            }
            Event::ToolFinished {
                output,
                is_error: true,
                ..
            } => {
                let first = output.lines().next().unwrap_or_default();
                format!("{indent}  ✗ {}", truncate_chars(first, 120)).dark_red()
            }
            Event::AgentFinished { status, .. } => match status {
                TurnStatus::Completed => format!("{indent}✓ {label} finished").dark_green(),
                TurnStatus::Cancelled => format!("{indent}○ {label} cancelled").dark_yellow(),
                TurnStatus::Failed => format!("{indent}✗ {label} failed").red(),
            },
            Event::Notice { level, message } => {
                view.break_line();
                print_notice(*level, message, &format!("{indent}[{label}] "));
                return;
            }
            _ => return,
        };
        view.break_line();
        println!("{line}");
    }
}

impl View {
    fn break_line(&mut self) {
        if !self.at_line_start {
            println!();
            self.at_line_start = true;
        }
    }
}

fn print_notice(level: Level, message: &str, prefix: &str) {
    match level {
        Level::Info => println!("{}", format!("{prefix}{message}").dark_grey()),
        Level::Warning => println!("{}", format!("{prefix}! {message}").yellow()),
        Level::Error => println!("{}", format!("{prefix}✗ {message}").red()),
    }
}

fn footer(view: &View, model: &str, status: TurnStatus) -> String {
    let mut parts = vec![model.to_owned()];
    if view.context > 0 {
        parts.push(format!("{} context", tokens(view.context)));
    }
    if view.turn_usage.output_tokens > 0 {
        parts.push(format!("{} out", tokens(view.turn_usage.output_tokens)));
    }
    if view.turn_usage.cost > 0.0 {
        parts.push(format!("${:.4}", view.turn_usage.cost));
    }
    if let Some(started) = view.turn_started {
        parts.push(format!("{:.1}s", started.elapsed().as_secs_f64()));
    }
    match status {
        TurnStatus::Completed => {}
        TurnStatus::Cancelled => parts.push("cancelled".into()),
        TurnStatus::Failed => parts.push("failed".into()),
    }
    format!("─ {}", parts.join(" · "))
}

pub(crate) fn tokens(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// `--json`: one envelope per line on stdout.
pub fn spawn_json(
    mut rx: broadcast::Receiver<Arc<Envelope>>,
    main: Arc<RwLock<AgentId>>,
    turn_done: mpsc::UnboundedSender<()>,
) {
    tokio::spawn(async move {
        while let Ok(envelope) = rx.recv().await {
            if let Ok(line) = serde_json::to_string(envelope.as_ref()) {
                println!("{line}");
            }
            let finished = matches!(envelope.event, Event::TurnFinished { .. });
            if finished && *main.read().unwrap() == envelope.agent {
                let _ = turn_done.send(());
            }
        }
    });
}
