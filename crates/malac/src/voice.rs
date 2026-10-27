//! `malac voice`: what dictation, speech and talk mode offer, and using
//! them from a terminal: dictating, reading aloud, playing a file and a
//! spoken conversation. The full-screen interface uses the same talk
//! session, transcription and settings.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::style::Paint;
use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use crossterm::event::{Event as TermEvent, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as Frame;

use crate::audio::{self, Capture, Speaker};
use crate::client::{self, DaemonFile};
use crate::connections::{self, Endpoint};
use crate::daemon::ServeArgs;
use crate::protocol::{VoiceFeature, VoiceStatus};

/// Dictation records at this rate: every transcription model takes it.
pub const DICTATION_RATE: u32 = 16_000;
/// Talk mode's audio, both ways.
pub const TALK_RATE: u32 = 24_000;
/// About 100 ms of talk mode's audio, as the daemon expects a frame.
const TALK_FRAME_BYTES: usize = 4_800;
/// How long talk mode waits for the daemon to confirm it ended.
const STOP_GRACE: Duration = Duration::from_secs(2);

#[derive(Args, Debug, Clone)]
pub struct VoiceArgs {
    #[command(subcommand)]
    pub action: Option<VoiceAction>,
    /// Use the daemon of a saved connection (`malac remote list`); the
    /// microphone and speaker are this machine's.
    #[arg(short = 'C', long = "connect", value_name = "NAME", global = true)]
    pub connect: Option<String>,
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum VoiceAction {
    /// What dictation, speech and talk mode offer and use (the default).
    Status {
        #[arg(long)]
        json: bool,
        /// Read the providers' model lists again first.
        #[arg(long)]
        refresh: bool,
    },
    /// Change a setting: dictation, speech or talk (true or false),
    /// dictation_model, language, speech_model, speech_voice, talk_model or
    /// talk_voice; `none` removes one.
    Set { key: String, value: String },
    /// Record from the microphone until Enter, the end of input or
    /// --max-seconds, and print what was said. Ctrl-C or SIGTERM cancels.
    Dictate {
        /// Print `{"text": …}` (or `{"error": …}`) and nothing else on stdout.
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 120, value_name = "N")]
        max_seconds: u64,
    },
    /// Play a WAV or MP3 file and return when it is done.
    Play { file: PathBuf },
    /// Read text aloud with the speech model.
    Say {
        #[arg(required = true)]
        text: Vec<String>,
    },
    /// Talk with malac: a voice model hands what you say to the tab's
    /// agent and tells you what it did. Esc, Ctrl-C or SIGTERM ends it.
    Talk {
        /// The tab to work in (default: the active tab of the last workspace).
        #[arg(long, value_name = "ID")]
        tab: Option<String>,
        /// The agent to work through (default: the tab's active agent).
        #[arg(long, value_name = "ID")]
        agent: Option<String>,
        /// Keep the microphone open while malac speaks, for headphones;
        /// without it the microphone pauses so the speaker is not heard.
        #[arg(long)]
        duplex: bool,
    },
}

pub async fn run(args: VoiceArgs) -> Result<i32> {
    let action = args.action.unwrap_or(VoiceAction::Status {
        json: false,
        refresh: false,
    });
    if let VoiceAction::Play { file } = &action {
        audio::play_file(file).await?;
        return Ok(0);
    }
    if let VoiceAction::Dictate { json: true, .. } = action {
        // Nothing but the answer goes to stdout.
        let result = match daemon(args.connect.as_deref()).await {
            Ok(file) => dictate(&file, max_seconds(&action)).await,
            Err(err) => Err(err),
        };
        return Ok(match result {
            Ok(text) => {
                println!("{}", json!({ "text": text }));
                0
            }
            Err(err) => {
                println!("{}", json!({ "error": format!("{err:#}") }));
                1
            }
        });
    }
    let file = daemon(args.connect.as_deref()).await?;
    match action {
        VoiceAction::Status { json, refresh } => {
            let path = match refresh {
                true => "/api/voice?refresh=true",
                false => "/api/voice",
            };
            let status: VoiceStatus = client::get(&file, path).await?;
            match json {
                true => println!("{}", serde_json::to_string_pretty(&status)?),
                false => print_status(&status),
            }
        }
        VoiceAction::Set { key, value } => {
            let changes = setting(&key, &value)?;
            let status: VoiceStatus = client::put(&file, "/api/voice", &changes).await?;
            print_status(&status);
        }
        VoiceAction::Dictate { max_seconds, .. } => {
            println!("{}", dictate(&file, max_seconds).await?);
        }
        VoiceAction::Say { text } => {
            let text = text.join(" ");
            let (audio, media_type) =
                client::post_for_bytes(&file, "/api/voice/speak", &json!({ "text": text })).await?;
            audio::play_bytes(&audio, &media_type).await?;
        }
        VoiceAction::Talk { tab, agent, duplex } => return talk(file, tab, agent, duplex).await,
        VoiceAction::Play { .. } => unreachable!("played above"),
    }
    Ok(0)
}

/// The daemon `-C` names, or this machine's, started if needed.
async fn daemon(connect: Option<&str>) -> Result<DaemonFile> {
    match connections::resolve(connect)? {
        Endpoint::Local => client::ensure_daemon(&ServeArgs::default()).await,
        Endpoint::Saved(connection) => Ok(connection.file()),
    }
}

/// Whether a feature can be used now: on, with a model.
pub fn ready(feature: &VoiceFeature) -> bool {
    feature.enabled && feature.model.is_some()
}

/// What a feature needs when no model can do it.
fn needs(name: &str) -> &'static str {
    match name {
        "dictation" => {
            "a transcription model (whisper-1, gpt-4o-mini-transcribe) or a chat model that takes \
             audio, from an OpenAI-style provider"
        }
        "speech" => "a speech model (gpt-4o-mini-tts, tts-1), or say (macOS) or espeak-ng (Linux)",
        _ => "a Realtime model (gpt-realtime) from an OpenAI-style provider",
    }
}

/// Why a feature cannot be used, and what to do; `set` is how settings
/// change where it shows (`malac voice set`, `/voice`).
pub fn unavailable(name: &str, feature: &VoiceFeature, set: &str) -> String {
    match (feature.enabled, &feature.model) {
        (false, _) => format!("{name} is off: `{set} {} true` turns it on", key(name)),
        (true, None) => format!("{name} needs {}", needs(name)),
        (true, Some(_)) => format!("{name} is ready"),
    }
}

fn key(name: &str) -> &str {
    match name {
        "talk mode" => "talk",
        other => other,
    }
}

/// One feature's state and what it uses, as `malac voice` and `/voice` show them.
pub fn describe(name: &str, feature: &VoiceFeature) -> (&'static str, String) {
    let state = match (feature.enabled, &feature.model) {
        (false, _) => "off",
        (true, None) => "unavailable",
        (true, Some(_)) => "ready",
    };
    let detail = match &feature.model {
        None if feature.enabled => format!("needs {}", needs(name)),
        None => "no model can do it".to_owned(),
        Some(model) => match &feature.voice {
            Some(voice) => format!("{model} · voice {voice}"),
            None => model.clone(),
        },
    };
    (state, detail)
}

fn print_status(status: &VoiceStatus) {
    for (name, feature) in [
        ("dictation", &status.dictation),
        ("speech", &status.speech),
        ("talk", &status.talk),
    ] {
        let (state, detail) = describe(name, feature);
        let state = match state {
            "ready" => format!("{state:<11}").green().to_string(),
            "off" => format!("{state:<11}").dark_grey().to_string(),
            _ => format!("{state:<11}").yellow().to_string(),
        };
        println!("{} {state} {detail}", format!("{name:<10}").bold());
        let others: Vec<&str> = feature
            .models
            .iter()
            .map(String::as_str)
            .filter(|m| Some(*m) != feature.model.as_deref())
            .collect();
        if !others.is_empty() {
            println!(
                "{}",
                format!("{:22} also: {}", "", others.join(", ")).dark_grey()
            );
        }
    }
    let language = status
        .language
        .as_deref()
        .unwrap_or("detected by the model");
    println!("{} {language}", format!("{:<10}", "language").bold());
    println!();
    println!(
        "{}",
        "`malac voice set speech true` turns a feature on · `malac voice set talk_voice marin` \
         picks a voice · `malac voice set speech_model none` goes back to the first model"
            .dark_grey()
    );
    if !status.voices.is_empty() {
        println!(
            "{}",
            format!("voices: {}", status.voices.join(", ")).dark_grey()
        );
    }
}

/// `PUT /api/voice`'s body for `key` set to `value`: `true`/`false` for the
/// switches, text otherwise, `none` or nothing to remove it.
pub fn setting(key: &str, value: &str) -> Result<Map<String, Value>> {
    const SWITCHES: [&str; 3] = ["dictation", "speech", "talk"];
    const TEXTS: [&str; 6] = [
        "dictation_model",
        "language",
        "speech_model",
        "speech_voice",
        "talk_model",
        "talk_voice",
    ];
    let value = value.trim();
    let parsed = if value.is_empty() || value.eq_ignore_ascii_case("none") {
        Value::Null
    } else if SWITCHES.contains(&key) {
        match value.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Value::Bool(true),
            "false" | "off" | "no" | "0" => Value::Bool(false),
            _ => bail!("{key} is true or false"),
        }
    } else if TEXTS.contains(&key) {
        Value::String(value.to_owned())
    } else {
        bail!(
            "no voice setting `{key}`; there are {} and {}",
            SWITCHES.join(", "),
            TEXTS.join(", ")
        );
    };
    let mut changes = Map::new();
    changes.insert(key.to_owned(), parsed);
    Ok(changes)
}

/// Sends a recording to the daemon's dictation model; the text, trimmed.
pub async fn transcribe(file: &DaemonFile, samples: &[i16]) -> Result<String> {
    if samples.is_empty() {
        bail!("nothing was recorded");
    }
    if audio::is_silent(samples) {
        bail!("{}", audio::silence_hint());
    }
    let wav = audio::wav::encode(samples, DICTATION_RATE);
    let reply: Value = client::upload(file, "/api/voice/transcribe", "audio/wav", wav).await?;
    Ok(reply["text"].as_str().unwrap_or_default().trim().to_owned())
}

fn max_seconds(action: &VoiceAction) -> u64 {
    match action {
        VoiceAction::Dictate { max_seconds, .. } => *max_seconds,
        _ => 120,
    }
}

/// Records until a line on stdin, its end or the time limit, then
/// transcribes. Ctrl-C or SIGTERM throws the recording away. What it is
/// doing goes to stderr.
async fn dictate(file: &DaemonFile, max_seconds: u64) -> Result<String> {
    const CANCELLED: &str = "dictation cancelled; nothing was transcribed";
    let mut interrupts = Interrupts::catch();
    let status: VoiceStatus = client::get(file, "/api/voice").await?;
    if !ready(&status.dictation) {
        bail!(
            "{}",
            unavailable("dictation", &status.dictation, "malac voice set")
        );
    }
    let mut capture = tokio::select! {
        started = tokio::task::spawn_blocking(|| Capture::start(DICTATION_RATE)) => {
            started.context("the recorder did not start")??
        }
        _ = interrupts.next() => bail!(CANCELLED),
    };
    let tty = std::io::stderr().is_terminal();
    let clear = || {
        if tty {
            eprint!("\r\x1b[2K");
        }
    };
    let limit = Duration::from_secs(max_seconds.max(1));
    eprintln!(
        "{}",
        format!(
            "recording · Enter stops, Ctrl-C cancels (at most {})",
            minutes(limit.as_secs())
        )
        .dark_grey()
    );
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let deadline = tokio::time::sleep(limit);
    tokio::pin!(deadline);
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let started = Instant::now();
    let mut samples = Vec::new();
    loop {
        tokio::select! {
            chunk = capture.next() => match chunk {
                Some(chunk) => samples.extend(chunk),
                None => break,
            },
            _ = lines.next_line() => break,
            _ = &mut deadline => break,
            _ = interrupts.next() => {
                clear();
                bail!(CANCELLED);
            }
            _ = tick.tick(), if tty => {
                eprint!("\r\x1b[2K{} recording {}", "●".red(), minutes(started.elapsed().as_secs()));
            }
        }
    }
    drop(capture);
    clear();
    eprintln!("{}", "transcribing…".dark_grey());
    tokio::select! {
        text = transcribe(file, &samples) => text,
        _ = interrupts.next() => bail!(CANCELLED),
    }
}

/// Ctrl-C and SIGTERM, caught from the moment this exists: they cancel
/// dictation and end talk mode.
struct Interrupts {
    #[cfg(unix)]
    interrupt: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    terminate: Option<tokio::signal::unix::Signal>,
}

impl Interrupts {
    fn catch() -> Self {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Self {
                interrupt: signal(SignalKind::interrupt()).ok(),
                terminate: signal(SignalKind::terminate()).ok(),
            }
        }
        #[cfg(not(unix))]
        Self {}
    }

    /// Waits for the next one.
    async fn next(&mut self) {
        #[cfg(unix)]
        {
            async fn one(signal: &mut Option<tokio::signal::unix::Signal>) {
                match signal {
                    Some(signal) => {
                        signal.recv().await;
                    }
                    None => std::future::pending().await,
                }
            }
            tokio::select! {
                _ = one(&mut self.interrupt) => {}
                _ = one(&mut self.terminate) => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

/// `0:07`, `2:00`.
pub fn minutes(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

// Talk mode

/// What a talk session is doing, for the line that shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TalkState {
    Connecting,
    /// The microphone is open and nothing plays.
    Listening,
    /// The user is speaking.
    Hearing,
    /// The voice model works out what to say.
    Thinking,
    /// The voice speaks; without `duplex` the microphone is paused.
    Speaking,
    Muted,
}

impl TalkState {
    pub fn label(self, duplex: bool) -> &'static str {
        match self {
            TalkState::Connecting => "connecting…",
            TalkState::Listening => "listening",
            TalkState::Hearing => "hearing you",
            TalkState::Thinking => "thinking…",
            TalkState::Speaking if duplex => "speaking",
            TalkState::Speaking => "speaking · microphone paused",
            TalkState::Muted => "muted",
        }
    }
}

/// What a talk session reports.
#[derive(Debug, Clone, PartialEq)]
pub enum TalkEvent {
    Ready {
        model: String,
        voice: String,
    },
    State(TalkState),
    /// A finished sentence: the user's (when the provider transcribes) or the voice's.
    Said {
        you: bool,
        text: String,
    },
    /// The voice model asked malac something, checked on it, answered an approval or stopped it.
    Tool(String),
    Error(String),
    /// The session is over; nothing follows.
    Closed(String),
}

enum TalkCommand {
    Text(String),
    Interrupt,
    Mute(bool),
    Stop,
}

/// Where a talk session runs.
#[derive(Debug, Clone)]
pub struct TalkOptions {
    pub file: DaemonFile,
    pub tab: Option<String>,
    pub agent: Option<String>,
    /// Keep the microphone open while the voice speaks.
    pub duplex: bool,
}

/// A spoken conversation over the daemon's talk socket: the microphone
/// streams once the daemon is ready, and the voice plays as it arrives.
/// Dropping it ends the conversation.
pub struct Talk {
    commands: mpsc::UnboundedSender<TalkCommand>,
}

impl Talk {
    /// Starts a session; `report` hears every event, the last one `Closed`.
    pub fn start(options: TalkOptions, report: impl Fn(TalkEvent) + Send + Sync + 'static) -> Self {
        let (commands, queue) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let reason = match converse(options, queue, &report).await {
                Ok(reason) => reason,
                Err(err) => {
                    report(TalkEvent::Error(format!("{err:#}")));
                    "talk mode failed".to_owned()
                }
            };
            report(TalkEvent::Closed(reason));
        });
        Self { commands }
    }

    /// A handle with no session behind it, for the interface's tests.
    #[cfg(test)]
    pub fn idle() -> Self {
        let (commands, _) = mpsc::unbounded_channel();
        Self { commands }
    }

    /// Typed input, taken as if spoken.
    pub fn text(&self, text: &str) {
        let _ = self.commands.send(TalkCommand::Text(text.to_owned()));
    }

    /// Stops the voice's reply.
    pub fn interrupt(&self) {
        let _ = self.commands.send(TalkCommand::Interrupt);
    }

    pub fn mute(&self, muted: bool) {
        let _ = self.commands.send(TalkCommand::Mute(muted));
    }

    pub fn stop(&self) {
        let _ = self.commands.send(TalkCommand::Stop);
    }
}

/// `/api/voice/talk` with the tab and agent asked for.
fn talk_path(tab: Option<&str>, agent: Option<&str>) -> String {
    let mut url = reqwest::Url::parse("http://x/api/voice/talk").expect("a valid URL");
    if let Some(tab) = tab {
        url.query_pairs_mut().append_pair("tab", tab);
    }
    if let Some(agent) = agent {
        url.query_pairs_mut().append_pair("agent", agent);
    }
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    }
}

async fn next_chunk(capture: &mut Option<Capture>) -> Option<Vec<i16>> {
    match capture {
        Some(capture) => capture.next().await,
        None => std::future::pending().await,
    }
}

/// The session itself; returns why it ended.
async fn converse(
    options: TalkOptions,
    mut commands: mpsc::UnboundedReceiver<TalkCommand>,
    report: &(impl Fn(TalkEvent) + Send + Sync),
) -> Result<String> {
    let mut state = TalkState::Connecting;
    report(TalkEvent::State(state));
    let request = options
        .file
        .socket_request(&talk_path(options.tab.as_deref(), options.agent.as_deref()))?;
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .with_context(|| format!("cannot open talk mode at {}", options.file.url))?;
    let (mut up, mut down) = socket.split();
    let mut speaker = tokio::task::spawn_blocking(|| Speaker::open(TALK_RATE))
        .await
        .context("the speaker did not open")??;
    let mut capture: Option<Capture> = None;
    let mut frame: Vec<u8> = Vec::with_capacity(TALK_FRAME_BYTES);
    let mut muted = false;
    let mut stopping: Option<Instant> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let set = |next: TalkState, state: &mut TalkState| {
        if *state != next {
            *state = next;
            report(TalkEvent::State(next));
        }
    };
    loop {
        tokio::select! {
            message = down.next() => {
                let Some(message) = message else {
                    return Ok("the daemon closed talk mode".to_owned());
                };
                match message.context("talk mode's connection failed")? {
                    Frame::Binary(pcm) => {
                        speaker.push(&pcm);
                        set(TalkState::Speaking, &mut state);
                    }
                    Frame::Text(text) => {
                        let message: Value = serde_json::from_str(text.as_str()).unwrap_or_default();
                        let text_of = |key: &str| message[key].as_str().unwrap_or_default().to_owned();
                        match message["type"].as_str().unwrap_or_default() {
                            "ready" => {
                                report(TalkEvent::Ready { model: text_of("model"), voice: text_of("voice") });
                                if capture.is_none() {
                                    let started = tokio::task::spawn_blocking(|| Capture::start(TALK_RATE))
                                        .await
                                        .context("the microphone did not start")??;
                                    capture = Some(started);
                                }
                                let next = if muted { TalkState::Muted } else { TalkState::Listening };
                                set(next, &mut state);
                            }
                            "listening" => {
                                speaker.clear();
                                set(TalkState::Hearing, &mut state);
                            }
                            "thinking" => set(TalkState::Thinking, &mut state),
                            "transcript" if message["final"] == true => {
                                let said = text_of("text");
                                if !said.trim().is_empty() {
                                    report(TalkEvent::Said { you: message["role"] == "user", text: said.trim().to_owned() });
                                }
                            }
                            "tool" => report(TalkEvent::Tool(text_of("summary"))),
                            "error" => report(TalkEvent::Error(text_of("message"))),
                            "closed" => return Ok(text_of("reason")),
                            _ => {}
                        }
                    }
                    Frame::Close(_) => return Ok("talk mode ended".to_owned()),
                    _ => {}
                }
            }
            chunk = next_chunk(&mut capture) => {
                let Some(chunk) = chunk else {
                    capture = None;
                    report(TalkEvent::Error("the microphone stopped".to_owned()));
                    continue;
                };
                // Without echo cancellation the voice would hear itself.
                let paused = muted || stopping.is_some() || (!options.duplex && speaker.playing());
                if paused {
                    frame.clear();
                    continue;
                }
                for sample in chunk {
                    frame.extend(sample.to_le_bytes());
                    if frame.len() >= TALK_FRAME_BYTES {
                        let full = std::mem::replace(&mut frame, Vec::with_capacity(TALK_FRAME_BYTES));
                        up.send(Frame::Binary(full.into())).await?;
                    }
                }
            }
            command = commands.recv(), if stopping.is_none() => {
                let message = match command {
                    Some(TalkCommand::Text(text)) => json!({ "type": "text", "text": text }),
                    Some(TalkCommand::Interrupt) => {
                        speaker.clear();
                        json!({ "type": "interrupt" })
                    }
                    Some(TalkCommand::Mute(on)) => {
                        muted = on;
                        if state == TalkState::Listening || state == TalkState::Muted {
                            let next = if on { TalkState::Muted } else { TalkState::Listening };
                            set(next, &mut state);
                        }
                        continue;
                    }
                    // The handle went away, or the user ended it.
                    Some(TalkCommand::Stop) | None => {
                        speaker.clear();
                        stopping = Some(Instant::now());
                        json!({ "type": "stop" })
                    }
                };
                up.send(Frame::Text(message.to_string().into())).await?;
            }
            _ = tick.tick() => {
                if stopping.is_some_and(|at| at.elapsed() > STOP_GRACE) {
                    return Ok("talk mode ended".to_owned());
                }
                if state == TalkState::Speaking && !speaker.playing() {
                    let next = if muted { TalkState::Muted } else { TalkState::Listening };
                    set(next, &mut state);
                }
            }
        }
    }
}

/// Lines for talk mode in a plain terminal: in raw mode a line ends with
/// `\r\n`, and the state line at the bottom is redrawn under new output.
struct Screen {
    raw: bool,
    tty: bool,
    status: String,
}

impl Screen {
    fn line(&mut self, text: &str) {
        let end = if self.raw { "\r\n" } else { "\n" };
        if self.tty {
            eprint!("\r\x1b[2K");
        }
        print!("{text}{end}");
        self.redraw();
    }

    fn status(&mut self, status: String) {
        self.status = status;
        self.redraw();
    }

    fn redraw(&self) {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        if self.tty {
            eprint!("\r\x1b[2K{}", self.status);
        }
    }

    fn clear(&self) {
        if self.tty {
            eprint!("\r\x1b[2K");
        }
    }
}

/// `malac voice talk`. On a terminal, keys control it: Esc, Ctrl-C or q
/// ends it, m mutes, space stops the voice. Piped, each line read is typed
/// input, and the end of input ends it.
async fn talk(
    file: DaemonFile,
    tab: Option<String>,
    agent: Option<String>,
    duplex: bool,
) -> Result<i32> {
    let status: VoiceStatus = client::get(&file, "/api/voice").await?;
    if !ready(&status.talk) {
        bail!(
            "{}",
            unavailable("talk mode", &status.talk, "malac voice set")
        );
    }
    let (events_tx, mut events) = mpsc::unbounded_channel();
    let talk = Talk::start(
        TalkOptions {
            file,
            tab,
            agent,
            duplex,
        },
        move |event| {
            let _ = events_tx.send(event);
        },
    );
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let raw = tty && crossterm::terminal::enable_raw_mode().is_ok();
    let mut screen = Screen {
        raw,
        tty: tty && std::io::stderr().is_terminal(),
        status: String::new(),
    };
    let mut keys = raw.then(EventStream::new);
    let mut lines = (!tty).then(|| tokio::io::BufReader::new(tokio::io::stdin()).lines());
    let mut interrupts = Interrupts::catch();
    let mut muted = false;
    let mut failed = false;
    let mut ending = false;
    let hint = "Esc ends · m mutes · space stops the voice";
    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { break };
                match event {
                    TalkEvent::Ready { model, voice } => {
                        let keys = if raw { format!(" · {hint}") } else { String::new() };
                        screen.line(&format!("talk mode · {model} · voice {voice}{keys}").dark_grey().to_string());
                    }
                    TalkEvent::State(state) => {
                        let dot = match state {
                            TalkState::Speaking | TalkState::Hearing => "●".green().to_string(),
                            TalkState::Muted => "●".yellow().to_string(),
                            _ => "●".dark_grey().to_string(),
                        };
                        screen.status(format!("{dot} {}", state.label(duplex)));
                    }
                    TalkEvent::Said { you: true, text } => screen.line(&format!("{} {text}", "you:".bold())),
                    TalkEvent::Said { you: false, text } => screen.line(&format!("{} {text}", "malac:".bold())),
                    TalkEvent::Tool(summary) => screen.line(&format!("  {summary}").dark_grey().to_string()),
                    TalkEvent::Error(message) => {
                        failed = true;
                        screen.line(&format!("✗ {message}").red().to_string());
                    }
                    TalkEvent::Closed(_) => break,
                }
            }
            key = next_key(&mut keys) => {
                let Some(key) = key else {
                    keys = None;
                    continue;
                };
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => talk.stop(),
                    KeyCode::Char('c' | 'd') if ctrl => talk.stop(),
                    KeyCode::Char('m') => {
                        muted = !muted;
                        talk.mute(muted);
                    }
                    KeyCode::Char(' ') => talk.interrupt(),
                    _ => {}
                }
            }
            line = next_line(&mut lines) => match line {
                Some(text) if !text.trim().is_empty() => talk.text(text.trim()),
                Some(_) => {}
                None => {
                    lines = None;
                    talk.stop();
                }
            },
            _ = interrupts.next(), if !ending => {
                ending = true;
                talk.stop();
            }
        }
    }
    screen.clear();
    if raw {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    Ok(i32::from(failed))
}

async fn next_key(keys: &mut Option<EventStream>) -> Option<crossterm::event::KeyEvent> {
    let Some(stream) = keys else {
        return std::future::pending().await;
    };
    loop {
        match stream.next().await? {
            Ok(TermEvent::Key(key)) if key.kind != KeyEventKind::Release => return Some(key),
            Ok(_) => continue,
            Err(_) => return None,
        }
    }
}

async fn next_line(
    lines: &mut Option<tokio::io::Lines<tokio::io::BufReader<tokio::io::Stdin>>>,
) -> Option<String> {
    match lines {
        Some(lines) => lines.next_line().await.ok().flatten(),
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_parse_like_the_daemon_wants_them() {
        let one = |key: &str, value: &str| setting(key, value).unwrap()[key].clone();
        assert_eq!(one("speech", "true"), Value::Bool(true));
        assert_eq!(one("talk", "off"), Value::Bool(false));
        assert_eq!(one("speech_voice", "marin"), Value::String("marin".into()));
        assert_eq!(one("dictation_model", "none"), Value::Null);
        assert_eq!(one("language", ""), Value::Null);
        assert!(setting("speech", "loud").is_err());
        assert!(setting("volume", "11").is_err());
    }

    #[test]
    fn features_say_what_they_need() {
        let ready_one = VoiceFeature {
            enabled: true,
            model: Some("audio/tts-1".into()),
            voice: Some("alloy".into()),
            ..Default::default()
        };
        assert!(ready(&ready_one));
        assert_eq!(
            describe("speech", &ready_one),
            ("ready", "audio/tts-1 · voice alloy".to_owned())
        );
        let off = VoiceFeature {
            enabled: false,
            ..ready_one.clone()
        };
        assert!(!ready(&off));
        assert_eq!(describe("speech", &off).0, "off");
        assert!(unavailable("talk mode", &off, "/voice").contains("`/voice talk true`"));
        let missing = VoiceFeature {
            enabled: true,
            ..Default::default()
        };
        assert_eq!(describe("dictation", &missing).0, "unavailable");
        assert!(unavailable("dictation", &missing, "/voice").contains("transcription model"));
    }

    #[test]
    fn talk_sockets_name_the_tab_and_agent() {
        assert_eq!(talk_path(None, None), "/api/voice/talk");
        assert_eq!(
            talk_path(Some("t 1"), Some("a1")),
            "/api/voice/talk?tab=t+1&agent=a1"
        );
        assert_eq!(minutes(7), "0:07");
        assert_eq!(minutes(125), "2:05");
    }
}
