//! The full-screen terminal interface, a client of the malac daemon: a
//! workspace's tabs, the agents in each, a menu bar, a status line and
//! function keys, with mouse support throughout. Agents run in the daemon,
//! so they keep working when the interface quits, and other terminals or
//! the web UI see the same tabs.

mod agentmap;
mod app;
mod attach;
mod bars;
mod dialog;
mod editor;
mod files;
mod folder;
mod jobs;
mod markdown;
mod menu;
mod pig;
mod select;
mod term;
mod text;
mod theme;
mod transcript;
mod ui;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{Event as TermEvent, EventStream, KeyCode, KeyEventKind, MouseEventKind};
use crossterm::style::Stylize;
use futures::{FutureExt, StreamExt};
use malac_core::config::Config;
use tokio::sync::mpsc;

use crate::RunArgs;
use crate::client::{Client, Events};
use crate::protocol::Snapshot;
use app::{App, Backend};
pub use term::log_writer;

/// Frames are drawn at most this often; streaming text in between is coalesced.
const FRAME: Duration = Duration::from_millis(16);

/// Opens what the command line asks for (`launch`) and runs the interface
/// until the user quits. The terminal is restored on every way out, panics
/// included.
/// `config` is the user's, for what only the interface uses: whether
/// thinking shows and the models' context windows.
pub async fn run(
    (client, events, snapshot): (Client, Events, Snapshot),
    args: &RunArgs,
    config: Config,
    cwd: &Path,
    prompt: String,
) -> Result<()> {
    let launched = client.launch(args.launch(cwd, &prompt)).await?;

    let (updates_tx, updates) = mpsc::unbounded_channel();
    let (connections_tx, connections) = mpsc::unbounded_channel();
    let backend = Backend::new(client, updates_tx, connections_tx);
    let app = App::new(
        backend,
        config,
        snapshot,
        &launched.workspace,
        launched.tab.as_deref(),
        cwd.to_owned(),
    );

    let mut terminal = term::enter()?;
    let result = event_loop(&mut terminal, app, events, updates, connections).await;
    term::leave();
    let app = result?;
    println!("{}", goodbye(&app).await.dark_grey());
    Ok(())
}

/// One line on what keeps running and how to come back.
async fn goodbye(app: &App) -> String {
    let info = match app.backend.file() {
        Some(file) => tokio::time::timeout(
            Duration::from_secs(2),
            crate::client::get::<crate::protocol::DaemonInfo>(file, "/api/daemon"),
        )
        .await
        .ok()
        .and_then(Result::ok),
        None => None,
    };
    let running = info.map_or_else(|| app.running_count(), |i| i.agents_running);
    let agents = match running {
        0 => "no agents are running".to_owned(),
        1 => "1 agent keeps running in the daemon".to_owned(),
        n => format!("{n} agents keep running in the daemon"),
    };
    format!("{agents} · `malac` reattaches to {}", app.workspace_name)
}

async fn event_loop(
    terminal: &mut term::Tui,
    mut app: App,
    mut events: Events,
    mut updates: mpsc::UnboundedReceiver<app::Update>,
    mut connections: mpsc::UnboundedReceiver<app::Connection>,
) -> Result<App> {
    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_draw = Instant::now() - FRAME;
    let mut listening = true;
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
                let mut batch = vec![event.context("reading the terminal")?];
                while let Some(Some(Ok(next))) = input.next().now_or_never() {
                    batch.push(next);
                }
                for (i, event) in batch.iter().enumerate() {
                    let more = batch[i + 1..].iter().any(is_typing);
                    handle(&mut app, event, more).await;
                }
            }
            message = events.recv(), if listening => match message {
                Some(message) => {
                    app.on_message(message);
                    while let Ok(message) = events.try_recv() {
                        app.on_message(message);
                    }
                }
                None => {
                    listening = false;
                    app.disconnected();
                    app.dirty = true;
                }
            },
            Some(connection) = connections.recv() => {
                let (client, fresh, snapshot) = connection;
                events = fresh;
                listening = true;
                app.reconnected(client, snapshot);
                app.dirty = true;
            }
            Some(update) = updates.recv() => {
                // Events that came before a reply apply first: a history
                // read after them must not be followed by them again.
                while let Ok(message) = events.try_recv() {
                    app.on_message(message);
                }
                app.on_update(update);
            }
            _ = tick.tick() => {
                if app.tick() {
                    app.dirty = true;
                }
            }
            _ = next_frame, if app.dirty => {}
        }
    }
    Ok(app)
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
