//! The daemon's voice: which models it offers for dictation, speech and
//! talk mode, the sentences it reads aloud, and which device plays them.
//!
//! Speech goes to one device, the listening one the user touched last, so
//! a phone and a laptop never talk over each other. Audio is made once per
//! sentence, when it is announced, and kept a few minutes for clients that
//! fetch it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::Utc;
use malac_core::config::{self, Config};
use malac_core::event::{Envelope, Event, TurnStatus};
use malac_core::voice;
use tokio::sync::OnceCell;

use super::hub::Hub;
use crate::protocol::{ServerMessage, VoiceFeature, VoiceStatus};

/// How long a sentence's audio stays fetchable.
const CLIP_KEEP: Duration = Duration::from_secs(10 * 60);
/// A turn this long ends with a spoken "Done" when the agent said nothing.
const LONG_TURN: Duration = Duration::from_secs(30);
/// Longest spoken summary of a reply.
const SUMMARY_CHARS: usize = 160;

type Audio = std::result::Result<(Arc<Vec<u8>>, &'static str), String>;

struct Clip {
    text: String,
    created: Instant,
    audio: Arc<OnceCell<Audio>>,
}

#[derive(Default)]
struct Listeners {
    next: u64,
    /// Listening connections and when each last sent a command.
    active: HashMap<u64, Instant>,
}

/// What speech needs to know about an agent between events.
#[derive(Default)]
struct Turn {
    top_level: bool,
    started: Option<Instant>,
    said: bool,
}

#[derive(Default)]
pub struct Voice {
    status: Mutex<VoiceStatus>,
    /// The providers' model lists, as last discovered.
    available: Mutex<Vec<(String, Vec<String>)>>,
    clips: Mutex<HashMap<String, Clip>>,
    listeners: Mutex<Listeners>,
    turns: Mutex<HashMap<String, Turn>>,
    /// Talk mode sessions under way; their voice model speaks for malac.
    talking: std::sync::atomic::AtomicUsize,
}

impl Voice {
    pub fn status(&self) -> VoiceStatus {
        self.status.lock().unwrap().clone()
    }

    /// Recomputes what is offered from `config` and the last model lists.
    /// Returns whether anything changed.
    fn update(&self, config: &Config) -> bool {
        let available = self.available.lock().unwrap().clone();
        let offers = voice::offers(config, &available, voice::has_system_voice());
        let settings = &config.voice;
        let feature = |enabled: bool,
                       configured: &Option<String>,
                       voice: Option<&Option<String>>,
                       models: Vec<String>| {
            VoiceFeature {
                enabled,
                model: voice::choose(configured.as_deref(), &models),
                configured: configured.clone(),
                voice: voice.map(|v| v.clone().unwrap_or_else(|| voice::DEFAULT_VOICE.to_owned())),
                models,
            }
        };
        let mut speech = feature(
            settings.speech,
            &settings.speech_model,
            Some(&settings.speech_voice),
            offers.speech,
        );
        if speech.model.as_deref() == Some(voice::SYSTEM) && settings.speech_voice.is_none() {
            speech.voice = None;
        }
        let status = VoiceStatus {
            dictation: feature(
                settings.dictation,
                &settings.dictation_model,
                None,
                offers.dictation,
            ),
            speech,
            talk: feature(
                settings.talk,
                &settings.talk_model,
                Some(&settings.talk_voice),
                offers.talk,
            ),
            language: settings.language.clone(),
            voices: voice::VOICES.iter().map(|v| (*v).to_owned()).collect(),
            checked: self.status.lock().unwrap().checked,
        };
        let mut current = self.status.lock().unwrap();
        let changed = *current != status;
        *current = status;
        changed
    }

    /// Marks a talk session as running until the guard drops.
    pub fn talking(&self) -> TalkGuard<'_> {
        self.talking
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        TalkGuard(self)
    }

    // Listeners

    pub fn listen(&self) -> u64 {
        let mut listeners = self.listeners.lock().unwrap();
        listeners.next += 1;
        let id = listeners.next;
        listeners.active.insert(id, Instant::now());
        id
    }

    pub fn unlisten(&self, id: u64) {
        self.listeners.lock().unwrap().active.remove(&id);
    }

    /// A listening connection sent a command: it is where the user is.
    pub fn touch(&self, id: u64) {
        if let Some(seen) = self.listeners.lock().unwrap().active.get_mut(&id) {
            *seen = Instant::now();
        }
    }

    fn chosen(&self) -> Option<u64> {
        let listeners = self.listeners.lock().unwrap();
        listeners
            .active
            .iter()
            .max_by_key(|(_, seen)| **seen)
            .map(|(id, _)| *id)
    }

    // Clips

    /// The audio for an announced sentence, made on first use.
    pub async fn clip(&self, id: &str, config: &Config) -> Result<(Arc<Vec<u8>>, &'static str)> {
        let (cell, text) = {
            let clips = self.clips.lock().unwrap();
            let clip = clips
                .get(id)
                .context("no such speech; it may have expired")?;
            (clip.audio.clone(), clip.text.clone())
        };
        let (spec, speaker) = {
            let status = self.status.lock().unwrap();
            (status.speech.model.clone(), status.speech.voice.clone())
        };
        let spec = spec.context("no speech model")?;
        cell.get_or_init(|| async move {
            voice::synthesize(config, &spec, speaker.as_deref(), &text)
                .await
                .map(|(audio, media_type)| (Arc::new(audio), media_type))
                .map_err(|err| format!("{err:#}"))
        })
        .await
        .clone()
        .map_err(anyhow::Error::msg)
    }

    fn add_clip(&self, text: &str) -> String {
        let id = uuid::Uuid::now_v7().simple().to_string();
        let mut clips = self.clips.lock().unwrap();
        clips.retain(|_, clip| clip.created.elapsed() < CLIP_KEEP);
        clips.insert(
            id.clone(),
            Clip {
                text: text.to_owned(),
                created: Instant::now(),
                audio: Arc::new(OnceCell::new()),
            },
        );
        id
    }

    /// What to read aloud for an agent event, if anything.
    fn line_for(&self, agent: &str, event: &Event) -> Option<(String, &'static str)> {
        let mut turns = self.turns.lock().unwrap();
        match event {
            Event::AgentStarted { parent, .. } => {
                turns.entry(agent.to_owned()).or_default().top_level = parent.is_none();
                None
            }
            Event::UserMessage { .. } => {
                let turn = turns.entry(agent.to_owned()).or_insert_with(|| Turn {
                    top_level: true,
                    ..Default::default()
                });
                turn.started = Some(Instant::now());
                turn.said = false;
                None
            }
            Event::Say { text } => {
                if let Some(turn) = turns.get_mut(agent) {
                    turn.said = true;
                }
                Some((text.clone(), "agent"))
            }
            Event::PermissionRequested { tool, summary, .. } => Some((
                format!("malac needs your approval to {}.", action(tool, summary)),
                "approval",
            )),
            Event::TurnFinished { status, text } => {
                let turn = turns.get_mut(agent)?;
                let long = turn
                    .started
                    .take()
                    .is_some_and(|t| t.elapsed() >= LONG_TURN);
                if !turn.top_level || turn.said {
                    return None;
                }
                match status {
                    TurnStatus::Completed if long => Some((done_line(text), "finished")),
                    TurnStatus::Failed => {
                        Some(("malac stopped with an error.".to_owned(), "failed"))
                    }
                    _ => None,
                }
            }
            Event::AgentFinished { .. } => {
                turns.remove(agent);
                None
            }
            _ => None,
        }
    }
}

pub struct TalkGuard<'a>(&'a Voice);

impl Drop for TalkGuard<'_> {
    fn drop(&mut self) {
        self.0
            .talking
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Hub {
    /// Rediscovers the providers' models, then updates what is offered.
    pub async fn refresh_voice(&self) {
        let config = self.base_config();
        let found = malac_core::provider::discover_models(&config).await;
        let available: Vec<(String, Vec<String>)> = found
            .into_iter()
            .map(|p| (p.provider, p.available))
            .collect();
        *self.voice().available.lock().unwrap() = available;
        self.voice().status.lock().unwrap().checked = Some(Utc::now());
        self.voice_changed(&config, true);
    }

    /// Recomputes the offer after a settings change, telling clients when it moved.
    pub fn voice_changed(&self, config: &Config, force: bool) {
        if self.voice().update(config) || force {
            self.publish(&ServerMessage::Voice {
                voice: self.voice().status(),
            });
        }
    }

    /// Writes `[voice]` keys to the user's config (a `null` removes one),
    /// then applies them.
    pub async fn set_voice(
        &self,
        changes: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<VoiceStatus> {
        const KEYS: &[&str] = &[
            "dictation",
            "dictation_model",
            "language",
            "speech",
            "speech_model",
            "speech_voice",
            "talk",
            "talk_model",
            "talk_voice",
        ];
        let path = config::user_config_path();
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        for (key, value) in changes {
            if !KEYS.contains(&key.as_str()) {
                anyhow::bail!("unknown voice setting `{key}`");
            }
            let value = (!value.is_null() && value != "").then(|| value.clone());
            text = malac_core::tools::edit_config_text(
                &text,
                &["voice".to_owned(), key.clone()],
                value,
            )?;
        }
        let table: toml::Table = text.parse().context("the result is not valid TOML")?;
        config::validate_user_table(&table)?;
        config::write_private(&path, &text)?;
        // Tabs pick up `say` when they next load; reloading now is best effort.
        self.refresh_base()?;
        let _ = self.reload().await;
        let config = self.base_config();
        self.voice_changed(&config, true);
        Ok(self.voice().status())
    }

    /// Reads an agent event aloud when speech is on and a device listens.
    pub fn voice_observe(&self, tab: &str, envelope: &Envelope) {
        let Some((text, reason)) = self.voice().line_for(&envelope.agent, &envelope.event) else {
            return;
        };
        let talking = self
            .voice()
            .talking
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0;
        let ready = !talking && {
            let status = self.voice().status.lock().unwrap();
            status.speech.enabled && status.speech.model.is_some()
        };
        let Some(listener) = ready.then(|| self.voice().chosen()).flatten() else {
            return;
        };
        let id = self.voice().add_clip(&text);
        // Start making the audio now; the client's fetch waits for it.
        let config = self.base_config();
        let voice = self.voice().clone();
        let clip_id = id.clone();
        tokio::spawn(async move {
            if let Err(err) = voice.clip(&clip_id, &config).await {
                tracing::warn!("cannot make speech: {err:#}");
            }
        });
        self.publish(&ServerMessage::Say {
            id,
            text,
            reason: reason.to_owned(),
            tab: Some(tab.to_owned()),
            agent: Some(envelope.agent.clone()),
            listener,
        });
    }
}

/// What a tool call waiting for approval would do, in a few spoken words.
fn action(tool: &str, summary: &str) -> String {
    let short: String = summary
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");
    let file = || {
        summary
            .rsplit('/')
            .next()
            .unwrap_or(summary)
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned()
    };
    match tool {
        "bash" if !short.is_empty() => format!("run {short}"),
        "write" | "edit" => format!("change {}", file()),
        _ if tool.starts_with("browser_") => "use the browser".to_owned(),
        _ if tool.starts_with("computer_") => "use your computer".to_owned(),
        _ => format!("use {}", tool.replace('_', " ")),
    }
}

/// "Done." and the reply's first sentence, without markdown.
fn done_line(reply: &str) -> String {
    let plain: String = reply
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("```") && !line.starts_with('|'))
        .unwrap_or_default()
        .trim_start_matches(['#', '-', '*', '>', ' '])
        .chars()
        .filter(|c| !matches!(c, '*' | '`' | '_'))
        .collect();
    let sentence = match plain.find(". ") {
        Some(end) => &plain[..=end],
        None => plain.as_str(),
    };
    let mut sentence: String = sentence.trim().chars().take(SUMMARY_CHARS).collect();
    if sentence.is_empty() {
        return "Done.".to_owned();
    }
    if !sentence.ends_with(['.', '!', '?']) {
        sentence.push('.');
    }
    format!("Done. {sentence}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaks_long_turns_and_approvals() {
        let voice = Voice::default();
        let agent = "a1";
        assert!(
            voice
                .line_for(
                    agent,
                    &Event::AgentStarted {
                        label: "main".into(),
                        parent: None,
                        model: "m".into(),
                        cwd: "/".into()
                    }
                )
                .is_none()
        );
        voice.line_for(agent, &Event::UserMessage { text: "go".into() });
        let approval = voice.line_for(
            agent,
            &Event::PermissionRequested {
                request_id: "r".into(),
                call_id: "c".into(),
                tool: "bash".into(),
                summary: "cargo publish --dry-run".into(),
                input: serde_json::Value::Null,
            },
        );
        assert_eq!(
            approval.unwrap().0,
            "malac needs your approval to run cargo publish --dry-run."
        );
        // A short turn ends quietly.
        let finished = Event::TurnFinished {
            status: TurnStatus::Completed,
            text: "All **done**. More.".into(),
        };
        assert!(voice.line_for(agent, &finished).is_none());
        // A long one says so, unless the agent spoke for itself.
        voice.line_for(agent, &Event::UserMessage { text: "go".into() });
        voice.turns.lock().unwrap().get_mut(agent).unwrap().started =
            Some(Instant::now() - LONG_TURN);
        assert_eq!(
            voice.line_for(agent, &finished).unwrap().0,
            "Done. All done."
        );
        voice.line_for(agent, &Event::UserMessage { text: "go".into() });
        voice.turns.lock().unwrap().get_mut(agent).unwrap().started =
            Some(Instant::now() - LONG_TURN);
        assert_eq!(
            voice
                .line_for(
                    agent,
                    &Event::Say {
                        text: "Tests pass.".into()
                    }
                )
                .unwrap()
                .1,
            "agent"
        );
        assert!(voice.line_for(agent, &finished).is_none());
    }

    #[test]
    fn speaks_on_the_device_used_last() {
        let voice = Voice::default();
        assert_eq!(voice.chosen(), None);
        let laptop = voice.listen();
        std::thread::sleep(Duration::from_millis(2));
        let phone = voice.listen();
        assert_eq!(voice.chosen(), Some(phone));
        std::thread::sleep(Duration::from_millis(2));
        voice.touch(laptop);
        assert_eq!(voice.chosen(), Some(laptop));
        voice.unlisten(laptop);
        assert_eq!(voice.chosen(), Some(phone));
    }

    #[test]
    fn spoken_actions_are_short() {
        assert_eq!(action("edit", "src/main.rs (3 lines)"), "change main.rs");
        assert_eq!(action("browser_click", "[12]"), "use the browser");
        assert_eq!(action("malac_configure", "model"), "use malac configure");
        assert_eq!(
            done_line("## Fixed the bug\n\nDetails"),
            "Done. Fixed the bug."
        );
        assert_eq!(done_line(""), "Done.");
    }
}
