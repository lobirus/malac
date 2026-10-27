//! What voice does in the interface: dictation into the prompt, speech on
//! this terminal, talk mode, `/voice`, `/speak`, and opening the files and
//! images malac sent.

use malac_core::shared;
use tokio::sync::oneshot;

use super::*;
use crate::protocol::Command;
use crate::tui::transcript::ToolState;
use crate::tui::voice::{self as ui, DICTATION_LIMIT};
use crate::voice::{self, TalkEvent, TalkState};

impl App {
    /// The daemon's voice offer changed.
    pub(super) fn voice_changed(&mut self, status: VoiceStatus) {
        self.voice = status;
        self.sync_speech();
    }

    /// Listens for speech when this terminal should speak and speech is
    /// ready, and stops otherwise.
    pub(super) fn sync_speech(&mut self) {
        let wanted = self.speak && voice::ready(&self.voice.speech) && self.link.connected;
        if wanted && self.listener.is_none() && !self.listen_asked {
            self.listen_asked = true;
            self.backend
                .request(Command::VoiceListen { on: true }, Reply::Listen);
        } else if !wanted && self.listener.is_some() {
            self.listener = None;
            self.backend.command(Command::VoiceListen { on: false });
        }
    }

    /// The daemon's answer to `voice_listen`, or `None` when there is no
    /// listener (any more).
    pub(super) fn listened(&mut self, listener: Option<u64>) {
        self.listen_asked = false;
        self.listener = listener;
        if listener.is_some() {
            // Turned off while the answer was on its way.
            self.sync_speech();
        }
    }

    /// A sentence to read aloud: played here when it is this terminal's.
    pub(super) fn on_say(
        &mut self,
        id: &str,
        text: &str,
        reason: &str,
        agent: Option<&str>,
        listener: u64,
    ) {
        if self.listener != Some(listener) || self.talk.is_some() {
            return;
        }
        self.backend.say(id);
        // What an agent says with its `say` tool shows as the tool call.
        if reason == "agent" {
            return;
        }
        let notice = Entry::Notice(Level::Info, format!("said aloud: {text}"));
        match agent.and_then(|a| self.agents.get_mut(a)) {
            Some(view) => view.transcript.push(notice),
            None => {
                if !self.push_entry(notice) {
                    self.flash(format!("said aloud: {text}"));
                }
            }
        }
    }

    /// `/speak on|off`: whether this terminal plays speech.
    pub(super) fn set_speak(&mut self, on: bool) {
        self.speak = on;
        ui::Local { speak: on }.save();
        if !on {
            self.backend.hush();
        }
        self.sync_speech();
        match (on, voice::ready(&self.voice.speech)) {
            (true, true) => self.flash_ok("this terminal speaks when malac does"),
            (true, false) => self.flash(format!(
                "this terminal will speak once speech works: {}",
                voice::unavailable("speech", &self.voice.speech, "/voice")
            )),
            (false, _) => self.flash_ok("this terminal stays quiet"),
        }
    }

    /// `/voice`: what dictation, speech and talk mode offer, and how to change it.
    pub(super) fn show_voice(&mut self) {
        let lines = ui::status_lines(&self.voice, self.speak, self.talk.is_some());
        self.info(lines);
    }

    /// `/voice [refresh | <setting> <value>]`.
    pub(super) fn voice_command(&mut self, arg: &str) -> Option<Action> {
        let (key, value) = arg.split_once(' ').unwrap_or((arg, ""));
        match (key, value.trim()) {
            ("", _) => Some(Action::ShowVoice),
            ("refresh", "") => {
                self.backend.voice_status(true, true);
                self.flash("asking the providers which models they have…");
                None
            }
            (_, "") => {
                self.notice(
                    Level::Error,
                    "usage: /voice <setting> <value>, e.g. /voice speech true",
                );
                None
            }
            (key, value) => Some(Action::SetVoice(key.to_owned(), value.to_owned())),
        }
    }

    pub(super) fn set_voice(&mut self, key: &str, value: &str) {
        match voice::setting(key, value) {
            Ok(changes) => self.backend.set_voice(changes),
            Err(err) => self.notice(Level::Error, format!("{err:#}")),
        }
    }

    /// Starts dictation, or stops it and types what was said.
    pub(super) fn dictate(&mut self) {
        if let Some(dictation) = &mut self.dictation {
            dictation.finish();
            return;
        }
        if self.talk.is_some() {
            return self.flash("talk mode has the microphone · Esc ends it");
        }
        if !voice::ready(&self.voice.dictation) {
            return self.flash(voice::unavailable(
                "dictation",
                &self.voice.dictation,
                "/voice",
            ));
        }
        let (stop, stopped) = oneshot::channel();
        self.backend.dictate(stopped);
        self.dictation = Some(Dictation::new(stop));
        self.focus = Focus::Input;
    }

    /// Keys while dictating: Ctrl-X or Enter stop and type, Esc and Ctrl-C
    /// throw the recording away. Others go on as usual.
    pub(super) fn dictation_key(&mut self, key: KeyEvent) -> Option<Option<Action>> {
        let dictation = self.dictation.as_mut()?;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('x') if ctrl => Some(Some(Action::Dictate)),
            // Enter while the text is on its way would send half a prompt.
            KeyCode::Enter if plain && dictation.transcribing => Some(None),
            KeyCode::Enter if plain => Some(Some(Action::Dictate)),
            KeyCode::Esc => {
                dictation.cancel();
                self.dictation = None;
                self.flash("dictation cancelled");
                Some(None)
            }
            KeyCode::Char('c') if ctrl => {
                dictation.cancel();
                self.dictation = None;
                self.flash("dictation cancelled");
                Some(None)
            }
            _ => None,
        }
    }

    /// The text dictation heard goes into the prompt at the cursor.
    pub(super) fn dictated(&mut self, result: Result<String, String>) {
        // Cancelled meanwhile.
        if self.dictation.take().is_none() {
            return;
        }
        match result {
            Ok(text) if text.is_empty() => self.flash("dictation heard nothing"),
            Ok(text) => {
                let before = self.editor.text()[..self.editor.cursor_offset()]
                    .chars()
                    .next_back();
                let text = match before {
                    Some(c) if !c.is_whitespace() => format!(" {text}"),
                    _ => text,
                };
                self.editor.insert(&text);
                self.completion_dismissed = false;
                self.focus = Focus::Input;
            }
            Err(err) => self.flash(format!("dictation: {}", text::first_line(&err))),
        }
    }

    /// The prompt's bottom line while dictating.
    pub fn dictation_hint(&self) -> Option<(bool, String)> {
        let dictation = self.dictation.as_ref()?;
        Some(match dictation.transcribing {
            true => (false, "transcribing…".to_owned()),
            false => (
                true,
                format!(
                    "recording {} · ^X or enter stops · esc cancels",
                    voice::minutes(dictation.started.elapsed().as_secs())
                ),
            ),
        })
    }

    /// Starts talk mode through the tab's agent, or ends it.
    pub(super) fn toggle_talk(&mut self) {
        if let Some(view) = &self.talk {
            view.talk.stop();
            return self.flash("ending talk mode…");
        }
        if self.dictation.is_some() {
            return self.flash("dictation has the microphone · Esc cancels it");
        }
        if !voice::ready(&self.voice.talk) {
            return self.flash(voice::unavailable("talk mode", &self.voice.talk, "/voice"));
        }
        let Some(tab) = self.tab_id() else {
            return;
        };
        let agent = self.tab().root().map(str::to_owned);
        self.talks += 1;
        let Some(talk) = self.backend.talk(self.talks, Some(tab), agent.clone()) else {
            return self.flash("not connected to the daemon");
        };
        self.backend.hush();
        self.talk = Some(TalkView {
            talk,
            session: self.talks,
            agent,
            state: TalkState::Connecting,
        });
    }

    pub(super) fn on_talk(&mut self, session: u64, event: TalkEvent) {
        let Some(view) = self.talk.as_mut().filter(|v| v.session == session) else {
            return;
        };
        match event {
            TalkEvent::Ready { model, voice } => self.talk_notice(
                Level::Info,
                format!("talk mode · {model} · voice {voice} · speak, or Esc to end"),
            ),
            TalkEvent::State(state) => view.state = state,
            TalkEvent::Said { you, text } => {
                let who = if you { "you" } else { "voice" };
                self.talk_notice(Level::Info, format!("{who} · {text}"));
            }
            TalkEvent::Tool(summary) => self.talk_notice(Level::Info, format!("voice · {summary}")),
            TalkEvent::Error(message) => {
                self.talk_notice(Level::Error, format!("talk mode: {message}"));
            }
            TalkEvent::Closed(_) => {
                self.talk = None;
                self.flash("talk mode ended");
            }
        }
    }

    /// A talk mode line in the transcript of the agent it works through.
    fn talk_notice(&mut self, level: Level, text: String) {
        let agent = self.talk.as_ref().and_then(|v| v.agent.clone());
        match agent.and_then(|a| self.agents.get_mut(&a)) {
            Some(view) => view.transcript.push(Entry::Notice(level, text)),
            None => self.notice(level, text),
        }
    }

    /// The status line during talk mode.
    pub(super) fn talk_activity(&self, view: &TalkView) -> Vec<Span<'static>> {
        let t = theme();
        let style = match view.state {
            TalkState::Speaking | TalkState::Hearing => t.fg(t.accent),
            TalkState::Muted => t.fg(t.warning),
            _ => t.dim(),
        };
        vec![
            Span::styled("● ", style),
            Span::raw(format!("talk · {}", view.state.label(false))),
            Span::styled(" · Esc ends", t.dim()),
        ]
    }

    /// Keeps dictation within its limit; returns whether the screen changes.
    pub(super) fn voice_tick(&mut self) -> bool {
        let Some(dictation) = &mut self.dictation else {
            return false;
        };
        if !dictation.transcribing && dictation.started.elapsed() >= DICTATION_LIMIT {
            dictation.finish();
        }
        true
    }

    /// What malac sent in the transcript shown, newest first.
    pub fn openable(&self) -> Vec<Openable> {
        let count = self.shown().map_or(0, |v| v.transcript.items().len());
        (0..count)
            .rev()
            .flat_map(|index| self.entry_openable(index).into_iter().rev())
            .collect()
    }

    /// The files or images of one transcript entry, in order.
    pub fn entry_openable(&self, index: usize) -> Vec<Openable> {
        let Some(view) = self.shown() else {
            return Vec::new();
        };
        let Some(Entry::Tool(tool)) = view.transcript.items().get(index).map(|i| &i.entry) else {
            return Vec::new();
        };
        if tool.state != ToolState::Done {
            return Vec::new();
        }
        if tool.name == "send_to_user" {
            return shared::parse_tags(&tool.output)
                .into_iter()
                .map(Openable::Shared)
                .collect();
        }
        (0..ui::image_count(&tool.output))
            .map(|index| Openable::Image {
                agent: view.id.clone(),
                call: tool.call_id.clone(),
                tool: tool.name.clone(),
                index,
            })
            .collect()
    }

    /// Whether an entry shows files malac sent, which a click or Enter opens.
    pub fn is_file_card(&self, index: usize) -> bool {
        let sent = self.shown().is_some_and(|v| {
            matches!(
                v.transcript.items().get(index).map(|i| &i.entry),
                Some(Entry::Tool(tool)) if tool.name == "send_to_user"
            )
        });
        sent && !self.entry_openable(index).is_empty()
    }

    /// `/open [n]`: the nth newest file or image, from 1.
    pub(super) fn open_nth(&mut self, n: usize) {
        let items = self.openable();
        match items.get(n.saturating_sub(1)) {
            Some(item) => self.open_item(item.clone()),
            None if items.is_empty() => {
                self.flash("nothing to open: malac has sent no files or images here")
            }
            None => self.flash(format!(
                "there are {} files and images here; /open 1 is the newest",
                items.len()
            )),
        }
    }

    /// Opens the `file`th file or image of a transcript entry.
    pub(super) fn open_entry(&mut self, index: usize, file: usize) {
        let items = self.entry_openable(index);
        match items.get(file).or(items.first()) {
            Some(item) => self.open_item(item.clone()),
            None => self.flash("nothing to open here"),
        }
    }

    fn open_item(&mut self, item: Openable) {
        self.flash(format!("opening {}…", item.name()));
        self.backend.open(item);
    }

    /// `/open`, `/speak` and `/voice`; `None` for other commands.
    pub(super) fn voice_slash(&mut self, name: &str, arg: &str) -> Option<Option<Action>> {
        Some(match name {
            "talk" => Some(Action::Talk),
            "voice" => self.voice_command(arg),
            "speak" => match arg {
                "" => Some(Action::Speak(!self.speak)),
                "on" | "true" | "yes" => Some(Action::Speak(true)),
                "off" | "false" | "no" => Some(Action::Speak(false)),
                _ => {
                    self.notice(Level::Error, "usage: /speak [on|off]");
                    None
                }
            },
            "open" => match arg {
                "" => Some(Action::OpenFile(1)),
                n => match n.parse::<usize>() {
                    Ok(n) if n > 0 => Some(Action::OpenFile(n)),
                    _ => {
                        self.notice(Level::Error, "usage: /open [n], 1 being the newest");
                        None
                    }
                },
            },
            _ => return None,
        })
    }
}
