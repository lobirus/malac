//! `GET /api/usage`: token and cost totals from the session transcripts,
//! for editor and dashboard views. Read-only.
//!
//! Usage records in a transcript carry no timestamp or model, so a session
//! counts on the local day its transcript was last written, under the model
//! it started with.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::Context;
use axum::Json;
use axum::extract::Query;
use chrono::{DateTime, Days, Local, NaiveDate};
use malac_core::message::Usage;
use malac_core::session::{self, Record};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::settings::ApiError;

const DEFAULT_DAYS: u64 = 30;
const MAX_DAYS: u64 = 366;

#[derive(Deserialize)]
pub(super) struct UsageQuery {
    /// Days to cover, today included (default 30, at most 366).
    days: Option<u64>,
}

#[derive(Debug, Default, Clone, Copy, Serialize)]
struct Bucket {
    #[serde(flatten)]
    usage: Usage,
    sessions: usize,
}

impl Bucket {
    fn add(&mut self, usage: &Usage) {
        self.usage.add(usage);
        self.sessions += 1;
    }
}

/// One transcript's totals.
struct SessionUsage {
    model: String,
    day: NaiveDate,
    usage: Usage,
}

pub(super) async fn summary(Query(query): Query<UsageQuery>) -> Result<Json<Value>, ApiError> {
    let days = query.days.unwrap_or(DEFAULT_DAYS).clamp(1, MAX_DAYS);
    let today = Local::now().date_naive();
    let report =
        tokio::task::spawn_blocking(move || aggregate(&session::sessions_dir(), today, days))
            .await
            .context("the usage task failed")?;
    Ok(Json(report))
}

fn aggregate(dir: &Path, today: NaiveDate, days: u64) -> Value {
    let since = today.checked_sub_days(Days::new(days - 1)).unwrap_or(today);
    let week = today.checked_sub_days(Days::new(6)).unwrap_or(today);
    let (mut day_total, mut week_total, mut total) =
        (Bucket::default(), Bucket::default(), Bucket::default());
    let mut by_model: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_day: BTreeMap<NaiveDate, Bucket> = BTreeMap::new();
    for session in read_sessions(dir, since) {
        if session.usage == Usage::default() {
            continue;
        }
        total.add(&session.usage);
        if session.day >= week {
            week_total.add(&session.usage);
        }
        if session.day == today {
            day_total.add(&session.usage);
        }
        by_model
            .entry(session.model)
            .or_default()
            .add(&session.usage);
        by_day.entry(session.day).or_default().add(&session.usage);
    }
    let mut models: Vec<(String, Bucket)> = by_model.into_iter().collect();
    models.sort_by(|a, b| tokens(&b.1.usage).cmp(&tokens(&a.1.usage)));
    json!({
        "days": days,
        "since": since,
        "today": day_total,
        "week": week_total,
        "total": total,
        "by_model": models
            .into_iter()
            .map(|(model, bucket)| merge(json!({ "model": model }), &bucket))
            .collect::<Vec<_>>(),
        "by_day": by_day
            .into_iter()
            .map(|(date, bucket)| merge(json!({ "date": date }), &bucket))
            .collect::<Vec<_>>(),
    })
}

fn tokens(usage: &Usage) -> u64 {
    usage.input_tokens + usage.output_tokens + usage.cache_read_tokens + usage.cache_write_tokens
}

fn merge(mut head: Value, bucket: &Bucket) -> Value {
    if let (Some(head), Ok(Value::Object(fields))) =
        (head.as_object_mut(), serde_json::to_value(bucket))
    {
        head.extend(fields);
    }
    head
}

/// Every transcript written on or after `since`, subagents included.
fn read_sessions(dir: &Path, since: NaiveDate) -> Vec<SessionUsage> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| {
            let modified: DateTime<Local> = e.metadata().ok()?.modified().ok()?.into();
            let day = modified.date_naive();
            (day >= since)
                .then(|| read_session(&e.path(), day))
                .flatten()
        })
        .collect()
}

/// Sums a transcript's usage records. Only the metadata and usage lines are
/// decoded; messages, which can hold images, are skipped unparsed.
fn read_session(path: &Path, day: NaiveDate) -> Option<SessionUsage> {
    let file = File::open(path).ok()?;
    let mut model = None;
    let mut usage = Usage::default();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if line.starts_with(r#"{"kind":"usage""#) {
            if let Ok(Record::Usage { usage: u }) = serde_json::from_str::<Record>(&line) {
                usage.add(&u);
            }
        } else if model.is_none() && line.starts_with(r#"{"kind":"meta""#) {
            if let Ok(Record::Meta(meta)) = serde_json::from_str::<Record>(&line) {
                model = Some(meta.model);
            }
        }
    }
    Some(SessionUsage {
        model: model?,
        day,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_session(dir: &Path, id: &str, model: &str, turns: &[(u64, u64, f64)]) {
        let meta = session::SessionMeta {
            id: id.into(),
            label: "main".into(),
            parent: None,
            cwd: dir.to_path_buf(),
            model: model.into(),
            created: chrono::Utc::now(),
        };
        let mut lines = vec![serde_json::to_string(&Record::Meta(meta)).unwrap()];
        lines.push(r#"{"kind":"message","message":{"role":"user","content":[{"type":"text","text":"{\"kind\":\"usage\"}"}]}}"#.into());
        for &(input, output, cost) in turns {
            let usage = Usage {
                input_tokens: input,
                output_tokens: output,
                cost,
                ..Default::default()
            };
            lines.push(serde_json::to_string(&Record::Usage { usage }).unwrap());
        }
        std::fs::write(dir.join(format!("{id}.jsonl")), lines.join("\n")).unwrap();
    }

    #[test]
    fn sums_sessions_by_day_and_model() {
        let dir = tempfile::tempdir().unwrap();
        write_session(
            dir.path(),
            "a",
            "lora/fast",
            &[(100, 10, 0.5), (200, 20, 0.25)],
        );
        write_session(dir.path(), "b", "lora/smart", &[(1000, 5, 1.0)]);
        write_session(dir.path(), "c", "lora/fast", &[]);
        let today = Local::now().date_naive();
        let report = aggregate(dir.path(), today, 7);
        assert_eq!(report["today"]["input_tokens"], 1300);
        assert_eq!(report["today"]["output_tokens"], 35);
        assert_eq!(report["today"]["sessions"], 2);
        assert_eq!(report["week"]["cost"], 1.75);
        assert_eq!(report["by_model"][0]["model"], "lora/smart");
        assert_eq!(report["by_model"][1]["input_tokens"], 300);
        assert_eq!(report["by_day"][0]["date"], today.to_string());
        // A window that ends before today leaves today's sessions out.
        let tomorrow = today.checked_add_days(Days::new(1)).unwrap();
        assert_eq!(aggregate(dir.path(), tomorrow, 1)["total"]["sessions"], 0);
    }
}
