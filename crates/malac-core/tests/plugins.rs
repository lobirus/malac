//! Plugins end to end: installing from a git marketplace (a temporary bare
//! repository), updating, and a runtime that uses a plugin's command,
//! skill, script and hooks against a scripted provider.

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex, Once};

use anyhow::Result;
use async_trait::async_trait;
use malac_core::config::{self, Config, PluginsConfig, ProviderConfig};
use malac_core::message::{Block, Message, Role, StopReason, Usage};
use malac_core::permission::Mode;
use malac_core::plugins::{self, Manager, NeedsApproval, Scope};
use malac_core::provider::{Completion, DeltaSink, Provider, Request};
use malac_core::{AgentSpec, Runtime};
use serde_json::{Value, json};

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

/// A bare repository at `<dir>/<name>.git` holding what `fill` writes, and its working copy.
fn repository(dir: &Path, name: &str, fill: impl FnOnce(&Path)) -> (String, std::path::PathBuf) {
    let bare = dir.join(format!("{name}.git"));
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "--bare", "--quiet"]);
    let work = dir.join(format!("{name}-work"));
    git(
        dir,
        &[
            "clone",
            "--quiet",
            bare.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    fill(&work);
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "initial"]);
    git(&work, &["push", "--quiet", "origin", "HEAD:main"]);
    (format!("file://{}", bare.display()), work)
}

#[tokio::test]
async fn installs_and_updates_from_a_git_marketplace() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    // A portable plugin in a repository of its own.
    let (portable_url, _) = repository(dir.path(), "portable", |work| {
        write(
            work,
            "plugin.json",
            r#"{"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json", "name": "portable", "version": "2.0.0"}"#,
        );
        write(
            work,
            "skills/summarize/SKILL.md",
            "---\nname: summarize\ndescription: Summarize text\n---\nBe short.",
        );
    });
    // The marketplace: one plugin inside it, one pointing at the repository above.
    let catalog = format!(
        r#"{{"name": "fixtures", "owner": {{"name": "Tests"}}, "plugins": [
            {{"name": "review-kit", "source": "./plugins/review-kit"}},
            {{"name": "portable", "source": {{"source": "url", "url": "{portable_url}"}}}}
        ]}}"#
    );
    let (market_url, market_work) = repository(dir.path(), "market", |work| {
        write(work, ".claude-plugin/marketplace.json", &catalog);
        write(
            work,
            "plugins/review-kit/.claude-plugin/plugin.json",
            r#"{"name": "review-kit", "version": "1.0.0", "description": "Reviews"}"#,
        );
        write(
            work,
            "plugins/review-kit/skills/review/SKILL.md",
            "---\nname: review\ndescription: Review a diff\n---\nRun scripts/diffstat.sh first.",
        );
        write(
            work,
            "plugins/review-kit/skills/review/scripts/diffstat.sh",
            "#!/bin/sh\ngit diff --stat\n",
        );
        write(
            work,
            "plugins/review-kit/commands/review.md",
            "Review $ARGUMENTS",
        );
        write(
            work,
            "plugins/review-kit/agents/critic.md",
            "---\nname: critic\ndescription: Finds bugs\n---\nBe harsh.",
        );
    });

    let manager = Manager::new(&home, Some(&project), &project, false);
    let added = manager
        .add_marketplace(&market_url, Scope::User)
        .await
        .unwrap();
    assert_eq!(added.name, "fixtures");
    assert!(
        home.join("plugins/marketplaces/fixtures/.claude-plugin/marketplace.json")
            .exists()
    );

    let kit = manager.install("review-kit").await.unwrap();
    assert!(
        kit.root
            .starts_with(std::fs::canonicalize(home.join("plugins/cache")).unwrap())
    );
    assert_eq!(kit.components.skills.len(), 1);
    assert_eq!(kit.components.agents.len(), 1);
    assert_eq!(kit.executables(), vec!["scripts"]);
    let err = manager
        .enable("review-kit", Scope::User, false)
        .unwrap_err();
    assert!(err.downcast_ref::<NeedsApproval>().is_some());
    manager.enable("review-kit", Scope::User, true).unwrap();

    let portable = manager.install("portable@fixtures").await.unwrap();
    assert_eq!(portable.meta.version.as_deref(), Some("2.0.0"));
    manager.enable("portable", Scope::User, false).unwrap();

    let installed = manager.store().installed().unwrap();
    let record = &installed.plugins["review-kit@fixtures"];
    assert_eq!(record.commit.as_ref().map(String::len), Some(40));
    assert_eq!(record.approved, vec!["scripts"]);

    let config = PluginsConfig {
        enabled: true,
        defaults: false,
    };
    let loaded = plugins::load(&home, &project, &config);
    let ids: Vec<&str> = loaded.plugins.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, vec!["portable@fixtures", "review-kit@fixtures"]);

    // A new release upstream reaches the cache only through an update.
    write(
        &market_work,
        "plugins/review-kit/.claude-plugin/plugin.json",
        r#"{"name": "review-kit", "version": "1.1.0", "description": "Reviews"}"#,
    );
    git(&market_work, &["commit", "--quiet", "-am", "1.1.0"]);
    git(&market_work, &["push", "--quiet", "origin", "HEAD:main"]);
    let (plugin, _) = manager.load_installed("review-kit@fixtures").unwrap();
    assert_eq!(plugin.meta.version.as_deref(), Some("1.0.0"));
    let report = manager.update(Some("review-kit")).await.unwrap();
    assert!(
        report
            .iter()
            .any(|l| l == "review-kit@fixtures: 1.0.0 → 1.1.0"),
        "{report:?}"
    );
    let (plugin, _) = manager.load_installed("review-kit@fixtures").unwrap();
    assert_eq!(plugin.meta.version.as_deref(), Some("1.1.0"));

    manager.uninstall("review-kit").unwrap();
    assert!(!kit.root.exists());
}

/// Replies in turn and records each request's system prompt and messages.
struct Scripted {
    replies: Mutex<Vec<Message>>,
    requests: Mutex<Vec<(String, Vec<Message>)>>,
}

#[async_trait]
impl Provider for Scripted {
    async fn complete(&self, req: &Request<'_>, _: DeltaSink<'_>) -> Result<Completion> {
        self.requests
            .lock()
            .unwrap()
            .push((req.system.to_owned(), req.messages.to_vec()));
        let mut replies = self.replies.lock().unwrap();
        let message = if replies.is_empty() {
            Message::assistant(vec![Block::text("done")])
        } else {
            replies.remove(0)
        };
        let stop = if message.tool_uses().next().is_some() {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
        Ok(Completion {
            message,
            usage: Usage::default(),
            stop,
        })
    }
}

fn call(id: &str, name: &str, input: Value) -> Message {
    Message::assistant(vec![Block::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
    }])
}

fn result(messages: &[Message], id: &str) -> (String, bool) {
    messages
        .iter()
        .filter(|m| m.role == Role::User)
        .flat_map(|m| &m.content)
        .find_map(|b| match b {
            Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            } if tool_use_id == id => Some((
                content
                    .iter()
                    .filter_map(|c| match c {
                        Block::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect(),
                *is_error,
            )),
            _ => None,
        })
        .unwrap()
}

fn first_user_text(messages: &[Message]) -> String {
    messages[0]
        .content
        .iter()
        .filter_map(|b| match b {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn isolated_home() {
    static HOME: Once = Once::new();
    HOME.call_once(|| {
        let dir = tempfile::tempdir().unwrap().keep();
        unsafe { std::env::set_var("MALAC_HOME", dir) };
    });
}

#[tokio::test]
async fn a_session_uses_plugin_commands_skills_scripts_and_hooks() {
    isolated_home();
    let home = config::home_dir();
    let dir = tempfile::tempdir().unwrap();
    let market = dir.path().join("market");
    write(
        &market,
        ".claude-plugin/plugin.json",
        r#"{"name": "kit", "version": "0.1.0"}"#,
    );
    write(
        &market,
        "commands/hello.md",
        "---\ndescription: Greets\n---\nSay hello to $ARGUMENTS.",
    );
    write(
        &market,
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: Greets with a script\n---\nRun $SKILL_DIR/scripts/greet.sh.",
    );
    write(
        &market,
        "skills/greet/scripts/greet.sh",
        "#!/bin/sh\necho \"greeted from $(basename \"$SKILL_DIR\")\"\n",
    );
    write(
        &market,
        "hooks/hooks.json",
        r#"{"hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": "echo 'Prefer small diffs.'"}]}],
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "echo 'The user is in a hurry.'"}]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command",
                "command": "grep -q forbidden && { echo 'not that' >&2; exit 2; } || exit 0"}]}]
        }}"#,
    );
    let project = dir.path().join("project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    write(&project, "AGENTS.md", "Run `make check` before finishing.");
    write(&project, "README.md", "# Demo\n\nA demo project.");
    write(&project, "src/lib.rs", "pub fn demo() {}\n");
    let project = std::fs::canonicalize(&project).unwrap();

    let manager = Manager::new(&home, Some(&project), &project, true);
    manager
        .add_marketplace(&market.display().to_string(), Scope::User)
        .await
        .unwrap();
    manager.install("kit").await.unwrap();
    manager.enable("kit", Scope::Project, true).unwrap();

    let mut config = Config {
        model: Some("mock/test".into()),
        mode: Mode::Autopilot,
        ..Default::default()
    };
    config
        .providers
        .insert("mock".into(), ProviderConfig::default());
    config.control.enabled = false;
    let runtime = Runtime::new(config, project.clone()).unwrap();
    let names: Vec<&str> = runtime.commands.iter().map(|c| c.name()).collect();
    assert!(
        names.contains(&"kit:hello") && names.contains(&"kit:greet"),
        "{names:?}"
    );
    assert!(
        runtime.skills.iter().any(|s| s.name == "ponytail:ponytail"),
        "the default plugins"
    );
    assert!(
        runtime.skills.iter().any(|s| s.name == "design:design"),
        "the default plugins"
    );
    let system = malac_core::prompt::build(&malac_core::prompt::PromptInputs {
        runtime: &runtime,
        cwd: &project,
        model: "mock/test",
        depth: 0,
        agent_type: None,
    });
    assert!(
        system.contains("# Design") && system.contains("\n- design:design: "),
        "the prompt points UI work at the listed design skill"
    );

    let script = Arc::new(Scripted {
        replies: Mutex::new(vec![
            call("1", "bash", json!({"command": "echo forbidden"})),
            call("2", "skill", json!({"name": "greet"})),
            call(
                "3",
                "bash",
                json!({"command": "sh \"$SKILL_DIR/scripts/greet.sh\""}),
            ),
            call("4", "project_index", json!({"query": "demo"})),
        ]),
        requests: Mutex::default(),
    });
    runtime.insert_provider("mock", script.clone());
    let agent = runtime.spawn_agent(AgentSpec::default()).unwrap();
    let answer = agent
        .prompt(vec![Block::text("/hello world")])
        .await
        .unwrap();
    assert_eq!(answer, "done");

    let requests = script.requests.lock().unwrap().clone();
    let (system, messages) = requests.last().unwrap();
    assert!(system.contains("Run `make check` before finishing."));
    assert!(system.contains("# Context from plugins\n\nPrefer small diffs."));
    assert!(system.contains("- kit:greet: Greets with a script"));
    assert!(system.contains("# Project index"));
    assert!(system.contains("- README.md — Demo: A demo project."));
    let prompt = first_user_text(messages);
    assert!(prompt.contains("/hello world"));
    assert!(prompt.contains("<command-expansion>\nSay hello to world."));
    assert!(prompt.contains("The user is in a hurry."));
    let (blocked, is_error) = result(messages, "1");
    assert!(is_error && blocked.contains("not that"), "{blocked}");
    let (skill, _) = result(messages, "2");
    assert!(skill.contains("scripts/greet.sh"));
    let (greeted, is_error) = result(messages, "3");
    assert!(
        !is_error && greeted.contains("greeted from greet"),
        "{greeted}"
    );
    let (index, is_error) = result(messages, "4");
    assert!(
        !is_error && index.contains("src/lib.rs: pub fn demo"),
        "{index}"
    );

    let report = agent.prompt(vec![Block::text("/context")]).await.unwrap();
    assert!(report.contains("AGENTS.md"));
    assert!(report.contains("kit@market"));
    assert_eq!(
        script.requests.lock().unwrap().len(),
        requests.len(),
        "no model call"
    );
    let unknown = agent.prompt(vec![Block::text("/nonexistent")]).await;
    assert!(
        unknown
            .unwrap_err()
            .to_string()
            .contains("unknown command /nonexistent")
    );
}

#[tokio::test]
async fn the_example_marketplace_installs_and_its_hook_guards_force_pushes() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let manager = Manager::new(&home, Some(dir.path()), dir.path(), false);
    let catalog = manager
        .add_marketplace(
            &examples.join("marketplace").display().to_string(),
            Scope::User,
        )
        .await
        .unwrap();
    assert_eq!(catalog.name, "malac-examples");
    let plugin = manager.install("release-tools").await.unwrap();
    assert_eq!(
        plugin.formats,
        vec![plugins::Format::AgentPlugins, plugins::Format::ClaudeCode]
    );
    assert!(plugin.warnings.is_empty(), "{:?}", plugin.warnings);
    assert_eq!(plugin.command_names(), vec!["release-notes"]);
    let needs = manager
        .enable("release-tools", Scope::User, false)
        .unwrap_err();
    assert_eq!(
        needs.downcast_ref::<NeedsApproval>().unwrap().parts,
        vec!["hooks", "scripts"]
    );
    manager.enable("release-tools", Scope::User, true).unwrap();

    let config = PluginsConfig {
        enabled: true,
        defaults: false,
    };
    let loaded = plugins::load(&home, dir.path(), &config);
    let session = plugins::hooks::Session {
        id: "s",
        cwd: dir.path(),
        project_dir: dir.path(),
    };
    let push = |command: &str| json!({ "command": command });
    let blocked = loaded
        .hooks
        .pre_tool(&session, "bash", &push("git push --force origin main"))
        .await;
    assert!(matches!(
        blocked.decision,
        Some(plugins::hooks::Decision::Deny(_))
    ));
    let fine = loaded
        .hooks
        .pre_tool(&session, "bash", &push("git push --force-with-lease"))
        .await;
    assert!(fine.decision.is_none());

    let file: malac_core::plugins::store::PluginsFile =
        toml::from_str(&std::fs::read_to_string(examples.join("plugins.toml")).unwrap()).unwrap();
    assert_eq!(file.plugins.get("ponytail@malac"), Some(&false));
}
