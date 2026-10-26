//! The project's Markdown docs as a map for the prompt: path, title and one
//! line each, so the agent knows where to look without reading them all.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::frontmatter;
use crate::tools::truncate_chars;

/// Names (without extension, upper-cased) of top-level docs worth listing.
const TOP_LEVEL: &[&str] = &[
    "README",
    "ARCHITECTURE",
    "CONTRIBUTING",
    "DESIGN",
    "DEVELOPMENT",
    "HACKING",
    "SECURITY",
    "CHANGELOG",
    "ROADMAP",
    "TESTING",
    "DEPLOYMENT",
    "RUNBOOK",
];
/// Directories whose Markdown files are docs.
const DOC_DIRS: &[&str] = &[
    "docs",
    "doc",
    "documentation",
    "adr",
    "adrs",
    "decisions",
    "rfcs",
];
/// Instruction files are loaded already.
const INSTRUCTION_NAMES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    "MALAC.md",
    "CLAUDE.local.md",
];
const MAX_DEPTH: usize = 4;
/// Markdown files looked at under one docs directory at most.
const MAX_WALKED: usize = 2000;
/// Decision records beyond this many are summarized as one entry.
const MAX_LISTED_RECORDS: usize = 8;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Doc {
    /// Relative to the project root.
    pub path: PathBuf,
    pub title: String,
    pub summary: String,
}

/// Docs at the project root, in each directory down to `cwd`, and under
/// their `docs/`-like directories, at most `limit`.
pub fn discover(project_root: &Path, cwd: &Path, limit: usize) -> Vec<Doc> {
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in super::instructions::directory_chain(project_root, cwd) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut here: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        here.sort();
        for path in &here {
            if path.is_file() && is_top_level_doc(path) {
                files.push(path.clone());
            }
        }
        for name in DOC_DIRS {
            let docs = dir.join(name);
            if docs.is_dir() {
                files.extend(markdown_under(&docs));
            }
        }
    }
    files.dedup();

    let (records, others): (Vec<PathBuf>, Vec<PathBuf>) =
        files.into_iter().partition(|p| is_decision_record(p));
    let mut docs: Vec<Doc> = others
        .iter()
        .take(limit)
        .filter_map(|path| describe(path, project_root))
        .collect();
    if records.len() > MAX_LISTED_RECORDS {
        let mut dirs: Vec<PathBuf> = records
            .iter()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect();
        dirs.dedup();
        for dir in dirs {
            let count = records.iter().filter(|p| p.parent() == Some(&dir)).count();
            docs.push(Doc {
                path: relative(&dir, project_root),
                title: "Decision records".into(),
                summary: format!("{count} architecture decision records"),
            });
        }
    } else {
        docs.extend(records.iter().filter_map(|p| describe(p, project_root)));
    }
    docs.truncate(limit);
    docs
}

fn is_top_level_doc(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if INSTRUCTION_NAMES.contains(&name) {
        return false;
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let is_markdown = matches!(ext.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdx");
    is_markdown && TOP_LEVEL.contains(&stem.to_ascii_uppercase().as_str())
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdx"))
}

fn markdown_under(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = ignore::WalkBuilder::new(dir)
        .max_depth(Some(MAX_DEPTH))
        .require_git(false)
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
        .filter(|p| is_markdown(p))
        .take(MAX_WALKED)
        .collect();
    files.sort();
    files
}

/// ADRs: files under an `adr`/`decisions` directory, or named like `0001-title.md`.
fn is_decision_record(path: &Path) -> bool {
    let in_adr_dir = path.components().any(|c| {
        let name = c.as_os_str().to_string_lossy().to_ascii_lowercase();
        matches!(name.as_str(), "adr" | "adrs" | "decisions")
    });
    let numbered = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.split_once('-'))
        .is_some_and(|(number, _)| number.len() >= 3 && number.chars().all(|c| c.is_ascii_digit()));
    in_adr_dir || numbered
}

fn relative(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn describe(path: &Path, root: &Path) -> Option<Doc> {
    let text = std::fs::read_to_string(path).ok()?;
    let (title, summary) = title_and_summary(&text);
    Some(Doc {
        path: relative(path, root),
        title: title.unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        }),
        summary,
    })
}

/// The first heading (or a frontmatter `title`) and the first sentence of
/// prose after it.
pub fn title_and_summary(text: &str) -> (Option<String>, String) {
    let (front, body) = frontmatter::parse(text);
    let mut title = front.string("title");
    let mut summary = front.string("description").unwrap_or_default();
    let mut in_code = false;
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        if in_code || line.is_empty() {
            continue;
        }
        if let Some(heading) = line.strip_prefix('#') {
            if title.is_none() {
                title = Some(heading.trim_start_matches('#').trim().to_owned());
            }
            continue;
        }
        let decorative = line.starts_with(['<', '!', '[', '|', '>', '-', '*', '='])
            && !line.starts_with("- ")
            && !line.starts_with("* ");
        if summary.is_empty() && !decorative {
            summary = first_sentence(line);
        }
        if title.is_some() && !summary.is_empty() {
            break;
        }
    }
    (title.filter(|t| !t.is_empty()), summary)
}

fn first_sentence(line: &str) -> String {
    let line = line.trim_start_matches(['-', '*', ' ']);
    let end = line
        .match_indices(". ")
        .map(|(i, _)| i + 1)
        .next()
        .unwrap_or(line.len());
    truncate_chars(&line[..end], 160)
}

/// Lines for the prompt's docs map.
pub fn render(docs: &[Doc]) -> String {
    docs.iter()
        .map(|d| {
            let summary = if d.summary.is_empty() || d.summary == d.title {
                String::new()
            } else {
                format!(": {}", d.summary)
            };
            format!("- {} — {}{summary}", d.path.display(), d.title)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn maps_docs_without_reading_them_into_the_prompt() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        write(
            &root.join("README.md"),
            "# Widget\n\n[![ci](badge.svg)](x)\n\nWidget renders widgets. It is fast.\n",
        );
        write(&root.join("AGENTS.md"), "# Rules\n");
        write(
            &root.join("docs/guide/setup.md"),
            "---\ntitle: Setup\n---\nInstall it with `make`.\n",
        );
        for n in 1..=10 {
            write(
                &root.join(format!("docs/adr/{n:04}-choice.md")),
                "# Choice\n\nWe chose.\n",
            );
        }
        write(&root.join("app/README.md"), "# App\n\nThe frontend.\n");

        let docs = discover(root, &root.join("app"), 50);
        let rendered = render(&docs);
        assert!(rendered.contains("- README.md — Widget: Widget renders widgets."));
        assert!(rendered.contains("- docs/guide/setup.md — Setup: Install it with `make`."));
        assert!(
            rendered.contains("- docs/adr — Decision records: 10 architecture decision records")
        );
        assert!(rendered.contains("- app/README.md — App: The frontend."));
        assert!(!rendered.contains("AGENTS.md"));
    }
}
