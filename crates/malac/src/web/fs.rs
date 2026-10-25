//! `GET /api/fs?path=`: what a directory holds, for folder and file
//! pickers: its subdirectories, and with `files=true` its files too. Only
//! directories below `daemon.browse_root` (the home directory by default)
//! are listed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::AppState;

/// Files and directories that tell what kind of directory this is.
const MARKERS: &[&str] = &[
    ".git",
    ".malac",
    "AGENTS.md",
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
];
const MAX_ENTRIES: usize = 1000;

#[derive(Deserialize)]
pub(super) struct FsQuery {
    /// Absolute, `~/...`, or relative to the root. Defaults to the root.
    path: Option<String>,
    /// Include entries whose name starts with a dot.
    #[serde(default)]
    hidden: bool,
    /// List files as well as directories.
    #[serde(default)]
    files: bool,
}

#[derive(Debug, Serialize)]
struct Listing {
    path: PathBuf,
    /// `null` at the root.
    parent: Option<PathBuf>,
    root: PathBuf,
    /// Markers found in `path` itself.
    markers: Vec<&'static str>,
    entries: Vec<Entry>,
    truncated: bool,
}

#[derive(Debug, Serialize)]
struct Entry {
    name: String,
    path: PathBuf,
    kind: Kind,
    /// Bytes, for files.
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    /// For directories.
    markers: Vec<&'static str>,
}

/// Directories sort before files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Directory,
    File,
}

pub(super) async fn list(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FsQuery>,
) -> Response {
    let root = state.hub.options().browse_root.clone();
    let listing = tokio::task::spawn_blocking(move || {
        listing(&root, query.path.as_deref(), query.hidden, query.files)
    })
    .await;
    match listing {
        Ok(Ok(listing)) => Json(listing).into_response(),
        Ok(Err(err)) => {
            let status = if err.is::<OutsideRoot>() {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(json!({ "error": format!("{err:#}") }))).into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Debug)]
struct OutsideRoot(PathBuf);

impl std::fmt::Display for OutsideRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is outside the directories malac may list",
            self.0.display()
        )
    }
}

impl std::error::Error for OutsideRoot {}

fn listing(root: &Path, path: Option<&str>, hidden: bool, files: bool) -> Result<Listing> {
    let root = root
        .canonicalize()
        .with_context(|| format!("cannot open {}", root.display()))?;
    let requested = match path {
        Some(path) if !path.is_empty() => malac_core::attach::resolve(path, &root),
        _ => root.clone(),
    };
    let path = requested
        .canonicalize()
        .with_context(|| format!("cannot open {}", requested.display()))?;
    if !path.starts_with(&root) {
        return Err(OutsideRoot(path).into());
    }
    if !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }
    let mut entries: Vec<Entry> = std::fs::read_dir(&path)
        .with_context(|| format!("cannot list {}", path.display()))?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                return None;
            }
            let full = path.join(&name);
            // Symbolic links count as what they point to.
            let meta = std::fs::metadata(&full).ok()?;
            if meta.is_dir() {
                Some(Entry {
                    markers: markers(&full),
                    path: full,
                    name,
                    kind: Kind::Directory,
                    size: None,
                })
            } else if files && meta.is_file() {
                Some(Entry {
                    path: full,
                    name,
                    kind: Kind::File,
                    size: Some(meta.len()),
                    markers: Vec::new(),
                })
            } else {
                None
            }
        })
        .collect();
    entries.sort_by_key(|e| (e.kind, e.name.to_lowercase()));
    let truncated = entries.len() > MAX_ENTRIES;
    entries.truncate(MAX_ENTRIES);
    Ok(Listing {
        parent: (path != root)
            .then(|| path.parent().map(Path::to_path_buf))
            .flatten(),
        markers: markers(&path),
        root,
        path,
        entries,
        truncated,
    })
}

fn markers(dir: &Path) -> Vec<&'static str> {
    MARKERS
        .iter()
        .copied()
        .filter(|m| dir.join(m).exists())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_subdirectories_with_markers() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("app");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::write(project.join("Cargo.toml"), "").unwrap();
        std::fs::create_dir(root.path().join("Notes")).unwrap();
        std::fs::create_dir(root.path().join(".cache")).unwrap();
        std::fs::write(root.path().join("file.txt"), "").unwrap();

        let top = listing(root.path(), None, false, false).unwrap();
        let names: Vec<&str> = top.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["app", "Notes"]);
        assert_eq!(top.entries[0].markers, [".git", "Cargo.toml"]);
        assert!(top.parent.is_none());
        assert_eq!(
            listing(root.path(), None, true, false)
                .unwrap()
                .entries
                .len(),
            3
        );

        let with_files = listing(root.path(), None, false, true).unwrap();
        let names: Vec<&str> = with_files.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["app", "Notes", "file.txt"], "folders first");
        let file = with_files.entries.last().unwrap();
        assert_eq!(file.kind, Kind::File);
        assert_eq!(file.size, Some(0));
        assert!(with_files.entries[0].size.is_none());

        let inner = listing(root.path(), Some("app"), false, false).unwrap();
        assert_eq!(inner.markers, [".git", "Cargo.toml"]);
        assert_eq!(inner.parent.as_deref(), Some(top.path.as_path()));
    }

    #[test]
    fn refuses_paths_outside_the_root() {
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("inside");
        std::fs::create_dir(&inside).unwrap();
        let err = listing(&inside, Some(".."), false, true).unwrap_err();
        assert!(err.is::<OutsideRoot>());
        let err = listing(&inside, Some("/"), false, true).unwrap_err();
        assert!(err.is::<OutsideRoot>());
    }
}
