//! Standing instructions: AGENTS.md and the files other coding agents read
//! (CLAUDE.md, GEMINI.md, Copilot, Cursor, Windsurf and Cline rules), from
//! the user's home and from every directory between the project root and
//! the working directory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::frontmatter;
use crate::tools::truncate_middle;

/// Files larger than this are not instructions.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// A file cut below this many characters is skipped instead.
const MIN_USEFUL_CHARS: usize = 1000;
/// `@path` imports followed this deep.
const MAX_IMPORT_DEPTH: usize = 3;

/// Where to look.
pub struct Sources<'a> {
    pub malac_home: &'a Path,
    /// The user's home directory, for `~/.claude/CLAUDE.md`.
    pub user_home: Option<&'a Path>,
    pub project_root: &'a Path,
    pub cwd: &'a Path,
    /// `[context] instruction_files`.
    pub extra: &'a [String],
    pub max_chars: usize,
}

/// A file whose text is part of the system prompt.
#[derive(Debug, Clone, Serialize)]
pub struct Instructions {
    pub path: PathBuf,
    /// What kind of file it is, e.g. `AGENTS.md` or `Cursor rule`.
    pub kind: String,
    #[serde(skip)]
    pub text: String,
    /// Characters before any cut.
    pub chars: usize,
    pub truncated: bool,
}

/// A rule that applies to some files only, or that the agent reads when
/// its description fits: listed in the prompt, not pasted.
#[derive(Debug, Clone, Serialize)]
pub struct Rule {
    pub path: PathBuf,
    pub kind: String,
    pub description: String,
    pub globs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Discovered {
    /// In prompt order: user files, then the project root down to the
    /// working directory, then configured files.
    pub loaded: Vec<Instructions>,
    pub rules: Vec<Rule>,
    pub skipped: Vec<Skipped>,
}

struct Candidate {
    path: PathBuf,
    kind: &'static str,
    /// Lower keeps its text first when the budget runs out.
    priority: u8,
    /// 0 for user files, then one per directory below the project root.
    depth: usize,
    text: String,
}

/// Plain instruction files read in every directory, primary first.
const MARKDOWN: &[(&str, &str, u8)] = &[
    ("AGENTS.md", "AGENTS.md", 1),
    ("MALAC.md", "MALAC.md", 2),
    ("CLAUDE.md", "CLAUDE.md", 3),
    ("CLAUDE.local.md", "CLAUDE.local.md", 3),
    ("GEMINI.md", "GEMINI.md", 4),
    (".github/copilot-instructions.md", "Copilot instructions", 5),
    (".cursorrules", ".cursorrules", 6),
    (".windsurfrules", ".windsurfrules", 7),
];

pub fn discover(sources: &Sources<'_>) -> Discovered {
    let mut found = Discovered::default();
    let mut candidates = Vec::new();
    let mut seen_paths = HashSet::new();

    for name in ["AGENTS.md", "MALAC.md", "CLAUDE.md"] {
        let path = sources.malac_home.join(name);
        let (kind, priority) = kind_of(name);
        push_file(
            &mut candidates,
            &mut found,
            path,
            kind,
            priority,
            0,
            sources.malac_home,
        );
    }
    if let Some(home) = sources.user_home {
        let claude = home.join(".claude");
        push_file(
            &mut candidates,
            &mut found,
            claude.join("CLAUDE.md"),
            "CLAUDE.md",
            3,
            0,
            &claude,
        );
    }

    for (depth, dir) in directory_chain(sources.project_root, sources.cwd)
        .iter()
        .enumerate()
    {
        let depth = depth + 1;
        for (name, kind, priority) in MARKDOWN {
            push_file(
                &mut candidates,
                &mut found,
                dir.join(name),
                kind,
                *priority,
                depth,
                sources.project_root,
            );
        }
        push_rule_dir(
            &mut candidates,
            &mut found,
            &dir.join(".cursor/rules"),
            "mdc",
            "Cursor rule",
            6,
            depth,
        );
        push_rule_dir(
            &mut candidates,
            &mut found,
            &dir.join(".windsurf/rules"),
            "md",
            "Windsurf rule",
            7,
            depth,
        );
        push_rule_dir(
            &mut candidates,
            &mut found,
            &dir.join(".github/instructions"),
            "md",
            "Copilot instructions",
            5,
            depth,
        );
        let cline = dir.join(".clinerules");
        if cline.is_dir() {
            push_rule_dir(
                &mut candidates,
                &mut found,
                &cline,
                "md",
                "Cline rule",
                7,
                depth,
            );
        } else {
            push_file(
                &mut candidates,
                &mut found,
                cline,
                ".clinerules",
                7,
                depth,
                sources.project_root,
            );
        }
    }

    for entry in sources.extra {
        let path = resolve(entry, sources.project_root, sources.user_home);
        if !path.is_file() {
            found.skipped.push(Skipped {
                path,
                reason: "configured in [context] instruction_files but missing".into(),
            });
            continue;
        }
        let root = path.parent().unwrap_or(sources.project_root).to_path_buf();
        push_file(
            &mut candidates,
            &mut found,
            path,
            "configured",
            0,
            usize::MAX,
            &root,
        );
    }

    // The same file reached twice (a symlink, a configured path) or the
    // same text in two files counts once, where it first appears.
    let mut seen_texts: Vec<(String, PathBuf)> = Vec::new();
    let mut unique = Vec::new();
    for candidate in candidates {
        let key = std::fs::canonicalize(&candidate.path).unwrap_or_else(|_| candidate.path.clone());
        let text = candidate.text.replace("\r\n", "\n");
        let original = seen_texts
            .iter()
            .find(|(seen, _)| *seen == text)
            .map(|(_, p)| p);
        match original {
            Some(original) => found.skipped.push(Skipped {
                reason: format!("same text as {}", original.display()),
                path: candidate.path,
            }),
            None if !seen_paths.insert(key) => {}
            None => {
                seen_texts.push((text, candidate.path.clone()));
                unique.push(candidate);
            }
        }
    }

    allocate(unique, sources.max_chars, &mut found);
    found
}

/// The project root, then each directory down to `cwd`.
pub fn directory_chain(project_root: &Path, cwd: &Path) -> Vec<PathBuf> {
    if !cwd.starts_with(project_root) {
        return vec![cwd.to_path_buf()];
    }
    let mut chain: Vec<PathBuf> = cwd
        .ancestors()
        .take_while(|d| d.starts_with(project_root))
        .map(Path::to_path_buf)
        .collect();
    chain.reverse();
    chain
}

fn kind_of(name: &str) -> (&'static str, u8) {
    MARKDOWN
        .iter()
        .find(|(file, _, _)| *file == name)
        .map_or(("instructions", 8), |(_, kind, priority)| {
            (*kind, *priority)
        })
}

fn resolve(entry: &str, project_root: &Path, user_home: Option<&Path>) -> PathBuf {
    match (entry.strip_prefix("~/"), user_home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => project_root.join(entry),
    }
}

fn read(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

#[allow(clippy::too_many_arguments)]
fn push_file(
    candidates: &mut Vec<Candidate>,
    found: &mut Discovered,
    path: PathBuf,
    kind: &'static str,
    priority: u8,
    depth: usize,
    import_root: &Path,
) {
    let Some(text) = read(&path) else {
        return;
    };
    let text = expand_imports(&text, &path, import_root, 0);
    if text.trim().is_empty() {
        found.skipped.push(Skipped {
            path,
            reason: "empty".into(),
        });
        return;
    }
    candidates.push(Candidate {
        path,
        kind,
        priority,
        depth,
        text: text.trim().to_owned(),
    });
}

/// How a rule with frontmatter applies.
enum Activation {
    Always,
    Globs(Vec<String>),
    Requested(String),
    Manual,
}

fn activation(kind: &str, front: &frontmatter::Frontmatter) -> Activation {
    let globs = |key: &str| front.list(key, false).filter(|g| !g.is_empty());
    let description = front.string("description").unwrap_or_default();
    match kind {
        "Cursor rule" => {
            if front.bool("alwaysApply") == Some(true) {
                Activation::Always
            } else if let Some(globs) = globs("globs") {
                Activation::Globs(globs)
            } else if !description.is_empty() {
                Activation::Requested(description)
            } else {
                Activation::Manual
            }
        }
        "Windsurf rule" => match front.str("trigger").unwrap_or("always_on") {
            "always_on" => Activation::Always,
            "glob" => globs("globs").map_or(Activation::Manual, Activation::Globs),
            "model_decision" if !description.is_empty() => Activation::Requested(description),
            _ => Activation::Manual,
        },
        "Copilot instructions" => match globs("applyTo") {
            Some(globs) if globs.iter().all(|g| g == "**" || g == "**/*") => Activation::Always,
            Some(globs) => Activation::Globs(globs),
            None => Activation::Manual,
        },
        _ => globs("paths").map_or(Activation::Always, Activation::Globs),
    }
}

fn push_rule_dir(
    candidates: &mut Vec<Candidate>,
    found: &mut Discovered,
    dir: &Path,
    extension: &str,
    kind: &'static str,
    priority: u8,
    depth: usize,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == extension))
        .collect();
    files.sort();
    for path in files {
        let Some(text) = read(&path) else {
            continue;
        };
        let (front, body) = frontmatter::parse(&text);
        match activation(kind, &front) {
            Activation::Always if body.trim().is_empty() => {}
            Activation::Always => candidates.push(Candidate {
                path,
                kind,
                priority,
                depth,
                text: body.trim().to_owned(),
            }),
            Activation::Globs(globs) => found.rules.push(Rule {
                description: front.string("description").unwrap_or_default(),
                path,
                kind: kind.into(),
                globs,
            }),
            Activation::Requested(description) => found.rules.push(Rule {
                path,
                kind: kind.into(),
                description,
                globs: Vec::new(),
            }),
            Activation::Manual => found.skipped.push(Skipped {
                path,
                reason: "applied only when mentioned".into(),
            }),
        }
    }
}

/// Replaces lines that are only `@path` (CLAUDE.md and GEMINI.md imports)
/// with that file, if it is inside `root`.
fn expand_imports(text: &str, file: &Path, root: &Path, depth: usize) -> String {
    if depth >= MAX_IMPORT_DEPTH || !text.lines().any(|l| l.trim().starts_with('@')) {
        return text.to_owned();
    }
    let base = file.parent().unwrap_or(root);
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let target = line
            .trim()
            .strip_prefix('@')
            .filter(|t| !t.is_empty() && !t.contains(char::is_whitespace));
        let imported = target.and_then(|target| {
            let path = std::fs::canonicalize(base.join(target)).ok()?;
            if !path.starts_with(&root) || path == file {
                return None;
            }
            let text = read(&path)?;
            Some(expand_imports(&text, &path, &root, depth + 1))
        });
        match imported {
            Some(imported) => out.push_str(imported.trim_end()),
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// Fits the files into `max_chars`, keeping the most important ones whole.
fn allocate(mut candidates: Vec<Candidate>, max_chars: usize, found: &mut Discovered) {
    let order: Vec<PathBuf> = candidates.iter().map(|c| c.path.clone()).collect();
    candidates.sort_by_key(|c| (c.priority, std::cmp::Reverse(c.depth)));

    let mut remaining = max_chars;
    let mut loaded = Vec::new();
    for candidate in candidates {
        let chars = candidate.text.chars().count();
        let (text, truncated) = if candidate.text.len() <= remaining {
            (candidate.text, false)
        } else if remaining >= MIN_USEFUL_CHARS {
            (truncate_middle(&candidate.text, remaining), true)
        } else {
            found.skipped.push(Skipped {
                path: candidate.path,
                reason: format!(
                    "over the instruction budget ([context] max_instruction_chars = {max_chars})"
                ),
            });
            continue;
        };
        remaining = remaining.saturating_sub(text.len());
        loaded.push(Instructions {
            path: candidate.path,
            kind: candidate.kind.into(),
            text,
            chars,
            truncated,
        });
    }
    loaded.sort_by_key(|i| order.iter().position(|p| *p == i.path));
    found.loaded = loaded;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn discover_in(home: &Path, root: &Path, cwd: &Path, max_chars: usize) -> Discovered {
        discover(&Sources {
            malac_home: &home.join(".malac"),
            user_home: Some(home),
            project_root: root,
            cwd,
            extra: &["docs/agent-rules.md".into(), "missing.md".into()],
            max_chars,
        })
    }

    #[test]
    fn reads_every_form() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        write(&home.path().join(".malac/AGENTS.md"), "User rules.");
        write(&home.path().join(".claude/CLAUDE.md"), "Claude user rules.");
        write(&root.join("AGENTS.md"), "Use tabs.");
        write(&root.join("GEMINI.md"), "Gemini notes.");
        write(
            &root.join(".github/copilot-instructions.md"),
            "Copilot notes.",
        );
        write(&root.join(".cursorrules"), "Cursor legacy.");
        write(&root.join(".windsurfrules"), "Windsurf notes.");
        write(&root.join(".clinerules/style.md"), "Cline style.");
        write(
            &root.join(".cursor/rules/always.mdc"),
            "---\nalwaysApply: true\n---\nAlways rule.",
        );
        write(
            &root.join(".cursor/rules/react.mdc"),
            "---\ndescription: React conventions\nglobs: src/**/*.tsx\nalwaysApply: false\n---\nUse hooks.",
        );
        write(
            &root.join(".cursor/rules/manual.mdc"),
            "---\nalwaysApply: false\n---\nManual.",
        );
        write(&root.join("docs/agent-rules.md"), "Configured rules.");
        let sub = root.join("crates/a");
        write(&sub.join("CLAUDE.md"), "Crate rules.");

        let found = discover_in(home.path(), root, &sub, 40_000);
        let texts: Vec<&str> = found.loaded.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "User rules.",
                "Claude user rules.",
                "Use tabs.",
                "Gemini notes.",
                "Copilot notes.",
                "Cursor legacy.",
                "Windsurf notes.",
                "Always rule.",
                "Cline style.",
                "Crate rules.",
                "Configured rules.",
            ]
        );
        assert_eq!(found.rules.len(), 1);
        assert_eq!(found.rules[0].globs, vec!["src/**/*.tsx"]);
        assert_eq!(found.rules[0].description, "React conventions");
        let reasons: Vec<&str> = found.skipped.iter().map(|s| s.reason.as_str()).collect();
        assert!(reasons.contains(&"applied only when mentioned"));
        assert!(reasons.iter().any(|r| r.contains("missing")));
    }

    #[test]
    fn dedupes_symlinks_imports_and_copies() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        write(&root.join("AGENTS.md"), "Shared rules.\n");
        #[cfg(unix)]
        std::os::unix::fs::symlink("AGENTS.md", root.join("CLAUDE.md")).unwrap();
        write(&root.join("GEMINI.md"), "@AGENTS.md\n");
        write(&root.join("sub/AGENTS.md"), "Shared rules.");
        write(
            &root.join("sub/CLAUDE.md"),
            "Intro\n@../AGENTS.md\n@/etc/passwd\n",
        );

        let found = discover_in(home.path(), root, &root.join("sub"), 40_000);
        let texts: Vec<&str> = found.loaded.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["Shared rules.", "Intro\nShared rules.\n@/etc/passwd"]
        );
        assert!(
            found
                .skipped
                .iter()
                .any(|s| s.reason.starts_with("same text as"))
        );
    }

    #[test]
    fn keeps_primary_files_within_budget() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        write(&root.join("AGENTS.md"), &"a".repeat(3000));
        write(&root.join("CLAUDE.md"), &"c".repeat(3000));
        write(&root.join(".cursorrules"), &"r".repeat(3000));

        let found = discover_in(home.path(), root, root, 4500);
        assert_eq!(found.loaded.len(), 2);
        assert_eq!(found.loaded[0].kind, "AGENTS.md");
        assert!(!found.loaded[0].truncated);
        assert!(found.loaded[1].truncated);
        assert_eq!(found.loaded[1].kind, "CLAUDE.md");
        assert!(found.skipped.iter().any(|s| s.reason.contains("budget")));
    }
}
