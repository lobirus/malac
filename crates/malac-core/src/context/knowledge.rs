//! `.malac/MALAC.md`: what malac has learned about a project, kept in the
//! repository so it travels to other machines and teammates.
//!
//! Each piece of knowledge is a project memory, one file in
//! `.malac/memory/`. MALAC.md is their readable index, grouped by kind
//! (commands, conventions, gotchas, decisions, notes) between two markers
//! that malac rewrites. Everything outside the markers is the user's.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::memory::Entry;

pub const FILE: &str = "MALAC.md";
const START: &str =
    "<!-- malac:memories (generated from memory/; edit those files or ask malac) -->";
const END: &str = "<!-- /malac:memories -->";
/// Bodies up to this long are shown inline.
const INLINE_BODY: usize = 400;

/// Memory kinds and their section titles, in order.
pub const KINDS: &[(&str, &str)] = &[
    ("command", "Commands"),
    ("convention", "Conventions"),
    ("gotcha", "Gotchas"),
    ("decision", "Decisions"),
    ("note", "Notes"),
];

const TITLE: &str = "# Project knowledge";
const INTRO: &str = "What malac has learned about this project: how to build, test and run it, its conventions, \
the gotchas, and the decisions behind them. It is committed, so it travels with the repository. \
Each entry is a file in `memory/`; malac rewrites the list between the markers when it saves \
or forgets one. Text outside the markers is yours and is kept.";

pub fn path(project_root: &Path) -> PathBuf {
    project_root.join(".malac").join(FILE)
}

pub fn read(project_root: &Path) -> Option<String> {
    std::fs::read_to_string(path(project_root))
        .ok()
        .filter(|t| !t.trim().is_empty())
}

/// The file without what malac writes for human readers (its title,
/// introduction and markers), for the system prompt.
pub fn for_prompt(text: &str) -> String {
    let text = text.trim_start();
    let text = text.strip_prefix(TITLE).unwrap_or(text);
    let text = text.replace(INTRO, "").replace(START, "").replace(END, "");
    let mut out = String::new();
    // Removing parts leaves runs of blank lines.
    for line in text.trim().lines() {
        if !(line.trim().is_empty() && out.ends_with("\n\n")) {
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out.trim().to_owned()
}

/// Rewrites the generated part of `<malac_dir>/MALAC.md` from `entries`,
/// creating the file when there is something to list.
pub fn write(malac_dir: &Path, entries: &[Entry]) -> Result<()> {
    let path = malac_dir.join(FILE);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.is_empty() && entries.is_empty() {
        return Ok(());
    }
    let generated = format!("{START}\n{}{END}", render(entries));
    let text = match (existing.find(START), existing.find(END)) {
        (Some(start), Some(end)) if end > start => format!(
            "{}{generated}{}",
            &existing[..start],
            &existing[end + END.len()..]
        ),
        _ if existing.trim().is_empty() => format!("{TITLE}\n\n{INTRO}\n\n{generated}\n"),
        _ => format!("{}\n\n{generated}\n", existing.trim_end()),
    };
    std::fs::create_dir_all(malac_dir)
        .with_context(|| format!("cannot create {}", malac_dir.display()))?;
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))
}

fn render(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return "\n_Nothing saved yet._\n\n".into();
    }
    let mut out = String::new();
    let kind_of = |e: &Entry| {
        let kind = e.kind.as_deref().unwrap_or("note");
        if KINDS.iter().any(|(k, _)| *k == kind) {
            kind.to_owned()
        } else {
            "note".to_owned()
        }
    };
    for (kind, title) in KINDS {
        let group: Vec<&Entry> = entries.iter().filter(|e| kind_of(e) == *kind).collect();
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!("\n## {title}\n\n"));
        for entry in group {
            let file = format!("memory/{}.md", entry.name);
            out.push_str(&format!("- **{}** — {}", entry.name, entry.description));
            let body = entry.body.trim();
            if body.len() <= INLINE_BODY && body != entry.description.trim() && !body.is_empty() {
                out.push('\n');
                for line in body.lines() {
                    if line.trim().is_empty() {
                        out.push('\n');
                    } else {
                        out.push_str(&format!("  {line}\n"));
                    }
                }
            } else if body.len() > INLINE_BODY {
                out.push_str(&format!(" ([details]({file}))\n"));
            } else {
                out.push('\n');
            }
        }
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Scope;

    fn entry(name: &str, kind: Option<&str>, body: &str) -> Entry {
        Entry {
            name: name.into(),
            description: format!("about {name}"),
            kind: kind.map(str::to_owned),
            scope: Scope::Project,
            body: body.into(),
            path: PathBuf::from(format!("{name}.md")),
        }
    }

    #[test]
    fn keeps_user_text_around_the_generated_list() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &[]).unwrap();
        assert!(!dir.path().join(FILE).exists(), "nothing to write");

        let entries = vec![
            entry("build", Some("command"), "Run `make release`."),
            entry("tabs", Some("convention"), "Indent with tabs."),
            entry("odd", Some("whatever"), &"x".repeat(500)),
        ];
        write(dir.path(), &entries).unwrap();
        let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(text.starts_with("# Project knowledge"));
        assert!(text.contains("## Commands\n\n- **build** — about build\n  Run `make release`.\n"));
        assert!(text.contains("## Conventions"));
        assert!(text.contains("## Notes\n\n- **odd** — about odd ([details](memory/odd.md))"));

        let edited = text.replace(
            "# Project knowledge",
            "# Project knowledge\n\nAsk Ana about deploys.",
        );
        let edited = format!("{edited}\nFooter stays.\n");
        std::fs::write(dir.path().join(FILE), edited).unwrap();
        write(dir.path(), &entries[..1]).unwrap();
        let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(text.contains("Ask Ana about deploys."));
        let prompt = for_prompt(&text);
        assert!(
            prompt.starts_with("Ask Ana about deploys.\n\n## Commands"),
            "{prompt}"
        );
        assert!(!prompt.contains("<!--"));
        assert!(text.contains("Footer stays."));
        assert!(!text.contains("## Conventions"));
        assert_eq!(text.matches(START).count(), 1);
    }
}
