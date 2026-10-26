//! The interface as a whole. Most tests build it from a made-up daemon
//! snapshot over a backend that records what it would send; the last ones
//! run two terminals against a real daemon in this process.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use malac_core::agent::AgentInfo;
use malac_core::config::Config;
use malac_core::event::{Envelope, Event, TurnStatus};
use malac_core::jobs::{JobInfo, JobOutput, JobStatus};
use malac_core::message::{Block, Message, StopReason, Usage};
use malac_core::permission::{Decision, Mode};
use malac_core::provider::{Completion, Delta, DeltaSink, Provider, Request};
use malac_core::{AgentSnapshot, AgentStatus, update};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::app::{Action, App, Backend, Overlay, Reply, Rest, Update};
use super::text;
use super::theme::theme;
use super::ui::draw;
use crate::client::{Client, DaemonFile, Events};
use crate::daemon::{Daemon, ServeArgs};
use crate::protocol::{
    AgentView, Command, ControlStatus, DaemonStatus, JobView, Launch, ServerMessage, Snapshot,
    TabOptions, TabView, WorkspaceChoice, WorkspaceSummary, WorkspaceView,
};

const WORKSPACE: &str = "w1";
const TAB: &str = "t1";
const AGENT: &str = "a1";

/// A malac home for these tests: sessions and workspaces written here, and
/// a mock provider for the daemon tests.
fn scratch_home() -> &'static PathBuf {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let dir = std::env::temp_dir().join("malac-tui-tests");
        // Workspaces and sessions of earlier runs would only get in the way.
        for old in ["workspaces", "sessions", "state.json"] {
            let path = dir.join(old);
            let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
        }
        std::fs::create_dir_all(dir.join("project")).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            r#"
model = "mock/test"
mode = "ask"

[providers.mock]
kind = "openai"
base_url = "http://127.0.0.1:9/v1"
models = ["test"]

[control]
enabled = false

[update]
check = false

[memory]
enabled = false

[daemon]
idle_minutes = 0
"#,
        )
        .unwrap();
        unsafe { std::env::set_var("MALAC_HOME", &dir) };
        dir
    })
}

fn project() -> PathBuf {
    scratch_home().join("project").canonicalize().unwrap()
}

fn agent(id: &str, label: &str, parent: Option<&str>) -> AgentView {
    AgentView {
        snapshot: AgentSnapshot {
            info: AgentInfo {
                id: id.into(),
                label: label.into(),
                parent: parent.map(Into::into),
                depth: u32::from(parent.is_some()),
                read_only: false,
                cwd: project(),
                created: chrono::Utc::now(),
            },
            model: "lora/smart".into(),
            status: AgentStatus::Idle,
            activity: String::new(),
            usage: Usage::default(),
            report: None,
        },
        live: true,
        title: String::new(),
    }
}

fn tab(id: &str, name: &str, agents: &[&str]) -> TabView {
    TabView {
        id: id.into(),
        name: name.into(),
        cwd: project(),
        agents: agents.iter().map(|a| (*a).to_owned()).collect(),
        active_agent: agents.first().map(|a| (*a).to_owned()),
        mode: Mode::Ask,
        options: TabOptions::default(),
        loaded: true,
        snapshots: agents.iter().map(|a| agent(a, "main", None)).collect(),
    }
}

fn workspace(tabs: Vec<TabView>) -> WorkspaceView {
    WorkspaceView {
        id: WORKSPACE.into(),
        name: "default".into(),
        active_tab: tabs.first().map(|t| t.id.clone()),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
        tabs,
    }
}

fn summary(id: &str, name: &str) -> WorkspaceSummary {
    WorkspaceSummary {
        id: id.into(),
        name: name.into(),
        tabs: 1,
        active_tab: None,
        running: 0,
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        client_ref: None,
        daemon: DaemonStatus {
            version: update::CURRENT.into(),
            pid: 1,
            started: chrono::Utc::now(),
            version_mismatch: false,
            clients: 1,
            attached: false,
            remote: false,
            user_home: None,
        },
        workspaces: vec![summary(WORKSPACE, "default"), summary("w2", "release")],
        workspace: None,
        pending: Vec::new(),
        jobs: Vec::new(),
        update: update::Status {
            current: update::CURRENT.into(),
            latest: None,
            available: false,
            installed: None,
            checked_at: None,
            method: update::Method::SelfUpdate,
            release_url: String::new(),
            hint: None,
        },
        control: ControlStatus::default(),
        models: vec!["lora/smart".into(), "lora/fast".into()],
        default_model: Some("lora/smart".into()),
    }
}

/// An interface on a one-tab workspace whose agent has no history yet.
fn app() -> App {
    app_with(snapshot(), workspace(vec![tab(TAB, "project", &[AGENT])]))
}

fn app_with(snapshot: Snapshot, workspace: WorkspaceView) -> App {
    scratch_home();
    let (backend, _) = Backend::recording();
    let tab = workspace.active_tab.clone();
    let mut app = App::new(
        backend,
        Config::default(),
        snapshot,
        &workspace,
        tab.as_deref(),
        project(),
    );
    answer_history(&mut app);
    app.recent.clear();
    app.branch = Some("main".into());
    app
}

/// Answers every history request so far with an empty conversation.
fn answer_history(app: &mut App) {
    let asked: Vec<Rest> = std::mem::take(&mut *app.backend.rests.lock().unwrap());
    for rest in asked {
        if let Rest::History { .. } = rest {
            app.on_update(Update::Rest(rest, Ok(json!({}))));
        }
    }
}

/// The commands the interface sent since the last look.
fn sent(app: &App) -> Vec<Command> {
    std::mem::take(&mut *app.backend.sent.lock().unwrap())
}

/// The same, as the JSON that goes over the wire.
fn sent_json(app: &App) -> Vec<Value> {
    sent(app)
        .iter()
        .map(|c| serde_json::to_value(c).unwrap())
        .collect()
}

fn envelope(agent: &str, event: Event) -> Box<Envelope> {
    Box::new(Envelope {
        agent: agent.into(),
        seq: 0,
        at: chrono::Utc::now(),
        event,
    })
}

fn emit_as(app: &mut App, agent: &str, event: Event) {
    app.on_message(ServerMessage::Event {
        tab: Some(TAB.into()),
        workspace: Some(WORKSPACE.into()),
        envelope: envelope(agent, event),
    });
}

fn emit(app: &mut App, event: Event) {
    emit_as(app, AGENT, event);
}

fn draw_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    app.tick();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn screen(buffer: &Buffer) -> String {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            let row: String = (0..area.width).map(|x| buffer[(x, y)].symbol()).collect();
            row.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render(app: &mut App, width: u16, height: u16) -> String {
    screen(&draw_buffer(app, width, height))
}

fn copied() -> Option<String> {
    super::term::COPIED.with(|c| c.borrow_mut().take())
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(press(KeyCode::Char(c)), false);
    }
}

/// Presses a key and performs what it asks for.
async fn key(app: &mut App, code: KeyCode) {
    if let Some(action) = app.on_key(press(code), false) {
        app.perform(action).await;
    }
}

fn mouse(app: &mut App, kind: MouseEventKind, x: u16, y: u16) -> Option<Action> {
    app.on_mouse(MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    })
}

/// Presses and releases; what either asks for.
fn click(app: &mut App, x: u16, y: u16, button: MouseButton) -> Option<Action> {
    let pressed = mouse(app, MouseEventKind::Down(button), x, y);
    let released = mouse(app, MouseEventKind::Up(button), x, y);
    pressed.or(released)
}

/// Where `needle` is on screen.
fn find(frame: &str, needle: &str) -> (u16, u16) {
    for (y, line) in frame.lines().enumerate() {
        if let Some(i) = line.find(needle) {
            return (text::width(&line[..i]) as u16, y as u16);
        }
    }
    panic!("`{needle}` is not on screen:\n{frame}");
}

fn a_turn(app: &mut App) {
    emit(
        app,
        Event::UserMessage {
            text: "run the tests".into(),
        },
    );
    emit(
        app,
        Event::TextDelta {
            text: "Running the **test suite** now.".into(),
        },
    );
    emit(
        app,
        Event::ToolStarted {
            call_id: "c1".into(),
            tool: "bash".into(),
            summary: "cargo test".into(),
            input: json!({"command": "cargo test"}),
        },
    );
    emit(
        app,
        Event::ToolFinished {
            call_id: "c1".into(),
            tool: "bash".into(),
            output: "running 46 tests\ntest result: ok. 46 passed".into(),
            is_error: false,
            millis: 2300,
        },
    );
    emit(
        app,
        Event::TextDelta {
            text: "All 46 tests pass.".into(),
        },
    );
    emit(
        app,
        Event::TurnFinished {
            status: TurnStatus::Completed,
            text: "All 46 tests pass.".into(),
        },
    );
}

fn answer(app: &mut App, prompt: &str, text: &str) {
    emit(
        app,
        Event::UserMessage {
            text: prompt.into(),
        },
    );
    emit(app, Event::TextDelta { text: text.into() });
    emit(
        app,
        Event::TurnFinished {
            status: TurnStatus::Completed,
            text: text.into(),
        },
    );
}

#[tokio::test]
async fn answers_lead_and_tool_calls_recede() {
    let mut app = app();
    a_turn(&mut app);
    let buffer = draw_buffer(&mut app, 90, 24);
    let frame = screen(&buffer);
    println!("{frame}");
    let t = theme();
    let (x, y) = find(&frame, "All 46 tests pass.");
    assert_eq!(buffer[(x - 2, y)].symbol(), "┃", "the answer has a rule");
    assert_eq!(buffer[(x - 2, y)].fg, t.accent, "in the accent");
    assert_eq!(
        buffer[(x, y)].fg,
        Color::Reset,
        "its text is the plain foreground"
    );
    assert!(!buffer[(x, y)].modifier.contains(Modifier::DIM));
    let (x, y) = find(&frame, "Running the");
    assert_eq!(buffer[(x - 2, y)].symbol(), "│");
    assert_eq!(
        buffer[(x - 2, y)].fg,
        t.faint,
        "text before the answer has a quiet rule"
    );
    let (tool_x, tool_y) = find(&frame, "bash cargo test");
    assert_eq!(buffer[(tool_x, tool_y)].fg, t.dim, "tool calls are grey");
    assert_eq!(tool_x, x + 2, "and nested under the text");
    assert_eq!(
        frame.lines().nth(tool_y as usize + 1),
        Some(""),
        "on one line"
    );
    let (x, y) = find(&frame, "› run the tests");
    assert_eq!(buffer[(x + 2, y)].bg, t.code_bg, "prompts keep their band");
}

#[tokio::test]
async fn dragging_selects_text_and_copies_it() {
    let mut app = app();
    answer(
        &mut app,
        "why",
        "The parser drops the last token when the input ends without a newline. \
         A final check fixes it.\n\nRun `cargo test` to see.",
    );
    let frame = render(&mut app, 60, 24);
    let (x0, y0) = find(&frame, "parser");
    let (x1, y1) = find(&frame, "final");
    assert!(y1 > y0, "the paragraph wraps");
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x0, y0);
    mouse(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        x1 + 4,
        y1,
    );
    let buffer = draw_buffer(&mut app, 60, 24);
    let t = theme();
    assert_eq!(
        buffer[(x0, y0)].bg,
        t.highlight_bg,
        "the selection is highlighted"
    );
    assert_ne!(buffer[(x0 - 1, y0)].bg, t.highlight_bg);
    assert_eq!(buffer[(x1 + 4, y1)].bg, t.highlight_bg);
    assert_ne!(buffer[(x1 + 5, y1)].bg, t.highlight_bg);
    let released = mouse(&mut app, MouseEventKind::Up(MouseButton::Left), x1 + 4, y1);
    assert_eq!(released, None, "a drag is not a click");
    let text = copied().unwrap();
    assert!(text.starts_with("parser drops the last token"), "{text}");
    assert!(
        text.ends_with("newline. A final"),
        "wrapped rows join with a space: {text}"
    );
    assert!(render(&mut app, 60, 24).contains("✓ Copied"));

    app.on_key(press(KeyCode::Esc), false);
    assert!(app.selection.is_none(), "Esc clears the selection");

    let (x, y) = find(&frame, "cargo");
    click(&mut app, x + 1, y, MouseButton::Left);
    click(&mut app, x + 1, y, MouseButton::Left);
    assert_eq!(
        copied().as_deref(),
        Some("cargo"),
        "a double click takes a word"
    );
    let (x, y) = find(&frame, "without");
    for _ in 0..3 {
        click(&mut app, x, y, MouseButton::Left);
    }
    let line = copied().unwrap();
    assert!(
        line.starts_with("The parser") && line.ends_with("fixes it."),
        "{line}"
    );
    click(&mut app, 0, 0, MouseButton::Left);
    assert!(app.selection.is_none(), "a click elsewhere clears it");
}

#[tokio::test]
async fn dragging_past_the_top_scrolls_the_selection_along() {
    let mut app = app();
    let rows: Vec<String> = (1..=40).map(|i| format!("row {i}")).collect();
    answer(&mut app, "count", &rows.join("\n\n"));
    let frame = render(&mut app, 60, 24);
    let first_shown = (1..=40)
        .find(|i| frame.contains(&format!("row {i}\n")))
        .unwrap();
    let (x, y) = find(&frame, "row 40");
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x + 5, y);
    let above = app.geometry.transcript.y - 1;
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), x, above);
    assert!(
        app.tab().scroll.is_some(),
        "dragging past the top scrolls up"
    );
    for _ in 0..6 {
        app.tick();
    }
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), x, above);
    let text = copied().unwrap();
    assert!(text.ends_with("row 39\n\nrow 40"), "{text}");
    let earlier = format!("row {}", first_shown - 2);
    assert!(
        text.contains(&earlier),
        "holding past the edge keeps scrolling: {text}"
    );
}

#[tokio::test]
async fn the_prompt_selects_with_the_mouse_too() {
    let mut app = app();
    type_text(&mut app, "rename the parser module");
    let frame = render(&mut app, 80, 20);
    let (x, y) = find(&frame, "parser");
    click(&mut app, x, y, MouseButton::Left);
    click(&mut app, x, y, MouseButton::Left);
    assert_eq!(copied().as_deref(), Some("parser"));
    let buffer = draw_buffer(&mut app, 80, 20);
    assert_eq!(buffer[(x, y)].bg, theme().highlight_bg);
    type_text(&mut app, "lexer");
    assert_eq!(
        app.editor.text(),
        "rename the lexer module",
        "typing replaces it"
    );
}

#[tokio::test]
async fn the_agent_map_draws_the_tree() {
    let mut app = app();
    let start = |app: &mut App, id: &str, label: &str, parent: &str| {
        emit_as(
            app,
            id,
            Event::AgentStarted {
                label: label.into(),
                parent: Some(parent.into()),
                model: "lora/fast".into(),
                cwd: "/work".into(),
            },
        );
    };
    start(&mut app, "lister", "lister", AGENT);
    start(&mut app, "grep", "grep", "lister");
    start(&mut app, "tester", "tester", AGENT);
    answer_history(&mut app);
    emit_as(
        &mut app,
        "grep",
        Event::PermissionRequested {
            request_id: "r1".into(),
            call_id: "c1".into(),
            tool: "bash".into(),
            summary: "rg parse".into(),
            input: json!({"command": "rg parse"}),
        },
    );
    app.approvals_hidden = true;
    let frame = render(&mut app, 120, 34);
    assert!(frame.contains("3 subagents"), "the footer counts them");
    let (x, y) = find(&frame, "3 subagents");
    let action = click(&mut app, x, y, MouseButton::Left).unwrap();
    assert_eq!(action, Action::AgentMap);
    app.perform(action).await;
    let frame = render(&mut app, 120, 34);
    println!("{frame}");
    for needle in [" main ", " tab 1 ", " lister ", " d1 ", " d2 "] {
        assert!(frame.contains(needle), "{needle}");
    }
    assert!(
        frame.contains("? awaiting approval"),
        "grep waits for approval"
    );
    assert!(frame.contains("┃──┬──▸│"), "children hang off their parent");
    assert!(frame.contains("│─────▸│"));
    let (lister_x, lister_y) = find(&frame, " lister ");
    let (tester_x, tester_y) = find(&frame, " tester ");
    let (grep_x, _) = find(&frame, " grep ");
    assert_eq!(lister_x, tester_x, "siblings share a column");
    assert!(tester_y > lister_y && grep_x > lister_x);
    assert!(frame.contains("┏"), "the shown agent is selected");

    click(&mut app, tester_x + 2, tester_y + 2, MouseButton::Left);
    let action = click(&mut app, tester_x + 2, tester_y + 2, MouseButton::Left);
    assert_eq!(
        action,
        Some(Action::OpenAgent("tester".into())),
        "a double click opens"
    );
    app.perform(Action::OpenAgent("tester".into())).await;
    assert_eq!(
        app.tab().view,
        ["tester"],
        "a subagent opens inside its agent"
    );
    app.perform(Action::AgentMap).await;
    render(&mut app, 120, 34);
    app.on_key(press(KeyCode::Up), false);
    app.on_key(press(KeyCode::Up), false);
    assert_eq!(
        app.on_key(press(KeyCode::Char('x')), false),
        Some(Action::StopAgent("lister".into()))
    );
    assert_eq!(
        app.on_key(press(KeyCode::F(3)), false),
        Some(Action::AgentList)
    );
    app.perform(Action::AgentList).await;
    assert!(matches!(app.overlay, Some(Overlay::Agents(_))));
    app.overlay = None;
    key(&mut app, KeyCode::F(3)).await;
    assert!(
        matches!(app.overlay, Some(Overlay::Agents(_))),
        "F3 opens the last view"
    );
}

#[tokio::test]
async fn at_attaches_files_and_prompts_send_their_paths() {
    let mut app = app();
    let root = app.tab_cwd();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("target/main.o"), "").unwrap();
    std::fs::write(root.join("notes.md"), "# notes\n").unwrap();
    app.files = super::app::FileIndex {
        root: root.clone(),
        list: Some(super::files::index(&root)),
        loading: false,
    };
    type_text(&mut app, "explain @mai");
    let frame = render(&mut app, 90, 24);
    assert!(frame.contains("src/main.rs"));
    assert!(!frame.contains("target/main.o"), ".gitignore is honored");
    app.on_key(press(KeyCode::Tab), false);
    assert_eq!(app.editor.text(), "explain ");
    let frame = render(&mut app, 90, 24);
    println!("{frame}");
    assert!(
        frame.contains("▤ src/main.rs · 13 B ×"),
        "the file is a chip"
    );

    app.on_paste(&root.join("notes.md").display().to_string());
    assert_eq!(app.attachments.len(), 2, "a dropped path attaches");
    let frame = render(&mut app, 90, 24);
    let (x, y) = find(&frame, "notes.md · 8 B ×");
    click(&mut app, x + 14, y, MouseButton::Left);
    assert_eq!(app.attachments.len(), 1, "clicking × removes a chip");

    key(&mut app, KeyCode::Enter).await;
    let sent = sent(&app);
    let Some(Command::Prompt {
        tab,
        agent,
        text,
        attachments,
        ..
    }) = sent.last()
    else {
        panic!("a prompt, not {sent:?}");
    };
    assert_eq!((tab.as_deref(), agent.as_deref()), (Some(TAB), Some(AGENT)));
    assert_eq!(text, "explain");
    assert_eq!(attachments.len(), 1);
    assert!(attachments[0].path.ends_with("src/main.rs"));
    assert!(app.attachments.is_empty(), "the chips went with it");

    emit(
        &mut app,
        Event::UserMessage {
            text: "explain\n\n[attached: src/main.rs]".into(),
        },
    );
    let frame = render(&mut app, 90, 24);
    assert!(frame.contains("› explain"));
    assert!(
        frame.contains("▤ src/main.rs"),
        "the transcript shows it too"
    );
    assert!(!frame.contains("[attached"));
}

#[tokio::test]
async fn a_refused_prompt_comes_back_to_the_editor() {
    let mut app = app();
    app.send("look at missing.txt".into());
    assert!(app.busy(), "busy while the prompt is on its way");
    let Some(Command::Prompt { text, .. }) = sent(&app).pop() else {
        panic!("a prompt");
    };
    let reply = Reply::Prompt {
        tab: TAB.into(),
        prompt: text.as_str().into(),
    };
    app.on_update(Update::Reply(
        reply,
        Err("cannot attach missing.txt".into()),
    ));
    assert!(!app.busy());
    assert_eq!(app.editor.text(), "look at missing.txt");
    assert!(render(&mut app, 90, 24).contains("cannot attach missing.txt"));
}

#[tokio::test]
async fn choosing_a_folder_opens_a_tab_there() {
    let mut app = app();
    let dir = app.tab_cwd().join("elsewhere");
    std::fs::create_dir_all(&dir).unwrap();
    type_text(&mut app, "/cd");
    let action = app.on_key(press(KeyCode::Enter), false).unwrap();
    assert_eq!(action, Action::FolderDialog);
    app.perform(action).await;
    let frame = render(&mut app, 100, 30);
    assert!(frame.contains("Open folder"));
    assert!(frame.contains("Work in this folder"));
    assert!(frame.contains("elsewhere/"));
    type_text(&mut app, "elsew");
    app.on_key(press(KeyCode::Enter), false);
    let action = app.on_key(press(KeyCode::Enter), false).unwrap();
    assert_eq!(action, Action::OpenFolder(dir.clone()));
    app.perform(action).await;
    let Some(Command::TabCreate(spec)) = sent(&app).pop() else {
        panic!("tab_create expected");
    };
    assert_eq!(spec.cwd, dir);
    assert!(spec.reuse, "a folder that has a tab gets that tab");

    // The daemon answers, then announces the tab to every client.
    let ack = ServerMessage::Ack {
        client_ref: None,
        agent: None,
        tab: Some("t2".into()),
        workspace: None,
        attachments: Vec::new(),
    };
    app.on_update(Update::Reply(
        Reply::TabCreated { placeholder: false },
        Ok(ack),
    ));
    let mut view = tab("t2", "elsewhere", &[]);
    view.cwd = dir.clone();
    app.on_message(ServerMessage::TabCreated {
        workspace: WORKSPACE.into(),
        index: 1,
        tab: Box::new(view),
    });
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.tab_cwd(), dir, "the new tab is shown");
    assert!(
        sent_json(&app).contains(&json!({"type": "tab_activate", "tab": "t2"})),
        "and is the one `malac` returns to"
    );
}

#[tokio::test]
async fn tabs_follow_the_daemon() {
    let mut app = app();
    a_turn(&mut app);
    let created = |id: &str, name: &str, index| ServerMessage::TabCreated {
        workspace: WORKSPACE.into(),
        index,
        tab: Box::new(tab(id, name, &[])),
    };
    app.on_message(created("t2", "notes", 1));
    app.on_message(created("t3", "web", 2));
    let frame = render(&mut app, 100, 26);
    let header = frame.lines().next().unwrap();
    println!("{header}");
    assert!(
        header.contains(" │ 1 project ✓ ×  2 notes  3 web "),
        "{header}"
    );
    assert!(
        header.trim_end().ends_with("default"),
        "the workspace's name"
    );
    assert_eq!(app.active, 0, "another client's tab does not take over");

    let mut renamed = tab("t2", "docs", &[]);
    renamed.mode = Mode::Autopilot;
    app.on_message(ServerMessage::TabUpdated {
        workspace: WORKSPACE.into(),
        tab: Box::new(renamed),
    });
    app.on_message(ServerMessage::TabsReordered {
        workspace: WORKSPACE.into(),
        tabs: vec!["t3".into(), TAB.into(), "t2".into()],
    });
    let names: Vec<&str> = app.tabs.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["web", "project", "docs"]);
    assert_eq!(app.tab().name, "project", "the shown tab stays shown");
    app.on_message(ServerMessage::TabClosed {
        workspace: WORKSPACE.into(),
        tab: "t3".into(),
    });
    app.on_message(ServerMessage::WorkspaceUpdated {
        workspace: summary(WORKSPACE, "renamed"),
    });
    let frame = render(&mut app, 100, 26);
    let header = frame.lines().next().unwrap();
    assert!(header.contains(" │ 1 project ✓ ×  2 docs "), "{header}");
    assert!(header.trim_end().ends_with("renamed"));

    app.perform(Action::MoveTab(1)).await;
    assert_eq!(
        sent_json(&app),
        [json!({"type": "tab_move", "tab": TAB, "index": 1})],
        "the daemon moves it and tells every client"
    );
    let (x, y) = find(&frame, "2 docs");
    let action = click(&mut app, x, y, MouseButton::Left).unwrap();
    app.perform(action).await;
    assert!(
        render(&mut app, 100, 26).contains("│ autopilot │"),
        "modes are per tab"
    );
}

#[tokio::test]
async fn closing_a_working_tab_asks_first() {
    let mut app = app_with(
        snapshot(),
        workspace(vec![
            tab(TAB, "project", &[AGENT]),
            tab("t2", "web", &["a2"]),
        ]),
    );
    emit(&mut app, Event::UserMessage { text: "go".into() });
    key(&mut app, KeyCode::F(10)).await;
    assert!(app.quit, "quitting leaves agents working in the daemon");
    app.quit = false;
    app.perform(Action::CloseTab(0)).await;
    let frame = render(&mut app, 100, 26);
    assert!(frame.contains("Agents are working in project"), "{frame}");
    assert!(sent(&app).is_empty(), "nothing closes before the answer");
    key(&mut app, KeyCode::Char('y')).await;
    assert_eq!(
        sent_json(&app),
        [json!({"type": "tab_close", "tab": TAB, "force": true})]
    );
    app.perform(Action::CloseTab(1)).await;
    assert_eq!(
        sent_json(&app),
        [json!({"type": "tab_close", "tab": "t2", "force": false})],
        "an idle tab closes at once"
    );
}

#[tokio::test]
async fn tabs_switch_between_their_agents() {
    let mut view = tab(TAB, "project", &[AGENT, "a2"]);
    view.snapshots[1].snapshot.info.label = "review".into();
    let mut app = app_with(snapshot(), workspace(vec![view]));
    emit_as(
        &mut app,
        "a2",
        Event::UserMessage {
            text: "review the diff".into(),
        },
    );
    let frame = render(&mut app, 100, 26);
    println!("{frame}");
    assert!(frame.contains(" main "), "{frame}");
    assert!(frame.contains(" review "));
    assert!(frame.contains("+ agent"));
    let (x, y) = find(&frame, " review ");
    let action = click(&mut app, x + 1, y, MouseButton::Left).unwrap();
    assert_eq!(action, Action::ShowTabAgent("a2".into()));
    app.perform(action).await;
    assert_eq!(app.tab().agent.as_deref(), Some("a2"));
    assert!(
        sent_json(&app).contains(&json!({"type": "agent_activate", "tab": TAB, "agent": "a2"}))
    );
    assert!(render(&mut app, 100, 26).contains("› review the diff"));
    let step = app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT), false);
    assert_eq!(step, Some(Action::StepAgent(1)));
    app.perform(Action::StepAgent(1)).await;
    assert_eq!(
        app.tab().agent.as_deref(),
        Some(AGENT),
        "Alt-↓ wraps around"
    );

    let new = app.on_key(
        KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
        false,
    );
    assert_eq!(new, Some(Action::NewAgent));
    app.perform(Action::NewAgent).await;
    assert!(matches!(
        sent(&app).last(),
        Some(Command::AgentStart { tab, .. }) if tab == TAB
    ));
}

#[tokio::test]
async fn approvals_come_and_go_with_any_client() {
    let mut pending = snapshot();
    pending.pending = vec![crate::protocol::PendingApproval {
        request: malac_core::permission::PermissionRequest {
            id: "r0".into(),
            call_id: "c0".into(),
            agent: AGENT.into(),
            agent_label: "main".into(),
            tool: "edit".into(),
            summary: "src/lib.rs".into(),
            input: json!({"path": "src/lib.rs"}),
        },
        tab: Some(TAB.into()),
    }];
    let mut app = app_with(pending, workspace(vec![tab(TAB, "project", &[AGENT])]));
    assert_eq!(app.approvals.len(), 1, "open requests show on connect");
    app.on_message(ServerMessage::Event {
        tab: Some(TAB.into()),
        workspace: Some(WORKSPACE.into()),
        envelope: envelope(
            AGENT,
            Event::PermissionResolved {
                request_id: "r0".into(),
                allowed: true,
            },
        ),
    });
    assert!(app.approvals.is_empty(), "another client answered");

    emit(
        &mut app,
        Event::PermissionRequested {
            request_id: "r1".into(),
            call_id: "c1".into(),
            tool: "bash".into(),
            summary: "rm -rf target".into(),
            input: json!({"command": "rm -rf target"}),
        },
    );
    let frame = render(&mut app, 90, 30);
    println!("{frame}");
    assert!(frame.contains("main wants to run bash"));
    assert!(frame.contains("command: rm -rf target"));
    assert!(frame.contains(" Allow  y    Always  a    Deny  n "));
    assert!(frame.contains("? 1 waiting"));
    assert!(
        app.on_key(press(KeyCode::Char('y')), false).is_none(),
        "too soon"
    );
    app.approvals[0].arrived = Instant::now() - Duration::from_secs(1);
    let (x, y) = find(&frame, "Always  a");
    let action = click(&mut app, x, y, MouseButton::Left).unwrap();
    app.perform(action).await;
    assert_eq!(
        sent_json(&app),
        [json!({"type": "approve", "request_id": "r1", "decision": Decision::AllowSession})]
    );
    assert!(!render(&mut app, 90, 30).contains("Permission"));
}

#[tokio::test]
async fn background_jobs_come_from_the_daemon() {
    let mut app = app();
    let job = |status, exit_code| JobView {
        job: JobInfo {
            id: "0a1b2c3d".into(),
            command: "npm run dev".into(),
            cwd: project(),
            pid: Some(4821),
            log: scratch_home().join("jobs/0a1b2c3d.log"),
            started: chrono::Utc::now(),
            finished: None,
            exit_code,
            status,
            agent: Some(AGENT.into()),
        },
        tab: Some(TAB.into()),
    };
    app.on_message(ServerMessage::JobStarted {
        job: job(JobStatus::Running, None),
    });
    let frame = render(&mut app, 120, 30);
    let (x, y) = find(&frame, "1 job");
    let action = click(&mut app, x, y, MouseButton::Left).unwrap();
    assert_eq!(action, Action::Jobs);
    app.perform(action).await;
    let asked = sent(&app);
    assert!(
        matches!(asked[0], Command::JobList),
        "the list is refreshed"
    );
    let Some(Command::JobOutput { job: id, .. }) = asked.last().cloned() else {
        panic!("the view asks for the log");
    };
    let output = JobOutput {
        job: job(JobStatus::Running, None).job,
        text: "compiling\nlistening on :3000\n".into(),
        size: 30,
        truncated: false,
    };
    app.on_update(Update::Reply(
        Reply::JobOutput { job: id.clone() },
        Ok(ServerMessage::JobOutput {
            client_ref: None,
            output: Box::new(output),
        }),
    ));
    let frame = render(&mut app, 120, 30);
    println!("{frame}");
    assert!(frame.contains("running"));
    assert!(frame.contains("npm run dev"));
    assert!(frame.contains("listening on :3000"), "the log's tail");
    let action = app.on_key(press(KeyCode::Char('k')), false).unwrap();
    assert_eq!(action, Action::StopJob(id.clone()));
    app.perform(action).await;
    assert_eq!(sent_json(&app), [json!({"type": "job_kill", "job": id})]);
    app.on_message(ServerMessage::JobFinished {
        job: job(JobStatus::Exited, Some(1)),
    });
    assert!(
        render(&mut app, 120, 30).contains("exited 1"),
        "real exit codes"
    );
}

#[tokio::test]
async fn daemon_and_control_notices() {
    let mut old = snapshot();
    old.daemon.version = "0.0.9".into();
    old.daemon.version_mismatch = true;
    old.control = ControlStatus {
        enabled: true,
        connected: true,
        tools: 12,
        error: None,
        permissions: Some(json!({"screen_recording": false, "accessibility": true})),
    };
    let mut app = app_with(old, workspace(vec![tab(TAB, "project", &[AGENT])]));
    let frame = render(&mut app, 160, 30);
    println!("{frame}");
    assert!(frame.contains("daemon 0.0.9 running; restart it when idle"));
    let status = frame.lines().nth(28).unwrap();
    let (x, _) = find(status, "control needs permissions");
    let action = click(&mut app, x, 28, MouseButton::Left).unwrap();
    assert_eq!(action, Action::RequestPermissions);
    app.perform(action).await;
    assert_eq!(
        sent_json(&app),
        [json!({"type": "control_permissions", "request": true})]
    );
    let (x, _) = find(status, "daemon 0.0.9");
    let action = click(&mut app, x, 28, MouseButton::Left).unwrap();
    assert_eq!(action, Action::RestartDaemon);
    app.perform(action).await;
    assert!(
        matches!(
            app.backend.rests.lock().unwrap().last(),
            Some(Rest::DaemonStopped)
        ),
        "nothing runs, so the daemon is asked to stop and a fresh one starts"
    );
    app.on_message(ServerMessage::Lagged { skipped: 40 });
    assert_eq!(
        sent_json(&app),
        [json!({"type": "snapshot", "workspace": WORKSPACE})],
        "falling behind resyncs"
    );
}

#[tokio::test]
async fn workspaces_switch_rename_and_go() {
    let mut app = app();
    key(&mut app, KeyCode::F(9)).await;
    key(&mut app, KeyCode::Enter).await;
    let frame = render(&mut app, 100, 30);
    for item in [
        "New workspace…",
        "Switch workspace…",
        "Rename workspace…",
        "Delete workspace…",
    ] {
        assert!(frame.contains(item), "{item}");
    }
    app.overlay = None;
    app.perform(Action::Workspaces).await;
    assert_eq!(sent_json(&app), [json!({"type": "workspace_list"})]);
    let frame = render(&mut app, 100, 30);
    assert!(frame.contains("● default"), "{frame}");
    type_text(&mut app, "rel");
    let action = app.on_key(press(KeyCode::Enter), false).unwrap();
    assert_eq!(action, Action::OpenWorkspace("w2".into()));
    app.perform(action).await;
    assert_eq!(
        sent_json(&app),
        [json!({"type": "workspace_open", "workspace": "w2"})]
    );
    let other = WorkspaceView {
        id: "w2".into(),
        name: "release".into(),
        active_tab: Some("t9".into()),
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
        tabs: vec![tab("t9", "api", &["a9"])],
    };
    app.on_update(Update::Reply(
        Reply::WorkspaceOpened { fresh: false },
        Ok(ServerMessage::WorkspaceOpened {
            client_ref: None,
            workspace: Box::new(other),
        }),
    ));
    answer_history(&mut app);
    assert_eq!(app.workspace_name, "release");
    assert_eq!(app.tab().name, "api");
    assert_eq!(
        sent_json(&app),
        [json!({"type": "snapshot", "workspace": "w2"})],
        "a full snapshot brings its questions and jobs"
    );

    app.perform(Action::NewWorkspace).await;
    type_text(&mut app, "spike");
    key(&mut app, KeyCode::Enter).await;
    assert_eq!(
        sent_json(&app),
        [json!({"type": "workspace_create", "name": "spike"})]
    );
    let fresh = WorkspaceView {
        id: "w3".into(),
        name: "spike".into(),
        active_tab: None,
        created: chrono::Utc::now(),
        updated: chrono::Utc::now(),
        tabs: Vec::new(),
    };
    app.on_update(Update::Reply(
        Reply::WorkspaceOpened { fresh: true },
        Ok(ServerMessage::WorkspaceOpened {
            client_ref: None,
            workspace: Box::new(fresh),
        }),
    ));
    let sent = sent(&app);
    assert!(
        sent.iter().any(
            |c| matches!(c, Command::TabCreate(spec) if spec.workspace.as_deref() == Some("w3"))
        ),
        "a new workspace gets a tab for the folder: {sent:?}"
    );
    assert!(render(&mut app, 100, 30).contains("spike"));
}

#[tokio::test]
async fn welcome_screen() {
    let mut app = app_with(snapshot(), workspace(vec![tab(TAB, "project", &[])]));
    let frame = render(&mut app, 100, 30);
    println!("{frame}");
    let banner = format!(
        "malac {} · Multi-Agent Loop for Autonomous Coding",
        env!("CARGO_PKG_VERSION")
    );
    assert!(frame.contains(&banner));
    assert!(frame.contains("(  (oo)  )"), "the small pig fits 30 rows");
    assert!(frame.contains("lora/smart"));
    assert!(frame.contains("space   default"));
    let frame = render(&mut app, 160, 44);
    assert!(frame.contains("▀"), "the illustrated pig needs room");
    assert!(frame.contains("Describe a task and press Enter"));
}

#[tokio::test]
async fn tool_cards_expand_on_click() {
    let mut app = app();
    a_turn(&mut app);
    let frame = render(&mut app, 90, 24);
    assert!(frame.contains("● bash cargo test"));
    assert!(frame.contains("2 lines  2.3s ▸"));
    let (x, y) = find(&frame, "● bash");
    let action = click(&mut app, x, y, MouseButton::Left).unwrap();
    app.perform(action).await;
    let frame = render(&mut app, 90, 24);
    assert!(frame.contains("│ $ cargo test"));
    assert!(frame.contains("│ test result: ok. 46 passed"));
    assert!(frame.contains("▾"));
}

#[tokio::test]
async fn model_picker_filters() {
    let mut app = app();
    let frame = render(&mut app, 90, 30);
    let (x, y) = find(&frame, "4Model");
    let action = click(&mut app, x, y, MouseButton::Left).unwrap();
    assert_eq!(action, Action::PickModel);
    app.perform(action).await;
    let frame = render(&mut app, 90, 30);
    assert!(frame.contains(" Model "));
    assert!(frame.contains("● smart"));
    assert!(frame.contains("  fast"));
    app.on_key(press(KeyCode::Char('f')), false);
    let action = app.on_key(press(KeyCode::Enter), false).unwrap();
    assert_eq!(action, Action::SetModel("lora/fast".into()));
    app.perform(action).await;
    assert_eq!(
        sent_json(&app),
        [json!({"type": "model", "agent": AGENT, "model": "lora/fast"})]
    );
}

#[tokio::test]
async fn menus_and_the_mode() {
    let mut app = app();
    key(&mut app, KeyCode::F(9)).await;
    key(&mut app, KeyCode::Enter).await;
    let frame = render(&mut app, 100, 30);
    assert!(frame.contains("New tab here"));
    key(&mut app, KeyCode::Right).await;
    assert!(render(&mut app, 100, 30).contains("All models…"));
    assert_eq!(
        app.on_key(press(KeyCode::Char('a')), false),
        Some(Action::PickModel)
    );
    app.overlay = None;
    let frame = render(&mut app, 100, 30);
    let (x, y) = find(&frame, "Mode ");
    click(&mut app, x, y, MouseButton::Left);
    let frame = render(&mut app, 100, 30);
    let (x, y) = find(&frame, "Autopilot");
    let action = click(&mut app, x, y, MouseButton::Left).unwrap();
    assert_eq!(action, Action::SetMode(Mode::Autopilot));
    app.perform(action).await;
    assert_eq!(
        sent_json(&app),
        [json!({"type": "mode", "mode": "autopilot", "tab": TAB})]
    );
}

#[tokio::test]
async fn context_menu_on_entries() {
    let mut app = app();
    a_turn(&mut app);
    let frame = render(&mut app, 90, 24);
    let (x, y) = find(&frame, "● bash");
    click(&mut app, x + 4, y, MouseButton::Right);
    let frame = render(&mut app, 90, 24);
    assert!(frame.contains("Copy"));
    assert!(frame.contains("Expand"));
    let action = app.on_key(press(KeyCode::Char('e')), false).unwrap();
    assert!(matches!(action, Action::ToggleEntry(_)));
}

#[tokio::test]
async fn footer_segments_are_clickable() {
    let mut app = app();
    a_turn(&mut app);
    let frame = render(&mut app, 120, 30);
    let status = frame.lines().nth(28).unwrap();
    assert!(status.contains("lora/smart │ ask │"), "{status}");
    assert!(status.contains("│ main │"));
    let (x, y) = find(&frame, "│ ask");
    let action = click(&mut app, x + 2, y, MouseButton::Left).unwrap();
    assert_eq!(action, Action::CycleMode);
    let (x, y) = find(&frame, "10Quit");
    assert_eq!(click(&mut app, x, y, MouseButton::Left), Some(Action::Quit));
}

#[tokio::test]
async fn prompts_queue_while_busy_and_stop_drops_them() {
    let mut app = app();
    app.send("first".into());
    app.send("second".into());
    app.send("third".into());
    assert!(app.busy());
    let queued: Vec<&str> = app.tab().queued.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(queued, ["second", "third"]);
    assert_eq!(sent(&app).len(), 1, "only the first went out");
    let frame = render(&mut app, 90, 24);
    assert!(frame.contains("› second  queued"), "{frame}");
    app.perform(Action::Stop).await;
    assert!(app.tab().queued.is_empty());
    assert_eq!(sent_json(&app), [json!({"type": "cancel", "agent": AGENT})]);
    assert!(render(&mut app, 90, 24).contains("dropped 2 queued"));
}

#[tokio::test]
async fn update_segment_opens_the_dialog() {
    let mut app = app();
    app.update = update::Status {
        current: "0.1.0".into(),
        latest: Some("0.2.0".into()),
        available: true,
        installed: None,
        checked_at: None,
        method: update::Method::SelfUpdate,
        release_url: "https://github.com/lobirus/malac/releases/tag/v0.2.0".into(),
        hint: None,
    };
    let frame = render(&mut app, 120, 30);
    let (x, y) = find(&frame, "update 0.2.0");
    let action = click(&mut app, x, y, MouseButton::Left).unwrap();
    assert_eq!(action, Action::UpdateDialog);
    app.perform(action).await;
    let frame = render(&mut app, 120, 30);
    assert!(frame.contains("latest     0.2.0"));
    assert!(frame.contains(" Install now  i "));
}

#[tokio::test]
async fn saved_conversations_load_from_the_daemon() {
    scratch_home();
    let (backend, _) = Backend::recording();
    let view = workspace(vec![tab(TAB, "project", &[AGENT])]);
    let mut app = App::new(
        backend,
        Config::default(),
        snapshot(),
        &view,
        Some(TAB),
        project(),
    );
    // Events that come while the history is read wait for it.
    emit(
        &mut app,
        Event::UserMessage {
            text: "and the lexer?".into(),
        },
    );
    let history = json!({"history": [
        {"role": "user", "content": [{"type": "text", "text": "fix the parser"}]},
        {"role": "assistant", "content": [{"type": "text", "text": "Fixed the **off-by-one**."}]},
    ]});
    let rest = Rest::History {
        agent: AGENT.into(),
    };
    app.on_update(Update::Rest(rest, Ok(history)));
    let frame = render(&mut app, 90, 24);
    println!("{frame}");
    let first = frame.find("› fix the parser").expect("the saved prompt");
    let then = frame.find("› and the lexer?").expect("the new prompt");
    assert!(first < then, "the history comes first");
    assert!(frame.contains("Fixed the off-by-one."));
}

#[tokio::test]
async fn slash_completion_runs_the_highlighted_command() {
    let mut app = app();
    render(&mut app, 90, 24);
    type_text(&mut app, "/he");
    let frame = render(&mut app, 90, 24);
    assert!(frame.contains("/help"));
    assert_eq!(app.on_key(press(KeyCode::Enter), false), Some(Action::Help));
}

// Against a real daemon.

/// Replies with queued messages, as a provider.
#[derive(Default)]
struct Script(Mutex<VecDeque<Message>>);

#[async_trait]
impl Provider for Script {
    async fn complete(&self, _req: &Request<'_>, on_delta: DeltaSink<'_>) -> Result<Completion> {
        let message = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Message::assistant(vec![Block::text("(script exhausted)")]));
        let text = message.text();
        if !text.is_empty() {
            on_delta(Delta::Text(&text));
        }
        let stop = match message.tool_uses().next() {
            Some(_) => StopReason::ToolUse,
            None => StopReason::EndTurn,
        };
        Ok(Completion {
            message,
            usage: Usage::default(),
            stop,
        })
    }
}

/// One terminal: the interface and the channels its event loop would drain.
struct Term {
    app: App,
    events: Events,
    updates: mpsc::UnboundedReceiver<Update>,
}

impl Term {
    async fn open(file: &DaemonFile, launch: Launch) -> Self {
        let (client, events, snapshot) = Client::connect(file).await.unwrap();
        let cwd = launch.cwd.clone();
        let launched = client.launch(launch).await.unwrap();
        let (updates_tx, updates) = mpsc::unbounded_channel();
        let (connections, _) = mpsc::unbounded_channel();
        let backend = Backend::new(client, updates_tx, connections);
        let app = App::new(
            backend,
            Config::default(),
            snapshot,
            &launched.workspace,
            launched.tab.as_deref(),
            cwd,
        );
        Self {
            app,
            events,
            updates,
        }
    }

    /// Feeds the interface what the daemon sends until `done` holds.
    async fn until(&mut self, what: &str, done: impl Fn(&App) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !done(&self.app) {
            tokio::select! {
                Some(message) = self.events.recv() => self.app.on_message(message),
                Some(update) = self.updates.recv() => self.app.on_update(update),
                _ = tokio::time::sleep(Duration::from_millis(20)) => {
                    self.app.tick();
                }
                _ = tokio::time::sleep_until(deadline) => panic!("timed out waiting: {what}"),
            }
        }
    }

    fn answer(&self) -> Option<String> {
        self.app
            .shown()?
            .transcript
            .last_answer()
            .map(str::to_owned)
    }
}

fn shows(app: &App, text: &str) -> bool {
    app.shown().is_some_and(|v| {
        v.transcript
            .items()
            .iter()
            .any(|i| i.entry.plain_text().contains(text))
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn two_terminals_share_a_workspace() {
    let home = scratch_home();
    let script = Arc::new(Script::default());
    script.0.lock().unwrap().extend([
        Message::assistant(vec![Block::text("hello there")]),
        Message::assistant(vec![Block::ToolUse {
            id: "c1".into(),
            name: "bash".into(),
            input: json!({"command": "echo shared-run"}),
        }]),
        Message::assistant(vec![Block::text("ran it")]),
    ]);
    let args = ServeArgs {
        port: Some(0),
        host: Some("127.0.0.1".into()),
        ..Default::default()
    };
    let daemon = Daemon::start(&args, false).await.unwrap();
    daemon.hub().insert_provider("mock", script.clone());
    let file = daemon.file().clone();
    let hub = daemon.hub().clone();
    let server = tokio::spawn(daemon.run());

    let shared = home.join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    let shared = shared.canonicalize().unwrap();
    let mut a = Term::open(
        &file,
        Launch {
            workspace: WorkspaceChoice::New,
            cwd: shared.clone(),
            name: Some("pair".into()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(a.app.workspace_name, "pair");
    a.app.send("say hello".into());
    a.until("the first answer", |app| shows(app, "hello there"))
        .await;

    // B joins later and finds the conversation so far.
    let mut b = Term::open(
        &file,
        Launch {
            workspace: WorkspaceChoice::Open("pair".into()),
            cwd: shared.clone(),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(b.app.workspace_id, a.app.workspace_id);
    b.until("B reads the history", |app| shows(app, "hello there"))
        .await;
    assert!(shows(&b.app, "say hello"));

    // A prompts; B watches it run and answers the question.
    a.app.send("run it".into());
    a.until("A is asked", |app| !app.approvals.is_empty()).await;
    b.until("B is asked too", |app| !app.approvals.is_empty())
        .await;
    b.until("B shows A's prompt", |app| shows(app, "run it"))
        .await;
    b.app.approvals[0].arrived = Instant::now() - Duration::from_secs(1);
    let decision = b.app.on_key(press(KeyCode::Char('y')), false).unwrap();
    b.app.perform(decision).await;
    a.until("A's question closes", |app| app.approvals.is_empty())
        .await;
    b.until("B sees the answer", |b| {
        b.shown()
            .and_then(|v| v.transcript.last_answer())
            .is_some_and(|t| t == "ran it")
    })
    .await;
    a.until("A sees it too", |app| shows(app, "ran it")).await;
    assert_eq!(b.answer().as_deref(), Some("ran it"));

    // A opens a tab; B sees it.
    let notes = shared.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    a.app.open_folder(&notes);
    a.until("A shows its new tab", |app| {
        app.tabs.len() == 2 && app.active == 1
    })
    .await;
    b.until("B has it", |app| app.tabs.len() == 2).await;
    assert_eq!(b.app.active, 0, "B stays where it was");
    assert_eq!(b.app.tabs[1].cwd, notes.canonicalize().unwrap());

    hub.request_stop("test");
    server.await.unwrap().unwrap();
}
