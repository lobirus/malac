//! What the transcript pane shows. Every agent gets an [`AgentView`] built
//! from the event stream; its entries are rendered to lines once per width
//! and cached until they change.

use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use malac_core::attach as core_attach;
use malac_core::event::{AgentId, Envelope, Event, Level, Todo, TodoStatus, TurnStatus};
use malac_core::message::{Message, Role, Usage};
use malac_core::permission::Decision;
use malac_core::session::DecisionRecord;
use malac_core::shared::{SharedFile, parse_tags};
use malac_core::{AgentSnapshot, Block};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::attach;
use super::markdown;
use super::text::{self, RowText, sanitize};
use super::theme::theme;
use super::voice;
use crate::render::tokens;

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Where the agent tools put a session that was compacted.
const COMPACTED_PREFIX: &str = "This conversation was compacted.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Asking,
    Running,
    Done,
    Failed,
    Denied,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub call_id: String,
    pub request_id: Option<String>,
    pub name: String,
    pub summary: String,
    pub input: Value,
    pub state: ToolState,
    pub output: String,
    pub millis: u64,
    pub started: Instant,
    pub expanded: bool,
    /// The approval it waited for: the answer and where it was given.
    pub decision: Option<(Decision, Option<String>)>,
}

#[derive(Debug, Clone)]
pub struct AgentCard {
    pub id: AgentId,
    pub label: String,
    pub model: String,
    pub started: Instant,
    pub finished: Option<(TurnStatus, Duration)>,
    pub report: String,
}

#[derive(Debug, Clone)]
pub enum Entry {
    User(String),
    Assistant {
        text: String,
        /// The turn's closing text: its answer.
        last: bool,
    },
    Thinking {
        text: String,
        expanded: bool,
    },
    Tool(ToolCall),
    Agent(AgentCard),
    Todos(Vec<Todo>),
    Notice(Level, String),
    Compacted,
    Footer(String),
    /// Output of a slash command or menu action.
    Info(Vec<Line<'static>>),
}

impl Entry {
    /// Entries whose look changes with time, so they are drawn every frame.
    fn is_live(&self) -> bool {
        match self {
            Entry::Tool(tool) => matches!(tool.state, ToolState::Asking | ToolState::Running),
            Entry::Agent(card) => card.finished.is_none(),
            _ => false,
        }
    }

    /// Tool calls shown in another form: spawning an agent appears as its
    /// card, updating the todo list as the list itself.
    fn is_hidden(&self) -> bool {
        match self {
            Entry::Tool(tool) => {
                matches!(tool.name.as_str(), "agent_spawn" | "todo")
                    && tool.state != ToolState::Failed
            }
            _ => false,
        }
    }

    /// Secondary entries sit together without blank rows between them.
    fn is_compact(&self) -> bool {
        matches!(
            self,
            Entry::Tool(_) | Entry::Agent(_) | Entry::Thinking { .. } | Entry::Todos(_)
        )
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Entry::Assistant {
            text: text.into(),
            last: false,
        }
    }

    /// The text copied for this entry.
    pub fn plain_text(&self) -> String {
        match self {
            Entry::Assistant { text, .. } => text.clone(),
            Entry::User(text) => attach::split_user_text(text).0,
            Entry::Footer(text) => text.clone(),
            Entry::Thinking { text, .. } => text.clone(),
            Entry::Tool(tool) => {
                let mut out = format!("{} {}", tool.name, tool.summary);
                if !tool.output.is_empty() {
                    out.push_str("\n\n");
                    out.push_str(&tool.output);
                }
                out
            }
            Entry::Agent(card) => format!("{}: {}", card.label, card.report),
            Entry::Todos(items) => items
                .iter()
                .map(|t| {
                    let mark = match t.status {
                        TodoStatus::Completed => "[x]",
                        TodoStatus::InProgress => "[~]",
                        TodoStatus::Pending => "[ ]",
                    };
                    format!("{mark} {}", t.content)
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Entry::Notice(_, message) => message.clone(),
            Entry::Compacted => "conversation compacted".into(),
            Entry::Info(lines) => lines
                .iter()
                .map(Line::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

struct Rendered {
    width: usize,
    lines: Vec<Line<'static>>,
    rows: Vec<RowText>,
    code: Vec<markdown::CodeBlock>,
}

pub struct Item {
    pub entry: Entry,
    rendered: Option<Rendered>,
    gap: bool,
}

impl Item {
    fn new(entry: Entry) -> Self {
        Self {
            entry,
            rendered: None,
            gap: false,
        }
    }

    /// Rows this item takes, including the blank row above it.
    pub fn height(&self) -> usize {
        self.lines().len() + usize::from(self.gap)
    }

    pub fn gap(&self) -> bool {
        self.gap
    }

    pub fn lines(&self) -> &[Line<'static>] {
        self.rendered.as_ref().map_or(&[], |r| &r.lines)
    }

    /// What of each line is text, for selecting and copying.
    pub fn rows(&self) -> &[RowText] {
        self.rendered.as_ref().map_or(&[], |r| &r.rows)
    }

    /// The code block shown on `line` (relative to the item's first line),
    /// or the item's last code block.
    pub fn code_block(&self, line: Option<usize>) -> Option<&str> {
        let code = &self.rendered.as_ref()?.code;
        line.and_then(|l| code.iter().find(|c| c.lines.contains(&l)))
            .or(code.last())
            .map(|c| c.text.as_str())
    }
}

/// What rendering needs besides the entry itself.
pub struct Ctx<'a> {
    pub width: usize,
    pub now: Instant,
    pub snapshots: &'a HashMap<AgentId, AgentSnapshot>,
}

impl Ctx<'_> {
    fn spinner(&self) -> &'static str {
        let tick = self.now.elapsed().as_millis() as usize;
        SPINNER[(tick / 80) % SPINNER.len()]
    }
}

#[derive(Default)]
pub struct Transcript {
    items: Vec<Item>,
    /// `cwd/`, stripped from tool summaries.
    cwd: String,
    /// Deltas arrived since the last assistant message.
    streamed: bool,
}

impl Transcript {
    pub fn new(cwd: &str) -> Self {
        Self {
            cwd: format!("{}/", cwd.trim_end_matches('/')),
            ..Default::default()
        }
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn push(&mut self, entry: Entry) {
        self.items.push(Item::new(entry));
    }

    /// Mutable access to an entry; its cached lines are dropped.
    pub fn entry_mut(&mut self, index: usize) -> Option<&mut Entry> {
        let item = self.items.get_mut(index)?;
        item.rendered = None;
        Some(&mut item.entry)
    }

    fn last_mut(&mut self) -> Option<&mut Entry> {
        let index = self.items.len().checked_sub(1)?;
        self.entry_mut(index)
    }

    fn tool_mut(&mut self, call_id: &str) -> Option<&mut ToolCall> {
        let index = self
            .items
            .iter()
            .rposition(|i| matches!(&i.entry, Entry::Tool(t) if t.call_id == call_id))?;
        match self.entry_mut(index) {
            Some(Entry::Tool(tool)) => Some(tool),
            _ => None,
        }
    }

    fn has_card(&self, id: &str) -> bool {
        self.items
            .iter()
            .any(|i| matches!(&i.entry, Entry::Agent(c) if c.id == id))
    }

    fn agent_card_mut(&mut self, id: &str) -> Option<&mut AgentCard> {
        let index = self
            .items
            .iter()
            .rposition(|i| matches!(&i.entry, Entry::Agent(c) if c.id == id))?;
        match self.entry_mut(index) {
            Some(Entry::Agent(card)) => Some(card),
            _ => None,
        }
    }

    /// Expands or collapses a tool call or thinking block.
    pub fn toggle(&mut self, index: usize) -> bool {
        match self.entry_mut(index) {
            Some(Entry::Tool(tool)) => tool.expanded = !tool.expanded,
            Some(Entry::Thinking { expanded, .. }) => *expanded = !*expanded,
            _ => return false,
        }
        true
    }

    pub fn set_thinking_expanded(&mut self, show: bool) {
        for item in &mut self.items {
            if let Entry::Thinking { expanded, .. } = &mut item.entry {
                *expanded = show;
                item.rendered = None;
            }
        }
    }

    /// The newest answer, for copying.
    pub fn last_answer(&self) -> Option<&str> {
        self.items.iter().rev().find_map(|i| match &i.entry {
            Entry::Assistant { text, .. } if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        })
    }

    /// Marks the newest text of the turn since the last prompt as its answer.
    fn mark_answer(&mut self) {
        let index = self
            .items
            .iter()
            .rposition(|i| matches!(i.entry, Entry::Assistant { .. } | Entry::User(_)));
        if let Some(Entry::Assistant { last, .. }) = index.and_then(|i| self.entry_mut(i)) {
            *last = true;
        }
    }

    /// Marks every turn's answer, for a loaded conversation.
    fn mark_answers(&mut self) {
        let mut answer = None;
        for index in 0..self.items.len() {
            match self.items[index].entry {
                Entry::Assistant { .. } => answer = Some(index),
                Entry::User(_) => {
                    if let Some(Entry::Assistant { last, .. }) =
                        answer.take().and_then(|i| self.entry_mut(i))
                    {
                        *last = true;
                    }
                }
                _ => {}
            }
        }
        if let Some(Entry::Assistant { last, .. }) = answer.and_then(|i| self.entry_mut(i)) {
            *last = true;
        }
    }

    /// Renders entries that changed, or all of them after a resize.
    pub fn prepare(&mut self, ctx: &Ctx<'_>) {
        let mut previous_compact = None;
        for item in &mut self.items {
            let stale =
                item.rendered.as_ref().is_none_or(|r| r.width != ctx.width) || item.entry.is_live();
            if stale {
                item.rendered = Some(render(&item.entry, &self.cwd, ctx));
            }
            let visible = !item.lines().is_empty();
            let compact = item.entry.is_compact();
            let footer = matches!(item.entry, Entry::Footer(_));
            item.gap = visible && !footer && previous_compact.is_some_and(|p| !(p && compact));
            if visible {
                previous_compact = Some(compact);
            }
        }
    }

    /// The lines drawn at rows `range` and what of each is text; blank
    /// rows between entries have none.
    pub fn rows_in(&self, range: Range<usize>) -> Vec<Option<(&Line<'static>, RowText)>> {
        let mut out = Vec::with_capacity(range.len());
        let mut row = 0;
        for item in &self.items {
            let height = item.height();
            if row + height <= range.start {
                row += height;
                continue;
            }
            if row >= range.end {
                break;
            }
            for i in 0..height {
                let at = row + i;
                if at < range.start || at >= range.end {
                    continue;
                }
                let line = i.checked_sub(usize::from(item.gap));
                out.push(line.map(|l| (&item.lines()[l], item.rows()[l])));
            }
            row += height;
        }
        out
    }

    /// Entries that are the answers of their turns, for jumping between them.
    pub fn answers(&self) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, i)| matches!(i.entry, Entry::Assistant { last: true, .. }))
            .map(|(n, _)| n)
            .collect()
    }

    /// Total rows after [`Transcript::prepare`].
    pub fn height(&self) -> usize {
        self.items.iter().map(Item::height).sum()
    }

    /// Entries overlapping rows `range`, as (index, first row of the entry).
    pub fn visible(&self, range: Range<usize>) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut row = 0;
        for (index, item) in self.items.iter().enumerate() {
            let height = item.height();
            if row + height > range.start && row < range.end && height > 0 {
                out.push((index, row));
            }
            row += height;
            if row >= range.end {
                break;
            }
        }
        out
    }

    /// The first row of an entry, for scrolling it into view.
    pub fn row_of(&self, index: usize) -> usize {
        self.items[..index.min(self.items.len())]
            .iter()
            .map(Item::height)
            .sum()
    }

    /// Fills a transcript from a saved session's messages. A prompt reads
    /// as its `user_message` event did: the typed text, then the files it
    /// attached.
    pub fn load(
        &mut self,
        history: &[Message],
        decisions: &[DecisionRecord],
        summarize: impl Fn(&str, &Value) -> String,
    ) {
        let mut calls: HashMap<String, usize> = HashMap::new();
        for message in history {
            let mut typed = Vec::new();
            let mut attached = Vec::new();
            for block in &message.content {
                match (message.role, block) {
                    (Role::User, Block::Text { text }) if text.starts_with(COMPACTED_PREFIX) => {
                        self.push(Entry::Compacted);
                    }
                    (Role::User, Block::Text { text })
                        if text.starts_with("<recalled-memories>") => {}
                    (Role::User, Block::Text { text }) => match core_attach::attached_path(text) {
                        Some(path) => attached.push(path),
                        None => typed.push(text.as_str()),
                    },
                    (Role::Assistant, Block::Text { text }) if !text.trim().is_empty() => {
                        self.push(Entry::assistant(text.clone()));
                    }
                    (Role::Assistant, Block::Thinking { text, .. }) if !text.trim().is_empty() => {
                        self.push(Entry::Thinking {
                            text: text.clone(),
                            expanded: false,
                        });
                    }
                    (_, Block::ToolUse { id, name, input }) => {
                        calls.insert(id.clone(), self.items.len());
                        self.push(Entry::Tool(ToolCall {
                            call_id: id.clone(),
                            request_id: None,
                            name: name.clone(),
                            summary: summarize(name, input),
                            input: input.clone(),
                            state: ToolState::Done,
                            output: String::new(),
                            millis: 0,
                            started: Instant::now(),
                            expanded: false,
                            decision: None,
                        }));
                    }
                    (
                        _,
                        Block::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        },
                    ) => {
                        let Some(&index) = calls.get(tool_use_id) else {
                            continue;
                        };
                        if let Some(Entry::Tool(tool)) = self.entry_mut(index) {
                            tool.output = content
                                .iter()
                                .map(|b| match b {
                                    Block::Text { text } => text.as_str(),
                                    _ => "[image]",
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            if *is_error {
                                tool.state = ToolState::Failed;
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !typed.is_empty() || !attached.is_empty() {
                let text = attach::user_text(&typed.join("\n"), &attached);
                self.push(Entry::User(text));
            }
        }
        for record in decisions {
            if let Some(Entry::Tool(tool)) =
                calls.get(&record.call_id).and_then(|&i| self.entry_mut(i))
            {
                tool.decision = Some((record.decision, record.by.clone()));
                if record.decision == Decision::Deny {
                    tool.state = ToolState::Denied;
                }
            }
        }
        self.mark_answers();
    }
}

/// One agent as the interface knows it.
pub struct AgentView {
    pub id: AgentId,
    pub label: String,
    pub parent: Option<AgentId>,
    pub model: String,
    pub transcript: Transcript,
    pub running: bool,
    pub last_status: Option<TurnStatus>,
    pub turn_started: Option<Instant>,
    pub turn_usage: Usage,
    pub total: Usage,
    pub context: u64,
    pub todos: Vec<Todo>,
    /// The tool call the model is writing right now.
    pub writing: Option<String>,
    /// Events that arrived while the saved conversation was being read.
    pending: Option<Vec<Envelope>>,
    started: Instant,
}

impl AgentView {
    pub fn new(id: &str, label: &str, parent: Option<AgentId>, model: &str, cwd: &str) -> Self {
        Self {
            id: id.to_owned(),
            label: label.to_owned(),
            parent,
            model: model.to_owned(),
            transcript: Transcript::new(cwd),
            running: false,
            last_status: None,
            turn_started: None,
            turn_usage: Usage::default(),
            total: Usage::default(),
            context: 0,
            todos: Vec::new(),
            writing: None,
            pending: None,
            started: Instant::now(),
        }
    }

    pub fn from_snapshot(snapshot: &AgentSnapshot) -> Self {
        let info = &snapshot.info;
        let mut view = Self::new(
            &info.id,
            &info.label,
            info.parent.clone(),
            &snapshot.model,
            &info.cwd.display().to_string(),
        );
        view.total = snapshot.usage;
        view
    }

    /// Holds back events until [`Agents::loaded`] brings the history.
    pub fn loading(mut self) -> Self {
        self.pending = Some(Vec::new());
        self
    }

    fn apply(&mut self, event: &Event, show_thinking: bool) {
        let t = &mut self.transcript;
        match event {
            // `say` shows as its tool call; the daemon reads it aloud.
            Event::AgentStarted { .. } | Event::Say { .. } => {}
            Event::ModelChanged { model } => {
                self.model = model.clone();
                t.push(Entry::Notice(Level::Info, format!("model: {model}")));
            }
            Event::UserMessage { text } => {
                t.push(Entry::User(text.clone()));
                t.streamed = false;
                self.running = true;
                self.turn_started = Some(Instant::now());
                self.turn_usage = Usage::default();
            }
            Event::TextDelta { text: delta } => {
                t.streamed = true;
                match t.last_mut() {
                    Some(Entry::Assistant { text, .. }) => text.push_str(delta),
                    _ => t.push(Entry::assistant(delta.clone())),
                }
            }
            Event::ThinkingDelta { text: delta } => {
                t.streamed = true;
                match t.last_mut() {
                    Some(Entry::Thinking { text, .. }) => text.push_str(delta),
                    _ => t.push(Entry::Thinking {
                        text: delta.clone(),
                        expanded: show_thinking,
                    }),
                }
            }
            Event::ToolCallStreaming { tool } => self.writing = Some(tool.clone()),
            Event::AssistantMessage { message } => {
                self.writing = None;
                if !std::mem::take(&mut t.streamed) {
                    for block in &message.content {
                        match block {
                            Block::Text { text } if !text.trim().is_empty() => {
                                t.push(Entry::assistant(text.clone()));
                            }
                            Block::Thinking { text, .. } if !text.trim().is_empty() => {
                                t.push(Entry::Thinking {
                                    text: text.clone(),
                                    expanded: show_thinking,
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
            Event::PermissionRequested {
                request_id,
                call_id,
                tool,
                summary,
                input,
            } => t.push(Entry::Tool(ToolCall {
                call_id: call_id.clone(),
                request_id: Some(request_id.clone()),
                name: tool.clone(),
                summary: summary.clone(),
                input: input.clone(),
                state: ToolState::Asking,
                output: String::new(),
                millis: 0,
                started: Instant::now(),
                expanded: false,
                decision: None,
            })),
            Event::PermissionResolved {
                request_id,
                allowed,
                decision,
                by,
            } => {
                let index = t.items.iter().rposition(|i| {
                    matches!(&i.entry, Entry::Tool(c) if c.request_id.as_ref() == Some(request_id))
                });
                if let Some(Entry::Tool(tool)) = index.and_then(|i| t.entry_mut(i)) {
                    tool.state = if *allowed {
                        ToolState::Running
                    } else {
                        ToolState::Denied
                    };
                    tool.decision = decision.map(|d| (d, by.clone()));
                }
            }
            Event::ToolStarted {
                call_id,
                tool,
                summary,
                input,
            } => match t.tool_mut(call_id) {
                Some(existing) => {
                    existing.state = ToolState::Running;
                    existing.started = Instant::now();
                }
                None => t.push(Entry::Tool(ToolCall {
                    call_id: call_id.clone(),
                    request_id: None,
                    name: tool.clone(),
                    summary: summary.clone(),
                    input: input.clone(),
                    state: ToolState::Running,
                    output: String::new(),
                    millis: 0,
                    started: Instant::now(),
                    expanded: false,
                    decision: None,
                })),
            },
            Event::ToolFinished {
                call_id,
                output,
                is_error,
                millis,
                ..
            } => {
                if let Some(tool) = t.tool_mut(call_id) {
                    tool.output = output.clone();
                    tool.millis = *millis;
                    tool.state = if *is_error {
                        ToolState::Failed
                    } else {
                        ToolState::Done
                    };
                }
            }
            Event::Todos { items } => {
                self.todos = items.clone();
                match t.last_mut() {
                    Some(Entry::Todos(list)) => *list = items.clone(),
                    _ => t.push(Entry::Todos(items.clone())),
                }
            }
            Event::Usage {
                turn,
                total,
                context_tokens,
            } => {
                self.turn_usage.add(turn);
                self.total = *total;
                self.context = *context_tokens;
            }
            Event::Compacted { .. } => {
                t.push(Entry::Compacted);
                self.context = 0;
            }
            Event::Notice { level, message } => t.push(Entry::Notice(*level, message.clone())),
            Event::TurnFinished { status, .. } => {
                self.running = false;
                self.writing = None;
                self.last_status = Some(*status);
                if *status == TurnStatus::Completed {
                    t.mark_answer();
                }
                let footer = self.footer(*status);
                self.transcript.push(Entry::Footer(footer));
            }
            Event::AgentFinished { .. } | Event::Update { .. } => {}
        }
    }

    fn footer(&self, status: TurnStatus) -> String {
        let mut parts = vec![self.model.clone()];
        if self.context > 0 {
            parts.push(format!("{} context", tokens(self.context)));
        }
        if self.turn_usage.output_tokens > 0 {
            parts.push(format!("{} out", tokens(self.turn_usage.output_tokens)));
        }
        if self.turn_usage.cost > 0.0 {
            parts.push(format!("${:.4}", self.turn_usage.cost));
        }
        if let Some(started) = self.turn_started {
            parts.push(duration(started.elapsed()));
        }
        match status {
            TurnStatus::Completed => {}
            TurnStatus::Cancelled => parts.push("cancelled".into()),
            TurnStatus::Failed => parts.push("failed".into()),
        }
        parts.join(" · ")
    }
}

/// Every agent seen on the bus, in the order they started.
#[derive(Default)]
pub struct Agents {
    views: HashMap<AgentId, AgentView>,
    order: Vec<AgentId>,
    pub show_thinking: bool,
}

impl Agents {
    pub fn get(&self, id: &str) -> Option<&AgentView> {
        self.views.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut AgentView> {
        self.views.get_mut(id)
    }

    pub fn insert(&mut self, view: AgentView) -> &mut AgentView {
        let id = view.id.clone();
        if !self.views.contains_key(&id) {
            self.order.push(id.clone());
        }
        self.views.entry(id).or_insert(view)
    }

    /// Direct children of `id`, oldest first.
    pub fn children<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a AgentView> + 'a {
        self.order
            .iter()
            .filter_map(|i| self.views.get(i))
            .filter(move |v| v.parent.as_deref() == Some(id))
    }

    /// `id` and all its descendants, depth first, with their depth.
    pub fn tree(&self, id: &str) -> Vec<(usize, &AgentView)> {
        fn walk<'a>(
            agents: &'a Agents,
            id: &str,
            depth: usize,
            out: &mut Vec<(usize, &'a AgentView)>,
        ) {
            if let Some(view) = agents.get(id) {
                out.push((depth, view));
                for child in agents.children(id) {
                    walk(agents, &child.id, depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(self, id, 0, &mut out);
        out
    }

    /// The top-level agent `id` belongs to.
    pub fn root_of<'a>(&'a self, mut id: &'a str) -> &'a str {
        while let Some(parent) = self.views.get(id).and_then(|v| v.parent.as_deref()) {
            id = parent;
        }
        id
    }

    /// Shows a subagent as a card in its parent's transcript, once.
    pub fn ensure_card(&mut self, id: &str, finished: Option<TurnStatus>) {
        let Some(view) = self.views.get(id) else {
            return;
        };
        let card = AgentCard {
            id: id.to_owned(),
            label: view.label.clone(),
            model: view.model.clone(),
            started: Instant::now(),
            finished: finished.map(|status| (status, Duration::ZERO)),
            report: String::new(),
        };
        let Some(parent) = view.parent.clone() else {
            return;
        };
        if let Some(parent) = self.views.get_mut(&parent) {
            if !parent.transcript.has_card(id) {
                parent.transcript.push(Entry::Agent(card));
            }
        }
    }

    /// Fills a loading view from its saved conversation, then applies the
    /// events that came in meanwhile.
    pub fn loaded(
        &mut self,
        id: &str,
        history: &[Message],
        decisions: &[DecisionRecord],
        usage: Option<Usage>,
        summarize: impl Fn(&str, &Value) -> String,
    ) {
        let Some(view) = self.views.get_mut(id) else {
            return;
        };
        let Some(buffered) = view.pending.take() else {
            return;
        };
        // Cards of subagents that started meanwhile go after the history.
        let interim = std::mem::take(&mut view.transcript.items);
        view.transcript.load(history, decisions, summarize);
        // A prompt that began while the history was read is in both.
        let started = buffered.iter().find_map(|e| match &e.event {
            Event::UserMessage { text } => Some(text),
            _ => None,
        });
        let last_prompt = view
            .transcript
            .items
            .iter()
            .rposition(|i| matches!(&i.entry, Entry::User(_)));
        if let (Some(text), Some(index)) = (started, last_prompt) {
            if matches!(&view.transcript.items[index].entry, Entry::User(t) if t == text) {
                view.transcript.items.truncate(index);
            }
        }
        view.transcript.items.extend(interim);
        if let Some(usage) = usage {
            view.total = usage;
        }
        for envelope in &buffered {
            self.apply(envelope);
        }
    }

    /// Ties a request that was waiting when the history was read to its
    /// tool call, so its answer lands on the right card.
    pub fn claim_request(&mut self, agent: &str, call_id: &str, request_id: &str) {
        let tool = self
            .views
            .get_mut(agent)
            .and_then(|v| v.transcript.tool_mut(call_id));
        if let Some(tool) = tool.filter(|t| t.request_id.is_none()) {
            tool.request_id = Some(request_id.to_owned());
        }
    }

    pub fn apply(&mut self, envelope: &Envelope) {
        let id = &envelope.agent;
        if let Event::AgentStarted {
            label,
            parent,
            model,
            cwd,
        } = &envelope.event
        {
            let view = self.insert(AgentView::new(id, label, parent.clone(), model, cwd));
            view.model = model.clone();
            self.ensure_card(id, None);
            return;
        }
        let show_thinking = self.show_thinking;
        let Some(view) = self.views.get_mut(id) else {
            return;
        };
        match &mut view.pending {
            Some(buffered) => buffered.push(envelope.clone()),
            None => view.apply(&envelope.event, show_thinking),
        }
        let parent = view.parent.clone();
        let elapsed = view.started.elapsed();
        let model = view.model.clone();
        let Some(parent) = parent.and_then(|p| self.views.get_mut(&p)) else {
            return;
        };
        match &envelope.event {
            Event::AgentFinished { status, report } => {
                if let Some(card) = parent.transcript.agent_card_mut(id) {
                    card.finished = Some((*status, elapsed));
                    card.report = text::first_line(report).to_owned();
                }
            }
            Event::UserMessage { .. } => {
                if let Some(card) = parent.transcript.agent_card_mut(id) {
                    card.finished = None;
                }
            }
            Event::ModelChanged { .. } => {
                if let Some(card) = parent.transcript.agent_card_mut(id) {
                    card.model = model;
                }
            }
            _ => {}
        }
    }
}

/// `1.2s`, `42s`, `3m 05s`.
pub fn duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 10.0 {
        format!("{secs:.1}s")
    } else if secs < 60.0 {
        format!("{secs:.0}s")
    } else {
        format!("{}m {:02}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

/// Rendered lines with what of each is text.
#[derive(Default)]
struct Out {
    lines: Vec<Line<'static>>,
    rows: Vec<RowText>,
}

impl Out {
    fn push(&mut self, line: Line<'static>, inset: usize) {
        self.row(line, RowText::inset(inset));
    }

    fn row(&mut self, line: Line<'static>, row: RowText) {
        self.lines.push(line);
        self.rows.push(row);
    }
}

/// Secondary entries (tool calls, thinking, subagents, todos) are indented
/// under the answer text so the text keeps the left edge.
const NEST: &str = "  ";

fn render(entry: &Entry, cwd: &str, ctx: &Ctx<'_>) -> Rendered {
    let t = theme();
    let width = ctx.width.max(10);
    let body = width - 2;
    let mut code = Vec::new();
    let mut out = Out::default();
    match entry {
        _ if entry.is_hidden() => {}
        Entry::User(text) => render_user(text, width, &mut out),
        Entry::Assistant { text, last } => {
            let rendered = markdown::render(text, body);
            code = rendered.code;
            // The answer gets the accent rule; text on the way there a quiet one.
            let rule = if *last {
                Span::styled("┃ ", t.fg(t.accent))
            } else {
                Span::styled("│ ", t.faint())
            };
            for (mut line, row) in rendered.lines.into_iter().zip(rendered.rows) {
                line.spans.insert(0, rule.clone());
                out.row(
                    line,
                    RowText {
                        inset: row.inset + 2,
                        ..row
                    },
                );
            }
        }
        Entry::Thinking { text, expanded } => {
            let style = t.faint().add_modifier(Modifier::ITALIC);
            let mut head = vec![Span::raw(NEST), Span::styled("∴ Thinking", style)];
            if *expanded {
                out.push(Line::from(head), 4);
                let style = t.dim().add_modifier(Modifier::ITALIC);
                for source in sanitize(text).trim().split('\n') {
                    let spans = [Span::styled(source.to_owned(), style)];
                    for (row, join) in text::wrap_rows(&spans, body.saturating_sub(2)) {
                        let mut spans = vec![Span::raw("    ")];
                        spans.extend(row);
                        out.row(Line::from(spans), RowText { inset: 4, join });
                    }
                }
            } else {
                let preview = text::first_line(text);
                if !preview.is_empty() {
                    let room = body.saturating_sub(14);
                    head.push(Span::styled(
                        format!("  {}", text::truncate(&sanitize(preview), room)),
                        style,
                    ));
                }
                out.push(Line::from(head), 4);
            }
        }
        Entry::Tool(tool) => render_tool(tool, cwd, width, ctx, &mut out),
        Entry::Agent(card) => render_card(card, width, ctx, &mut out),
        Entry::Todos(items) => {
            for item in items {
                let (mark, mark_style, text_style) = match item.status {
                    TodoStatus::Completed => (
                        "✓",
                        t.fg(t.accent),
                        t.dim().add_modifier(Modifier::CROSSED_OUT),
                    ),
                    TodoStatus::InProgress => ("▸", t.bold(), t.bold()),
                    TodoStatus::Pending => ("○", t.dim(), t.dim()),
                };
                let content =
                    text::truncate(&sanitize(&item.content), body.saturating_sub(2)).into_owned();
                let line = Line::from(vec![
                    Span::raw(NEST),
                    Span::styled(format!("{mark} "), mark_style),
                    Span::styled(content, text_style),
                ]);
                out.push(line, 4);
            }
        }
        Entry::Notice(level, message) => {
            let (mark, style) = match level {
                Level::Info => ("  ", t.dim()),
                Level::Warning => ("! ", t.fg(t.warning)),
                Level::Error => ("✗ ", t.fg(t.error)),
            };
            let spans = [Span::styled(sanitize(message).into_owned(), style)];
            for (i, (row, join)) in text::wrap_rows(&spans, body).into_iter().enumerate() {
                let mut spans = vec![Span::styled(if i == 0 { mark } else { "  " }, style)];
                spans.extend(row);
                out.row(Line::from(spans), RowText { inset: 2, join });
            }
        }
        Entry::Compacted => {
            let label = " conversation compacted ";
            let side = width.saturating_sub(text::width(label)) / 2;
            let rule = "─".repeat(side);
            out.push(Line::styled(format!("{rule}{label}{rule}"), t.faint()), 0);
        }
        Entry::Footer(footer) => {
            let line = text::truncate(&format!("  ─ {footer}"), width).into_owned();
            out.push(Line::styled(line, t.faint()), 4);
        }
        Entry::Info(lines) => {
            for line in lines {
                out.push(text::truncate_line(line.clone(), width), 0);
            }
        }
    }
    Rendered {
        width: ctx.width,
        lines: out.lines,
        rows: out.rows,
        code,
    }
}

/// A prompt on its band, with the files and images it carried as chips.
fn render_user(text: &str, width: usize, out: &mut Out) {
    let t = theme();
    let band = Style::new().bg(t.code_bg);
    let marker = band.fg(t.dim).add_modifier(Modifier::BOLD);
    let (prompt, mentions) = attach::split_user_text(text);
    let body = width.saturating_sub(3);
    let lead = |out: &Out| if out.lines.is_empty() { "› " } else { "  " };
    if !prompt.trim().is_empty() || mentions.is_empty() {
        for source in sanitize(&prompt).trim_end().split('\n') {
            for (row, join) in text::wrap_rows(&[Span::raw(source.to_owned())], body) {
                let mut spans = vec![Span::styled(lead(out), marker)];
                spans.extend(row);
                let line = pad(Line::from(spans).style(band), width);
                out.row(line, RowText { inset: 2, join });
            }
        }
    }
    if mentions.is_empty() {
        return;
    }
    let chips: Vec<Span<'static>> = mentions
        .iter()
        .enumerate()
        .flat_map(|(i, mention)| {
            let gap = (i > 0).then(|| Span::raw("   "));
            gap.into_iter()
                .chain([Span::styled(mention.label(), band.fg(t.dim))])
        })
        .collect();
    for (row, join) in text::wrap_rows(&chips, body) {
        let mut spans = vec![Span::styled(lead(out), marker)];
        spans.extend(row);
        let line = pad(Line::from(spans).style(band), width);
        out.row(line, RowText { inset: 2, join });
    }
}

/// Extends a line with spaces so its background spans the full width.
fn pad(mut line: Line<'static>, width: usize) -> Line<'static> {
    let used = line.width();
    if used < width {
        line.spans.push(Span::raw(" ".repeat(width - used)));
    }
    line
}

/// `left` and `right` on one row, `left` truncated to make room.
fn header(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let right_width: usize = right.iter().map(|s| text::width(&s.content)).sum();
    let left = text::truncate_line(Line::from(left), width.saturating_sub(right_width + 1));
    let gap = width.saturating_sub(left.width() + right_width);
    let mut spans = left.spans;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    Line::from(spans)
}

/// The caption `send_to_user` put above its files, if any.
pub fn caption(output: &str) -> Option<&str> {
    let start = output.find("<caption>")? + "<caption>".len();
    let end = start + output[start..].find("</caption>")?;
    Some(output[start..end].trim()).filter(|c| !c.is_empty())
}

/// Files malac sent: the caption, then a row per file that opens it.
fn render_shared(tool: &ToolCall, files: &[SharedFile], width: usize, out: &mut Out) {
    let t = theme();
    let what = match files.len() {
        1 => "sent you a file".to_owned(),
        n => format!("sent you {n} files"),
    };
    let mut left = vec![
        Span::raw(NEST),
        Span::styled("● ", t.faint()),
        Span::styled(what, t.dim().add_modifier(Modifier::BOLD)),
    ];
    if let Some(caption) = caption(&tool.output) {
        left.push(Span::raw("  "));
        left.push(Span::raw(sanitize(caption).into_owned()));
    }
    let meta = vec![Span::styled("click or /open", t.faint())];
    out.push(header(left, meta, width), 4);
    for file in files {
        let detail = format!(
            "  {} · {}",
            core_attach::human_size(file.size),
            voice::kind_label(file)
        );
        let row = vec![
            Span::raw(format!("{NEST}  ")),
            Span::styled(sanitize(&file.name).into_owned(), t.fg(t.accent)),
            Span::styled(detail, t.dim()),
        ];
        out.push(text::truncate_line(Line::from(row), width), 6);
    }
}

/// What the user answered when the call asked for approval, and where.
fn decision_text(decision: Decision, by: Option<&str>) -> String {
    let verb = match decision {
        Decision::Allow => "allowed",
        Decision::AllowSession => "allowed for the session",
        Decision::Deny => "denied",
    };
    match by {
        Some(by) => format!("{verb} · {by}"),
        None => verb.to_owned(),
    }
}

/// A tool call: one quiet line until expanded.
fn render_tool(tool: &ToolCall, cwd: &str, width: usize, ctx: &Ctx<'_>, out: &mut Out) {
    let t = theme();
    if tool.name == "send_to_user" && tool.state == ToolState::Done {
        let files = parse_tags(&tool.output);
        if !files.is_empty() {
            return render_shared(tool, &files, width, out);
        }
    }
    let (mark, mark_style) = match tool.state {
        ToolState::Asking => ("?", t.fg(t.warning).add_modifier(Modifier::BOLD)),
        ToolState::Running => (ctx.spinner(), t.fg(t.accent)),
        ToolState::Done => ("●", t.faint()),
        ToolState::Failed | ToolState::Denied => ("✗", t.fg(t.error)),
    };
    let output_lines = tool.output.lines().filter(|l| !l.trim().is_empty()).count();
    let mut meta = Vec::new();
    match tool.state {
        ToolState::Asking => meta.push(Span::styled("awaiting approval", t.fg(t.warning))),
        ToolState::Denied => meta.push(Span::styled(
            decision_text(
                Decision::Deny,
                tool.decision.as_ref().and_then(|(_, by)| by.as_deref()),
            ),
            t.fg(t.error),
        )),
        ToolState::Running => meta.push(Span::styled(duration(tool.started.elapsed()), t.dim())),
        ToolState::Done | ToolState::Failed => {
            if tool.state == ToolState::Failed && !tool.expanded {
                let error = sanitize(text::first_line(&tool.output)).into_owned();
                let room = (width / 3).max(12);
                meta.push(Span::styled(
                    format!("{}  ", text::truncate(&error, room)),
                    t.fg(t.error),
                ));
            } else if output_lines > 1 {
                meta.push(Span::styled(format!("{output_lines} lines  "), t.faint()));
            }
            let images = voice::image_count(&tool.output);
            if images > 0 && tool.state == ToolState::Done {
                let noun = if images == 1 { "image" } else { "images" };
                meta.push(Span::styled(
                    format!("{images} {noun} · /open  "),
                    t.faint(),
                ));
            }
            if tool.millis > 0 {
                meta.push(Span::styled(
                    duration(Duration::from_millis(tool.millis)),
                    t.faint(),
                ));
            }
        }
    }
    if let Some((decision, by)) = &tool.decision {
        if *decision != Decision::Deny {
            meta.push(Span::styled(
                format!("  {}", decision_text(*decision, by.as_deref())),
                t.faint(),
            ));
        }
    }
    let expandable = !tool.output.is_empty() || !tool.input.is_null();
    if expandable {
        meta.push(Span::styled(
            if tool.expanded { " ▾" } else { " ▸" },
            t.faint(),
        ));
    }
    let summary = sanitize(&tool.summary).replace(cwd, "");
    let head = header(
        vec![
            Span::raw(NEST),
            Span::styled(format!("{mark} "), mark_style),
            Span::styled(tool.name.clone(), t.dim().add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(summary, t.dim()),
        ],
        meta,
        width,
    );
    out.push(head, 4);
    if !tool.expanded {
        return;
    }
    let body = width.saturating_sub(6).max(4);
    let bar = Span::styled(format!("{NEST}  │ "), t.faint());
    for (row, text) in input_lines(tool, body) {
        let mut spans = vec![bar.clone()];
        spans.extend(row.spans);
        out.row(
            Line::from(spans),
            RowText {
                inset: text.inset + 6,
                ..text
            },
        );
    }
    if !tool.output.is_empty() {
        out.push(Line::from(bar.clone()), 6);
        let style = if tool.state == ToolState::Failed {
            t.fg(t.error)
        } else {
            t.dim()
        };
        for source in sanitize(&tool.output).trim_end().split('\n') {
            let mut join = None;
            for row in text::hard_wrap(source, body) {
                let next = text::hard_join(&row);
                let line = Line::from(vec![bar.clone(), Span::styled(row, style)]);
                out.row(
                    line,
                    RowText {
                        inset: 6,
                        join: join.replace(next),
                    },
                );
            }
        }
    }
}

/// The tool's input, shaped for the common tools and as `key: value` otherwise.
fn input_lines(tool: &ToolCall, width: usize) -> Vec<(Line<'static>, RowText)> {
    const MAX: usize = 60;
    let t = theme();
    let mut lines = Vec::new();
    let block = |text: &str,
                 prefix: &'static str,
                 style: Style,
                 lines: &mut Vec<(Line<'static>, RowText)>| {
        for source in sanitize(text).split('\n') {
            let mut join = None;
            for row in text::hard_wrap(source, width.saturating_sub(prefix.len())) {
                let next = text::hard_join(&row);
                let line = Line::from(vec![Span::styled(prefix, style), Span::styled(row, style)]);
                let row = RowText {
                    inset: prefix.len(),
                    join: join.replace(next),
                };
                lines.push((line, row));
            }
        }
    };
    let input = &tool.input;
    match (tool.name.as_str(), input.as_object()) {
        ("bash", Some(map)) if map.contains_key("command") => {
            let command = map["command"].as_str().unwrap_or_default();
            block(command, "$ ", Style::new(), &mut lines);
        }
        ("edit", Some(map)) => {
            let path = map.get("path").and_then(Value::as_str).unwrap_or_default();
            lines.push((Line::styled(path.to_owned(), t.bold()), RowText::default()));
            let old = map
                .get("old_string")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let new = map
                .get("new_string")
                .and_then(Value::as_str)
                .unwrap_or_default();
            block(old, "- ", t.fg(t.error), &mut lines);
            block(new, "+ ", t.fg(t.accent), &mut lines);
        }
        (_, Some(map)) => {
            for (key, value) in map {
                match value.as_str() {
                    Some(text) if text.contains('\n') => {
                        let line = Line::styled(format!("{key}:"), t.dim());
                        lines.push((line, RowText::default()));
                        block(text, "  ", Style::new(), &mut lines);
                    }
                    Some(text) => block(&format!("{key}: {text}"), "", Style::new(), &mut lines),
                    None => block(&format!("{key}: {value}"), "", Style::new(), &mut lines),
                }
            }
        }
        (_, None) if !input.is_null() => block(&input.to_string(), "", Style::new(), &mut lines),
        _ => {}
    }
    if lines.len() > MAX {
        let more = lines.len() - MAX;
        lines.truncate(MAX);
        let line = Line::styled(format!("… {more} more lines"), t.faint());
        lines.push((line, RowText::default()));
    }
    lines
}

/// A subagent on one line: what it does or what it reported.
fn render_card(card: &AgentCard, width: usize, ctx: &Ctx<'_>, out: &mut Out) {
    let t = theme();
    let snapshot = ctx.snapshots.get(&card.id);
    let (mark, mark_style, status) = match card.finished {
        None => (
            ctx.spinner(),
            t.fg(t.accent),
            Span::styled(duration(card.started.elapsed()), t.dim()),
        ),
        Some((TurnStatus::Completed, took)) => (
            "◆",
            t.fg(t.accent),
            Span::styled(format!("done · {}", duration(took)), t.faint()),
        ),
        Some((TurnStatus::Failed, _)) => {
            ("◆", t.fg(t.error), Span::styled("failed", t.fg(t.error)))
        }
        Some((TurnStatus::Cancelled, _)) => (
            "◆",
            t.fg(t.warning),
            Span::styled("cancelled", t.fg(t.warning)),
        ),
    };
    let detail = match (card.finished, snapshot) {
        (None, Some(snap)) if !snap.activity.is_empty() => snap.activity.clone(),
        (None, _) => "starting".to_owned(),
        (Some(_), _) => card.report.clone(),
    };
    let mut left = vec![
        Span::raw(NEST),
        Span::styled(format!("{mark} "), mark_style),
        Span::styled(card.label.clone(), t.bold()),
        Span::raw("  "),
    ];
    let detail = markdown::render(text::first_line(&detail), width)
        .lines
        .into_iter()
        .next();
    for span in detail.into_iter().flat_map(|l| l.spans) {
        let style = t.dim().patch(span.style);
        left.push(span.style(style));
    }
    let head = header(left, vec![status, Span::styled(" ›", t.faint())], width);
    out.push(head, 4);
}

#[cfg(test)]
mod tests {
    use super::*;
    use malac_core::event::Envelope;
    use serde_json::json;

    fn envelope(agent: &str, event: Event) -> Envelope {
        Envelope {
            agent: agent.into(),
            seq: 0,
            at: chrono::Utc::now(),
            event,
        }
    }

    fn started(agent: &str, parent: Option<&str>) -> Envelope {
        envelope(
            agent,
            Event::AgentStarted {
                label: if parent.is_some() { "helper" } else { "main" }.into(),
                parent: parent.map(str::to_owned),
                model: "lora/smart".into(),
                cwd: "/work".into(),
            },
        )
    }

    fn kinds(agents: &Agents, id: &str) -> Vec<&'static str> {
        agents
            .get(id)
            .unwrap()
            .transcript
            .items()
            .iter()
            .map(|i| match &i.entry {
                Entry::User(_) => "user",
                Entry::Assistant { .. } => "assistant",
                Entry::Thinking { .. } => "thinking",
                Entry::Tool(_) => "tool",
                Entry::Agent(_) => "agent",
                Entry::Todos(_) => "todos",
                Entry::Notice(..) => "notice",
                Entry::Compacted => "compacted",
                Entry::Footer(_) => "footer",
                Entry::Info(_) => "info",
            })
            .collect()
    }

    fn rendered(agents: &mut Agents, id: &str, width: usize) -> Vec<String> {
        let snapshots = HashMap::new();
        let ctx = Ctx {
            width,
            now: Instant::now(),
            snapshots: &snapshots,
        };
        let transcript = &mut agents.get_mut(id).unwrap().transcript;
        transcript.prepare(&ctx);
        transcript
            .items()
            .iter()
            .flat_map(|i| {
                let gap = i.gap().then(String::new);
                gap.into_iter().chain(
                    i.lines()
                        .iter()
                        .map(|l| l.to_string().trim_end().to_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn folds_a_turn_into_entries() {
        let mut agents = Agents::default();
        let events = [
            started("m", None),
            envelope(
                "m",
                Event::UserMessage {
                    text: "run the tests".into(),
                },
            ),
            envelope(
                "m",
                Event::ThinkingDelta {
                    text: "Let me ".into(),
                },
            ),
            envelope(
                "m",
                Event::ThinkingDelta {
                    text: "check.".into(),
                },
            ),
            envelope(
                "m",
                Event::TextDelta {
                    text: "Running ".into(),
                },
            ),
            envelope(
                "m",
                Event::TextDelta {
                    text: "**now**.".into(),
                },
            ),
            envelope(
                "m",
                Event::ToolStarted {
                    call_id: "c1".into(),
                    tool: "bash".into(),
                    summary: "cargo test".into(),
                    input: json!({"command": "cargo test"}),
                },
            ),
            envelope(
                "m",
                Event::ToolFinished {
                    call_id: "c1".into(),
                    tool: "bash".into(),
                    output: "ok\n2 passed".into(),
                    is_error: false,
                    millis: 1200,
                },
            ),
            envelope(
                "m",
                Event::TurnFinished {
                    status: TurnStatus::Completed,
                    text: "Running now.".into(),
                },
            ),
        ];
        for e in &events {
            agents.apply(e);
        }
        assert_eq!(
            kinds(&agents, "m"),
            vec!["user", "thinking", "assistant", "tool", "footer"]
        );
        let view = agents.get("m").unwrap();
        assert!(!view.running);
        assert_eq!(view.transcript.last_answer(), Some("Running **now**."));
        let lines = rendered(&mut agents, "m", 50);
        assert_eq!(lines[0], "› run the tests");
        assert_eq!(lines[2], "  ∴ Thinking  Let me check.");
        assert_eq!(
            lines[4], "┃ Running now.",
            "the turn's answer gets the accent rule"
        );
        assert_eq!(
            lines[6],
            format!("  ● bash cargo test{}2 lines  1.2s ▸", " ".repeat(16))
        );
        assert!(
            lines[7].starts_with("  ─ lora/smart"),
            "the footer hugs the turn"
        );
    }

    #[test]
    fn permission_requests_become_the_tool_card() {
        let mut agents = Agents::default();
        agents.apply(&started("m", None));
        agents.apply(&envelope(
            "m",
            Event::PermissionRequested {
                request_id: "r1".into(),
                call_id: "c1".into(),
                tool: "bash".into(),
                summary: "rm -rf build".into(),
                input: json!({"command": "rm -rf build"}),
            },
        ));
        let lines = rendered(&mut agents, "m", 50);
        assert!(lines[0].starts_with("  ? bash rm -rf build"));
        assert!(lines[0].contains("awaiting approval"));
        agents.apply(&envelope(
            "m",
            Event::PermissionResolved {
                request_id: "r1".into(),
                allowed: false,
                decision: None,
                by: None,
            },
        ));
        let lines = rendered(&mut agents, "m", 50);
        assert!(lines[0].contains("denied"));
        assert_eq!(kinds(&agents, "m"), vec!["tool"]);
    }

    #[test]
    fn expanding_a_tool_shows_input_and_output() {
        let mut agents = Agents::default();
        agents.apply(&started("m", None));
        agents.apply(&envelope(
            "m",
            Event::ToolStarted {
                call_id: "c1".into(),
                tool: "bash".into(),
                summary: "ls /work/src".into(),
                input: json!({"command": "ls /work/src"}),
            },
        ));
        agents.apply(&envelope(
            "m",
            Event::ToolFinished {
                call_id: "c1".into(),
                tool: "bash".into(),
                output: "main.rs\nlib.rs".into(),
                is_error: false,
                millis: 5,
            },
        ));
        let lines = rendered(&mut agents, "m", 40);
        assert!(lines[0].starts_with("  ● bash ls src"));
        assert!(agents.get_mut("m").unwrap().transcript.toggle(0));
        let lines = rendered(&mut agents, "m", 40);
        assert_eq!(
            &lines[1..],
            [
                "    │ $ ls /work/src",
                "    │",
                "    │ main.rs",
                "    │ lib.rs"
            ]
        );
    }

    #[test]
    fn subagents_get_cards_in_their_parent() {
        let mut agents = Agents::default();
        agents.apply(&started("m", None));
        agents.apply(&envelope(
            "m",
            Event::ToolStarted {
                call_id: "c1".into(),
                tool: "agent_spawn".into(),
                summary: "helper: look around".into(),
                input: json!({"task": "look around"}),
            },
        ));
        agents.apply(&started("s", Some("m")));
        agents.apply(&envelope(
            "s",
            Event::UserMessage {
                text: "look around".into(),
            },
        ));
        assert_eq!(kinds(&agents, "m"), vec!["tool", "agent"]);
        assert_eq!(agents.root_of("s"), "m");
        assert_eq!(agents.tree("m").len(), 2);
        agents.apply(&envelope(
            "s",
            Event::AgentFinished {
                status: TurnStatus::Completed,
                report: "Found it.\nDetails".into(),
            },
        ));
        let lines = rendered(&mut agents, "m", 50);
        assert_eq!(lines.len(), 1, "the agent_spawn call is shown as the card");
        assert!(
            lines[0].starts_with("  ◆ helper  Found it."),
            "{}",
            lines[0]
        );
        assert!(lines[0].contains("done ·"));
    }

    #[test]
    fn unstreamed_messages_are_shown_whole() {
        let mut agents = Agents::default();
        agents.apply(&started("m", None));
        agents.apply(&envelope(
            "m",
            Event::AssistantMessage {
                message: Message::assistant(vec![Block::text("All done.")]),
            },
        ));
        assert_eq!(kinds(&agents, "m"), vec!["assistant"]);
    }

    #[test]
    fn loads_saved_history() {
        let history = vec![
            Message::user_text("fix it"),
            Message::assistant(vec![
                Block::text("Looking."),
                Block::ToolUse {
                    id: "t1".into(),
                    name: "read".into(),
                    input: json!({"path": "a.rs"}),
                },
            ]),
            Message::user(vec![Block::ToolResult {
                tool_use_id: "t1".into(),
                content: vec![Block::text("no such file")],
                is_error: true,
            }]),
            Message::user_text(format!("{COMPACTED_PREFIX} Summary of everything")),
            Message::user(vec![
                Block::text("<attachment path=\"a.png\" kind=\"image\" size=\"9 B\"></attachment>"),
                Block::Image {
                    media_type: "image/png".into(),
                    data: String::new(),
                },
                Block::text("look"),
            ]),
        ];
        let mut transcript = Transcript::new("/work");
        transcript.load(&history, &[], |name, input| {
            format!("{name}:{}", input["path"])
        });
        let entries: Vec<String> = transcript
            .items()
            .iter()
            .map(|i| i.entry.plain_text())
            .collect();
        assert_eq!(entries[0], "fix it");
        assert_eq!(entries[1], "Looking.");
        assert_eq!(entries[2], "read read:\"a.rs\"\n\nno such file");
        assert!(
            matches!(&transcript.items()[2].entry, Entry::Tool(t) if t.state == ToolState::Failed)
        );
        assert!(matches!(transcript.items()[3].entry, Entry::Compacted));
        assert_eq!(entries[4], "look", "attachments join their prompt");
        assert!(
            matches!(&transcript.items()[4].entry, Entry::User(t) if t == "look\n\n[attached: a.png]")
        );
        assert_eq!(entries.len(), 5);
    }

    #[test]
    fn approvals_show_who_answered() {
        let history = vec![Message::assistant(vec![
            Block::ToolUse {
                id: "c1".into(),
                name: "bash".into(),
                input: json!({"command": "cargo publish"}),
            },
            Block::ToolUse {
                id: "c2".into(),
                name: "bash".into(),
                input: json!({"command": "rm -rf target"}),
            },
        ])];
        let record = |call: &str, decision, by: &str| DecisionRecord {
            call_id: call.into(),
            tool: "bash".into(),
            summary: String::new(),
            decision,
            by: Some(by.into()),
            at: chrono::Utc::now(),
        };
        let decisions = [
            record("c1", Decision::AllowSession, "Pixel 9"),
            record("c2", Decision::Deny, "this computer"),
        ];
        let mut transcript = Transcript::new("/work");
        transcript.load(&history, &decisions, |_, input| {
            input["command"].as_str().unwrap_or_default().to_owned()
        });
        let tools: Vec<&ToolCall> = transcript
            .items()
            .iter()
            .filter_map(|i| match &i.entry {
                Entry::Tool(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(
            tools[0].decision,
            Some((Decision::AllowSession, Some("Pixel 9".into())))
        );
        assert_eq!(tools[1].state, ToolState::Denied);
        assert_eq!(
            decision_text(Decision::AllowSession, Some("Pixel 9")),
            "allowed for the session · Pixel 9"
        );
        assert_eq!(
            decision_text(Decision::Deny, Some("this computer")),
            "denied · this computer"
        );
    }

    #[test]
    fn loading_views_hold_events_until_the_history_arrives() {
        let mut agents = Agents::default();
        agents.insert(AgentView::new("m", "main", None, "lora/smart", "/work").loading());
        let prompt = |text: &str| envelope("m", Event::UserMessage { text: text.into() });
        agents.apply(&prompt("second"));
        agents.apply(&envelope(
            "m",
            Event::TextDelta {
                text: "On it.".into(),
            },
        ));
        assert!(agents.get("m").unwrap().transcript.is_empty(), "held back");
        let history = vec![
            Message::user_text("first"),
            Message::assistant(vec![Block::text("Done.")]),
            Message::user_text("second"),
        ];
        agents.loaded("m", &history, &[], None, |_, _| String::new());
        assert!(agents.get("m").unwrap().pending.is_none());
        assert_eq!(
            kinds(&agents, "m"),
            vec!["user", "assistant", "user", "assistant"],
            "the prompt the history already had is not shown twice"
        );
    }

    #[test]
    fn visible_ranges_and_rows() {
        let mut agents = Agents::default();
        agents.apply(&started("m", None));
        for i in 0..5 {
            agents.apply(&envelope(
                "m",
                Event::Notice {
                    level: Level::Info,
                    message: format!("n{i}"),
                },
            ));
        }
        rendered(&mut agents, "m", 40);
        let t = &agents.get("m").unwrap().transcript;
        assert_eq!(t.height(), 9);
        assert_eq!(t.row_of(2), 3);
        assert_eq!(t.visible(3..6), vec![(2, 3), (3, 5)]);
    }
}
