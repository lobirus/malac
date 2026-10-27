//! Files an agent sent to the user with `send_to_user`. Each is copied to
//! `~/.malac/shared/<id>/<name>`, so it still shows after the original
//! changes, and clients fetch it by id. The tool result names each file in
//! a `<shared …/>` tag, which clients turn into an image or a download.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::config;

/// Shared files are removed after this long.
const KEEP: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SharedFile {
    pub id: String,
    pub name: String,
    pub media_type: String,
    pub size: u64,
}

pub fn dir() -> PathBuf {
    config::home_dir().join("shared")
}

/// Keeps `bytes` as a file called `name` (only its last component).
pub fn save(name: &str, bytes: &[u8]) -> Result<SharedFile> {
    prune();
    let name = clean_name(name);
    let id = uuid::Uuid::now_v7().simple().to_string();
    let folder = dir().join(&id);
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("cannot create {}", folder.display()))?;
    std::fs::write(folder.join(&name), bytes).context("cannot save the shared file")?;
    Ok(SharedFile {
        id,
        media_type: media_type(&name).to_owned(),
        name,
        size: bytes.len() as u64,
    })
}

/// The file shared under `id`, and where it is.
pub fn open(id: &str) -> Result<(PathBuf, SharedFile)> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("invalid shared file id");
    }
    let folder = dir().join(id);
    let entry = std::fs::read_dir(&folder)
        .ok()
        .and_then(|mut entries| entries.find_map(|e| e.ok().filter(|e| e.path().is_file())))
        .context("no such shared file")?;
    let name = entry.file_name().to_string_lossy().into_owned();
    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
    Ok((
        entry.path(),
        SharedFile {
            id: id.to_owned(),
            media_type: media_type(&name).to_owned(),
            name,
            size,
        },
    ))
}

impl SharedFile {
    pub fn is_image(&self) -> bool {
        self.media_type.starts_with("image/")
    }

    /// The tag clients look for in a tool result.
    pub fn tag(&self) -> String {
        format!(
            r#"<shared id="{}" name="{}" type="{}" size="{}"/>"#,
            self.id, self.name, self.media_type, self.size
        )
    }
}

/// The `<shared …/>` tags in a tool result.
pub fn parse_tags(text: &str) -> Vec<SharedFile> {
    let mut files = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<shared ") {
        let tail = &rest[start..];
        let Some(end) = tail.find("/>") else { break };
        let tag = &tail[..end];
        let attr = |key: &str| {
            let marker = format!(r#"{key}=""#);
            let from = tag.find(&marker)? + marker.len();
            let to = tag[from..].find('"')?;
            Some(tag[from..from + to].to_owned())
        };
        if let (Some(id), Some(name)) = (attr("id"), attr("name")) {
            files.push(SharedFile {
                id,
                name,
                media_type: attr("type").unwrap_or_else(|| "application/octet-stream".into()),
                size: attr("size").and_then(|s| s.parse().ok()).unwrap_or(0),
            });
        }
        rest = &tail[end..];
    }
    files
}

/// A file name that is safe on disk and inside the tag.
fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '"' | '<' | '>' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.');
    if cleaned.is_empty() {
        "file".to_owned()
    } else {
        cleaned.to_owned()
    }
}

pub fn media_type(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// The extension for an image block's media type.
pub fn image_extension(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "jpg",
    }
}

/// Removes shared files older than [`KEEP`].
fn prune() {
    let Ok(entries) = std::fs::read_dir(dir()) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|modified| now.duration_since(modified).unwrap_or_default() > KEEP);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_round_trip() {
        let file = SharedFile {
            id: "0193ab".into(),
            name: "shot 1.png".into(),
            media_type: "image/png".into(),
            size: 2048,
        };
        let text = format!("Sent to the user.\n{}\n{}", file.tag(), file.tag());
        assert_eq!(parse_tags(&text), vec![file.clone(), file]);
        assert!(parse_tags("<shared broken").is_empty());
    }

    #[test]
    fn names_stay_inside_their_folder() {
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name("a\"b<c>.txt"), "a_b_c_.txt");
        assert_eq!(clean_name(".."), "file");
        assert_eq!(clean_name("C:\\shots\\x.png"), "x.png");
        assert!(open("../x").is_err());
    }
}
