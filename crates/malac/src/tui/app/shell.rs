//! `/bash` and `!`: a shell command in the tab's folder, run by the daemon
//! on its machine (so on the server for a remote connection). Its card shows
//! in the transcript like the agent's own commands, and its output is
//! attached to the next prompt so the agent can see it. `/malac`: one of
//! malac's own commands, the same way, its output kept to the card.

use malac_core::tools::{Outcome, ScriptRun};
use serde_json::Value;

use super::*;
use crate::protocol::Command;
use crate::tui::transcript::{ToolCall, ToolState};

/// Seconds a `/bash` command may run before the daemon stops it.
pub const SHELL_TIMEOUT: u64 = 120;

/// How a `/bash` command ended, as the daemon reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct ShellResult {
    pub output: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
}

impl ShellResult {
    fn run(&self) -> ScriptRun {
        let outcome = if self.cancelled {
            Outcome::Cancelled
        } else if self.timed_out {
            Outcome::TimedOut
        } else {
            Outcome::Exited(self.exit_code)
        };
        ScriptRun {
            output: self.output.clone(),
            outcome,
        }
    }
}

impl App {
    /// Runs `command` in the active tab's folder on the daemon's machine.
    pub(super) fn run_shell(&mut self, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            self.flash("type a command: /bash git status, or !git status");
            return;
        }
        self.start_shell(command, true);
    }

    /// Runs one of malac's commands, `line` being what follows `malac`, if
    /// it is one: words that are not would make `malac` start a prompt.
    pub(super) fn run_malac(&mut self, line: &str) {
        let Some(words) = shlex::split(line) else {
            self.flash("a quote is not closed");
            return;
        };
        if let Err(err) = crate::cli::check_subcommand(&words) {
            self.flash(err);
            return;
        }
        // Quoted again, so the shell runs exactly the words checked.
        let words = std::iter::once("malac").chain(words.iter().map(String::as_str));
        match shlex::try_join(words) {
            Ok(command) => self.start_shell(&command, false),
            Err(err) => self.flash(format!("cannot run that: {err}")),
        }
    }

    /// Starts a `/bash` or `/malac` run; with `attach`, its output goes
    /// along with the next prompt.
    fn start_shell(&mut self, command: &str, attach: bool) {
        let Some(tab) = self.tab().id.clone() else {
            self.flash("this tab is still opening; try again in a moment");
            return;
        };
        let id = uuid::Uuid::new_v4().simple().to_string();
        let view = self.shell_view();
        if let Some(agent) = self.agents.get_mut(&view) {
            agent.transcript.push(Entry::Tool(ToolCall {
                call_id: id.clone(),
                request_id: None,
                name: "$".into(),
                summary: command.to_owned(),
                input: Value::Null,
                state: ToolState::Running,
                output: String::new(),
                millis: 0,
                started: Instant::now(),
                expanded: true,
                decision: None,
            }));
        }
        let active = self.tab_mut();
        active.scroll = None;
        active.shells.push(id.clone());
        self.backend.request(
            Command::Shell {
                tab: tab.clone(),
                id: id.clone(),
                command: command.to_owned(),
                timeout: Some(SHELL_TIMEOUT),
            },
            Reply::Shell {
                tab,
                view,
                id,
                command: command.to_owned(),
                attach,
            },
        );
    }

    /// The view a `/bash` card goes into: the shown agent's, or while the
    /// tab has none, one of its own.
    fn shell_view(&mut self) -> AgentId {
        if let Some(shown) = self.tab().shown() {
            return shown.to_owned();
        }
        if let Some(view) = self.tab().shell_view.clone() {
            return view;
        }
        let id = format!("shell-{}", uuid::Uuid::new_v4().simple());
        let cwd = self.tab_cwd().display().to_string();
        self.agents
            .insert(AgentView::new(&id, "shell", None, "", &cwd));
        self.tab_mut().shell_view = Some(id.clone());
        id
    }

    /// A `/bash` command ended, or the daemon refused it. Its card shows
    /// the output, which with `attach` goes along with the tab's next prompt.
    pub(super) fn shell_ended(
        &mut self,
        (tab, view, id, command, attach): (&TabId, &str, &str, &str, bool),
        result: Result<ShellResult, String>,
    ) {
        let index = self.tab_index(tab);
        if let Some(index) = index {
            self.tabs[index].shells.retain(|s| s != id);
        }
        let (report, ok) = match &result {
            Ok(result) => {
                let run = result.run();
                (
                    run.report(SHELL_TIMEOUT),
                    run.outcome == Outcome::Exited(Some(0)),
                )
            }
            Err(err) => (err.clone(), false),
        };
        if let Some(card) = self
            .agents
            .get_mut(view)
            .and_then(|v| v.transcript.tool_mut(id))
        {
            card.state = if ok {
                ToolState::Done
            } else {
                ToolState::Failed
            };
            card.millis = card.started.elapsed().as_millis() as u64;
            card.output = report.clone();
        }
        let (Some(index), Ok(_), true) = (index, &result, attach) else {
            return;
        };
        match shell_attachment(id, command, &report) {
            Ok(attachment) if index == self.active => self.attachments.push(attachment),
            Ok(attachment) => self.tabs[index].draft_attachments.push(attachment),
            Err(err) => self.flash(format!("cannot keep the output for the next prompt: {err}")),
        }
    }

    /// Stops this terminal's `/bash` commands running in the active tab.
    pub(super) fn stop_shells(&mut self) {
        for id in self.tab().shells.clone() {
            self.backend.command(Command::ShellCancel { id });
        }
    }
}

/// A `/bash` run's output as a text file attached to the next prompt, named
/// after the command, so the agent reads what was run and what it printed.
fn shell_attachment(id: &str, command: &str, report: &str) -> Result<Attachment, String> {
    let dir = std::env::temp_dir().join("malac").join("shell").join(id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.txt", file_name(command)));
    let contents = format!("$ {command}\n{report}\n");
    std::fs::write(&path, &contents).map_err(|e| e.to_string())?;
    Ok(Attachment {
        path,
        name: format!("$ {}", text::truncate(command, 40)),
        kind: attach::Kind::File,
        size: Some(contents.len() as u64),
        inline: None,
        on_daemon: false,
    })
}

/// A file name from a command line: its words, without characters file
/// systems or shells mind, at most 40 characters.
fn file_name(command: &str) -> String {
    let cleaned: String = command
        .chars()
        .map(|c| match c {
            c if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ') => c,
            _ => ' ',
        })
        .collect();
    let words = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    // A leading dot would hide the file.
    let name = text::truncate(words.trim_start_matches('.'), 40)
        .trim_end_matches('…')
        .trim()
        .to_owned();
    match name.chars().any(char::is_alphanumeric) {
        true => name,
        false => "shell output".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::file_name;

    #[test]
    fn names_the_output_after_the_command() {
        assert_eq!(file_name("cargo test -p malac"), "cargo test -p malac");
        assert_eq!(file_name("ls | grep 'x/y'"), "ls grep x y");
        assert_eq!(file_name("../.."), "shell output");
        assert_eq!(file_name(".env check"), "env check");
        assert!(file_name(&"a".repeat(80)).chars().count() <= 40);
    }
}
