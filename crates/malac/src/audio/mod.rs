//! The microphone and the speaker, for dictation, speech and talk mode.
//!
//! On macOS the microphone and talk mode's stream go through Core Audio
//! (`cpal`), and files play with `afplay`. Elsewhere malac links no audio
//! library, so it starts on servers without one: it records with
//! `pw-record`, `parecord` or `arecord` and plays with `pw-play`, `paplay`
//! or `aplay` (MP3 with `ffplay`, `mpg123` or `mpv`).
//!
//! For tests, `MALAC_AUDIO_INPUT=<wav file>` replaces the microphone (the
//! file plays in real time, then silence) and `MALAC_AUDIO_OUTPUT=<file>`
//! gets everything that would play, appended: files as they are, talk
//! mode's stream as raw 16-bit PCM.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc;

mod convert;
pub mod wav;

// The commands build everywhere, so every platform's checks cover them.
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod command;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
use command as system;
#[cfg(target_os = "macos")]
use macos as system;

pub use convert::{Resampler, downmix};

pub const INPUT_ENV: &str = "MALAC_AUDIO_INPUT";
pub const OUTPUT_ENV: &str = "MALAC_AUDIO_OUTPUT";

/// How much a test recording sends at a time.
const REPLAY_CHUNK: Duration = Duration::from_millis(20);
/// Audio still in the device's buffers after the last sample was handed over.
const TAIL: Duration = Duration::from_millis(250);

/// Where recorded chunks go: mono 16-bit samples at the rate asked for.
type Chunks = mpsc::UnboundedSender<Vec<i16>>;

/// A recording from the default microphone, mono at the rate it was
/// started with, in chunks as they come. It stops when dropped.
pub struct Capture {
    chunks: mpsc::UnboundedReceiver<Vec<i16>>,
    stop: Arc<AtomicBool>,
}

impl Capture {
    /// Starts recording. It may wait a moment for the device; call it off
    /// the async threads.
    pub fn start(rate: u32) -> Result<Self> {
        let (tx, chunks) = mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        match std::env::var_os(INPUT_ENV).filter(|v| !v.is_empty()) {
            Some(path) => replay(Path::new(&path), rate, tx, stop.clone())?,
            None if cfg!(test) => bail!("tests never use the microphone; set {INPUT_ENV}"),
            None => system::capture(rate, tx, stop.clone())?,
        }
        Ok(Self { chunks, stop })
    }

    /// The next chunk; `None` once the recorder stopped by itself.
    pub async fn next(&mut self) -> Option<Vec<i16>> {
        self.chunks.recv().await
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Feeds a WAV file as if it were the microphone: in real time, then silence.
fn replay(path: &Path, rate: u32, tx: Chunks, stop: Arc<AtomicBool>) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let pcm = wav::decode(&bytes).with_context(|| format!("{INPUT_ENV}={}", path.display()))?;
    let samples = Resampler::new(pcm.rate, rate).process(&downmix(&pcm.samples, pcm.channels));
    let per_chunk = (rate as usize * REPLAY_CHUNK.as_millis() as usize / 1000).max(1);
    std::thread::spawn(move || {
        let started = Instant::now();
        let silence = vec![0i16; per_chunk];
        let mut chunks = samples.chunks(per_chunk);
        let mut sent = 0u32;
        while !stop.load(Ordering::Relaxed) {
            let chunk = chunks.next().unwrap_or(&silence).to_vec();
            if tx.send(chunk).is_err() {
                break;
            }
            sent += 1;
            let due = started + REPLAY_CHUNK * sent;
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    });
    Ok(())
}

/// Where a device takes audio: mono 16-bit samples at the speaker's rate.
trait Output: Send {
    fn write(&mut self, samples: &[i16]);
    /// Drops whatever is still queued, at once.
    fn clear(&mut self);
}

/// Plays a stream of mono 16-bit PCM as it arrives, as talk mode does.
pub struct Speaker {
    rate: u32,
    output: Box<dyn Output>,
    /// When what was handed over will have played.
    until: Option<Instant>,
}

impl Speaker {
    /// The default output device at `rate`. It may wait a moment for the
    /// device; call it off the async threads.
    pub fn open(rate: u32) -> Result<Self> {
        let output: Box<dyn Output> = match output_override() {
            Some(path) => Box::new(FileOutput::open(&path)?),
            None if cfg!(test) => Box::new(Discard),
            None => system::speaker(rate)?,
        };
        Ok(Self {
            rate,
            output,
            until: None,
        })
    }

    /// Queues little-endian 16-bit samples.
    pub fn push(&mut self, pcm: &[u8]) {
        let samples = wav::samples(pcm);
        if samples.is_empty() {
            return;
        }
        self.output.write(&samples);
        let now = Instant::now();
        let start = self.until.filter(|u| *u > now).unwrap_or(now);
        let length = Duration::from_secs_f64(samples.len() as f64 / f64::from(self.rate));
        self.until = Some(start + length);
    }

    /// Stops playing at once.
    pub fn clear(&mut self) {
        self.output.clear();
        self.until = None;
    }

    /// Whether audio is still coming out.
    pub fn playing(&self) -> bool {
        self.until.is_some_and(|u| u + TAIL > Instant::now())
    }
}

struct Discard;

impl Output for Discard {
    fn write(&mut self, _: &[i16]) {}
    fn clear(&mut self) {}
}

/// `MALAC_AUDIO_OUTPUT`: what would play, appended to a file.
struct FileOutput(std::fs::File);

impl FileOutput {
    fn open(path: &Path) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        Ok(Self(file))
    }
}

impl Output for FileOutput {
    fn write(&mut self, samples: &[i16]) {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        if let Err(err) = self.0.write_all(&bytes) {
            tracing::warn!("cannot write {OUTPUT_ENV}: {err}");
        }
    }

    fn clear(&mut self) {}
}

fn output_override() -> Option<PathBuf> {
    std::env::var_os(OUTPUT_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// What a sound file holds, for picking a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Wav,
    Mp3,
}

impl Kind {
    fn of(head: &[u8], path: &Path) -> Result<Self> {
        if head.starts_with(b"RIFF") {
            return Ok(Kind::Wav);
        }
        if head.starts_with(b"ID3") || (head.len() > 1 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0)
        {
            return Ok(Kind::Mp3);
        }
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        match extension.as_deref() {
            Some("wav") => Ok(Kind::Wav),
            Some("mp3") => Ok(Kind::Mp3),
            _ => bail!("{} is not a WAV or MP3 file", path.display()),
        }
    }
}

/// Plays a WAV or MP3 file on the speaker and returns when it is done.
pub async fn play_file(path: &Path) -> Result<()> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("cannot read {}", path.display()))?;
    let kind = Kind::of(&bytes[..bytes.len().min(16)], path)?;
    if let Some(out) = output_override() {
        return append(&out, &bytes);
    }
    if cfg!(test) {
        return Ok(());
    }
    let (program, mut command) = system::file_player(path, kind)?;
    let status = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status()
        .await
        .with_context(|| format!("cannot run {program}"))?;
    if !status.success() {
        bail!("{program} could not play {}", path.display());
    }
    Ok(())
}

/// Plays audio the daemon sent (`audio/mpeg` or `audio/wav`).
pub async fn play_bytes(bytes: &[u8], media_type: &str) -> Result<()> {
    if let Some(out) = output_override() {
        return append(&out, bytes);
    }
    if cfg!(test) {
        return Ok(());
    }
    let extension = match media_type.contains("mpeg") || media_type.contains("mp3") {
        true => "mp3",
        false => "wav",
    };
    let path = std::env::temp_dir().join(format!(
        "malac-speech-{}.{extension}",
        uuid::Uuid::new_v4().simple()
    ));
    tokio::fs::write(&path, bytes)
        .await
        .with_context(|| format!("cannot write {}", path.display()))?;
    let result = play_file(&path).await;
    let _ = tokio::fs::remove_file(&path).await;
    result
}

fn append(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(bytes))
        .with_context(|| format!("cannot write {}", path.display()))
}

/// Whether a recording is nothing but zeros, which is what a microphone
/// the system does not let malac use records.
pub fn is_silent(samples: &[i16]) -> bool {
    samples.iter().all(|s| *s == 0)
}

/// What to do when the microphone gives nothing.
pub fn silence_hint() -> String {
    if cfg!(target_os = "macos") {
        format!(
            "the microphone recorded only silence: {}",
            permission_hint()
        )
    } else {
        "the microphone recorded only silence: check that the right input is selected and not \
         muted"
            .to_owned()
    }
}

/// Where macOS lets the app malac runs in use the microphone.
pub fn permission_hint() -> String {
    format!(
        "allow {} in System Settings > Privacy & Security > Microphone, then restart it",
        microphone_owner()
    )
}

/// The app macOS asks about the microphone for: the terminal malac runs
/// in, or the editor that started it.
pub fn microphone_owner() -> String {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let editor = !var("VSCODE_PID").is_empty() || !var("VSCODE_IPC_HOOK").is_empty();
    let cursor = !var("CURSOR_TRACE_ID").is_empty() || var("VSCODE_CWD").contains("Cursor");
    owner(
        &var("__CFBundleIdentifier"),
        &var("TERM_PROGRAM"),
        editor,
        cursor,
    )
    .to_owned()
}

fn owner(bundle: &str, term_program: &str, editor: bool, cursor: bool) -> &'static str {
    const APPS: &[(&str, &str)] = &[
        (
            "com.microsoft.VSCodeInsiders",
            "Visual Studio Code - Insiders",
        ),
        ("com.microsoft.VSCode", "Visual Studio Code"),
        ("com.todesktop.230313mzl4w4u92", "Cursor"),
        ("com.exafunction.windsurf", "Windsurf"),
        ("dev.zed.Zed", "Zed"),
        ("com.apple.Terminal", "Terminal"),
        ("com.googlecode.iterm2", "iTerm"),
        ("com.mitchellh.ghostty", "Ghostty"),
        ("net.kovidgoyal.kitty", "kitty"),
        ("com.github.wez.wezterm", "WezTerm"),
        ("dev.warp.Warp-Stable", "Warp"),
        ("org.alacritty", "Alacritty"),
    ];
    if let Some((_, name)) = APPS.iter().find(|(id, _)| *id == bundle) {
        return name;
    }
    match term_program {
        "vscode" if cursor => "Cursor",
        "vscode" => "Visual Studio Code",
        "Apple_Terminal" => "Terminal",
        "iTerm.app" => "iTerm",
        "ghostty" => "Ghostty",
        "WezTerm" => "WezTerm",
        "WarpTerminal" => "Warp",
        _ if editor && cursor => "Cursor",
        _ if editor => "Visual Studio Code",
        _ => "your terminal app",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_app_that_owns_the_microphone() {
        assert_eq!(
            owner("com.microsoft.VSCode", "", true, false),
            "Visual Studio Code"
        );
        assert_eq!(
            owner("com.todesktop.230313mzl4w4u92", "vscode", true, true),
            "Cursor"
        );
        assert_eq!(owner("", "vscode", false, true), "Cursor");
        assert_eq!(
            owner("com.googlecode.iterm2", "iTerm.app", false, false),
            "iTerm"
        );
        assert_eq!(owner("", "Apple_Terminal", false, false), "Terminal");
        // The extension host runs malac without a terminal.
        assert_eq!(owner("", "", true, false), "Visual Studio Code");
        assert_eq!(
            owner("org.example.unknown", "", false, false),
            "your terminal app"
        );
    }

    #[test]
    fn tells_files_apart() {
        let path = Path::new("x.bin");
        assert_eq!(Kind::of(b"RIFF....WAVE", path).unwrap(), Kind::Wav);
        assert_eq!(Kind::of(b"ID3\x04", path).unwrap(), Kind::Mp3);
        assert_eq!(Kind::of(&[0xFF, 0xFB, 0x90], path).unwrap(), Kind::Mp3);
        assert_eq!(Kind::of(b"", Path::new("a.MP3")).unwrap(), Kind::Mp3);
        assert!(Kind::of(b"%PDF", Path::new("a.pdf")).is_err());
    }

    #[test]
    fn speakers_know_when_they_are_done() {
        let mut speaker = Speaker {
            rate: 24_000,
            output: Box::new(Discard),
            until: None,
        };
        assert!(!speaker.playing());
        speaker.push(&vec![0u8; 48_000]);
        assert!(speaker.playing());
        let until = speaker.until.unwrap();
        speaker.push(&vec![0u8; 4_800]);
        let added = speaker.until.unwrap() - until;
        assert_eq!(added, Duration::from_millis(100));
        speaker.clear();
        assert!(!speaker.playing());
    }

    #[test]
    fn recordings_replay_in_real_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("in.wav");
        let tone: Vec<i16> = (0..4_800).map(|i| (i % 100) as i16 * 100).collect();
        std::fs::write(&path, wav::encode(&tone, 48_000)).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        replay(&path, 16_000, tx, stop.clone()).unwrap();
        let mut got = Vec::new();
        while got.len() < 3_200 {
            got.extend(rx.blocking_recv().unwrap());
        }
        stop.store(true, Ordering::Relaxed);
        // 100 ms of sound, then 100 ms of silence.
        assert!(started.elapsed() >= Duration::from_millis(180));
        assert!(got[..1_500].iter().any(|s| *s != 0));
        assert!(is_silent(&got[1_700..3_200]));
    }
}
