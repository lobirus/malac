//! Files and images attached to a prompt, as chips above it: what a file
//! is, recognising paths a terminal pastes for dropped files, and the
//! clipboard's image on macOS. The daemon reads attached files when the
//! prompt is sent; a pasted image has no file it could read, so its bytes
//! travel with the prompt. The transcript shows both from the
//! `[attached: …]` line the `user_message` event carries.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Image {
        /// Width and height in pixels, when the header says.
        pixels: Option<(u32, u32)>,
    },
    Directory,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub path: PathBuf,
    /// The path as shown: relative to the working directory when inside it.
    pub name: String,
    pub kind: Kind,
    /// Bytes, or entries for a directory; unknown for a file on the daemon's
    /// machine.
    pub size: Option<u64>,
    /// A pasted image's PNG bytes, sent with the prompt instead of a path.
    pub inline: Option<Inline>,
    /// A path on the daemon's machine, picked from its `@` index; this
    /// machine has no such file.
    pub on_daemon: bool,
}

/// Image bytes carried by an attachment.
#[derive(Clone, PartialEq)]
pub struct Inline(pub Vec<u8>);

impl std::fmt::Debug for Inline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Inline({} bytes)", self.0.len())
    }
}

impl Attachment {
    /// Looks at what `path` is; the daemon decides what to send of it.
    pub fn open(path: &Path, cwd: &Path) -> Result<Self, String> {
        let name = display_name(path, cwd);
        let meta = std::fs::metadata(path).map_err(|e| format!("{name}: {e}"))?;
        let (kind, size) = if meta.is_dir() {
            let entries = std::fs::read_dir(path).map_or(0, |d| d.count() as u64);
            (Kind::Directory, entries)
        } else if is_image(path) {
            let pixels = pixels(path);
            (Kind::Image { pixels }, meta.len())
        } else {
            (Kind::File, meta.len())
        };
        Ok(Self {
            path: path.to_owned(),
            name,
            kind,
            size: Some(size),
            inline: None,
            on_daemon: false,
        })
    }

    /// A file below `root` on the daemon's machine, `relative` to it.
    pub fn on_daemon(root: &Path, relative: &str) -> Self {
        let path = root.join(relative);
        let kind = match is_image(&path) {
            true => Kind::Image { pixels: None },
            false => Kind::File,
        };
        Self {
            path,
            name: relative.to_owned(),
            kind,
            size: None,
            inline: None,
            on_daemon: true,
        }
    }

    /// A PNG from the clipboard, named `name`.
    pub fn pasted(name: String, png: Vec<u8>) -> Self {
        Self {
            path: PathBuf::from(&name),
            kind: Kind::Image {
                pixels: pixels_of(&png),
            },
            size: Some(png.len() as u64),
            name,
            inline: Some(Inline(png)),
            on_daemon: false,
        }
    }

    /// `▤ src/main.rs · 2.1 KB`, `▣ shot.png · 640×480 · 88 KB`, `▥ src/ · 12 entries`.
    pub fn label(&self) -> String {
        let bytes = self
            .size
            .map(|b| format!(" · {}", size(b)))
            .unwrap_or_default();
        match self.kind {
            Kind::File => format!("▤ {}{bytes}", self.name),
            Kind::Image { pixels } => {
                let pixels = pixels
                    .map(|(w, h)| format!(" · {w}×{h}"))
                    .unwrap_or_default();
                format!("▣ {}{pixels}{bytes}", self.name)
            }
            Kind::Directory => {
                let entries = self.size.unwrap_or_default();
                let noun = if entries == 1 { "entry" } else { "entries" };
                format!("▥ {}/ · {entries} {noun}", self.name)
            }
        }
    }
}

/// `512 B`, `2.1 KB`, `1.4 MB`.
pub fn size(bytes: u64) -> String {
    malac_core::attach::human_size(bytes)
}

fn display_name(path: &Path, cwd: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(cwd) {
        if !relative.as_os_str().is_empty() {
            return relative.display().to_string();
        }
    }
    super::app::short_path(path)
}

fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp"
        )
    })
}

fn head(path: &Path, len: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(len);
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(len as u64).read_to_end(&mut buf);
    }
    buf
}

fn pixels(path: &Path) -> Option<(u32, u32)> {
    pixels_of(&head(path, 64 * 1024))
}

/// Image size from the PNG, GIF or JPEG header.
fn pixels_of(data: &[u8]) -> Option<(u32, u32)> {
    let be32 = |at: usize| Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let be16 = |at: usize| Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?) as u32);
    let le16 = |at: usize| Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?) as u32);
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((be32(16)?, be32(20)?));
    }
    if data.starts_with(b"GIF8") {
        return Some((le16(6)?, le16(8)?));
    }
    if data.starts_with(&[0xff, 0xd8]) {
        let mut at = 2;
        while at + 9 < data.len() {
            if data[at] != 0xff {
                return None;
            }
            let marker = data[at + 1];
            let len = be16(at + 2)? as usize;
            let frame = matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
            if frame {
                return Some((be16(at + 7)?, be16(at + 5)?));
            }
            at += 2 + len;
        }
    }
    None
}

/// Paths a terminal pasted for files dropped on it: absolute, `~/` or
/// `file://` paths to existing files or folders, quoted or with escaped
/// spaces. Anything else is ordinary text.
pub fn dropped_paths(text: &str) -> Option<Vec<PathBuf>> {
    let words = shell_words(text.trim())?;
    if words.is_empty() {
        return None;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    words
        .into_iter()
        .map(|word| {
            let word = match word.strip_prefix("file://") {
                Some(rest) => percent_decode(rest),
                None => word,
            };
            let path = match (word.strip_prefix("~/"), &home) {
                (Some(rest), Some(home)) => home.join(rest),
                _ => PathBuf::from(word),
            };
            (path.is_absolute() && path.exists()).then_some(path)
        })
        .collect()
}

/// Splits like a shell: whitespace separates, quotes and backslashes escape.
fn shell_words(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                word.push(chars.next()?);
                in_word = true;
            }
            '\'' | '"' => {
                for q in chars.by_ref() {
                    if q == c {
                        break;
                    }
                    word.push(q);
                }
                in_word = true;
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Some(words)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The clipboard's image as a PNG attachment, read through `osascript`
/// by way of a temporary file in `dir`.
pub fn clipboard_image(dir: &Path) -> Result<Attachment, String> {
    if !cfg!(target_os = "macos") {
        return Err("Ctrl-V attaches clipboard images on macOS only".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stamp = chrono::Local::now().format("%H%M%S");
    let path = dir.join(format!("pasted-{stamp}-{}.png", std::process::id()));
    let target = path
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let open = format!("set f to open for access (POSIX file \"{target}\") with write permission");
    let script = [
        "try",
        "set png to (the clipboard as «class PNGf»)",
        "on error",
        "return \"none\"",
        "end try",
        open.as_str(),
        "set eof f to 0",
        "write png to f",
        "close access f",
        "return \"ok\"",
    ];
    let mut command = Command::new("osascript");
    for line in script {
        command.arg("-e").arg(line);
    }
    let output = command.output().map_err(|e| format!("osascript: {e}"))?;
    match String::from_utf8_lossy(&output.stdout).trim() {
        "ok" => {
            let png = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()));
            let _ = std::fs::remove_file(&path);
            let name = format!("pasted-{stamp}.png");
            Ok(Attachment::pasted(name, png?))
        }
        "none" => Err("no image on the clipboard".into()),
        _ => Err(format!(
            "could not read the clipboard: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// A prompt's text as its `user_message` event reads: what was typed,
/// then a line naming the attached files. Saved conversations are shown
/// the same way.
pub fn user_text(typed: &str, attached: &[String]) -> String {
    match (attached.is_empty(), typed.is_empty()) {
        (true, _) => typed.to_owned(),
        (false, true) => format!("[attached: {}]", attached.join(", ")),
        (false, false) => format!("{typed}\n\n[attached: {}]", attached.join(", ")),
    }
}

/// A file a sent prompt carried, read back from its `[attached: …]` line.
#[derive(Debug, Clone, PartialEq)]
pub struct Mention {
    pub name: String,
}

impl Mention {
    pub fn label(&self) -> String {
        let icon = if is_image(Path::new(&self.name)) {
            "▣"
        } else {
            "▤"
        };
        format!("{icon} {}", self.name)
    }
}

/// Separates a prompt's typed text from the line naming its attachments.
pub fn split_user_text(text: &str) -> (String, Vec<Mention>) {
    let (typed, line) = match text.rsplit_once("\n\n") {
        Some((typed, line)) => (typed, line),
        None => ("", text),
    };
    let names = line
        .strip_prefix("[attached: ")
        .and_then(|rest| rest.strip_suffix(']'))
        .filter(|names| !names.contains('\n'));
    match names {
        Some(names) => {
            let mentions = names
                .split(", ")
                .filter(|n| !n.is_empty())
                .map(|name| Mention {
                    name: name.to_owned(),
                })
                .collect();
            (typed.trim_end().to_owned(), mentions)
        }
        None => (text.trim_end().to_owned(), Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("malac-attach-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut data = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        data.extend(width.to_be_bytes());
        data.extend(height.to_be_bytes());
        data.extend([8, 6, 0, 0, 0]);
        data
    }

    #[test]
    fn chips_say_what_a_path_is() {
        let dir = scratch("kinds");
        std::fs::write(dir.join("notes.md"), "# hi\n").unwrap();
        std::fs::write(dir.join("shot.png"), png(640, 480)).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "").unwrap();
        let label = |name: &str| Attachment::open(&dir.join(name), &dir).unwrap().label();
        assert_eq!(label("notes.md"), "▤ notes.md · 5 B");
        assert_eq!(label("shot.png"), "▣ shot.png · 640×480 · 29 B");
        assert_eq!(label("src"), "▥ src/ · 1 entry");
        assert!(Attachment::open(&dir.join("missing"), &dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recognises_dropped_paths() {
        let dir = scratch("drop dir");
        let file = dir.join("my file.txt");
        std::fs::write(&file, "x").unwrap();
        let escaped = file.display().to_string().replace(' ', "\\ ");
        assert_eq!(dropped_paths(&escaped), Some(vec![file.clone()]));
        let quoted = format!("'{}' '{}'", file.display(), dir.display());
        assert_eq!(dropped_paths(&quoted).unwrap().len(), 2, "folders too");
        let url = format!("file://{}", file.display().to_string().replace(' ', "%20"));
        assert_eq!(dropped_paths(&url), Some(vec![file.clone()]));
        assert_eq!(dropped_paths("fix the bug in main.rs"), None);
        assert_eq!(dropped_paths(&format!("{} and more", file.display())), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn attached_lines_round_trip() {
        let text = user_text("explain", &["src/a.rs".into(), "b.png".into()]);
        assert_eq!(text, "explain\n\n[attached: src/a.rs, b.png]");
        let (prompt, mentions) = split_user_text(&text);
        assert_eq!(prompt, "explain");
        let labels: Vec<String> = mentions.iter().map(Mention::label).collect();
        assert_eq!(labels, ["▤ src/a.rs", "▣ b.png"]);
        let (prompt, mentions) = split_user_text("[attached: notes.md]");
        assert_eq!((prompt.as_str(), mentions.len()), ("", 1));
        let plain = "a list:\n\n[not attached]";
        assert_eq!(split_user_text(plain), (plain.to_owned(), Vec::new()));
    }
}
