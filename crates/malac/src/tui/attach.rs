//! Files and images attached to a prompt: what a file is, recognising
//! paths a terminal pastes for dropped files, the clipboard's image on
//! macOS, and the content blocks a prompt becomes.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine;
use malac_core::Block;

/// Text files are cut to this many bytes when sent.
pub const TEXT_CAP: usize = 100 * 1024;
/// Providers reject larger images.
const IMAGE_CAP: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Text,
    Image {
        media_type: &'static str,
        /// Width and height in pixels, when the header says.
        pixels: Option<(u32, u32)>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub path: PathBuf,
    /// The path as shown: relative to the working directory when inside it.
    pub name: String,
    pub kind: Kind,
    pub bytes: u64,
}

impl Attachment {
    /// Looks at the file to tell text from images; directories, binary
    /// files and oversized images are refused.
    pub fn open(path: &Path, cwd: &Path) -> Result<Self, String> {
        let name = display_name(path, cwd);
        let meta = std::fs::metadata(path).map_err(|e| format!("{name}: {e}"))?;
        if meta.is_dir() {
            return Err(format!("{name} is a directory"));
        }
        let kind = match image_type(path) {
            Some(_) if meta.len() > IMAGE_CAP => {
                return Err(format!("{name} is larger than 5 MB ({})", size(meta.len())));
            }
            Some(media_type) => Kind::Image {
                media_type,
                pixels: pixels(path),
            },
            None if is_binary(path) => return Err(format!("{name} is a binary file")),
            None => Kind::Text,
        };
        Ok(Self {
            path: path.to_owned(),
            name,
            kind,
            bytes: meta.len(),
        })
    }

    pub fn is_image(&self) -> bool {
        matches!(self.kind, Kind::Image { .. })
    }

    /// `▤ src/main.rs · 2.1 KB`, `▣ shot.png · 640×480 · 88 KB`.
    pub fn label(&self) -> String {
        let pixels = match self.kind {
            Kind::Image { pixels, .. } => pixels,
            Kind::Text => None,
        };
        label(self.is_image(), &self.name, Some(self.bytes), pixels)
    }
}

fn label(image: bool, name: &str, bytes: Option<u64>, pixels: Option<(u32, u32)>) -> String {
    let mut out = format!("{} {name}", if image { "▣" } else { "▤" });
    if let Some((w, h)) = pixels {
        out.push_str(&format!(" · {w}×{h}"));
    }
    if let Some(bytes) = bytes {
        out.push_str(&format!(" · {}", size(bytes)));
    }
    out
}

/// `512 B`, `2.1 KB`, `1.4 MB`.
pub fn size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / KB / KB)
    }
}

fn display_name(path: &Path, cwd: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(cwd) {
        if !relative.as_os_str().is_empty() {
            return relative.display().to_string();
        }
    }
    // Pasted images live in a temporary folder whose path says nothing.
    let temp = std::env::temp_dir();
    let temp = temp.canonicalize().unwrap_or(temp);
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    match path.file_name() {
        Some(name) if canonical.starts_with(&temp) => name.to_string_lossy().into_owned(),
        _ => super::app::short_path(path),
    }
}

fn image_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

fn head(path: &Path, len: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(len);
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(len as u64).read_to_end(&mut buf);
    }
    buf
}

fn is_binary(path: &Path) -> bool {
    head(path, 8192).contains(&0)
}

/// Image size from the PNG, GIF or JPEG header.
fn pixels(path: &Path) -> Option<(u32, u32)> {
    let data = head(path, 64 * 1024);
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
/// `file://` paths to existing files, quoted or with escaped spaces.
/// Anything else is ordinary text.
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
            (path.is_absolute() && path.is_file()).then_some(path)
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

/// Saves the clipboard's image as a PNG in `dir`, through `osascript`.
pub fn clipboard_image(dir: &Path) -> Result<PathBuf, String> {
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
        "ok" => Ok(path),
        "none" => Err("no image on the clipboard".into()),
        _ => Err(format!(
            "could not read the clipboard: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// What a prompt sends: its text, then each attachment. Text files go as
/// fenced blocks cut at [`TEXT_CAP`], images as image blocks after a line
/// naming them. This is the one place prompts become content.
pub fn to_blocks(text: &str, attachments: &[Attachment]) -> Vec<Block> {
    let mut blocks = Vec::new();
    if !text.trim().is_empty() {
        blocks.push(Block::text(text));
    }
    for attachment in attachments {
        let path = escape(&attachment.name);
        let bytes = attachment.bytes;
        match &attachment.kind {
            Kind::Text => {
                let content = match std::fs::read(&attachment.path) {
                    Ok(content) => content,
                    Err(err) => {
                        blocks.push(Block::text(format!(
                            "<file path=\"{path}\" error=\"{}\"/>",
                            escape(&err.to_string())
                        )));
                        continue;
                    }
                };
                let cut = content.len() > TEXT_CAP;
                let mut text =
                    String::from_utf8_lossy(&content[..content.len().min(TEXT_CAP)]).into_owned();
                if cut {
                    text.push_str(&format!(
                        "\n[cut: the first {} of {}]",
                        size(TEXT_CAP as u64),
                        size(bytes)
                    ));
                }
                let lang = attachment
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or_default();
                let fence = "`".repeat(longest_run(&text, '`').max(2) + 1);
                let body = text.trim_end_matches('\n');
                blocks.push(Block::text(format!(
                    "<file path=\"{path}\" bytes=\"{bytes}\">\n{fence}{lang}\n{body}\n{fence}\n</file>"
                )));
            }
            Kind::Image { media_type, pixels } => {
                let data = match std::fs::read(&attachment.path) {
                    Ok(data) => data,
                    Err(err) => {
                        blocks.push(Block::text(format!(
                            "<image path=\"{path}\" error=\"{}\"/>",
                            escape(&err.to_string())
                        )));
                        continue;
                    }
                };
                let pixels = pixels
                    .map(|(w, h)| format!(" width=\"{w}\" height=\"{h}\""))
                    .unwrap_or_default();
                blocks.push(Block::text(format!(
                    "<image path=\"{path}\" bytes=\"{bytes}\"{pixels}/>"
                )));
                blocks.push(Block::Image {
                    media_type: (*media_type).to_owned(),
                    data: base64::engine::general_purpose::STANDARD.encode(data),
                });
            }
        }
    }
    blocks
}

fn longest_run(text: &str, c: char) -> usize {
    let (mut best, mut run) = (0, 0);
    for ch in text.chars() {
        run = if ch == c { run + 1 } else { 0 };
        best = best.max(run);
    }
    best
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('"', "&quot;")
}

fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"").replace("&amp;", "&")
}

/// A file or image a sent prompt carried, read back from its text.
#[derive(Debug, Clone, PartialEq)]
pub struct Mention {
    pub image: bool,
    pub name: String,
    pub bytes: Option<u64>,
    pub pixels: Option<(u32, u32)>,
}

impl Mention {
    pub fn label(&self) -> String {
        label(self.image, &self.name, self.bytes, self.pixels)
    }
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let start = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    let end = tag[start..].find('"')?;
    Some(unescape(&tag[start..start + end]))
}

/// Where the next attachment tag starts: `<file ` or `<image ` opening a line.
fn next_tag(text: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = text[from..].find('<') {
        let at = from + i;
        let opens_line = at == 0 || text[..at].ends_with('\n');
        let tag = &text[at..];
        if opens_line && (tag.starts_with("<file ") || tag.starts_with("<image ")) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// Separates a prompt's own text from the `<file>` and `<image>` blocks
/// [`to_blocks`] added, which the transcript shows as chips.
pub fn split_user_text(text: &str) -> (String, Vec<Mention>) {
    let mut prompt = String::new();
    let mut mentions = Vec::new();
    let mut rest = text;
    while let Some(at) = next_tag(rest) {
        let tag = rest[at..].find('>').map(|close| &rest[at..at + close + 1]);
        let Some((tag, name)) = tag.and_then(|t| Some((t, attribute(t, "path")?))) else {
            // Not one of ours: it stays text.
            prompt.push_str(&rest[..at + 1]);
            rest = &rest[at + 1..];
            continue;
        };
        prompt.push_str(&rest[..at]);
        let image = tag.starts_with("<image");
        let mut end = at + tag.len();
        if !image && !tag.ends_with("/>") {
            end = rest[end..]
                .find("</file>")
                .map_or(rest.len(), |i| end + i + "</file>".len());
        }
        rest = rest[end..].strip_prefix('\n').unwrap_or(&rest[end..]);
        let number = |name| attribute(tag, name).and_then(|v| v.parse::<u64>().ok());
        let pixel = |name| number(name).and_then(|v| u32::try_from(v).ok());
        mentions.push(Mention {
            image,
            name,
            bytes: number("bytes"),
            pixels: pixel("width").zip(pixel("height")),
        });
    }
    prompt.push_str(rest);
    (prompt.trim_end().to_owned(), mentions)
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
    fn tells_text_from_images_and_binaries() {
        let dir = scratch("kinds");
        std::fs::write(dir.join("notes.md"), "# hi\n").unwrap();
        std::fs::write(dir.join("shot.png"), png(640, 480)).unwrap();
        std::fs::write(dir.join("blob.bin"), [1, 0, 2]).unwrap();
        let text = Attachment::open(&dir.join("notes.md"), &dir).unwrap();
        assert_eq!(text.label(), "▤ notes.md · 5 B");
        let image = Attachment::open(&dir.join("shot.png"), &dir).unwrap();
        assert_eq!(image.label(), "▣ shot.png · 640×480 · 29 B");
        assert!(Attachment::open(&dir.join("blob.bin"), &dir).is_err());
        assert!(Attachment::open(&dir, &dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recognises_dropped_paths() {
        let dir = scratch("drop dir");
        let file = dir.join("my file.txt");
        std::fs::write(&file, "x").unwrap();
        let escaped = file.display().to_string().replace(' ', "\\ ");
        assert_eq!(dropped_paths(&escaped), Some(vec![file.clone()]));
        let quoted = format!("'{}' '{}'", file.display(), file.display());
        assert_eq!(dropped_paths(&quoted).unwrap().len(), 2);
        let url = format!("file://{}", file.display().to_string().replace(' ', "%20"));
        assert_eq!(dropped_paths(&url), Some(vec![file.clone()]));
        assert_eq!(dropped_paths("fix the bug in main.rs"), None);
        assert_eq!(dropped_paths(&format!("{} and more", file.display())), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prompts_become_blocks_and_read_back_as_chips() {
        let dir = scratch("blocks");
        std::fs::write(dir.join("a.rs"), "fn main() {\n    // ```\n}\n").unwrap();
        std::fs::write(dir.join("b.png"), png(2, 3)).unwrap();
        let attachments = vec![
            Attachment::open(&dir.join("a.rs"), &dir).unwrap(),
            Attachment::open(&dir.join("b.png"), &dir).unwrap(),
        ];
        let blocks = to_blocks("explain", &attachments);
        assert_eq!(blocks.len(), 4);
        let Block::Text { text } = &blocks[1] else {
            panic!("a text block");
        };
        assert!(text.starts_with("<file path=\"a.rs\" bytes=\"25\">\n````rs\nfn main()"));
        assert!(matches!(&blocks[3], Block::Image { media_type, .. } if media_type == "image/png"));
        let joined: Vec<&str> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let (prompt, mentions) = split_user_text(&joined.join("\n"));
        assert_eq!(prompt, "explain");
        let labels: Vec<String> = mentions.iter().map(Mention::label).collect();
        assert_eq!(labels, ["▤ a.rs · 25 B", "▣ b.png · 2×3 · 29 B"]);
        let (prompt, mentions) = split_user_text("see <file path=\"x\"> inline");
        assert_eq!(prompt, "see <file path=\"x\"> inline");
        assert!(mentions.is_empty());
        let xml = "parse this:\n<file name=\"a\">x</file>";
        assert_eq!(split_user_text(xml), (xml.to_owned(), Vec::new()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn large_text_files_are_cut() {
        let dir = scratch("cap");
        std::fs::write(dir.join("big.txt"), "x".repeat(TEXT_CAP + 10)).unwrap();
        let big = Attachment::open(&dir.join("big.txt"), &dir).unwrap();
        let Block::Text { text } = &to_blocks("", &[big])[0] else {
            panic!("a text block");
        };
        assert!(text.contains("[cut: the first 100.0 KB of 100.0 KB]"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
