//! Discovers instruction files, agent types and skills on disk.

use std::path::{Path, PathBuf};

use crate::memory::split_frontmatter;

/// Files read as standing instructions, in priority order.
const INSTRUCTION_FILES: &[&str] = &["AGENTS.md", "MALAC.md", "CLAUDE.md"];
const MAX_INSTRUCTION_CHARS: usize = 30_000;

#[derive(Debug, Clone)]
pub struct Instructions {
    pub path: PathBuf,
    pub text: String,
}

/// User-wide instructions, then one file per directory from the project root down to `cwd`.
pub fn instructions(home: &Path, project_root: &Path, cwd: &Path) -> Vec<Instructions> {
    let mut dirs = vec![home.to_path_buf()];
    let mut chain: Vec<&Path> = cwd
        .ancestors()
        .take_while(|d| d.starts_with(project_root))
        .collect();
    chain.reverse();
    dirs.extend(chain.into_iter().map(Path::to_path_buf));

    dirs.iter()
        .filter_map(|dir| {
            INSTRUCTION_FILES.iter().find_map(|name| {
                let path = dir.join(name);
                let text = std::fs::read_to_string(&path).ok()?;
                Some(Instructions {
                    path,
                    text: crate::tools::truncate_middle(text.trim(), MAX_INSTRUCTION_CHARS),
                })
            })
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct AgentType {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
    /// Tool names this agent may use; `None` means all.
    pub tools: Option<Vec<String>>,
    pub instructions: String,
}

/// `*.md` files in `~/.malac/agents` and `<project>/.malac/agents`; project files win.
pub fn agent_types(home: &Path, project_root: &Path) -> Vec<AgentType> {
    let mut types: Vec<AgentType> = Vec::new();
    for dir in [
        home.join("agents"),
        project_root.join(".malac").join("agents"),
    ] {
        for path in markdown_files(&dir) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let (front, body) = split_frontmatter(&text);
            let name = field(front, "name").unwrap_or_else(|| stem(&path));
            types.retain(|t| t.name != name);
            types.push(AgentType {
                description: field(front, "description").unwrap_or_default(),
                model: field(front, "model"),
                tools: field(front, "tools").map(|t| {
                    t.split(',')
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .collect()
                }),
                instructions: body.trim().to_owned(),
                name,
            });
        }
    }
    types
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

/// `<dir>/<skill>/SKILL.md` under the malac and Claude skill folders, user then project.
pub fn skills(home: &Path, project_root: &Path) -> Vec<Skill> {
    let user_home = dirs::home_dir().unwrap_or_default();
    let roots = [
        home.join("skills"),
        user_home.join(".claude").join("skills"),
        project_root.join(".malac").join("skills"),
        project_root.join(".claude").join("skills"),
    ];
    let mut skills: Vec<Skill> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path().join("SKILL.md");
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let (front, _) = split_frontmatter(&text);
            let name = field(front, "name")
                .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned());
            skills.retain(|s| s.name != name);
            skills.push(Skill {
                description: field(front, "description").unwrap_or_default(),
                name,
                path,
            });
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    files
}

fn field(front: &str, key: &str) -> Option<String> {
    front.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        let v = v.trim().trim_matches('"');
        (k.trim() == key && !v.is_empty()).then(|| v.to_owned())
    })
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_agents_and_instructions() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let agents = project.path().join(".malac/agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews diffs\ntools: read, grep\n---\nBe strict.",
        )
        .unwrap();
        std::fs::write(project.path().join("AGENTS.md"), "Use tabs.").unwrap();
        let sub = project.path().join("crates/a");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("CLAUDE.md"), "Crate rules.").unwrap();

        let types = agent_types(home.path(), project.path());
        assert_eq!(types.len(), 1);
        assert_eq!(
            types[0].tools.as_deref(),
            Some(&["read".to_owned(), "grep".to_owned()][..])
        );
        assert_eq!(types[0].instructions, "Be strict.");

        let found = instructions(home.path(), project.path(), &sub);
        let texts: Vec<&str> = found.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(texts, vec!["Use tabs.", "Crate rules."]);
    }
}
