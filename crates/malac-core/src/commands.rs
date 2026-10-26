//! Custom slash commands: Markdown prompts (Claude Code's format) and TOML
//! prompts (Gemini CLI's) from plugins and command folders, and skills run
//! as `/<skill>`. Typing `/name args` expands the command into the prompt
//! the agent receives.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::catalog::Skill;
use crate::frontmatter;
use crate::plugins::{CommandSource, Plugin};

/// A command as front ends list it, e.g. for completion.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    /// `user`, `project`, `plugin <id>`, or `skill`.
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct Command {
    pub info: CommandInfo,
    body: String,
    kind: Kind,
}

#[derive(Debug, Clone)]
enum Kind {
    Prompt { plugin: Option<(PathBuf, PathBuf)> },
    Skill(Box<Skill>),
}

/// What running a command produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Expansion {
    pub text: String,
    /// The skill's directory, for `$SKILL_DIR` in its scripts.
    pub skill_dir: Option<PathBuf>,
}

impl Command {
    pub fn name(&self) -> &str {
        &self.info.name
    }

    pub fn expand(&self, args: &str) -> Expansion {
        let args = args.trim();
        match &self.kind {
            Kind::Prompt { plugin } => {
                let mut text = arguments(&self.body, args);
                if let Some((root, data)) = plugin {
                    text = crate::catalog::substitute_plugin(&text, root, data);
                }
                Expansion {
                    text,
                    skill_dir: None,
                }
            }
            Kind::Skill(skill) => {
                let instructions = skill
                    .instructions()
                    .unwrap_or_else(|err| format!("(cannot read {}: {err})", skill.path.display()));
                let dir = skill.dir().display();
                let text = format!(
                    "Run the `{}` skill for this request. Its directory is {dir}; bundled scripts are under \
                     scripts/ there and run with bash (SKILL_DIR is set), references under references/.\n\n{}",
                    skill.name,
                    arguments(&instructions, args)
                );
                Expansion {
                    text,
                    skill_dir: Some(skill.dir().to_path_buf()),
                }
            }
        }
    }
}

/// Writes the arguments in for `$ARGUMENTS`, `{{args}}` and `$1`…`$9`, or
/// appends them when the text has no placeholder.
fn arguments(body: &str, args: &str) -> String {
    let positional = split_args(args);
    let has_placeholder = body.contains("$ARGUMENTS")
        || body.contains("{{args}}")
        || (1..=9).any(|n| body.contains(&format!("${n}")));
    let mut text = body.replace("$ARGUMENTS", args).replace("{{args}}", args);
    for n in (1..=9).rev() {
        let value = positional
            .get(n - 1)
            .map(String::as_str)
            .unwrap_or_default();
        text = text.replace(&format!("${n}"), value);
    }
    if !has_placeholder && !args.is_empty() {
        text.push_str(&format!("\n\nARGUMENTS: {args}"));
    }
    text.trim().to_owned()
}

/// Splits on whitespace, keeping quoted parts together.
fn split_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut started = false;
    for c in args.chars() {
        match (quote, c) {
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (Some(q), c) if c == q => quote = None,
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (_, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(current);
    }
    out
}

/// `/name args` from a prompt: the name has no `/` (so `/tmp/x` is a path).
pub fn parse_invocation(text: &str) -> Option<(&str, &str)> {
    let rest = text.trim_start().strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_:.".contains(c));
    valid.then_some((name, args.trim()))
}

/// The command `name` means: an exact match, else the only one whose part
/// after `plugin:` matches. `Err` lists the candidates when several do.
pub fn find<'a>(commands: &'a [Command], name: &str) -> Result<Option<&'a Command>, String> {
    if let Some(exact) = commands.iter().find(|c| c.name() == name) {
        return Ok(Some(exact));
    }
    let matches: Vec<&Command> = commands
        .iter()
        .filter(|c| c.name().rsplit(':').next() == Some(name))
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        many => Err(format!(
            "/{name} is ambiguous: {}",
            many.iter()
                .map(|c| format!("/{}", c.name()))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Plugin commands (`<plugin>:<name>`), then the user's and the project's
/// command folders (later wins), then every skill that may run as a command.
pub fn discover(
    home: &Path,
    user_home: Option<&Path>,
    project_root: &Path,
    plugins: &[Plugin],
    skills: &[Skill],
) -> Vec<Command> {
    fn add(commands: &mut Vec<Command>, command: Command) {
        commands.retain(|c| c.name() != command.name());
        commands.push(command);
    }
    let mut commands: Vec<Command> = Vec::new();
    for plugin in plugins {
        let source = format!("plugin {}", plugin.id);
        let paths = Some((plugin.root.clone(), plugin.data.clone()));
        for item in &plugin.components.commands {
            let command = match item {
                CommandSource::File(path) => read(path, None, &source, paths.clone()),
                CommandSource::Inline {
                    name,
                    content,
                    description,
                    argument_hint,
                } => Some(prompt(
                    name,
                    content,
                    description.clone(),
                    argument_hint.clone(),
                    &source,
                    paths.clone(),
                )),
            };
            if let Some(mut command) = command {
                command.info.name = format!("{}:{}", plugin.name, command.info.name);
                add(&mut commands, command);
            }
        }
    }
    let mut folders = vec![(home.join("commands"), "user")];
    if let Some(user_home) = user_home {
        folders.push((user_home.join(".claude/commands"), "user"));
        folders.push((user_home.join(".gemini/commands"), "user"));
    }
    folders.extend([
        (project_root.join(".claude/commands"), "project"),
        (project_root.join(".gemini/commands"), "project"),
        (project_root.join(".malac/commands"), "project"),
    ]);
    for (dir, source) in folders {
        for path in command_files(&dir) {
            if let Some(command) = read(&path, Some(&dir), source, None) {
                add(&mut commands, command);
            }
        }
    }
    for skill in skills.iter().filter(|s| s.user_invocable) {
        if commands.iter().any(|c| c.name() == skill.name) {
            continue;
        }
        commands.push(Command {
            info: CommandInfo {
                name: skill.name.clone(),
                description: skill.description.clone(),
                argument_hint: skill.argument_hint.clone(),
                source: "skill".into(),
            },
            body: String::new(),
            kind: Kind::Skill(Box::new(skill.clone())),
        });
    }
    commands.sort_by(|a, b| a.name().cmp(b.name()));
    commands
}

fn command_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = ignore::WalkBuilder::new(dir)
        .max_depth(Some(3))
        .require_git(false)
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
        .filter(|p| p.extension().is_some_and(|e| e == "md" || e == "toml"))
        .collect();
    files.sort();
    files
}

fn prompt(
    name: &str,
    body: &str,
    description: Option<String>,
    argument_hint: Option<String>,
    source: &str,
    plugin: Option<(PathBuf, PathBuf)>,
) -> Command {
    let description = description.unwrap_or_else(|| {
        body.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| crate::tools::truncate_chars(l.trim_start_matches('#').trim(), 80))
            .unwrap_or_default()
    });
    Command {
        info: CommandInfo {
            name: name.to_owned(),
            description,
            argument_hint,
            source: source.to_owned(),
        },
        body: body.trim().to_owned(),
        kind: Kind::Prompt { plugin },
    }
}

/// A `.md` command is named after its file; a `.toml` one after its path
/// below `base`, folders joined with `:` (Gemini CLI's convention).
fn read(
    path: &Path,
    base: Option<&Path>,
    source: &str,
    plugin: Option<(PathBuf, PathBuf)>,
) -> Option<Command> {
    let text = std::fs::read_to_string(path).ok()?;
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    if path.extension().is_some_and(|e| e == "toml") {
        let table: toml::Table = text.parse().ok()?;
        let body = table.get("prompt")?.as_str()?;
        let name = match base.and_then(|b| {
            path.with_extension("")
                .strip_prefix(b)
                .ok()
                .map(Path::to_path_buf)
        }) {
            Some(rel) => rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(":"),
            None => stem,
        };
        let description = table
            .get("description")
            .and_then(|d| d.as_str())
            .map(str::to_owned);
        return Some(prompt(&name, body, description, None, source, plugin));
    }
    let (front, body) = frontmatter::parse(&text);
    Some(prompt(
        &stem,
        body,
        front.string("description"),
        front.text("argument-hint"),
        source,
        plugin,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn expands_arguments() {
        assert_eq!(
            arguments("Fix issue #$1 in $2", "42 'src/a b.rs'"),
            "Fix issue #42 in src/a b.rs"
        );
        assert_eq!(
            arguments("Review $ARGUMENTS", "the diff"),
            "Review the diff"
        );
        assert_eq!(
            arguments("Switch to {{args}} mode", "ultra"),
            "Switch to ultra mode"
        );
        assert_eq!(
            arguments("Summarize.", "today"),
            "Summarize.\n\nARGUMENTS: today"
        );
        assert_eq!(arguments("Summarize.", ""), "Summarize.");
    }

    #[test]
    fn parses_invocations() {
        assert_eq!(
            parse_invocation("/review the diff"),
            Some(("review", "the diff"))
        );
        assert_eq!(
            parse_invocation("/ponytail:ponytail-review"),
            Some(("ponytail:ponytail-review", ""))
        );
        assert_eq!(parse_invocation("/tmp/file.txt is big"), None);
        assert_eq!(parse_invocation("fix /review"), None);
    }

    #[test]
    fn discovers_commands_and_skills() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        write(
            &project
                .path()
                .join(".claude/commands/frontend/component.md"),
            "---\ndescription: New component\nargument-hint: <name>\n---\nCreate a component named $1.",
        );
        write(
            &project.path().join(".gemini/commands/git/commit.toml"),
            "description = \"Commit\"\nprompt = \"Write a commit message for {{args}}\"\n",
        );
        write(
            &project.path().join(".malac/commands/deploy.md"),
            "# Deploy\n\nDeploy to $ARGUMENTS.",
        );
        let skill_dir = project.path().join(".malac/skills/release");
        write(
            &skill_dir.join("SKILL.md"),
            "---\nname: release\ndescription: Ship it\n---\nBump $ARGUMENTS.",
        );
        let (skills, _) = crate::catalog::skills(home.path(), None, project.path(), &[]);

        let commands = discover(home.path(), None, project.path(), &[], &skills);
        let names: Vec<&str> = commands.iter().map(|c| c.name()).collect();
        assert_eq!(names, vec!["component", "deploy", "git:commit", "release"]);
        let deploy = find(&commands, "deploy").unwrap().unwrap();
        assert_eq!(deploy.info.description, "Deploy");
        assert_eq!(
            deploy.expand("staging").text,
            "# Deploy\n\nDeploy to staging."
        );
        let commit = find(&commands, "commit").unwrap().unwrap();
        assert_eq!(
            commit.expand("the fix").text,
            "Write a commit message for the fix"
        );
        let release = find(&commands, "release").unwrap().unwrap().expand("1.2");
        assert!(release.text.ends_with("Bump 1.2."));
        assert_eq!(release.skill_dir.as_deref(), Some(skill_dir.as_path()));
        assert!(matches!(find(&commands, "nothing"), Ok(None)));
    }
}
