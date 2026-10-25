//! Files attached to a prompt, turned into content the model can read.
//!
//! Every attachment starts with an `<attachment path="...">` text block, so
//! front ends can tell attached content from what the user typed.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

use crate::message::Block;

/// Text beyond this is cut, with a note saying so.
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
/// Larger images are not attached.
pub const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
/// Directory listings stop after this many entries.
const MAX_LISTED: usize = 200;
/// A NUL byte in this prefix marks a file as binary.
const SNIFF_BYTES: usize = 8192;

const TAG: &str = "<attachment ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Text,
    Image,
    Directory,
    Binary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub path: PathBuf,
    pub kind: AttachmentKind,
    /// File size in bytes, or the number of entries for a directory.
    pub size: u64,
    /// Why the content was shortened or left out.
    pub note: Option<String>,
    /// The prompt content.
    #[serde(skip)]
    pub blocks: Vec<Block>,
}

/// Whether a text block is attached content rather than typed text.
pub fn is_attachment(text: &str) -> bool {
    text.starts_with(TAG)
}

/// Recalled memories are attached to a prompt in a block starting so.
const RECALLED: &str = "<recalled-memories>";

/// What the user typed, without attached files or recalled memories.
pub fn typed_text(content: &[Block]) -> String {
    content
        .iter()
        .filter_map(|b| match b {
            Block::Text { text } if !is_attachment(text) && !text.starts_with(RECALLED) => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The block that names an image sent with a prompt, as attached images have.
pub fn image_header(name: &str, bytes: u64) -> String {
    format!(
        "{TAG}path=\"{}\" kind=\"image\" size=\"{}\"></attachment>",
        escape(name),
        human_size(bytes)
    )
}

/// The path named by an attachment block.
pub fn attached_path(text: &str) -> Option<String> {
    let rest = text.strip_prefix(TAG)?.strip_prefix("path=\"")?;
    let end = rest.find('"')?;
    Some(rest[..end].replace("&quot;", "\"").replace("&amp;", "&"))
}

/// Resolves `path` like the file tools do: `~/` is the home directory and
/// relative paths start at `cwd`.
pub fn resolve(path: &str, cwd: &Path) -> PathBuf {
    let path = match path.strip_prefix("~/").zip(dirs::home_dir()) {
        Some((rest, home)) => home.join(rest),
        None => PathBuf::from(path),
    };
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

/// Reads `path` (absolute, or relative to `cwd`) into prompt content: text
/// files as a fenced block, images as image blocks, directories as a short
/// listing and other files as a note.
pub fn attach(path: &Path, cwd: &Path) -> Result<Attachment> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let meta = fs::metadata(&path).with_context(|| format!("cannot attach {}", path.display()))?;
    let shown = path
        .strip_prefix(cwd)
        .unwrap_or(&path)
        .display()
        .to_string();
    let shown = if shown.is_empty() { ".".into() } else { shown };
    if meta.is_dir() {
        return directory(path, &shown);
    }
    if !meta.is_file() {
        bail!("cannot attach {}: not a regular file", path.display());
    }
    if let Some(media_type) = image_type(&path) {
        return image(path, &shown, meta.len(), media_type);
    }
    file(path, &shown, meta.len())
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

fn image(path: PathBuf, shown: &str, size: u64, media_type: &str) -> Result<Attachment> {
    if size > MAX_IMAGE_BYTES {
        let header = format!(
            "{TAG}path=\"{}\" kind=\"image\" size=\"{}\">",
            escape(shown),
            human_size(size)
        );
        let note = format!(
            "the image is {}, more than the {} limit",
            human_size(size),
            human_size(MAX_IMAGE_BYTES)
        );
        return Ok(Attachment {
            blocks: vec![Block::text(format!(
                "{header}\nNot attached: {note}.\n</attachment>"
            ))],
            path,
            kind: AttachmentKind::Image,
            size,
            note: Some(note),
        });
    }
    let data = fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(Attachment {
        blocks: vec![
            Block::text(image_header(shown, size)),
            Block::Image {
                media_type: media_type.to_owned(),
                data: BASE64.encode(data),
            },
        ],
        path,
        kind: AttachmentKind::Image,
        size,
        note: None,
    })
}

fn file(path: PathBuf, shown: &str, size: u64) -> Result<Attachment> {
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .and_then(|f| f.take(MAX_TEXT_BYTES as u64).read_to_end(&mut bytes))
        .with_context(|| format!("cannot read {}", path.display()))?;
    let Some(text) = decode_text(&bytes) else {
        let note = format!("binary file ({}), contents not included", human_size(size));
        return Ok(Attachment {
            blocks: vec![Block::text(format!(
                "{TAG}path=\"{}\" kind=\"binary\" size=\"{}\">\nThis is a {note}. \
                 Inspect it with tools if needed.\n</attachment>",
                escape(shown),
                human_size(size)
            ))],
            path,
            kind: AttachmentKind::Binary,
            size,
            note: Some(note),
        });
    };
    let note = (size > bytes.len() as u64).then(|| {
        format!(
            "truncated to the first {} of {}",
            human_size(MAX_TEXT_BYTES as u64),
            human_size(size)
        )
    });
    let fence = fence(text);
    let lang = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let mut body = format!(
        "{TAG}path=\"{}\" size=\"{}\">\n{fence}{lang}\n{text}",
        escape(shown),
        human_size(size)
    );
    if !text.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(&fence);
    if let Some(note) = &note {
        body.push_str(&format!("\n[{note}]"));
    }
    body.push_str("\n</attachment>");
    Ok(Attachment {
        blocks: vec![Block::text(body)],
        path,
        kind: AttachmentKind::Text,
        size,
        note,
    })
}

/// UTF-8 text without NUL bytes; a multi-byte character cut off at the end
/// of a truncated read is dropped.
fn decode_text(bytes: &[u8]) -> Option<&str> {
    if bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0) {
        return None;
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(err) if err.error_len().is_none() => {
            std::str::from_utf8(&bytes[..err.valid_up_to()]).ok()
        }
        Err(_) => None,
    }
}

fn directory(path: PathBuf, shown: &str) -> Result<Attachment> {
    let mut entries: Vec<(bool, String)> = fs::read_dir(&path)
        .with_context(|| format!("cannot list {}", path.display()))?
        .flatten()
        .map(|e| {
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            (!is_dir, e.file_name().to_string_lossy().into_owned())
        })
        .collect();
    entries.sort();
    let total = entries.len();
    let mut lines: Vec<String> = entries
        .iter()
        .take(MAX_LISTED)
        .map(|(is_file, name)| {
            if *is_file {
                name.clone()
            } else {
                format!("{name}/")
            }
        })
        .collect();
    let note = (total > MAX_LISTED).then(|| format!("showing {MAX_LISTED} of {total} entries"));
    if total > MAX_LISTED {
        lines.push(format!("... {} more", total - MAX_LISTED));
    }
    if lines.is_empty() {
        lines.push("(empty directory)".into());
    }
    Ok(Attachment {
        blocks: vec![Block::text(format!(
            "{TAG}path=\"{}\" kind=\"directory\" entries=\"{total}\">\n{}\n</attachment>",
            escape(shown),
            lines.join("\n")
        ))],
        path,
        kind: AttachmentKind::Directory,
        size: total as u64,
        note,
    })
}

/// A backtick fence longer than any run of backticks in `text`.
fn fence(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat((longest + 1).max(3))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('"', "&quot;")
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(attachment: &Attachment) -> &str {
        match &attachment.blocks[0] {
            Block::Text { text } => text,
            other => panic!("text block expected, got {other:?}"),
        }
    }

    #[test]
    fn text_files_become_fenced_blocks() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
        let a = attach(Path::new("main.rs"), dir.path()).unwrap();
        assert_eq!(a.kind, AttachmentKind::Text);
        assert!(a.note.is_none());
        let text = text_of(&a);
        assert!(is_attachment(text));
        assert_eq!(attached_path(text).as_deref(), Some("main.rs"));
        assert!(text.contains("```rs\nfn main() {}\n```"));
    }

    #[test]
    fn fences_outgrow_backticks_in_the_content() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("README.md"), "```sh\nls\n```\n").unwrap();
        let a = attach(&dir.path().join("README.md"), dir.path()).unwrap();
        assert!(text_of(&a).contains("````md\n```sh"));
    }

    #[test]
    fn long_text_is_truncated_with_a_note() {
        let dir = tempfile::tempdir().unwrap();
        let line = "é".repeat(100) + "\n";
        fs::write(dir.path().join("big.txt"), line.repeat(3000)).unwrap();
        let a = attach(Path::new("big.txt"), dir.path()).unwrap();
        assert_eq!(a.kind, AttachmentKind::Text);
        assert!(a.note.as_deref().unwrap().starts_with("truncated"));
        assert!(text_of(&a).len() <= MAX_TEXT_BYTES + 200);
    }

    #[test]
    fn images_become_image_blocks() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("shot.PNG"), b"\x89PNG\r\n\x1a\nfake").unwrap();
        let a = attach(Path::new("shot.PNG"), dir.path()).unwrap();
        assert_eq!(a.kind, AttachmentKind::Image);
        assert!(matches!(
            &a.blocks[1],
            Block::Image { media_type, .. } if media_type == "image/png"
        ));
    }

    #[test]
    fn directories_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        let a = attach(dir.path(), dir.path()).unwrap();
        assert_eq!(a.kind, AttachmentKind::Directory);
        assert_eq!(a.size, 2);
        let text = text_of(&a);
        assert!(text.contains("path=\".\""));
        assert!(text.contains("src/\nCargo.toml"), "{text}");
    }

    #[test]
    fn binaries_get_a_note() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.bin"), [0u8, 1, 2, 3, 255]).unwrap();
        let a = attach(Path::new("a.bin"), dir.path()).unwrap();
        assert_eq!(a.kind, AttachmentKind::Binary);
        assert!(text_of(&a).contains("binary file (5 B)"));
    }

    #[test]
    fn missing_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = attach(Path::new("nope.txt"), dir.path()).unwrap_err();
        assert!(format!("{err:#}").contains("cannot attach"));
    }

    #[test]
    fn resolves_relative_and_home_paths() {
        let cwd = Path::new("/work");
        assert_eq!(resolve("a/b.txt", cwd), PathBuf::from("/work/a/b.txt"));
        assert_eq!(resolve("/etc/hosts", cwd), PathBuf::from("/etc/hosts"));
        if let Some(home) = dirs::home_dir() {
            assert_eq!(resolve("~/x", cwd), home.join("x"));
        }
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }
}
