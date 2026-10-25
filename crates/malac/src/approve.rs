//! Permission prompts in the terminal, and racing several approvers.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::Stylize;
use crossterm::terminal;
use futures::future::select_all;
use malac_core::permission::{Approver, Decision, PermissionRequest};
use tokio::sync::Mutex;

/// Serializes terminal output so prompts are never interleaved with streaming text.
pub type TerminalLock = Arc<Mutex<()>>;

pub struct TerminalApprover {
    lock: TerminalLock,
    main_agent: Arc<std::sync::RwLock<String>>,
}

impl TerminalApprover {
    pub fn new(lock: TerminalLock, main_agent: Arc<std::sync::RwLock<String>>) -> Self {
        Self { lock, main_agent }
    }
}

#[async_trait]
impl Approver for TerminalApprover {
    async fn approve(&self, request: &PermissionRequest) -> Decision {
        let _guard = self.lock.lock().await;
        let who = if *self.main_agent.read().unwrap() == request.agent {
            String::new()
        } else {
            format!("[{}] ", request.agent_label)
        };
        print!(
            "\n{} {who}{} {}\n  {}  {}  {} ",
            "?".yellow().bold(),
            request.tool.clone().bold(),
            request.summary,
            "[y] allow".green(),
            "[a] always".cyan(),
            "[n] deny".red()
        );
        let _ = std::io::stdout().flush();

        let abort = Arc::new(AtomicBool::new(false));
        let _stop = StopOnDrop(abort.clone());
        let decision = tokio::task::spawn_blocking(move || read_choice(&abort))
            .await
            .unwrap_or(Decision::Deny);
        let label = match decision {
            Decision::Allow => "allowed".green(),
            Decision::AllowSession => "allowed for this session".cyan(),
            Decision::Deny => "denied".red(),
        };
        println!(" {label}");
        decision
    }
}

/// Stops the key-reading thread when the prompt is answered elsewhere.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn read_choice(abort: &AtomicBool) -> Decision {
    if terminal::enable_raw_mode().is_err() {
        return Decision::Deny;
    }
    let decision = loop {
        if abort.load(Ordering::Relaxed) {
            break Decision::Deny;
        }
        if !event::poll(Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }
        let Ok(Event::Key(key)) = event::read() else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => break Decision::Allow,
            KeyCode::Char('a') | KeyCode::Char('A') => break Decision::AllowSession,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => break Decision::Deny,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                break Decision::Deny;
            }
            _ => {}
        }
    };
    let _ = terminal::disable_raw_mode();
    decision
}

/// Asks several approvers at once; the first answer wins.
pub struct FirstAnswer(pub Vec<Arc<dyn Approver>>);

#[async_trait]
impl Approver for FirstAnswer {
    async fn approve(&self, request: &PermissionRequest) -> Decision {
        let asks = self.0.iter().map(|a| a.approve(request));
        let (decision, _, _) = select_all(asks).await;
        decision
    }
}
