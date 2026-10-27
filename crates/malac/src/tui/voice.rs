//! Voice in the full-screen interface: dictation into the prompt, speech
//! on this terminal, talk mode, `/voice`, and opening the files and images
//! malac sends in the system viewer.

use std::path::{Path, PathBuf};
use std::time::Instant;

use malac_core::event::AgentId;
use malac_core::shared::SharedFile;
use ratatui::text::{Line, Span};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use super::theme::theme;
use crate::protocol::VoiceStatus;
use crate::voice::{self, Talk, TalkState};

/// Dictation stops by itself after this long.
pub const DICTATION_LIMIT: std::time::Duration = std::time::Duration::from_secs(120);

/// A recording for the prompt.
pub struct Dictation {
    pub started: Instant,
    /// Tells the recorder to stop: `true` transcribes, `false` throws it away.
    stop: Option<oneshot::Sender<bool>>,
    pub transcribing: bool,
}

impl Dictation {
    pub fn new(stop: oneshot::Sender<bool>) -> Self {
        Self {
            started: Instant::now(),
            stop: Some(stop),
            transcribing: false,
        }
    }

    /// Stops recording; the text arrives as an update.
    pub fn finish(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
            self.transcribing = true;
        }
    }

    pub fn cancel(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(false);
        }
    }
}

/// Talk mode in this terminal.
pub struct TalkView {
    pub talk: Talk,
    /// Which of this terminal's talk sessions it is, to tell their events apart.
    pub session: u64,
    /// The agent it works through, whose transcript gets its lines.
    pub agent: Option<AgentId>,
    pub state: TalkState,
}

/// What this terminal remembers about voice, in `~/.malac/terminal.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Local {
    /// Speak on this terminal when the daemon speaks.
    #[serde(default = "yes")]
    pub speak: bool,
}

fn yes() -> bool {
    true
}

impl Default for Local {
    fn default() -> Self {
        Self { speak: true }
    }
}

impl Local {
    fn path() -> PathBuf {
        malac_core::config::home_dir().join("terminal.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::path();
        let written = serde_json::to_string_pretty(self)
            .map_err(anyhow::Error::from)
            .and_then(|text| Ok(std::fs::write(&path, text)?));
        if let Err(err) = written {
            tracing::warn!("cannot write {}: {err:#}", path.display());
        }
    }
}

/// Something malac sent that the system viewer can show.
#[derive(Debug, Clone, PartialEq)]
pub enum Openable {
    /// A file from `send_to_user`.
    Shared(SharedFile),
    /// The `index`th image a tool call returned, such as a screenshot.
    Image {
        agent: AgentId,
        call: String,
        tool: String,
        index: usize,
    },
}

impl Openable {
    pub fn name(&self) -> String {
        match self {
            Openable::Shared(file) => file.name.clone(),
            Openable::Image { tool, index, .. } => format!("{tool} image {}", index + 1),
        }
    }
}

/// `image`, `PDF file`, `file`: what a shared file is, in a word or two.
pub fn kind_label(file: &SharedFile) -> String {
    let family = file.media_type.split('/').next().unwrap_or_default();
    if matches!(family, "image" | "video" | "audio") {
        return family.to_owned();
    }
    match file.name.rsplit_once('.') {
        Some((_, extension)) if !extension.is_empty() && extension.len() <= 5 => {
            format!("{} file", extension.to_ascii_uppercase())
        }
        _ => "file".to_owned(),
    }
}

/// How many images a tool's output names: `[image/png image]` live, `[image]`
/// in a saved conversation.
pub fn image_count(output: &str) -> usize {
    output.matches("[image/").count() + output.matches("[image]").count()
}

/// `/voice`: each feature's state, this terminal's speech, and how to change them.
pub fn status_lines(status: &VoiceStatus, speak: bool, talking: bool) -> Vec<Line<'static>> {
    let t = theme();
    let mut lines = Vec::new();
    let rows = [
        ("dictation", &status.dictation, "Ctrl-X in the prompt"),
        ("speech", &status.speech, ""),
        ("talk", &status.talk, "/talk"),
    ];
    for (name, feature, how) in rows {
        let (state, detail) = voice::describe(name, feature);
        let style = match state {
            "ready" => t.fg(t.accent),
            "off" => t.dim(),
            _ => t.fg(t.warning),
        };
        let how = match name {
            "speech" if voice::ready(feature) && speak => {
                "on this terminal · /speak off".to_owned()
            }
            "speech" if voice::ready(feature) => "not on this terminal · /speak on".to_owned(),
            "talk" if talking => "on now · Esc ends".to_owned(),
            _ if voice::ready(feature) => how.to_owned(),
            _ => String::new(),
        };
        let mut spans = vec![
            Span::raw(format!("  {name:<10} ")),
            Span::styled(format!("{state:<12}"), style),
            Span::styled(detail, t.dim()),
        ];
        if !how.is_empty() {
            spans.push(Span::styled(format!("  {how}"), t.faint()));
        }
        lines.push(Line::from(spans));
    }
    let language = status
        .language
        .as_deref()
        .unwrap_or("detected by the model");
    lines.push(Line::from(vec![
        Span::raw(format!("  {:<10} ", "language")),
        Span::styled(language.to_owned(), t.dim()),
    ]));
    lines.push(Line::styled(
        "  change: /voice <setting> <value>, e.g. /voice speech true, /voice talk_voice marin, \
         /voice speech_model none",
        t.faint(),
    ));
    lines
}

/// Opens a file in the system viewer: `open` on macOS, `xdg-open` elsewhere.
pub fn open_in_viewer(path: &Path) -> Result<(), String> {
    #[cfg(test)]
    {
        OPENED.lock().unwrap().push(path.to_owned());
        Ok(())
    }
    #[cfg(not(test))]
    {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let mut child = std::process::Command::new(opener)
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => {
                    format!(
                        "{opener} is missing: install xdg-utils, or find it at {}",
                        path.display()
                    )
                }
                _ => format!("cannot run {opener}: {err}"),
            })?;
        std::thread::spawn(move || child.wait());
        Ok(())
    }
}

/// What tests opened, instead of the viewer.
#[cfg(test)]
pub static OPENED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Where a downloaded file goes before the viewer opens it.
pub fn download_path(folder: &str, name: &str) -> PathBuf {
    let name: String = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_control() { '_' } else { c })
        .collect();
    let name = match name.trim_start_matches('.') {
        "" => "file".to_owned(),
        clean => clean.to_owned(),
    };
    std::env::temp_dir()
        .join("malac-open")
        .join(folder)
        .join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_what_was_sent() {
        let file = |name: &str, media_type: &str| SharedFile {
            id: "1".into(),
            name: name.into(),
            media_type: media_type.into(),
            size: 1,
        };
        assert_eq!(kind_label(&file("shot.png", "image/png")), "image");
        assert_eq!(
            kind_label(&file("report.pdf", "application/pdf")),
            "PDF file"
        );
        assert_eq!(
            kind_label(&file("Makefile", "application/octet-stream")),
            "file"
        );
        assert_eq!(image_count("[image/png image]\n[image/jpeg image]"), 2);
        assert_eq!(image_count("done [image]"), 1);
        assert_eq!(image_count("no pictures"), 0);
        let path = download_path("abc", "../../etc/passwd");
        assert!(
            path.ends_with("malac-open/abc/passwd"),
            "{}",
            path.display()
        );
        assert!(download_path("abc", "..").ends_with("abc/file"));
    }
}
