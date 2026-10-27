//! Linux and other systems: recording and playing through the sound
//! system's own commands, so the binary links no audio library and starts
//! on machines without one.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc as sync_mpsc};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::{Chunks, Kind, Output, wav};

/// Recorders, in order of preference, with the arguments for raw mono
/// 16-bit little-endian audio at `{rate}` on standard output.
const RECORDERS: &[(&str, &[&str])] = &[
    (
        "pw-record",
        &[
            "--raw",
            "--format",
            "s16",
            "--rate",
            "{rate}",
            "--channels",
            "1",
            "-",
        ],
    ),
    (
        "parecord",
        &["--raw", "--format=s16le", "--rate={rate}", "--channels=1"],
    ),
    (
        "arecord",
        &["-q", "-t", "raw", "-f", "S16_LE", "-r", "{rate}", "-c", "1"],
    ),
];

/// Players of the same raw audio on standard input.
const STREAM_PLAYERS: &[(&str, &[&str])] = &[
    (
        "pw-play",
        &[
            "--raw",
            "--format",
            "s16",
            "--rate",
            "{rate}",
            "--channels",
            "1",
            "-",
        ],
    ),
    (
        "paplay",
        &["--raw", "--format=s16le", "--rate={rate}", "--channels=1"],
    ),
    (
        "aplay",
        &[
            "-q", "-t", "raw", "-f", "S16_LE", "-r", "{rate}", "-c", "1", "-",
        ],
    ),
];

const WAV_PLAYERS: &[(&str, &[&str])] = &[("pw-play", &[]), ("paplay", &[]), ("aplay", &["-q"])];

const MP3_PLAYERS: &[(&str, &[&str])] = &[
    ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "quiet"]),
    ("mpg123", &["-q"]),
    ("mpv", &["--no-video", "--really-quiet"]),
];

pub fn file_player(path: &Path, kind: Kind) -> Result<(String, tokio::process::Command)> {
    let (players, install) = match kind {
        Kind::Wav => (
            WAV_PLAYERS,
            "install PipeWire (pw-play), PulseAudio (paplay) or ALSA's utilities (aplay)",
        ),
        Kind::Mp3 => (MP3_PLAYERS, "install ffmpeg (ffplay), mpg123 or mpv"),
    };
    let Some((program, args)) = players.iter().find(|(p, _)| usable(p)) else {
        bail!("nothing here can play {}: {install}", path.display());
    };
    let mut command = tokio::process::Command::new(program);
    command.args(*args).arg(path).stderr(Stdio::null());
    Ok(((*program).to_owned(), command))
}

/// A program on `PATH`; PipeWire's tools only when they take raw audio.
fn usable(program: &str) -> bool {
    let Some(path) = find(program) else {
        return false;
    };
    if !program.starts_with("pw-") {
        return true;
    }
    static RAW: OnceLock<bool> = OnceLock::new();
    *RAW.get_or_init(|| {
        Command::new(path)
            .arg("--help")
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("--raw"))
    })
}

fn find(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|p| p.is_file())
}

fn command(choices: &[(&str, &[&str])], rate: u32) -> Option<Command> {
    let (program, args) = choices.iter().find(|(p, _)| usable(p))?;
    let mut command = Command::new(program);
    command.args(args.iter().map(|a| a.replace("{rate}", &rate.to_string())));
    Some(command)
}

pub fn capture(rate: u32, tx: Chunks, stop: Arc<AtomicBool>) -> Result<()> {
    let mut command = command(RECORDERS, rate).context(
        "nothing here can record: install PipeWire (pw-record), PulseAudio (parecord) or \
         ALSA's utilities (arecord)",
    )?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("cannot start the recorder")?;
    // A recorder without a device gives up at once; say why.
    std::thread::sleep(Duration::from_millis(200));
    if let Ok(Some(status)) = child.try_wait() {
        let mut why = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut why);
        }
        let why = why.lines().next().unwrap_or("no details").to_owned();
        bail!("the recorder stopped ({status}): {why}");
    }
    let mut stdout = child.stdout.take().context("the recorder has no output")?;
    drop(child.stderr.take());
    std::thread::spawn(move || {
        let mut buffer = vec![0u8; 3200];
        // Bytes not sent yet: a header still arriving, or half a sample.
        let mut pending: Vec<u8> = Vec::new();
        let mut started = false;
        while let Ok(n) = stdout.read(&mut buffer) {
            if n == 0 || stop.load(Ordering::Relaxed) {
                break;
            }
            pending.extend_from_slice(&buffer[..n]);
            if !started {
                // Some versions write a WAV header even to a pipe.
                let Some(offset) = wav::data_offset(&pending) else {
                    continue;
                };
                pending.drain(..offset);
                started = true;
            }
            let whole = pending.len() / 2 * 2;
            let samples = wav::samples(&pending[..whole]);
            pending.drain(..whole);
            if !samples.is_empty() && tx.send(samples).is_err() {
                break;
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    });
    Ok(())
}

pub fn speaker(rate: u32) -> Result<Box<dyn Output>> {
    if command(STREAM_PLAYERS, rate).is_none() {
        bail!(
            "nothing here can play audio: install PipeWire (pw-play), PulseAudio (paplay) or \
             ALSA's utilities (aplay)"
        );
    }
    Ok(Box::new(StreamOutput { rate, player: None }))
}

/// A raw player fed from a thread, so a full pipe never blocks the caller.
struct Player {
    child: Child,
    feed: sync_mpsc::Sender<Vec<u8>>,
}

impl Player {
    fn start(rate: u32) -> Option<Self> {
        let mut child = command(STREAM_PLAYERS, rate)?
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut stdin: ChildStdin = child.stdin.take()?;
        let (feed, queue) = sync_mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            while let Ok(bytes) = queue.recv() {
                if stdin.write_all(&bytes).is_err() {
                    break;
                }
            }
        });
        Some(Self { child, feed })
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct StreamOutput {
    rate: u32,
    /// Started on the first sound, and again after [`Output::clear`].
    player: Option<Player>,
}

impl Output for StreamOutput {
    fn write(&mut self, samples: &[i16]) {
        if self.player.is_none() {
            self.player = Player::start(self.rate);
        }
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let sent = self
            .player
            .as_ref()
            .is_some_and(|p| p.feed.send(bytes).is_ok());
        if !sent {
            self.player = None;
        }
    }

    /// What the player buffered cannot be taken back: stop it.
    fn clear(&mut self) {
        self.player = None;
    }
}
