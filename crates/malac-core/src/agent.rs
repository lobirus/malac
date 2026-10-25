//! An agent is one conversation, driven by a task that owns its history and
//! takes commands through an [`AgentHandle`].

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use futures::future::join_all;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::catalog::AgentType;
use crate::config::ModelConfig;
use crate::event::{AgentId, Event, Level, TurnStatus};
use crate::message::{Block, Message, Role, StopReason, Usage};
use crate::permission::{Decision, PermissionRequest, Verdict};
use crate::prompt::{self, PromptInputs};
use crate::provider::{Completion, Delta, ProviderError, Request, ToolSpec};
use crate::runtime::{AgentSpec, Runtime};
use crate::session::{self, Record, SessionLog, SessionMeta, Transcript};
use crate::tools::{
    AGENT_TOOLS, FileTracker, Tool, ToolContext, ToolKind, ToolOutput, truncate_chars,
    truncate_middle,
};

const DEFAULT_MAX_TOKENS: u32 = 16_384;
const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
const DEFAULT_MAX_STEPS: u32 = 200;
const MAX_ATTEMPTS: u32 = 4;
const PREVIEW_CHARS: usize = 4000;
/// Images kept in the request; older screenshots are replaced by a placeholder.
const MAX_IMAGES: usize = 4;

#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub id: AgentId,
    pub label: String,
    pub parent: Option<AgentId>,
    pub depth: u32,
    /// Limited to tools that never need approval; its subagents inherit this.
    pub read_only: bool,
    pub cwd: PathBuf,
    pub created: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSnapshot {
    #[serde(flatten)]
    pub info: AgentInfo,
    pub model: String,
    pub status: AgentStatus,
    /// What the agent is doing right now, e.g. the running tool.
    pub activity: String,
    pub usage: Usage,
    pub report: Option<String>,
}

struct State {
    model: String,
    status: AgentStatus,
    activity: String,
    usage: Usage,
    report: Option<String>,
    cancel: Option<CancellationToken>,
    pending: usize,
}

struct Shared {
    info: AgentInfo,
    state: Mutex<State>,
    busy: watch::Sender<bool>,
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut State)) {
        let mut state = self.state.lock().unwrap();
        f(&mut state);
        let busy = state.pending > 0 || state.status == AgentStatus::Running;
        self.busy.send_replace(busy);
    }
}

enum Command {
    Prompt {
        content: Vec<Block>,
        reply: Option<oneshot::Sender<Result<String>>>,
    },
    SetModel(String),
    Compact,
}

#[derive(Clone)]
pub struct AgentHandle {
    pub id: AgentId,
    shared: Arc<Shared>,
    tx: mpsc::UnboundedSender<Command>,
}

impl AgentHandle {
    pub fn info(&self) -> &AgentInfo {
        &self.shared.info
    }

    pub fn snapshot(&self) -> AgentSnapshot {
        let state = self.shared.state.lock().unwrap();
        AgentSnapshot {
            info: self.shared.info.clone(),
            model: state.model.clone(),
            status: state.status,
            activity: state.activity.clone(),
            usage: state.usage,
            report: state.report.clone(),
        }
    }

    /// Queues a prompt without waiting for the answer.
    pub fn send(&self, content: Vec<Block>) {
        self.enqueue(Command::Prompt {
            content,
            reply: None,
        });
    }

    /// Runs a prompt and returns the agent's final text.
    pub async fn prompt(&self, content: Vec<Block>) -> Result<String> {
        let (reply, rx) = oneshot::channel();
        self.enqueue(Command::Prompt {
            content,
            reply: Some(reply),
        });
        rx.await.map_err(|_| anyhow!("agent stopped"))?
    }

    fn enqueue(&self, command: Command) {
        self.shared.update(|s| s.pending += 1);
        if self.tx.send(command).is_err() {
            self.shared.update(|s| s.pending -= 1);
        }
    }

    pub fn cancel(&self) {
        if let Some(token) = &self.shared.state.lock().unwrap().cancel {
            token.cancel();
        }
    }

    pub fn set_model(&self, spec: &str) {
        let _ = self.tx.send(Command::SetModel(spec.to_owned()));
    }

    pub fn compact(&self) {
        self.enqueue(Command::Compact);
    }

    /// Resolves once the agent has no running or queued work.
    pub async fn wait_idle(&self) {
        let mut busy = self.shared.busy.subscribe();
        let _ = busy.wait_for(|b| !*b).await;
    }

    pub fn is_busy(&self) -> bool {
        *self.shared.busy.borrow()
    }
}

/// Resolved model selection.
struct ModelChoice {
    spec: String,
    provider: String,
    model: String,
    config: ModelConfig,
}

impl ModelChoice {
    fn resolve(runtime: &Runtime, spec: &str) -> Result<Self> {
        let (provider, model) = runtime.config.resolve_model(spec)?;
        let spec = format!("{provider}/{model}");
        Ok(Self {
            config: runtime.config.model_config(&spec),
            provider: provider.to_owned(),
            model: model.to_owned(),
            spec,
        })
    }
}

pub(crate) fn spawn(
    runtime: Arc<Runtime>,
    spec: AgentSpec,
    transcript: Option<Transcript>,
) -> Result<AgentHandle> {
    let agent_type = spec
        .agent_type
        .as_deref()
        .map(|name| {
            runtime
                .agent_type(name)
                .cloned()
                .with_context(|| format!("unknown agent type `{name}`"))
        })
        .transpose()?;
    let model_spec = match spec
        .model
        .clone()
        .or_else(|| agent_type.as_ref()?.model.clone())
    {
        Some(model) => model,
        None => runtime.config.main_model()?.to_owned(),
    };
    let choice = ModelChoice::resolve(&runtime, &model_spec)?;
    let cwd = spec.cwd.clone().unwrap_or_else(|| runtime.cwd.clone());
    let label = match (&spec.label, &agent_type) {
        (label, _) if !label.is_empty() => label.clone(),
        (_, Some(agent_type)) => agent_type.name.clone(),
        _ if spec.depth == 0 => "main".into(),
        _ => "agent".into(),
    };

    let (info, messages, usage, log) = match transcript {
        Some(t) => {
            let log = SessionLog::reopen(&t.meta.id)?;
            let info = AgentInfo {
                id: t.meta.id,
                label,
                parent: t.meta.parent,
                depth: spec.depth,
                read_only: spec.read_only,
                cwd,
                created: t.meta.created,
            };
            (info, t.messages, t.usage, log)
        }
        None => {
            let info = AgentInfo {
                id: uuid::Uuid::now_v7().to_string(),
                label,
                parent: spec.parent.clone(),
                depth: spec.depth,
                read_only: spec.read_only,
                cwd,
                created: Utc::now(),
            };
            let log = SessionLog::create(&SessionMeta {
                id: info.id.clone(),
                label: info.label.clone(),
                parent: info.parent.clone(),
                cwd: info.cwd.clone(),
                model: choice.spec.clone(),
                created: info.created,
            })?;
            (info, Vec::new(), Usage::default(), log)
        }
    };

    let tools = select_tools(&runtime, spec.depth, agent_type.as_ref(), spec.read_only);
    let system = prompt::build(&PromptInputs {
        runtime: &runtime,
        cwd: &info.cwd,
        model: &choice.spec,
        depth: spec.depth,
        agent_type: agent_type.as_ref(),
    });

    let (busy, _) = watch::channel(false);
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            model: choice.spec.clone(),
            status: AgentStatus::Idle,
            activity: String::new(),
            usage,
            report: None,
            cancel: None,
            pending: 0,
        }),
        info,
        busy,
    });
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = AgentHandle {
        id: shared.info.id.clone(),
        shared: shared.clone(),
        tx,
    };

    runtime.bus.emit(
        &handle.id,
        Event::AgentStarted {
            label: shared.info.label.clone(),
            parent: shared.info.parent.clone(),
            model: choice.spec.clone(),
            cwd: shared.info.cwd.display().to_string(),
        },
    );

    let specs = tools
        .iter()
        .map(|t| ToolSpec {
            name: t.name().to_owned(),
            description: t.description(),
            input_schema: t.schema(),
        })
        .collect();
    let agent = Agent {
        runtime,
        shared,
        choice,
        system,
        messages,
        tools,
        specs,
        log,
        files: FileTracker::default(),
        last_context: 0,
        image_notes: HashMap::new(),
        recall_failed: false,
    };
    tokio::spawn(agent.run(rx));
    Ok(handle)
}

fn select_tools(
    runtime: &Runtime,
    depth: u32,
    agent_type: Option<&AgentType>,
    read_only: bool,
) -> Vec<Arc<dyn Tool>> {
    let max_depth = runtime.config.agents.max_depth;
    runtime
        .tools()
        .into_iter()
        .filter(|tool| {
            let name = tool.name();
            if depth >= max_depth && AGENT_TOOLS.contains(&name) {
                return false;
            }
            if !runtime.memory.enabled() && name.starts_with("memory_") {
                return false;
            }
            if read_only && tool.kind() != ToolKind::Safe {
                return false;
            }
            match agent_type.and_then(|t| t.tools.as_ref()) {
                Some(allowed) => allowed
                    .iter()
                    .any(|pattern| match pattern.strip_suffix('*') {
                        Some(prefix) => name.starts_with(prefix),
                        None => name == pattern,
                    }),
                None => true,
            }
        })
        .collect()
}

struct Agent {
    runtime: Arc<Runtime>,
    shared: Arc<Shared>,
    choice: ModelChoice,
    system: String,
    messages: Vec<Message>,
    tools: Vec<Arc<dyn Tool>>,
    specs: Vec<ToolSpec>,
    log: SessionLog,
    files: FileTracker,
    /// Context size reported for the latest request.
    last_context: u64,
    /// Descriptions of images, for models without vision.
    image_notes: HashMap<u64, String>,
    recall_failed: bool,
}

#[derive(Debug)]
struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

impl Agent {
    fn id(&self) -> &str {
        &self.shared.info.id
    }

    fn emit(&self, event: Event) {
        self.runtime.bus.emit(self.id(), event);
    }

    fn notice(&self, level: Level, message: impl Into<String>) {
        self.emit(Event::Notice {
            level,
            message: message.into(),
        });
    }

    fn set_activity(&self, activity: impl Into<String>) {
        let activity = activity.into();
        self.shared.update(|s| s.activity = activity);
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Command>) {
        while let Some(command) = rx.recv().await {
            match command {
                Command::Prompt { content, reply } => {
                    let result = self.prompt(content).await;
                    if let Some(reply) = reply {
                        let _ = reply.send(result);
                    }
                }
                Command::SetModel(spec) => match ModelChoice::resolve(&self.runtime, &spec) {
                    Ok(choice) => {
                        self.shared.update(|s| s.model = choice.spec.clone());
                        self.emit(Event::ModelChanged {
                            model: choice.spec.clone(),
                        });
                        self.choice = choice;
                    }
                    Err(err) => self.notice(Level::Error, format!("{err:#}")),
                },
                Command::Compact => {
                    if let Err(err) = self.compact().await {
                        self.notice(Level::Error, format!("compaction failed: {err:#}"));
                    }
                    self.shared.update(|s| s.pending -= 1);
                }
            }
        }
    }

    async fn prompt(&mut self, content: Vec<Block>) -> Result<String> {
        let runtime = self.runtime.clone();
        let _slot = if self.shared.info.depth > 0 {
            self.set_activity("waiting for a free agent slot");
            Some(runtime.agent_slots.acquire().await?)
        } else {
            None
        };
        let cancel = CancellationToken::new();
        self.shared.update(|s| {
            s.pending -= 1;
            s.status = AgentStatus::Running;
            s.cancel = Some(cancel.clone());
        });

        let result = self.turn(content, &cancel).await;
        let (status, text) = match &result {
            Ok(text) => (TurnStatus::Completed, text.clone()),
            Err(err) if err.is::<Cancelled>() => (TurnStatus::Cancelled, String::new()),
            Err(err) => (TurnStatus::Failed, format!("{err:#}")),
        };
        self.shared.update(|s| {
            s.status = match status {
                TurnStatus::Completed => AgentStatus::Completed,
                TurnStatus::Cancelled => AgentStatus::Cancelled,
                TurnStatus::Failed => AgentStatus::Failed,
            };
            s.activity.clear();
            s.cancel = None;
            s.report = Some(text.clone());
        });
        if status == TurnStatus::Failed {
            self.notice(Level::Error, text.clone());
        }
        self.emit(Event::TurnFinished {
            status,
            text: text.clone(),
        });
        if self.shared.info.parent.is_some() {
            self.emit(Event::AgentFinished {
                status,
                report: text,
            });
        }
        result
    }

    async fn turn(
        &mut self,
        mut content: Vec<Block>,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let text: String = content
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.emit(Event::UserMessage { text: text.clone() });
        if let Some(recalled) = self.recall(&text).await {
            content.insert(0, Block::text(recalled));
        }
        self.push_user(content);

        let max_steps = self.runtime.config.max_steps.unwrap_or(DEFAULT_MAX_STEPS);
        let mut steps = 0;
        loop {
            if cancel.is_cancelled() {
                return Err(Cancelled.into());
            }
            self.maybe_compact().await;
            self.set_activity("thinking");
            let completion = self.call_model(cancel).await?;
            self.record_usage(&completion);

            let message = completion.message;
            self.push(message.clone());
            self.emit(Event::AssistantMessage {
                message: message.clone(),
            });

            let calls: Vec<Call> = message
                .tool_uses()
                .map(|(id, name, input)| Call {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    input: input.clone(),
                })
                .collect();
            if calls.is_empty() {
                if completion.stop == StopReason::MaxTokens {
                    self.notice(Level::Warning, "the response hit the output token limit");
                }
                return Ok(message.text());
            }

            steps += 1;
            let results = if steps > max_steps {
                self.notice(Level::Warning, format!("stopped after {max_steps} steps"));
                calls
                    .iter()
                    .map(|c| {
                        result_block(c, ToolOutput::error("step limit reached; stop and report"))
                    })
                    .collect()
            } else {
                self.run_tools(&calls, cancel).await
            };
            self.push_user(results);
            if steps > max_steps {
                return Ok(message.text());
            }
        }
    }

    fn push(&mut self, message: Message) {
        self.log.append(&Record::Message {
            message: message.clone(),
        });
        self.messages.push(message);
    }

    /// Adds user content, merging into a trailing user message so roles keep alternating.
    fn push_user(&mut self, content: Vec<Block>) {
        if let Some(last) = self.messages.last_mut() {
            if last.role == Role::User {
                last.content.extend(content.clone());
                self.log.append(&Record::Message {
                    message: Message::user(content),
                });
                return;
            }
        }
        self.push(Message::user(content));
    }

    fn record_usage(&mut self, completion: &Completion) {
        let usage = completion.usage;
        self.last_context = usage.context_tokens();
        self.log.append(&Record::Usage { usage });
        let mut total = Usage::default();
        self.shared.update(|s| {
            s.usage.add(&usage);
            total = s.usage;
        });
        self.emit(Event::Usage {
            turn: usage,
            total,
            context_tokens: self.last_context,
        });
    }

    async fn recall(&mut self, query: &str) -> Option<String> {
        if self.recall_failed || !self.runtime.memory.has_vector_store() {
            return None;
        }
        match self.runtime.memory.recall(query).await {
            Ok(hits) if !hits.is_empty() => {
                let body: Vec<String> = hits
                    .iter()
                    .map(|h| {
                        format!(
                            "## {}\n{}\n\n{}",
                            h.entry.name, h.entry.description, h.entry.body
                        )
                    })
                    .collect();
                Some(format!(
                    "<recalled-memories>\nSaved memories that may be relevant to this request:\n\n{}\n</recalled-memories>",
                    body.join("\n\n")
                ))
            }
            Ok(_) => None,
            Err(err) => {
                self.recall_failed = true;
                self.notice(Level::Warning, format!("memory recall disabled: {err:#}"));
                None
            }
        }
    }

    async fn call_model(&mut self, cancel: &CancellationToken) -> Result<Completion> {
        let provider = self.runtime.provider(&self.choice.provider)?;
        let prepared = self.prepare_messages().await;
        let messages = prepared.as_deref().unwrap_or(&self.messages);
        let system = format!(
            "{}\n\n{}",
            self.system,
            prompt::autonomy(self.runtime.policy.mode(), self.shared.info.depth)
        );
        let request = Request {
            model: &self.choice.model,
            system: &system,
            messages,
            tools: &self.specs,
            max_tokens: self
                .choice
                .config
                .max_output_tokens
                .unwrap_or(DEFAULT_MAX_TOKENS),
            temperature: self.choice.config.temperature,
            thinking_budget: self.choice.config.thinking_budget,
        };

        let bus = &self.runtime.bus;
        let id = self.shared.info.id.as_str();
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut streamed = false;
            let mut sink = |delta: Delta<'_>| {
                streamed = true;
                let event = match delta {
                    Delta::Text(text) => Event::TextDelta {
                        text: text.to_owned(),
                    },
                    Delta::Thinking(text) => Event::ThinkingDelta {
                        text: text.to_owned(),
                    },
                    Delta::ToolCall { name } => Event::ToolCallStreaming {
                        tool: name.to_owned(),
                    },
                };
                bus.emit(id, event);
            };
            let result = tokio::select! {
                result = provider.complete(&request, &mut sink) => result,
                _ = cancel.cancelled() => return Err(Cancelled.into()),
            };
            let err = match result {
                Ok(completion) => return Ok(completion),
                Err(err) => err,
            };
            let retryable = err
                .downcast_ref::<ProviderError>()
                .is_none_or(ProviderError::retryable);
            if !retryable || streamed || attempt >= MAX_ATTEMPTS {
                return Err(err);
            }
            let delay = Duration::from_secs(2u64.pow(attempt));
            bus.emit(
                id,
                Event::Notice {
                    level: Level::Warning,
                    message: format!("{err:#}; retrying in {}s", delay.as_secs()),
                },
            );
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = cancel.cancelled() => return Err(Cancelled.into()),
            }
        }
    }

    async fn run_tools(&mut self, calls: &[Call], cancel: &CancellationToken) -> Vec<Block> {
        let mut outputs: Vec<Option<ToolOutput>> = vec![None; calls.len()];
        let mut approved: Vec<(usize, Arc<dyn Tool>)> = Vec::new();
        for (index, call) in calls.iter().enumerate() {
            let Some(tool) = self.tools.iter().find(|t| t.name() == call.name).cloned() else {
                outputs[index] = Some(ToolOutput::error(format!("unknown tool `{}`", call.name)));
                continue;
            };
            match self.authorize(tool.as_ref(), call, cancel).await {
                Ok(()) => approved.push((index, tool)),
                Err(reason) => outputs[index] = Some(ToolOutput::error(reason)),
            }
        }

        let ctx = ToolContext {
            runtime: self.runtime.clone(),
            agent: self.shared.info.id.clone(),
            depth: self.shared.info.depth,
            cwd: self.shared.info.cwd.clone(),
            cancel: cancel.clone(),
            files: self.files.clone(),
        };
        // Consecutive safe tools run together; anything else runs alone, in order.
        let mut rest = approved.as_slice();
        while !rest.is_empty() {
            let group_len = if rest[0].1.kind() == ToolKind::Safe {
                rest.iter()
                    .take_while(|(_, t)| t.kind() == ToolKind::Safe)
                    .count()
            } else {
                1
            };
            let (group, tail) = rest.split_at(group_len);
            rest = tail;
            let summary = group
                .iter()
                .map(|(i, t)| format!("{} {}", t.name(), t.summary(&calls[*i].input)))
                .collect::<Vec<_>>()
                .join(", ");
            self.set_activity(truncate_chars(&summary, 120));
            let runs = group
                .iter()
                .map(|(i, tool)| execute(&ctx, tool.as_ref(), &calls[*i]));
            for ((index, _), output) in group.iter().zip(join_all(runs).await) {
                outputs[*index] = Some(output);
            }
            if cancel.is_cancelled() {
                break;
            }
        }

        calls
            .iter()
            .zip(outputs)
            .map(|(call, output)| {
                result_block(
                    call,
                    output.unwrap_or_else(|| ToolOutput::error("cancelled by the user")),
                )
            })
            .collect()
    }

    async fn authorize(
        &self,
        tool: &dyn Tool,
        call: &Call,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        let policy = &self.runtime.policy;
        match policy.check(tool, &call.input) {
            Verdict::Allow => return Ok(()),
            Verdict::Deny(reason) => return Err(reason),
            Verdict::Ask => {}
        }
        let request = PermissionRequest {
            id: uuid::Uuid::now_v7().to_string(),
            agent: self.shared.info.id.clone(),
            agent_label: self.shared.info.label.clone(),
            tool: tool.name().to_owned(),
            summary: tool.summary(&call.input),
            input: call.input.clone(),
        };
        self.set_activity(format!("waiting for approval: {}", request.tool));
        self.emit(Event::PermissionRequested {
            request_id: request.id.clone(),
            call_id: call.id.clone(),
            tool: request.tool.clone(),
            summary: request.summary.clone(),
            input: request.input.clone(),
        });
        let approver = self.runtime.approver();
        let decision = tokio::select! {
            decision = approver.approve(&request) => decision,
            _ = cancel.cancelled() => Decision::Deny,
        };
        self.emit(Event::PermissionResolved {
            request_id: request.id,
            allowed: decision != Decision::Deny,
        });
        match decision {
            Decision::Allow => Ok(()),
            Decision::AllowSession => {
                policy.remember(tool, &call.input);
                Ok(())
            }
            Decision::Deny => Err(
                "The user denied this tool call. Try a different approach or ask the user.".into(),
            ),
        }
    }

    async fn maybe_compact(&mut self) {
        let window = self
            .choice
            .config
            .context_window
            .unwrap_or(DEFAULT_CONTEXT_WINDOW);
        if self.last_context < window * 8 / 10 {
            return;
        }
        self.set_activity("compacting the conversation");
        if let Err(err) = self.compact().await {
            self.notice(Level::Warning, format!("compaction failed: {err:#}"));
        }
    }

    async fn compact(&mut self) -> Result<()> {
        if self.messages.len() < 2 {
            return Ok(());
        }
        let spec = self
            .runtime
            .config
            .small_model
            .clone()
            .unwrap_or_else(|| self.choice.spec.clone());
        let choice = ModelChoice::resolve(&self.runtime, &spec)?;
        let provider = self.runtime.provider(&choice.provider)?;
        let transcript = truncate_middle(&render_transcript(&self.messages), 600_000);
        let request_messages = [Message::user_text(format!(
            "{}\n\n<conversation>\n{transcript}\n</conversation>",
            prompt::COMPACT
        ))];
        let request = Request {
            model: &choice.model,
            system: "You write precise handoff summaries of software engineering sessions.",
            messages: &request_messages,
            tools: &[],
            max_tokens: 8192,
            temperature: None,
            thinking_budget: None,
        };
        let completion = provider.complete(&request, &mut |_| {}).await?;
        let summary = completion.message.text();
        if summary.trim().is_empty() {
            bail!("the model returned an empty summary");
        }
        self.messages = vec![session::compaction_message(&summary)];
        self.log.append(&Record::Compaction {
            summary: summary.clone(),
        });
        self.last_context = 0;
        self.emit(Event::Compacted { summary });
        Ok(())
    }

    /// The history as sent to the model: only the newest images are kept,
    /// and for models without vision they become descriptions written by
    /// the configured vision model. Returns `None` when nothing changes.
    async fn prepare_messages(&mut self) -> Option<Vec<Message>> {
        let total = self.messages.iter().map(image_count).sum::<usize>();
        let describe = self.choice.config.vision == Some(false) && total > 0;
        if total <= MAX_IMAGES && !describe {
            return None;
        }
        let vision = self.runtime.config.vision_model.clone();
        let mut messages = self.messages.clone();
        drop_stale_images(&mut messages, MAX_IMAGES);
        if !describe {
            return Some(messages);
        }
        for slot in messages.iter_mut().flat_map(image_slots) {
            let Block::Image { media_type, data } = &*slot else {
                continue;
            };
            let key = hash(data);
            if let Entry::Vacant(entry) = self.image_notes.entry(key) {
                let note = match &vision {
                    Some(spec) => describe_image(&self.runtime, spec, media_type, data)
                        .await
                        .unwrap_or_else(|err| format!("(image could not be described: {err:#})")),
                    None => "(image omitted: this model has no vision and no vision_model is configured)".into(),
                };
                entry.insert(note);
            }
            *slot = Block::text(format!("[Image description] {}", self.image_notes[&key]));
        }
        Some(messages)
    }
}

#[derive(Debug, Clone)]
struct Call {
    id: String,
    name: String,
    input: Value,
}

fn result_block(call: &Call, output: ToolOutput) -> Block {
    Block::ToolResult {
        tool_use_id: call.id.clone(),
        content: output.content,
        is_error: output.is_error,
    }
}

async fn execute(ctx: &ToolContext, tool: &dyn Tool, call: &Call) -> ToolOutput {
    ctx.emit(Event::ToolStarted {
        call_id: call.id.clone(),
        tool: call.name.clone(),
        summary: tool.summary(&call.input),
        input: call.input.clone(),
    });
    let started = Instant::now();
    let output = tokio::select! {
        output = tool.run(call.input.clone(), ctx) => output,
        _ = ctx.cancel.cancelled() => ToolOutput::error("cancelled by the user"),
    };
    ctx.emit(Event::ToolFinished {
        call_id: call.id.clone(),
        tool: call.name.clone(),
        output: truncate_middle(&output.preview(), PREVIEW_CHARS),
        is_error: output.is_error,
        millis: started.elapsed().as_millis() as u64,
    });
    output
}

fn image_count(message: &Message) -> usize {
    message
        .content
        .iter()
        .map(|b| match b {
            Block::Image { .. } => 1,
            Block::ToolResult { content, .. } => content
                .iter()
                .filter(|c| matches!(c, Block::Image { .. }))
                .count(),
            _ => 0,
        })
        .sum()
}

/// Replaces all but the newest `keep` images with a placeholder.
fn drop_stale_images(messages: &mut [Message], keep: usize) {
    let mut slots: Vec<&mut Block> = messages.iter_mut().flat_map(image_slots).collect();
    let stale = slots.len().saturating_sub(keep);
    for slot in slots.iter_mut().take(stale) {
        **slot = Block::text("[older image removed to save context]");
    }
}

/// Every image block in a message, including those inside tool results, oldest first.
fn image_slots(message: &mut Message) -> Vec<&mut Block> {
    message
        .content
        .iter_mut()
        .flat_map(|block| match block {
            Block::ToolResult { content, .. } => content.iter_mut().collect::<Vec<_>>(),
            other => vec![other],
        })
        .filter(|b| matches!(b, Block::Image { .. }))
        .collect()
}

fn hash(data: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

async fn describe_image(
    runtime: &Runtime,
    spec: &str,
    media_type: &str,
    data: &str,
) -> Result<String> {
    let choice = ModelChoice::resolve(runtime, spec)?;
    let provider = runtime.provider(&choice.provider)?;
    let messages = [Message::user(vec![
        Block::Image {
            media_type: media_type.to_owned(),
            data: data.to_owned(),
        },
        Block::text(
            "Describe this image for an agent that cannot see it. Transcribe all visible text. \
             For screenshots, list the interactive elements with their approximate pixel \
             coordinates (x, y from the top-left of the image).",
        ),
    ])];
    let request = Request {
        model: &choice.model,
        system: "You describe images precisely and completely.",
        messages: &messages,
        tools: &[],
        max_tokens: 2000,
        temperature: None,
        thinking_budget: None,
    };
    Ok(provider
        .complete(&request, &mut |_| {})
        .await?
        .message
        .text())
}

/// Plain-text rendering of a conversation for summarization.
fn render_transcript(messages: &[Message]) -> String {
    let mut out = String::new();
    for message in messages {
        let role = match message.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        for block in &message.content {
            match block {
                Block::Text { text } => out.push_str(&format!("[{role}]\n{text}\n\n")),
                Block::Image { .. } => out.push_str(&format!("[{role}] (image)\n\n")),
                Block::Thinking { .. } => {}
                Block::ToolUse { name, input, .. } => out.push_str(&format!(
                    "[Tool call] {name} {}\n\n",
                    truncate_chars(&input.to_string(), 800)
                )),
                Block::ToolResult {
                    content, is_error, ..
                } => {
                    let text: String = content
                        .iter()
                        .map(|c| match c {
                            Block::Text { text } => text.as_str(),
                            _ => "(image)",
                        })
                        .collect();
                    let label = if *is_error {
                        "Tool error"
                    } else {
                        "Tool result"
                    };
                    out.push_str(&format!("[{label}]\n{}\n\n", truncate_middle(&text, 2000)));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(n: u8) -> Block {
        Block::Image {
            media_type: "image/png".into(),
            data: n.to_string(),
        }
    }

    #[test]
    fn keeps_only_newest_images() {
        let mut messages = vec![
            Message::user(vec![image(1), Block::text("look")]),
            Message::user(vec![Block::ToolResult {
                tool_use_id: "t".into(),
                content: vec![Block::text("shot"), image(2)],
                is_error: false,
            }]),
            Message::user(vec![image(3)]),
        ];
        drop_stale_images(&mut messages, 2);
        assert!(matches!(messages[0].content[0], Block::Text { .. }));
        let Block::ToolResult { content, .. } = &messages[1].content[0] else {
            panic!("tool result expected");
        };
        assert_eq!(content[1], image(2));
        assert_eq!(messages[2].content[0], image(3));
        assert_eq!(messages.iter().map(image_count).sum::<usize>(), 2);
    }

    #[test]
    fn renders_transcript_without_thinking() {
        let text = render_transcript(&[
            Message::user_text("fix it"),
            Message::assistant(vec![
                Block::Thinking {
                    text: "secret".into(),
                    signature: None,
                },
                Block::ToolUse {
                    id: "1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                },
            ]),
        ]);
        assert!(text.contains("[User]\nfix it"));
        assert!(text.contains("[Tool call] bash"));
        assert!(!text.contains("secret"));
    }
}
