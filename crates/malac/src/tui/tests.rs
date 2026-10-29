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
use malac_core::commands::CommandInfo;
use malac_core::config::Config;
use malac_core::context::{Layout, Summary};
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
use crate::connections::Endpoint;
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
keep_awake = "off"
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
        project: None,
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
        remote: Default::default(),
        awake: Default::default(),
        voice: Default::default(),
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
        list: Some(crate::files::index(&root).files),
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

/// What the completion popup lists, as typed into it.
fn completion_names(app: &App) -> Vec<String> {
    app.completions().into_iter().map(|c| c.name).collect()
}

/// Answers the `/cd` listing the interface asked for with these folders.
fn list_folders_for(app: &mut App, names: &[(&str, bool)]) {
    let names = names
        .iter()
        .map(|(n, git)| ((*n).to_owned(), *git))
        .collect();
    let (dir, hidden) = (app.folders.dir.clone(), app.folders.hidden);
    app.on_update(Update::Folders {
        dir,
        hidden,
        names: Ok(names),
    });
}

#[tokio::test]
async fn cd_completes_folders() {
    let mut app = app();
    let cwd = app.tab_cwd();
    type_text(&mut app, "/cd ");
    assert_eq!(app.folders.dir, cwd.display().to_string());
    assert!(
        app.completions().is_empty(),
        "nothing until the folder is listed"
    );
    list_folders_for(&mut app, &[("api", true), ("app", false), ("docs", false)]);
    assert_eq!(completion_names(&app), ["cd api/", "cd app/", "cd docs/"]);
    let frame = render(&mut app, 90, 24);
    assert!(
        frame.contains("api/") && frame.contains("repository"),
        "{frame}"
    );

    // Typing narrows the list, ignoring case; Tab fills in the folder and lists it.
    type_text(&mut app, "A");
    assert_eq!(completion_names(&app), ["cd api/", "cd app/"]);
    key(&mut app, KeyCode::Tab).await;
    assert_eq!(app.editor.text(), "/cd api/");
    assert_eq!(app.folders.dir, cwd.join("api").display().to_string());
    list_folders_for(&mut app, &[("src", false), ("tests", false)]);
    assert_eq!(completion_names(&app), ["cd api/src/", "cd api/tests/"]);

    // Enter opens what is typed, like a shell, not the first suggestion.
    let action = app.on_key(press(KeyCode::Enter), false).unwrap();
    assert_eq!(action, Action::OpenFolder(cwd.join("api/")));

    // Unless a folder was picked with the arrows.
    type_text(&mut app, "/cd api/");
    list_folders_for(&mut app, &[("src", false), ("tests", false)]);
    app.on_key(press(KeyCode::Down), false);
    let action = app.on_key(press(KeyCode::Enter), false).unwrap();
    assert_eq!(action, Action::OpenFolder(cwd.join("api/tests/")));
}

#[tokio::test]
async fn cd_lists_hidden_folders_only_when_asked() {
    let mut app = app();
    type_text(&mut app, "/cd ");
    assert!(!app.folders.hidden);
    type_text(&mut app, ".");
    assert!(
        app.folders.hidden,
        "a name starting with a dot lists hidden folders"
    );
    list_folders_for(&mut app, &[(".config", false), ("src", false)]);
    assert_eq!(completion_names(&app), ["cd .config/"]);
}

#[test]
fn cd_reads_local_folders() {
    let dir = std::env::temp_dir().join(format!("malac-cd-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("Beta/.git")).unwrap();
    std::fs::create_dir_all(dir.join("alpha")).unwrap();
    std::fs::create_dir_all(dir.join(".hidden")).unwrap();
    std::fs::write(dir.join("file.txt"), "").unwrap();
    let names = super::app::list_folders(&dir, false).unwrap();
    assert_eq!(
        names,
        [("alpha".to_owned(), false), ("Beta".to_owned(), true)]
    );
    let names = super::app::list_folders(&dir, true).unwrap();
    assert_eq!(names.len(), 3);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn cd_asks_a_remote_daemon_for_its_folders() {
    let mut app = app();
    remote(&mut app);
    app.backend.rests.lock().unwrap().clear();
    type_text(&mut app, "/cd ~/sr");
    let asked: Vec<Rest> = std::mem::take(&mut *app.backend.rests.lock().unwrap());
    let dirs: Vec<String> = asked
        .into_iter()
        .filter_map(|r| match r {
            Rest::Folders { dir, .. } => Some(dir),
            _ => None,
        })
        .collect();
    assert_eq!(dirs.last().map(String::as_str), Some("~"), "{dirs:?}");
    let listing = json!({"entries": [
        {"name": "src", "kind": "directory", "markers": [".git"]},
        {"name": "srv", "kind": "directory", "markers": []},
        {"name": "srt.txt", "kind": "file", "markers": []},
    ]});
    app.on_update(Update::Rest(
        Rest::Folders {
            dir: "~".into(),
            hidden: false,
        },
        Ok(listing),
    ));
    assert_eq!(completion_names(&app), ["cd ~/src/", "cd ~/srv/"]);
}

/// The daemon's answer to a `/bash` command whose card is in `view`.
fn shell_output(app: &mut App, view: &str, id: &str, command: &str, output: &str, code: i32) {
    let reply = Reply::Shell {
        tab: TAB.into(),
        view: view.into(),
        id: id.into(),
        command: command.into(),
        attach: true,
    };
    let message = ServerMessage::ShellOutput {
        client_ref: None,
        id: id.into(),
        output: output.into(),
        exit_code: Some(code),
        timed_out: false,
        cancelled: false,
    };
    app.on_update(Update::Reply(reply, Ok(message)));
}

#[tokio::test]
async fn bang_runs_a_shell_command_and_attaches_its_output() {
    let mut app = app();
    sent(&app);
    type_text(&mut app, "!cargo test");
    assert_eq!(app.on_key(press(KeyCode::Enter), false), None);
    let Some(Command::Shell {
        tab,
        id,
        command,
        timeout,
    }) = sent(&app).pop()
    else {
        panic!("a shell command");
    };
    assert_eq!((tab.as_str(), command.as_str()), (TAB, "cargo test"));
    assert_eq!(timeout, Some(120));
    assert!(app.editor.is_empty());
    assert!(render(&mut app, 90, 24).contains("$ cargo test"));

    // Esc stops it, as it stops a turn.
    let action = app.on_key(press(KeyCode::Esc), false).unwrap();
    assert_eq!(action, Action::Stop);
    app.perform(action).await;
    let stopped = sent(&app);
    assert!(
        matches!(stopped.as_slice(), [Command::ShellCancel { id: s }] if *s == id),
        "{stopped:?}"
    );

    shell_output(&mut app, AGENT, &id, "cargo test", "1 failed", 101);
    let frame = render(&mut app, 90, 24);
    assert!(
        frame.contains("1 failed") && frame.contains("[exit code 101]"),
        "{frame}"
    );
    assert!(app.tab().shells.is_empty());
    // Its output goes along with the next prompt, as a file the chip stands for.
    assert_eq!(app.attachments.len(), 1);
    assert_eq!(app.attachments[0].name, "$ cargo test");
    let saved = std::fs::read_to_string(&app.attachments[0].path).unwrap();
    assert_eq!(saved, "$ cargo test\n1 failed\n[exit code 101]\n");
}

#[tokio::test]
async fn bash_in_a_tab_without_an_agent_shows_its_own_view() {
    let mut app = app_with(snapshot(), workspace(vec![tab(TAB, "project", &[])]));
    assert!(
        render(&mut app, 90, 24).contains("Describe a task"),
        "the welcome screen"
    );
    sent(&app);
    type_text(&mut app, "/bash ls");
    app.on_key(press(KeyCode::Enter), false);
    let Some(Command::Shell { id, .. }) = sent(&app).pop() else {
        panic!("a shell command");
    };
    let view = app.tab().shell_view.clone().expect("a view of its own");
    assert!(
        app.tab().shown().is_none(),
        "commands still go to a new agent"
    );
    shell_output(&mut app, &view, &id, "ls", "notes.txt", 0);
    let frame = render(&mut app, 90, 24);
    assert!(
        frame.contains("$ ls") && frame.contains("notes.txt"),
        "{frame}"
    );
    assert!(!frame.contains("[exit code"), "a command that succeeded");
}

#[tokio::test]
async fn slash_malac_completes_and_runs_malac_commands() {
    let mut app = app();
    sent(&app);
    type_text(&mut app, "/malac prov");
    assert_eq!(completion_names(&app), ["malac provider"]);
    key(&mut app, KeyCode::Tab).await;
    assert_eq!(
        app.editor.text(),
        "/malac provider ",
        "a command that takes more"
    );
    let subcommands = completion_names(&app);
    assert!(
        subcommands.contains(&"malac provider list".to_owned()),
        "{subcommands:?}"
    );
    type_text(&mut app, "li");
    assert_eq!(completion_names(&app), ["malac provider list"]);
    app.on_key(press(KeyCode::Enter), false);
    let Some(Command::Shell { command, .. }) = sent(&app).pop() else {
        panic!("a shell command");
    };
    assert_eq!(command, "malac provider list");

    // Arguments are passed on quoted, exactly as checked.
    type_text(&mut app, "/malac sessions show 'my session'");
    app.on_key(press(KeyCode::Esc), false);
    app.on_key(press(KeyCode::Enter), false);
    let Some(Command::Shell { command, id, .. }) = sent(&app).pop() else {
        panic!("a shell command");
    };
    assert_eq!(command, "malac sessions show 'my session'");

    // Its output stays on its card.
    let reply = Reply::Shell {
        tab: TAB.into(),
        view: AGENT.into(),
        id: id.clone(),
        command,
        attach: false,
    };
    let message = ServerMessage::ShellOutput {
        client_ref: None,
        id,
        output: "no such session".into(),
        exit_code: Some(1),
        timed_out: false,
        cancelled: false,
    };
    app.on_update(Update::Reply(reply, Ok(message)));
    assert!(render(&mut app, 90, 24).contains("no such session"));
    assert!(app.attachments.is_empty());
}

#[tokio::test]
async fn slash_malac_refuses_prompts_and_terminal_commands() {
    let mut app = app();
    sent(&app);
    for (line, says) in [
        ("/malac fix the tests", "not a malac command"),
        ("/malac voice talk", "/talk"),
        ("/malac daemon restart", "Connection menu"),
        ("/malac models 'oops", "quote"),
    ] {
        type_text(&mut app, line);
        app.on_key(press(KeyCode::Esc), false);
        app.on_key(press(KeyCode::Enter), false);
        assert!(sent(&app).is_empty(), "{line} must not run");
        let frame = render(&mut app, 100, 24);
        assert!(frame.contains(says), "{line}: {frame}");
    }
}

#[tokio::test]
async fn bash_needs_a_command() {
    let mut app = app();
    sent(&app);
    type_text(&mut app, "/bash");
    app.on_key(press(KeyCode::Enter), false);
    assert!(sent(&app).is_empty());
    assert!(render(&mut app, 90, 24).contains("type a command"));
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
                decision: None,
                by: None,
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
async fn remote_access_dialog_pairing_and_keep_awake() {
    use crate::protocol::{AwakeStatus, RemoteStatus, RemoteUrl, UrlKind};
    use malac_core::config::KeepAwake;
    let mut app = app();
    let url = |kind, url: &str| RemoteUrl {
        kind,
        url: url.into(),
    };
    app.remote = RemoteStatus {
        enabled: true,
        listen: "0.0.0.0:7790".into(),
        urls: vec![
            url(UrlKind::Lan, "http://192.168.1.5:7790"),
            url(UrlKind::Local, "http://127.0.0.1:7790"),
        ],
        tunnel: None,
        devices: 1,
    };
    app.awake = AwakeStatus {
        mode: KeepAwake::Busy,
        awake: true,
        reason: "2 agents running".into(),
        method: Some("caffeinate".into()),
        error: None,
        since: None,
    };
    let frame = render(&mut app, 140, 30);
    assert!(
        frame.lines().nth(28).unwrap().contains("│ awake │"),
        "{frame}"
    );

    key(&mut app, KeyCode::F(9)).await;
    key(&mut app, KeyCode::Char('c')).await;
    let frame = render(&mut app, 140, 30);
    assert!(frame.contains("✓ This computer"), "{frame}");
    assert!(frame.contains("Remote access…"));
    let action = app.on_key(press(KeyCode::Char('r')), false).unwrap();
    assert_eq!(action, Action::RemoteDialog);
    app.perform(action).await;
    let asked = std::mem::take(&mut *app.backend.rests.lock().unwrap());
    assert!(asked.iter().any(|r| matches!(r, Rest::Devices)));
    let devices = json!({ "devices": [{
        "id": "a1b2c3d4", "name": "laptop", "kind": "cli", "created": "2026-10-26T07:00:00Z",
        "last_seen": null, "address": "192.168.1.20", "current": false
    }]});
    app.on_update(Update::Rest(Rest::Devices, Ok(devices)));
    let frame = render(&mut app, 140, 44);
    for text in [
        "Remote access",
        "allowed · listening on 0.0.0.0:7790",
        "http://192.168.1.5:7790  network address",
        "awake · 2 agents running · mode busy (caffeinate)",
        "laptop",
        "from 192.168.1.20",
        "ssh -N -L 7790:127.0.0.1:7790",
        "malac tunnel cloudflare",
    ] {
        assert!(frame.contains(text), "`{text}` missing:\n{frame}");
    }
    let action = app.on_key(press(KeyCode::Char('x')), false).unwrap();
    assert_eq!(action, Action::RevokeDevice("a1b2c3d4".into()));
    app.perform(action).await;
    assert!(render(&mut app, 140, 44).contains("Revoke laptop?"));
    app.perform(Action::RevokeNow("a1b2c3d4".into())).await;
    let asked = std::mem::take(&mut *app.backend.rests.lock().unwrap());
    assert!(asked.iter().any(|r| matches!(r, Rest::Revoked)));

    app.perform(Action::PairDevice).await;
    let pairing = json!({
        "code": "ABCDE-FGHJK", "expires": "2099-01-01T00:00:00Z",
        "links": [{ "kind": "lan", "base": "http://192.168.1.5:7790",
                    "url": "http://192.168.1.5:7790/#pair=ABCDE-FGHJK" }]
    });
    app.on_update(Update::Rest(Rest::Pairing, Ok(pairing)));
    let frame = render(&mut app, 140, 44);
    assert!(frame.contains("ABCDE-FGHJK"), "{frame}");
    assert!(frame.contains("▀▀▀▀▀▀▀"), "the QR code: {frame}");
    assert!(frame.contains("malac remote add <name> http://192.168.1.5:7790 --pair ABCDE-FGHJK"));
    key(&mut app, KeyCode::Esc).await;
    assert!(
        render(&mut app, 140, 44).contains("How to connect"),
        "back to the overview"
    );

    app.overlay = None;
    app.perform(Action::KeepAwake(KeepAwake::Always)).await;
    assert_eq!(
        sent_json(&app),
        [json!({"type": "keep_awake", "mode": "always"})]
    );
    app.on_message(ServerMessage::Awake {
        awake: AwakeStatus::default(),
    });
    let frame = render(&mut app, 140, 30);
    assert!(!frame.lines().nth(28).unwrap().contains("│ awake │"));
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

/// The repository the test project is one part of.
fn monorepo() -> PathBuf {
    project().parent().unwrap().to_owned()
}

/// A tab whose folder is one part of the repository at `monorepo()`, with
/// the given custom commands.
fn in_a_monorepo(commands: Vec<CommandInfo>) -> App {
    let root = monorepo();
    let mut view = tab(TAB, "project", &[AGENT]);
    view.project = Some(Summary {
        layout: Layout {
            root: root.clone(),
            subdir: Some("app".into()),
            suggest_root: Some(root),
            ..Default::default()
        },
        commands,
    });
    app_with(snapshot(), workspace(vec![view]))
}

#[tokio::test]
async fn a_folder_inside_a_repository_offers_all_of_it() {
    let root = monorepo();
    let mut app = in_a_monorepo(Vec::new());
    let frame = render(&mut app, 160, 24);
    println!("{frame}");
    assert!(frame.contains(&format!(
        "This folder is part of {}. Open the whole repository?",
        root.display()
    )));

    // By mouse: the tab for the root, made or reused.
    let (x, y) = find(&frame, "Open the whole repository?");
    let action = click(&mut app, x + 5, y, MouseButton::Left).unwrap();
    assert_eq!(action, Action::OpenFolder(root.clone()));
    app.perform(action).await;
    let Some(Command::TabCreate(spec)) = sent(&app).pop() else {
        panic!("tab_create expected");
    };
    assert_eq!((spec.cwd, spec.reuse), (root.clone(), true));

    // By keyboard, and from the menu.
    type_text(&mut app, "/root");
    app.on_key(press(KeyCode::Esc), false);
    let action = app.on_key(press(KeyCode::Enter), false).unwrap();
    assert_eq!(action, Action::OpenFolder(root.clone()));
    key(&mut app, KeyCode::F(9)).await;
    key(&mut app, KeyCode::Enter).await;
    let frame = render(&mut app, 100, 30);
    assert!(frame.contains("Open the whole repository"), "{frame}");
    assert_eq!(
        app.on_key(press(KeyCode::Char('p')), false),
        Some(Action::OpenFolder(root.clone()))
    );
    app.overlay = None;

    // Esc puts it away for this session.
    assert_eq!(
        app.on_key(press(KeyCode::Esc), false),
        Some(Action::HideSuggestion)
    );
    app.perform(Action::HideSuggestion).await;
    assert!(!render(&mut app, 160, 24).contains("This folder is part of"));
    assert!(app.suggested_root().is_none());
    assert_eq!(app.on_key(press(KeyCode::Esc), false), None);
}

#[tokio::test]
async fn a_tab_at_its_root_has_no_notice() {
    let mut app = app();
    assert!(!render(&mut app, 100, 24).contains("This folder is part of"));
    type_text(&mut app, "/root");
    app.on_key(press(KeyCode::Esc), false);
    assert_eq!(app.on_key(press(KeyCode::Enter), false), None);
    let flash = &app.flash.as_ref().expect("a flash").text;
    assert_eq!(flash, "this tab already works at its repository's root");
}

#[tokio::test]
async fn custom_commands_complete_run_and_have_help() {
    let mut app = in_a_monorepo(vec![
        CommandInfo {
            name: "deploy".into(),
            description: "ship the current branch".into(),
            argument_hint: Some("[env]".into()),
            source: "project".into(),
        },
        CommandInfo {
            name: "deps".into(),
            description: "check the dependencies".into(),
            argument_hint: None,
            source: "plugin acme".into(),
        },
        CommandInfo {
            name: "help".into(),
            description: "a custom help that the built-in one hides".into(),
            argument_hint: None,
            source: "user".into(),
        },
    ]);
    type_text(&mut app, "/de");
    let buffer = draw_buffer(&mut app, 100, 24);
    let frame = screen(&buffer);
    println!("{frame}");
    assert!(frame.contains("/deploy [env]"));
    assert!(frame.contains("ship the current branch · project"));
    assert!(frame.contains("check the dependencies · plugin acme"));
    let (x, y) = find(&frame, "· plugin acme");
    assert_eq!(
        buffer[(x, y)].fg,
        theme().faint,
        "where it comes from is quiet"
    );

    // Tab leaves room for its argument; Enter sends it for the agent to expand.
    app.on_key(press(KeyCode::Tab), false);
    assert_eq!(app.editor.text(), "/deploy ");
    type_text(&mut app, "staging");
    key(&mut app, KeyCode::Enter).await;
    let Some(Command::Prompt { text, .. }) = sent(&app).pop() else {
        panic!("a prompt");
    };
    assert_eq!(text, "/deploy staging");

    app.perform(Action::Help).await;
    if let Some(Overlay::Help(help)) = &mut app.overlay {
        help.scroll = usize::MAX / 2;
    }
    let frame = render(&mut app, 100, 40);
    println!("{frame}");
    assert!(frame.contains("/deploy [env]"));
    assert!(frame.contains("ship the current branch"));
    assert!(frame.contains("· project"));
    assert!(
        !frame.contains("a custom help"),
        "a built-in name stays built in"
    );
}

#[tokio::test]
async fn enabling_a_plugin_shows_what_it_brings() {
    let mut app = app();
    let contents = "acme 1.2.0: deploy helpers\n  commands: deploy\n  \
                    MCP servers (start programs): acme (npx acme-mcp)\n  \
                    hooks (run commands): PreToolUse: ./guard.sh\n  scripts: bin/release";
    emit(
        &mut app,
        Event::PermissionRequested {
            request_id: "r1".into(),
            call_id: "c1".into(),
            tool: "malac_configure".into(),
            summary: "enable acme".into(),
            input: json!({"plugin": "acme", "contents": contents}),
        },
    );
    let buffer = draw_buffer(&mut app, 100, 36);
    let frame = screen(&buffer);
    println!("{frame}");
    assert!(frame.contains("main wants to enable the plugin acme"));
    for line in [
        "acme 1.2.0: deploy helpers",
        "commands: deploy",
        "MCP servers (start programs): acme (npx acme-mcp)",
        "hooks (run commands): PreToolUse: ./guard.sh",
        "scripts: bin/release",
        "Allowing it lets these run as you",
    ] {
        assert!(frame.contains(line), "`{line}` is shown");
    }
    let warning = theme().warning;
    for part in ["hooks (run commands)", "MCP servers", "scripts:"] {
        let (x, y) = find(&frame, part);
        assert_eq!(buffer[(x, y)].fg, warning, "{part} stands out");
    }
    let (x, y) = find(&frame, "commands: deploy");
    assert_ne!(buffer[(x, y)].fg, warning, "the rest does not");
    assert!(frame.contains(" Allow  y "));
}

fn remote(app: &mut App) {
    app.backend
        .set_endpoint(Endpoint::Saved(crate::connections::Connection {
            name: "studio".into(),
            url: "http://192.0.2.1:7700".into(),
            token: "t".into(),
            device: None,
            added: chrono::Utc::now(),
        }));
}

#[tokio::test]
async fn files_from_here_go_to_a_daemon_elsewhere_as_contents() {
    use base64::Engine;
    let base64 = base64::engine::general_purpose::STANDARD;
    let mut app = app();
    remote(&mut app);
    let here = tempfile::tempdir().unwrap();
    for (dir, text) in [("one", "first notes\n"), ("two", "second notes\n")] {
        std::fs::create_dir_all(here.path().join(dir)).unwrap();
        std::fs::write(here.path().join(dir).join("notes.md"), text).unwrap();
    }
    app.on_paste(&here.path().join("one/notes.md").display().to_string());
    app.on_paste(&here.path().join("two/notes.md").display().to_string());
    app.on_paste(&here.path().join("one").display().to_string());
    assert_eq!(app.attachments.len(), 2, "a folder stays here");
    let flash = &app.flash.as_ref().expect("a flash").text;
    assert!(
        flash.ends_with("one/ stays on this machine: studio can be sent files, not folders"),
        "{flash}"
    );

    type_text(&mut app, "compare");
    key(&mut app, KeyCode::Enter).await;
    let sent = sent_json(&app);
    let prompt = sent
        .iter()
        .find(|c| c["type"] == "prompt")
        .expect("a prompt");
    assert_eq!(
        prompt["attachments"],
        json!([
            {"path": "notes.md", "data": base64.encode("first notes\n")},
            {"path": "notes (2).md", "data": base64.encode("second notes\n")},
        ]),
        "named by the file alone, told apart"
    );
}

#[tokio::test]
async fn a_daemon_elsewhere_takes_files_up_to_its_limits() {
    let mut app = app();
    remote(&mut app);
    let here = tempfile::tempdir().unwrap();
    let sized = |name: &str, mb: u64| {
        let path = here.path().join(name);
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(mb * 1024 * 1024).unwrap();
        path.display().to_string()
    };
    app.on_paste(&sized("huge.bin", 21));
    assert!(app.attachments.is_empty());
    let flash = |app: &App| app.flash.as_ref().expect("a flash").text.clone();
    assert!(
        flash(&app).ends_with("huge.bin is 21.0 MB, more than the 20.0 MB studio accepts"),
        "{}",
        flash(&app)
    );
    app.on_paste(&sized("a.bin", 15));
    app.on_paste(&sized("b.bin", 15));
    app.on_paste(&sized("c.bin", 15));
    assert_eq!(app.attachments.len(), 2);
    assert!(
        flash(&app).contains(
            "c.bin would bring this prompt's files to 45.0 MB, more than the 40.0 MB studio \
             accepts at once"
        ),
        "{}",
        flash(&app)
    );
}

#[tokio::test]
async fn at_lists_the_daemons_files_when_it_is_elsewhere() {
    let mut app = app();
    remote(&mut app);
    let root = app.tab_cwd();
    type_text(&mut app, "explain @app");
    let asked = std::mem::take(&mut *app.backend.rests.lock().unwrap());
    assert!(
        asked
            .iter()
            .any(|r| matches!(r, Rest::Files { root: r } if *r == root)),
        "the daemon indexes its own disk: {asked:?}"
    );
    assert!(render(&mut app, 90, 24).contains("indexing") || app.files.loading);
    app.on_update(Update::Rest(
        Rest::Files { root: root.clone() },
        Ok(json!({"root": root, "files": ["README.md", "src/app.rs"], "truncated": false})),
    ));
    let frame = render(&mut app, 90, 24);
    assert!(frame.contains("src/app.rs"), "{frame}");
    app.on_key(press(KeyCode::Tab), false);
    let frame = render(&mut app, 90, 24);
    println!("{frame}");
    assert!(frame.contains("▤ src/app.rs ×"), "a chip with no size");

    key(&mut app, KeyCode::Enter).await;
    let sent = sent_json(&app);
    let prompt = sent
        .iter()
        .find(|c| c["type"] == "prompt")
        .expect("a prompt");
    assert_eq!(
        prompt["attachments"],
        json!([{"path": root.join("src/app.rs")}]),
        "a path the daemon reads itself"
    );

    // A folder the daemon will not list says so.
    app.files = Default::default();
    type_text(&mut app, "@x");
    let asked = std::mem::take(&mut *app.backend.rests.lock().unwrap());
    let rest = asked
        .into_iter()
        .find(|r| matches!(r, Rest::Files { .. }))
        .unwrap();
    app.on_update(Update::Rest(
        rest,
        Err("403 outside the browse root".into()),
    ));
    let flash = &app.flash.as_ref().expect("a flash").text;
    assert_eq!(
        flash,
        "cannot list the files there: 403 outside the browse root"
    );
}

// Voice and the files malac sends.

/// Dictation, speech and talk mode all ready, as a daemon with an audio
/// provider reports them.
fn voice_ready() -> crate::protocol::VoiceStatus {
    let feature = |model: &str, voice: Option<&str>| crate::protocol::VoiceFeature {
        enabled: true,
        model: Some(model.into()),
        configured: None,
        voice: voice.map(Into::into),
        models: vec![model.into()],
    };
    crate::protocol::VoiceStatus {
        dictation: feature("audio/whisper-1", None),
        speech: feature("audio/tts-1", Some("alloy")),
        talk: feature("audio/gpt-realtime", Some("marin")),
        language: None,
        voices: vec!["alloy".into(), "marin".into()],
        checked: None,
    }
}

fn voice_app() -> App {
    let mut app = app();
    // Whatever another test left in terminal.json.
    app.speak = true;
    app.on_message(ServerMessage::Voice {
        voice: voice_ready(),
    });
    app
}

fn voice_calls(app: &App) -> Vec<String> {
    std::mem::take(&mut *app.backend.voice.lock().unwrap())
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn flash(app: &App) -> String {
    app.flash
        .as_ref()
        .map(|f| f.text.clone())
        .unwrap_or_default()
}

#[tokio::test]
async fn dictation_types_into_the_prompt() {
    let mut app = app();
    // Without a model the key says what is missing.
    let action = app.on_key(ctrl('x'), false).unwrap();
    app.perform(action).await;
    assert!(app.dictation.is_none());
    assert!(flash(&app).contains("dictation is off"), "{}", flash(&app));
    assert!(!render(&mut app, 100, 24).contains("^X dictate"));

    let mut app = voice_app();
    sent(&app);
    assert!(render(&mut app, 100, 24).contains("enter send · @ attach a file · ^X dictate"));
    type_text(&mut app, "make");
    let action = app.on_key(ctrl('x'), false).unwrap();
    assert_eq!(action, Action::Dictate);
    app.perform(action).await;
    assert_eq!(voice_calls(&app), ["dictate"]);
    let frame = render(&mut app, 100, 24);
    assert!(
        frame.contains("● recording 0:00 · ^X or enter stops · esc cancels"),
        "{frame}"
    );
    // Enter stops it rather than sending the prompt.
    key(&mut app, KeyCode::Enter).await;
    assert!(sent(&app).is_empty());
    assert!(render(&mut app, 100, 24).contains("transcribing…"));
    app.on_update(Update::Dictated(Ok("the button green".into())));
    assert!(app.dictation.is_none());
    assert_eq!(app.editor.text(), "make the button green");
    assert!(!render(&mut app, 100, 24).contains("transcribing"));

    // Esc throws a recording away; a late answer changes nothing.
    key(&mut app, KeyCode::F(1)).await;
    app.overlay = None;
    let action = app.on_key(ctrl('x'), false).unwrap();
    app.perform(action).await;
    key(&mut app, KeyCode::Esc).await;
    assert!(app.dictation.is_none());
    app.on_update(Update::Dictated(Ok("ignored".into())));
    assert_eq!(app.editor.text(), "make the button green");

    // Failures say why.
    let action = app.on_key(ctrl('x'), false).unwrap();
    app.perform(action).await;
    app.on_update(Update::Dictated(Err("no microphone".into())));
    assert_eq!(flash(&app), "dictation: no microphone");
}

#[tokio::test]
async fn speech_plays_the_sentences_meant_for_this_terminal() {
    let mut app = voice_app();
    assert_eq!(
        sent_json(&app),
        [json!({"type": "voice_listen", "on": true})]
    );
    app.on_update(Update::Reply(
        Reply::Listen,
        Ok(ServerMessage::VoiceListening {
            client_ref: None,
            listener: Some(7),
        }),
    ));
    assert_eq!(app.listener, Some(7));
    let say = |id: &str, reason: &str, listener: u64| ServerMessage::Say {
        id: id.into(),
        text: "malac needs your approval to run cargo publish.".into(),
        reason: reason.into(),
        tab: Some(TAB.into()),
        agent: Some(AGENT.into()),
        listener,
    };
    app.on_message(say("s1", "approval", 7));
    app.on_message(say("s2", "approval", 8));
    assert_eq!(voice_calls(&app), ["say s1"]);
    let frame = render(&mut app, 100, 24);
    assert!(frame.contains("said aloud: malac needs your approval to run cargo publish."));
    // What the agent says with `say` already shows as the tool call.
    app.on_message(say("s3", "agent", 7));
    assert_eq!(voice_calls(&app), ["say s3"]);
    // Answering an approval anywhere stops what is playing here.
    app.on_message(ServerMessage::SayCancelled {
        ids: vec!["s1".into(), "s3".into()],
    });
    assert_eq!(voice_calls(&app), ["cancel s1,s3"]);

    // `/speak off` stops listening; reconnecting listens again when on.
    type_text(&mut app, "/speak off");
    key(&mut app, KeyCode::Enter).await;
    assert!(!app.speak);
    assert_eq!(
        sent_json(&app),
        [json!({"type": "voice_listen", "on": false})]
    );
    app.on_message(say("s4", "approval", 7));
    assert!(voice_calls(&app).is_empty());
    type_text(&mut app, "/speak");
    key(&mut app, KeyCode::Enter).await;
    assert!(app.speak);
    assert_eq!(flash(&app), "this terminal speaks when malac does");
    assert_eq!(
        sent_json(&app),
        [json!({"type": "voice_listen", "on": true})]
    );
    app.disconnected();
    assert_eq!(app.listener, None);
}

#[tokio::test]
async fn voice_status_and_settings() {
    let mut app = voice_app();
    type_text(&mut app, "/voice");
    key(&mut app, KeyCode::Enter).await;
    let frame = render(&mut app, 120, 30);
    assert!(
        frame.contains("dictation  ready       audio/whisper-1  Ctrl-X in the prompt"),
        "{frame}"
    );
    assert!(frame.contains(
        "speech     ready       audio/tts-1 · voice alloy  on this terminal · /speak off"
    ));
    assert!(frame.contains("talk       ready       audio/gpt-realtime · voice marin  /talk"));
    assert!(frame.contains("change: /voice <setting> <value>"));

    type_text(&mut app, "/voice speech false");
    key(&mut app, KeyCode::Enter).await;
    assert_eq!(voice_calls(&app), [r#"set {"speech":false}"#]);
    type_text(&mut app, "/voice talk_voice cedar");
    key(&mut app, KeyCode::Enter).await;
    assert_eq!(voice_calls(&app), [r#"set {"talk_voice":"cedar"}"#]);
    type_text(&mut app, "/voice loudness 11");
    key(&mut app, KeyCode::Enter).await;
    assert!(voice_calls(&app).is_empty());
    assert!(render(&mut app, 120, 30).contains("no voice setting `loudness`"));

    // The daemon's answer updates what the interface shows.
    sent(&app);
    app.on_update(Update::Reply(
        Reply::Listen,
        Ok(ServerMessage::VoiceListening {
            client_ref: None,
            listener: Some(3),
        }),
    ));
    let mut off = voice_ready();
    off.speech.enabled = false;
    app.on_update(Update::Rest(
        Rest::VoiceSet,
        Ok(serde_json::to_value(&off).unwrap()),
    ));
    assert_eq!(flash(&app), "voice settings saved");
    assert!(!app.voice.speech.enabled);
    // Speech went off, so this terminal stops listening.
    assert_eq!(
        sent_json(&app),
        [json!({"type": "voice_listen", "on": false})]
    );
    assert_eq!(app.listener, None);

    // The View menu has the switches.
    let items = app.menu_items(4);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    for label in [
        "Dictate",
        "Talk mode",
        "Speak on this terminal",
        "Speech",
        "Voice settings…",
    ] {
        assert!(labels.contains(&label), "{labels:?}");
    }
    let speech = items.iter().find(|i| i.label == "Speech").unwrap();
    assert_eq!(speech.checked, Some(false));
    assert_eq!(
        speech.action,
        Some(Action::SetVoice("speech".into(), "true".into()))
    );
}

fn shared_tag(id: &str, name: &str, media_type: &str, size: u64) -> String {
    malac_core::shared::SharedFile {
        id: id.into(),
        name: name.into(),
        media_type: media_type.into(),
        size,
    }
    .tag()
}

fn tool_result(app: &mut App, call: &str, tool: &str, output: String) {
    emit(
        app,
        Event::ToolStarted {
            call_id: call.into(),
            tool: tool.into(),
            summary: String::new(),
            input: json!({}),
        },
    );
    emit(
        app,
        Event::ToolFinished {
            call_id: call.into(),
            tool: tool.into(),
            output,
            is_error: false,
            millis: 40,
        },
    );
}

#[tokio::test]
async fn files_malac_sends_show_and_open() {
    let mut app = app();
    type_text(&mut app, "/open");
    key(&mut app, KeyCode::Enter).await;
    assert_eq!(
        flash(&app),
        "nothing to open: malac has sent no files or images here"
    );

    tool_result(
        &mut app,
        "c1",
        "browser_screenshot",
        "Took a screenshot.\n[image/png image]".into(),
    );
    let output = format!(
        "Sent to the user: shot.png, notes.txt.\n<caption>The settings page after the fix</caption>\n{}\n{}",
        shared_tag("0193a1", "shot.png", "image/png", 84_211),
        shared_tag("0193a2", "notes.txt", "text/plain", 18),
    );
    tool_result(&mut app, "c2", "send_to_user", output);
    let frame = render(&mut app, 100, 24);
    assert!(
        frame.contains("● sent you 2 files  The settings page after the fix"),
        "{frame}"
    );
    assert!(frame.contains("shot.png  82.2 KB · image"), "{frame}");
    assert!(frame.contains("notes.txt  18 B · TXT file"));
    assert!(!frame.contains("<shared"));
    assert!(frame.contains("1 image · /open"));

    // `/open` is the newest, `/open 3` the screenshot before them.
    type_text(&mut app, "/open");
    key(&mut app, KeyCode::Enter).await;
    type_text(&mut app, "/open 3");
    key(&mut app, KeyCode::Enter).await;
    assert_eq!(
        voice_calls(&app),
        ["open notes.txt", "open browser_screenshot image 1"]
    );
    type_text(&mut app, "/open 4");
    key(&mut app, KeyCode::Enter).await;
    assert!(flash(&app).starts_with("there are 3 files and images here"));

    // A click on a file's row opens that file.
    let (x, y) = find(&frame, "shot.png");
    let action = click(&mut app, x, y, MouseButton::Left).unwrap();
    app.perform(action).await;
    assert_eq!(voice_calls(&app), ["open shot.png"]);
    // So does the context menu, and `o` on a selected entry.
    click(&mut app, x, y, MouseButton::Right);
    let action = app.on_key(press(KeyCode::Char('o')), false).unwrap();
    assert!(matches!(action, Action::OpenEntry(_, 0)), "{action:?}");
    app.overlay = None;
    let (x, y) = find(&frame, "browser_screenshot");
    click(&mut app, x, y, MouseButton::Right);
    let frame = render(&mut app, 100, 24);
    assert!(frame.contains("Open image"));
}

#[tokio::test]
async fn talk_mode_shows_in_the_transcript_and_the_status_line() {
    let mut app = app();
    type_text(&mut app, "/talk");
    key(&mut app, KeyCode::Enter).await;
    assert!(app.talk.is_none());
    assert!(flash(&app).contains("talk mode is off"), "{}", flash(&app));

    let mut app = voice_app();
    type_text(&mut app, "/talk");
    key(&mut app, KeyCode::Enter).await;
    assert_eq!(flash(&app), "not connected to the daemon");

    app.talk = Some(super::voice::TalkView {
        talk: crate::voice::Talk::idle(),
        session: 1,
        agent: Some(AGENT.into()),
        state: crate::voice::TalkState::Connecting,
    });
    use crate::voice::{TalkEvent, TalkState};
    app.on_update(Update::Talk(
        1,
        TalkEvent::Ready {
            model: "audio/gpt-realtime".into(),
            voice: "marin".into(),
        },
    ));
    app.on_update(Update::Talk(1, TalkEvent::State(TalkState::Speaking)));
    app.on_update(Update::Talk(
        1,
        TalkEvent::Said {
            you: true,
            text: "make the button green".into(),
        },
    ));
    app.on_update(Update::Talk(
        1,
        TalkEvent::Tool("Asked malac: make the button green".into()),
    ));
    // Another session's events are not this one's.
    app.on_update(Update::Talk(9, TalkEvent::Closed("old".into())));
    assert!(app.talk.is_some());
    app.flash = None;
    let frame = render(&mut app, 110, 24);
    assert!(
        frame.contains("talk mode · audio/gpt-realtime · voice marin"),
        "{frame}"
    );
    assert!(frame.contains("you · make the button green"));
    assert!(frame.contains("voice · Asked malac: make the button green"));
    assert!(
        frame.contains("● talk · speaking · microphone paused · Esc ends"),
        "{frame}"
    );
    // Dictation waits for talk mode to end, and Esc ends it.
    let action = app.on_key(ctrl('x'), false).unwrap();
    app.perform(action).await;
    assert!(app.dictation.is_none());
    assert_eq!(app.on_key(press(KeyCode::Esc), false), Some(Action::Talk));
    app.on_update(Update::Talk(1, TalkEvent::Closed("talk mode ended".into())));
    assert!(app.talk.is_none());
    assert_eq!(flash(&app), "talk mode ended");
}

#[tokio::test]
async fn help_lists_the_voice_keys_and_commands() {
    let mut app = app();
    key(&mut app, KeyCode::F(1)).await;
    let frame = render(&mut app, 120, 50);
    assert!(frame.contains("Ctrl-X"), "{frame}");
    assert!(frame.contains("dictate: speak"));
    for _ in 0..6 {
        key(&mut app, KeyCode::PageDown).await;
    }
    let frame = render(&mut app, 120, 50);
    for command in [
        "/open [n]",
        "/talk",
        "/speak [on|off]",
        "/voice [setting value]",
    ] {
        assert!(frame.contains(command), "{command}: {frame}");
    }
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
        let backend = Backend::new(client, Endpoint::Local, updates_tx, connections);
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

#[tokio::test(flavor = "multi_thread")]
async fn files_and_images_open_through_the_daemon() {
    let home = scratch_home();
    let dir = home.join("project/voice-files");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    std::fs::write(dir.join("notes.txt"), "remember the milk\n").unwrap();
    let png = b"\x89PNG\r\n\x1a\nnot really an image".to_vec();
    std::fs::write(dir.join("dot.png"), &png).unwrap();
    let script = Arc::new(Script::default());
    script.0.lock().unwrap().extend([
        Message::assistant(vec![Block::ToolUse {
            id: "r1".into(),
            name: "read".into(),
            input: json!({"path": "dot.png"}),
        }]),
        Message::assistant(vec![Block::ToolUse {
            id: "s1".into(),
            name: "send_to_user".into(),
            input: json!({"files": ["notes.txt"], "message": "the notes"}),
        }]),
        Message::assistant(vec![Block::text("shared them")]),
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

    let mut term = Term::open(
        &file,
        Launch {
            workspace: WorkspaceChoice::New,
            cwd: dir.clone(),
            name: Some("files".into()),
            ..Default::default()
        },
    )
    .await;
    term.app.send("share the notes".into());
    term.until("the turn ends", |app| shows(app, "shared them"))
        .await;
    let frame = render(&mut term.app, 100, 30);
    assert!(frame.contains("sent you a file  the notes"), "{frame}");
    assert!(frame.contains("notes.txt  18 B · TXT file"), "{frame}");
    assert!(frame.contains("1 image · /open"), "{frame}");

    // The newest is the shared file, downloaded from the daemon.
    let opened = |name: &str| {
        let name = name.to_owned();
        move |app: &App| {
            app.flash
                .as_ref()
                .is_some_and(|f| f.text == format!("opened {name}"))
        }
    };
    term.app.perform(Action::OpenFile(1)).await;
    term.until("the notes open", opened("notes.txt")).await;
    let path = super::voice::OPENED
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "remember the milk\n"
    );
    // Before it, the image `read` returned, from the saved conversation.
    term.app.perform(Action::OpenFile(2)).await;
    term.until("the image opens", opened("read image 1")).await;
    let path = super::voice::OPENED
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert_eq!(path.extension().unwrap(), "png");
    assert_eq!(std::fs::read(&path).unwrap(), png);
    let opened = std::mem::take(&mut *super::voice::OPENED.lock().unwrap());
    for path in opened {
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
    let _ = std::fs::remove_dir(std::env::temp_dir().join("malac-open"));

    hub.request_stop("test");
    server.await.unwrap().unwrap();
}
