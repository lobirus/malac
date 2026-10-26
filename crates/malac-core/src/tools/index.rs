//! Reads, searches and annotates the project index (`.malac/index.md`).

use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolKind, ToolOutput};

/// Reading an index older than this updates it first.
const FRESH: Duration = Duration::from_secs(30);

pub struct ProjectIndex;

#[async_trait]
impl Tool for ProjectIndex {
    fn name(&self) -> &str {
        "project_index"
    }

    fn description(&self) -> String {
        "The project's structural index (.malac/index.md), kept current from the files: directory tree with file \
         counts, languages, manifests and entry points, the top-level symbols of every source file, doc headings, \
         and notes from earlier sessions. Use it before searching broadly. Without arguments it returns the \
         overview and tree; `section` reads `docs`, `symbols` or `notes` (narrow with `path`, a directory or file \
         prefix); `query` finds paths, symbols and headings containing a word; `note` records where something \
         lives or how parts connect, for the next session."
            .into()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "section": { "type": "string", "enum": ["overview", "tree", "docs", "symbols", "notes", "all"] },
                "path": { "type": "string", "description": "Only entries under this path prefix" },
                "query": { "type": "string", "description": "Case-insensitive text to find" },
                "note": { "type": "string", "description": "One line to add to the notes" },
                "rebuild": { "type": "boolean", "description": "Regenerate from scratch" }
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Safe
    }

    async fn run(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let Some(index) = ctx.runtime.context.index.clone() else {
            return ToolOutput::error(
                "this directory has no project index: it is not a repository, or `[context] index = false`",
            );
        };
        let result = tokio::task::spawn_blocking(move || run(&index, &input)).await;
        match result {
            Ok(result) => result.into(),
            Err(err) => ToolOutput::error(format!("index task failed: {err}")),
        }
    }
}

fn run(index: &crate::context::index::Index, input: &Value) -> Result<ToolOutput> {
    if let Some(note) = input["note"].as_str() {
        index.add_note(note)?;
        return Ok(ToolOutput::text(format!(
            "Added the note to {}",
            index.path().display()
        )));
    }
    let rebuild = input["rebuild"].as_bool().unwrap_or(false);
    if rebuild || index.is_stale(FRESH) {
        index.refresh(rebuild).context("cannot update the index")?;
    }
    if let Some(query) = input["query"].as_str() {
        let hits = index.search(query, 80)?;
        return Ok(ToolOutput::text(if hits.is_empty() {
            format!("Nothing in the index matches `{query}`; try grep.")
        } else {
            hits.join("\n")
        }));
    }
    let path = input["path"].as_str().filter(|p| !p.is_empty());
    let text = match input["section"].as_str() {
        Some(section) => index.section(section, path)?,
        None => format!(
            "{}\n\n{}\n\nMore: section `docs`, `symbols` (with `path`) or `notes`, or a `query`.",
            index.section("overview", None)?,
            index.section("tree", None)?
        ),
    };
    Ok(ToolOutput::text(text))
}
