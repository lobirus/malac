//! The full-screen terminal interface: tabs over sessions, a menu bar, a
//! status line and function keys, with mouse support throughout.

mod app;
mod bars;
mod dialog;
mod editor;
mod markdown;
mod menu;
mod pig;
mod term;
mod text;
mod theme;
mod transcript;
mod ui;

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use crossterm::event::{Event as TermEvent, EventStream, KeyCode, KeyEventKind, MouseEventKind};
use crossterm::style::Stylize;
use futures::{FutureExt, StreamExt};
use malac_core::Envelope;
use malac_core::event::Level;
use malac_core::permission::{Approver, Decision, PermissionRequest};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::repl::Session;
use app::App;
pub use term::log_writer;

/// A permission request on its way to the interface.
pub struct Pending {
    pub request: PermissionRequest,
    pub reply: oneshot::Sender<Decision>,
}

pub type Approvals = mpsc::UnboundedReceiver<Pending>;

/// Routes permission requests into the interface. When the web UI answers
/// first, the request is dropped here and its dialog disappears.
struct TuiApprover(mpsc::UnboundedSender<Pending>);

#[async_trait]
impl Approver for TuiApprover {
    async fn approve(&self, request: &PermissionRequest) -> Decision {
        let (reply, answer) = oneshot::channel();
        let pending = Pending {
            request: request.clone(),
            reply,
        };
        if self.0.send(pending).is_err() {
            return Decision::Deny;
        }
        answer.await.unwrap_or(Decision::Deny)
    }
}

pub fn approver() -> (Arc<dyn Approver>, Approvals) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(TuiApprover(tx)), rx)
}

/// Frames are drawn at most this often; streaming text in between is coalesced.
const FRAME: Duration = Duration::from_millis(16);

/// Runs the interface until the user quits. The terminal is restored on
/// every way out, panics included.
pub async fn run(
    session: Session,
    events: broadcast::Receiver<Arc<Envelope>>,
    approvals: Approvals,
    prompt: String,
) -> Result<()> {
    let mut terminal = term::enter()?;
    let result = event_loop(&mut terminal, session, events, approvals, prompt).await;
    term::leave();
    if let Ok(Some(id)) = &result {
        println!(
            "{}",
            format!("resume this session with: malac -r {id}").dark_grey()
        );
    }
    result.map(|_| ())
}

/// Returns the session to mention on exit, if it has any turns.
async fn event_loop(
    terminal: &mut term::Tui,
    session: Session,
    mut events: broadcast::Receiver<Arc<Envelope>>,
    mut approvals: Approvals,
    prompt: String,
) -> Result<Option<String>> {
    let (tx, mut updates) = mpsc::unbounded_channel();
    let mut app = App::new(session, tx);
    if !prompt.trim().is_empty() {
        app.send(prompt);
    }
    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_draw = Instant::now() - FRAME;
    loop {
        if app.clear_screen {
            terminal.clear()?;
            app.clear_screen = false;
            app.dirty = true;
        }
        if app.dirty && last_draw.elapsed() >= FRAME {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            app.dirty = false;
            last_draw = Instant::now();
        }
        if app.quit {
            break;
        }
        let next_frame = tokio::time::sleep_until((last_draw + FRAME).into());
        tokio::select! {
            event = input.next() => {
                let Some(event) = event else { break };
                let mut batch = vec![event?];
                while let Some(Some(Ok(next))) = input.next().now_or_never() {
                    batch.push(next);
                }
                for (i, event) in batch.iter().enumerate() {
                    let more = batch[i + 1..].iter().any(is_typing);
                    handle(&mut app, event, more).await;
                }
            }
            received = events.recv() => match received {
                Ok(envelope) => {
                    app.on_bus(&envelope);
                    while let Ok(envelope) = events.try_recv() {
                        app.on_bus(&envelope);
                    }
                    app.dirty = true;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    app.notice(Level::Warning, format!("{n} events were skipped"));
                    app.dirty = true;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            Some(pending) = approvals.recv() => app.on_approval(pending),
            Some(update) = updates.recv() => app.on_update(update),
            _ = tick.tick() => {
                if app.tick() {
                    app.dirty = true;
                }
            }
            _ = next_frame, if app.dirty => {}
        }
    }
    let tab = app.tab();
    let had_turns = app
        .agents
        .get(tab.root())
        .is_some_and(|v| v.transcript.has_turns());
    Ok(had_turns.then(|| tab.root().to_owned()))
}

fn is_typing(event: &TermEvent) -> bool {
    matches!(event, TermEvent::Key(key) if key.kind != KeyEventKind::Release
        && matches!(key.code, KeyCode::Char(_) | KeyCode::Enter))
}

async fn handle(app: &mut App, event: &TermEvent, more_input: bool) {
    let action = match event {
        TermEvent::Key(key) if key.kind != KeyEventKind::Release => {
            app.dirty = true;
            app.on_key(*key, more_input)
        }
        TermEvent::Mouse(mouse) => {
            if mouse.kind != MouseEventKind::Moved {
                app.dirty = true;
            }
            app.on_mouse(*mouse)
        }
        TermEvent::Paste(text) => {
            app.on_paste(text);
            app.dirty = true;
            None
        }
        TermEvent::Resize(..) => {
            app.dirty = true;
            None
        }
        _ => None,
    };
    if let Some(action) = action {
        app.perform(action).await;
    }
}
