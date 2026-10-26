//! The files under a folder that `.gitignore` keeps, for `@` pickers: the
//! terminal's own, and `GET /api/files` for clients of a daemon on another
//! machine.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Indexing stops here, so a home directory cannot stall a picker.
pub const MAX_FILES: usize = 50_000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    /// Relative to the folder, sorted.
    pub files: Vec<String>,
    /// More files were there than [`MAX_FILES`].
    pub truncated: bool,
}

/// Files under `root`, relative to it. Hidden and ignored files are left
/// out, `.gitignore` honored inside and outside repositories.
pub fn index(root: &Path) -> Index {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .require_git(false)
        .build();
    let mut files: Vec<String> = walker
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| {
            let relative = e.path().strip_prefix(root).ok()?;
            Some(relative.to_string_lossy().into_owned())
        })
        .take(MAX_FILES + 1)
        .collect();
    let truncated = files.len() > MAX_FILES;
    files.truncate(MAX_FILES);
    files.sort();
    Index { files, truncated }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_what_gitignore_keeps() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "").unwrap();
        std::fs::write(dir.join("target/out.o"), "").unwrap();
        std::fs::write(dir.join(".env"), "").unwrap();
        let index = index(dir);
        assert_eq!(index.files, ["src/lib.rs"]);
        assert!(!index.truncated);
    }
}
