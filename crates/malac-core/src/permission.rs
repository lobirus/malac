//! Decides whether a tool call may run, and asks a human when it is unclear.

use std::str::FromStr;
use std::sync::Mutex;

use anyhow::{Result, bail};
use async_trait::async_trait;
use globset::{GlobBuilder, GlobMatcher};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::PermissionRules;
use crate::event::AgentId;
use crate::tools::{Tool, ToolKind};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Confirm edits and commands.
    #[default]
    Ask,
    /// Edit files freely, confirm commands.
    Edits,
    /// Never ask. Deny rules still apply.
    Autopilot,
}

impl FromStr for Mode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "ask" => Mode::Ask,
            "edits" => Mode::Edits,
            "autopilot" | "auto" | "yolo" => Mode::Autopilot,
            other => bail!("unknown mode `{other}` (expected ask, edits or autopilot)"),
        })
    }
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::Edits => "edits",
            Mode::Autopilot => "autopilot",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    /// Allow this and similar calls for the rest of the session.
    AllowSession,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: String,
    pub agent: AgentId,
    pub agent_label: String,
    pub tool: String,
    pub summary: String,
    pub input: Value,
}

#[async_trait]
pub trait Approver: Send + Sync {
    async fn approve(&self, request: &PermissionRequest) -> Decision;
}

/// Used when nobody is around to answer.
pub struct DenyAll;

#[async_trait]
impl Approver for DenyAll {
    async fn approve(&self, _: &PermissionRequest) -> Decision {
        Decision::Deny
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Ask,
    Deny(String),
}

/// `tool` or `tool(argument-glob)`, where both parts are globs.
#[derive(Debug, Clone)]
struct Rule {
    source: String,
    tool: GlobMatcher,
    argument: Option<GlobMatcher>,
}

impl Rule {
    fn parse(source: &str) -> Result<Self> {
        let (tool, argument) = match source.split_once('(') {
            Some((tool, rest)) => match rest.strip_suffix(')') {
                Some(arg) => (tool.trim(), Some(arg)),
                None => bail!("permission rule `{source}` is missing `)`"),
            },
            None => (source.trim(), None),
        };
        Ok(Self {
            source: source.to_owned(),
            tool: glob(tool)?,
            argument: argument.map(glob).transpose()?,
        })
    }

    fn matches(&self, tool: &str, key: Option<&str>) -> bool {
        if !self.tool.is_match(tool) {
            return false;
        }
        match (&self.argument, key) {
            (None, _) => true,
            (Some(pattern), Some(key)) => pattern.is_match(key),
            (Some(_), None) => false,
        }
    }
}

fn glob(pattern: &str) -> Result<GlobMatcher> {
    Ok(GlobBuilder::new(pattern)
        .literal_separator(false)
        .build()?
        .compile_matcher())
}

pub struct Policy {
    mode: Mutex<Mode>,
    allow: Vec<Rule>,
    deny: Vec<Rule>,
    session: Mutex<Vec<Rule>>,
}

impl Policy {
    pub fn new(mode: Mode, rules: &PermissionRules) -> Result<Self> {
        let parse = |list: &[String]| {
            list.iter()
                .map(|r| Rule::parse(r))
                .collect::<Result<Vec<_>>>()
        };
        Ok(Self {
            mode: Mutex::new(mode),
            allow: parse(&rules.allow)?,
            deny: parse(&rules.deny)?,
            session: Mutex::new(Vec::new()),
        })
    }

    pub fn mode(&self) -> Mode {
        *self.mode.lock().unwrap()
    }

    pub fn set_mode(&self, mode: Mode) {
        *self.mode.lock().unwrap() = mode;
    }

    pub fn check(&self, tool: &dyn Tool, input: &Value) -> Verdict {
        let name = tool.name();
        let keys = tool.permission_keys(input);
        let key_refs: Vec<Option<&str>> = if keys.is_empty() {
            vec![None]
        } else {
            keys.iter().map(|k| Some(k.as_str())).collect()
        };

        for rule in &self.deny {
            if key_refs.iter().any(|key| rule.matches(name, *key)) {
                return Verdict::Deny(format!("blocked by deny rule `{}`", rule.source));
            }
        }
        let kind = tool.kind();
        match (kind, self.mode()) {
            (ToolKind::Safe, _) | (_, Mode::Autopilot) | (ToolKind::Edit, Mode::Edits) => {
                return Verdict::Allow;
            }
            _ => {}
        }
        let session = self.session.lock().unwrap();
        let allowed = key_refs.iter().all(|key| {
            self.allow
                .iter()
                .chain(session.iter())
                .any(|rule| rule.matches(name, *key))
        });
        if allowed {
            Verdict::Allow
        } else {
            Verdict::Ask
        }
    }

    /// Remembers an "allow for this session" answer. Commands are generalized
    /// to their program name, other tools to the whole tool.
    pub fn remember(&self, tool: &dyn Tool, input: &Value) {
        let keys = tool.permission_keys(input);
        let mut session = self.session.lock().unwrap();
        if keys.is_empty() || tool.kind() != ToolKind::Execute {
            if let Ok(rule) = Rule::parse(tool.name()) {
                session.push(rule);
            }
            return;
        }
        for key in keys {
            let program = escape_glob(key.split_whitespace().next().unwrap_or_default());
            for pattern in [
                format!("{}({program})", tool.name()),
                format!("{}({program} *)", tool.name()),
            ] {
                if let Ok(rule) = Rule::parse(&pattern) {
                    session.push(rule);
                }
            }
        }
    }
}

fn escape_glob(text: &str) -> String {
    text.chars()
        .flat_map(|c| match c {
            '*' | '?' | '[' | ']' | '{' | '}' => vec!['[', c, ']'],
            _ => vec![c],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ToolContext, ToolOutput};
    use serde_json::json;

    struct Fake(&'static str, ToolKind);

    #[async_trait]
    impl Tool for Fake {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> String {
            String::new()
        }
        fn schema(&self) -> Value {
            json!({})
        }
        fn kind(&self) -> ToolKind {
            self.1
        }
        fn permission_keys(&self, input: &Value) -> Vec<String> {
            input["command"]
                .as_str()
                .map(|c| c.split("&&").map(|s| s.trim().to_owned()).collect())
                .unwrap_or_default()
        }
        async fn run(&self, _: Value, _: &ToolContext) -> ToolOutput {
            ToolOutput::text("")
        }
    }

    fn policy(mode: Mode, allow: &[&str], deny: &[&str]) -> Policy {
        let rules = PermissionRules {
            allow: allow.iter().map(|s| s.to_string()).collect(),
            deny: deny.iter().map(|s| s.to_string()).collect(),
        };
        Policy::new(mode, &rules).unwrap()
    }

    #[test]
    fn modes() {
        let bash = Fake("bash", ToolKind::Execute);
        let edit = Fake("edit", ToolKind::Edit);
        let read = Fake("read", ToolKind::Safe);
        let input = json!({"command": "ls"});
        let ask = policy(Mode::Ask, &[], &[]);
        assert_eq!(ask.check(&read, &input), Verdict::Allow);
        assert_eq!(ask.check(&edit, &input), Verdict::Ask);
        let edits = policy(Mode::Edits, &[], &[]);
        assert_eq!(edits.check(&edit, &input), Verdict::Allow);
        assert_eq!(edits.check(&bash, &input), Verdict::Ask);
        let auto = policy(Mode::Autopilot, &[], &[]);
        assert_eq!(auto.check(&bash, &input), Verdict::Allow);
    }

    #[test]
    fn rules_cover_every_command_segment() {
        let bash = Fake("bash", ToolKind::Execute);
        let p = policy(Mode::Autopilot, &["bash(cargo *)"], &["bash(rm -rf /*)"]);
        assert!(matches!(
            p.check(&bash, &json!({"command": "cargo build && rm -rf /"})),
            Verdict::Deny(_)
        ));
        let p = policy(Mode::Ask, &["bash(cargo *)"], &[]);
        assert_eq!(
            p.check(&bash, &json!({"command": "cargo test"})),
            Verdict::Allow
        );
        assert_eq!(
            p.check(&bash, &json!({"command": "cargo test && curl x"})),
            Verdict::Ask
        );
    }

    #[test]
    fn session_rules_generalize_to_program() {
        let bash = Fake("bash", ToolKind::Execute);
        let p = policy(Mode::Ask, &[], &[]);
        p.remember(&bash, &json!({"command": "npm test"}));
        assert_eq!(
            p.check(&bash, &json!({"command": "npm run build"})),
            Verdict::Allow
        );
        assert_eq!(p.check(&bash, &json!({"command": "make"})), Verdict::Ask);
        assert_eq!(p.check(&bash, &json!({"command": "npx foo"})), Verdict::Ask);
    }
}
