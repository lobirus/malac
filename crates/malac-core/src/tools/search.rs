use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use async_trait::async_trait;
use globset::GlobBuilder;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolKind, ToolOutput, str_arg, truncate_chars};

const MAX_GLOB_RESULTS: usize = 500;
const DEFAULT_MATCH_LIMIT: usize = 200;
/// A walk that runs this long (say, from `/`) stops with partial results.
const WALK_BUDGET: Duration = Duration::from_secs(20);

pub struct Glob;

#[async_trait]
impl Tool for Glob {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> String {
        "Find files by glob pattern such as `**/*.rs` or `src/**/mod.rs`, newest first. \
         Respects .gitignore."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string" },
                "path": { "type": "string", "description": "Directory to search (default: working directory)" }
            },
            "required": ["pattern"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    fn summary(&self, input: &Value) -> String {
        input["pattern"].as_str().unwrap_or_default().to_owned()
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let ctx = ctx.clone();
        tokio::task::spawn_blocking(move || glob(&input, &ctx))
            .await
            .unwrap_or_else(|err| Err(err.into()))
            .into()
    }
}

fn glob(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let pattern = str_arg(input, "pattern")?;
    let root = input["path"]
        .as_str()
        .map_or(ctx.cwd.clone(), |p| ctx.resolve(p));
    let matcher = GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .context("invalid glob pattern")?
        .compile_matcher();

    let mut hits: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut walk = Walk::new(ctx);
    for entry in WalkBuilder::new(&root).build().flatten() {
        if walk.should_stop() {
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
        if matcher.is_match(rel) {
            let modified = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            hits.push((modified, entry.into_path()));
        }
    }
    if hits.is_empty() {
        return Ok(ToolOutput::text(walk.annotate("No files found".into())));
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    let total = hits.len();
    let mut out: Vec<String> = hits
        .into_iter()
        .take(MAX_GLOB_RESULTS)
        .map(|(_, p)| ctx.display_path(&p))
        .collect();
    if total > MAX_GLOB_RESULTS {
        out.push(format!(
            "[{} more files not shown]",
            total - MAX_GLOB_RESULTS
        ));
    }
    Ok(ToolOutput::text(walk.annotate(out.join("\n"))))
}

pub struct Grep;

#[async_trait]
impl Tool for Grep {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> String {
        "Search file contents with a regular expression (Rust regex syntax). Returns `path:line: text` \
         matches, or just file paths with `files_only`. Respects .gitignore."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string" },
                "path": { "type": "string", "description": "File or directory (default: working directory)" },
                "glob": { "type": "string", "description": "Only search files matching this glob, e.g. `*.ts`" },
                "ignore_case": { "type": "boolean" },
                "files_only": { "type": "boolean" },
                "limit": { "type": "integer", "description": "Maximum matches (default 200)" }
            },
            "required": ["pattern"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    fn summary(&self, input: &Value) -> String {
        input["pattern"].as_str().unwrap_or_default().to_owned()
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let ctx = ctx.clone();
        tokio::task::spawn_blocking(move || grep(&input, &ctx))
            .await
            .unwrap_or_else(|err| Err(err.into()))
            .into()
    }
}

fn grep(input: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
    let regex = RegexBuilder::new(str_arg(input, "pattern")?)
        .case_insensitive(input["ignore_case"].as_bool().unwrap_or(false))
        .build()
        .context("invalid regex")?;
    let root = input["path"]
        .as_str()
        .map_or(ctx.cwd.clone(), |p| ctx.resolve(p));
    let filter = input["glob"]
        .as_str()
        .map(|g| GlobBuilder::new(g).build().map(|g| g.compile_matcher()))
        .transpose()
        .context("invalid glob")?;
    let files_only = input["files_only"].as_bool().unwrap_or(false);
    let limit = input["limit"]
        .as_u64()
        .map_or(DEFAULT_MATCH_LIMIT, |n| n as usize);

    let mut out = Vec::new();
    let mut truncated = false;
    let mut walk = Walk::new(ctx);
    'files: for entry in WalkBuilder::new(&root).build().flatten() {
        if walk.should_stop() {
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if let Some(filter) = &filter {
            let name = Path::new(path.file_name().unwrap_or_default());
            let rel = path.strip_prefix(&root).unwrap_or(path);
            if !filter.is_match(name) && !filter.is_match(rel) {
                continue;
            }
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if bytes[..bytes.len().min(8192)].contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let display = ctx.display_path(path);
        for (number, line) in text.lines().enumerate() {
            if !regex.is_match(line) {
                continue;
            }
            if out.len() == limit {
                truncated = true;
                break 'files;
            }
            if files_only {
                out.push(display.clone());
                continue 'files;
            }
            out.push(format!(
                "{display}:{}: {}",
                number + 1,
                truncate_chars(line.trim_end(), 300)
            ));
        }
    }
    if out.is_empty() {
        return Ok(ToolOutput::text(walk.annotate("No matches".into())));
    }
    if truncated {
        out.push(format!("[stopped at {limit} results; narrow the search]"));
    }
    Ok(ToolOutput::text(walk.annotate(out.join("\n"))))
}

/// Stops long directory walks: on cancellation, or once the time budget is spent.
struct Walk {
    cancel: tokio_util::sync::CancellationToken,
    started: Instant,
    stopped: bool,
}

impl Walk {
    fn new(ctx: &ToolContext) -> Self {
        Self {
            cancel: ctx.cancel.clone(),
            started: Instant::now(),
            stopped: false,
        }
    }

    fn should_stop(&mut self) -> bool {
        self.stopped =
            self.stopped || self.cancel.is_cancelled() || self.started.elapsed() > WALK_BUDGET;
        self.stopped
    }

    fn annotate(&self, text: String) -> String {
        if self.stopped {
            format!(
                "{text}\n[search stopped after {}s; results are incomplete, use a narrower path]",
                WALK_BUDGET.as_secs()
            )
        } else {
            text
        }
    }
}
