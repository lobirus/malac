//! Background jobs: processes an agent started with `bash` and
//! `background: true`, their logs, and stopping them. Jobs are read from
//! the transcript for now; the fields are what the daemon will report.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use malac_core::event::AgentId;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::app::{Action, HitMap, Outcome, Target, short_path};
use super::dialog::{frame_box, put};
use super::text::{self, sanitize};
use super::theme::theme;
use super::transcript::duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Exited,
    Stopped,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: String,
    pub agent: AgentId,
    pub command: String,
    pub cwd: PathBuf,
    pub pid: Option<u32>,
    pub log: PathBuf,
    pub status: JobStatus,
    /// Unknown until the daemon reports it.
    pub exit_code: Option<i32>,
    pub started: Instant,
}

impl Job {
    /// A job from a `bash` call that went to the background, read from the
    /// call's input and the tool's reply.
    pub fn from_tool(
        id: &str,
        agent: &str,
        cwd: &Path,
        input: &Value,
        output: &str,
    ) -> Option<Self> {
        if input["background"].as_bool() != Some(true) {
            return None;
        }
        let pid = output
            .split_once("(pid ")
            .and_then(|(_, rest)| rest.split_once(')'))
            .and_then(|(pid, _)| pid.parse().ok());
        let log = output.split_once("written to ")?.1.trim();
        Some(Self {
            id: id.to_owned(),
            agent: agent.to_owned(),
            command: input["command"].as_str().unwrap_or_default().to_owned(),
            cwd: cwd.to_owned(),
            pid,
            log: PathBuf::from(log),
            status: JobStatus::Running,
            exit_code: None,
            started: Instant::now(),
        })
    }

    pub fn is_running(&self) -> bool {
        self.status == JobStatus::Running
    }
}

/// Whether a process is still there.
#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // Signal 0 checks without sending; EPERM means it exists but is not ours.
    let sent = unsafe { libc::kill(pid, 0) } == 0;
    sent || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
pub fn alive(_pid: u32) -> bool {
    false
}

/// Stops a job's process group, which its shell leads.
#[cfg(unix)]
pub fn stop(pid: u32) -> Result<(), String> {
    let pid = libc::pid_t::try_from(pid).map_err(|e| e.to_string())?;
    match unsafe { libc::killpg(pid, libc::SIGTERM) } {
        0 => Ok(()),
        _ => Err(std::io::Error::last_os_error().to_string()),
    }
}

#[cfg(not(unix))]
pub fn stop(_pid: u32) -> Result<(), String> {
    Err("stopping jobs needs a Unix system".into())
}

/// The last `lines` lines of a log, read from its end.
pub fn tail(path: &Path, lines: usize) -> Vec<String> {
    const WINDOW: u64 = 64 * 1024;
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(WINDOW)));
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    let clean = sanitize(&text);
    let all: Vec<&str> = clean.lines().collect();
    all[all.len().saturating_sub(lines)..]
        .iter()
        .map(|l| (*l).to_owned())
        .collect()
}

/// The jobs list over the selected job's log.
#[derive(Default)]
pub struct JobsView {
    selected: usize,
}

impl JobsView {
    pub fn chosen<'a>(&self, jobs: &'a [Job]) -> Option<&'a Job> {
        jobs.get(self.selected.min(jobs.len().saturating_sub(1)))
    }

    pub fn click(&mut self, index: usize) -> Outcome {
        self.selected = index;
        Outcome::Stay
    }

    pub fn scroll_by(&mut self, delta: isize, jobs: &[Job]) {
        let last = jobs.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + delta).clamp(0, last.max(0)) as usize;
    }

    pub fn key(&mut self, key: KeyEvent, jobs: &[Job]) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Outcome::Close,
            KeyCode::Char('c') if ctrl => Outcome::Close,
            KeyCode::Up => {
                self.scroll_by(-1, jobs);
                Outcome::Stay
            }
            KeyCode::Down => {
                self.scroll_by(1, jobs);
                Outcome::Stay
            }
            KeyCode::Char('k' | 'x') | KeyCode::Delete => match self.chosen(jobs) {
                Some(job) if job.is_running() => Outcome::Run(Action::StopJob(job.id.clone())),
                _ => Outcome::Stay,
            },
            _ => Outcome::Stay,
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, jobs: &[Job], hits: &mut HitMap) {
        let t = theme();
        let screen = frame.area();
        let inner = frame_box(
            frame,
            110,
            screen.height.saturating_sub(4),
            "Jobs",
            "↑↓ select · k stop · esc close",
            hits,
        );
        if jobs.is_empty() {
            put(
                frame,
                inner,
                0,
                Line::styled("No background jobs.", t.dim()),
            );
            put(
                frame,
                inner,
                1,
                Line::styled(
                    "Agents start them with the bash tool and `background: true`, e.g. dev servers and watchers.",
                    t.faint(),
                ),
            );
            return;
        }
        self.selected = self.selected.min(jobs.len() - 1);
        let width = inner.width as usize;
        let list = (jobs.len() as u16).min(inner.height / 3).max(1);
        let first = self.selected.saturating_sub(list as usize - 1);
        for (row, (index, job)) in jobs
            .iter()
            .enumerate()
            .skip(first)
            .take(list as usize)
            .enumerate()
        {
            let selected = index == self.selected;
            let style = if selected { t.selected() } else { Style::new() };
            let (mark, mark_style, status) = match job.status {
                JobStatus::Running => ("●", t.fg(t.accent), "running".to_owned()),
                JobStatus::Exited => (
                    "○",
                    t.dim(),
                    match job.exit_code {
                        Some(code) => format!("exit {code}"),
                        None => "exited".to_owned(),
                    },
                ),
                JobStatus::Stopped => ("■", t.fg(t.warning), "stopped".to_owned()),
            };
            let quiet = if selected { style } else { t.dim() };
            let pid = job.pid.map_or("—".to_owned(), |p| p.to_string());
            let right = format!(
                "{}  {} ",
                short_path(&job.cwd),
                duration(job.started.elapsed())
            );
            let left = format!(" {status:<8} {pid:>7}  ");
            let room = width.saturating_sub(2 + text::width(&left) + text::width(&right));
            let command = text::truncate(text::first_line(&job.command), room).into_owned();
            let gap = room.saturating_sub(text::width(&command));
            let line = Line::from(vec![
                Span::styled(
                    format!(" {mark}"),
                    if selected { style } else { mark_style },
                ),
                Span::styled(left, quiet),
                Span::styled(command, style),
                Span::styled(" ".repeat(gap), style),
                Span::styled(right, quiet),
            ]);
            put(frame, inner, row as u16, line);
            hits.push(
                Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
                Target::Pick(index),
            );
        }
        let Some(job) = self.chosen(jobs) else {
            return;
        };
        let y = list + 1;
        put(
            frame,
            inner,
            y,
            Line::from(vec![
                Span::styled("log ", t.dim()),
                Span::styled(short_path(&job.log), t.faint()),
            ]),
        );
        let rows = inner.height.saturating_sub(y + 1) as usize;
        let log = tail(&job.log, rows);
        if log.is_empty() {
            put(
                frame,
                inner,
                y + 1,
                Line::styled("(no output yet)", t.faint()),
            );
        }
        for (i, line) in log.into_iter().enumerate() {
            put(frame, inner, y + 1 + i as u16, Line::styled(line, t.dim()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_jobs_from_background_bash_calls() {
        let output = "Started in background (pid 4821). Output is written to /tmp/logs/bash-1.log";
        let input = json!({"command": "npm run dev", "background": true});
        let job = Job::from_tool("c1", "main", Path::new("/work"), &input, output).unwrap();
        assert_eq!(job.pid, Some(4821));
        assert_eq!(job.log, PathBuf::from("/tmp/logs/bash-1.log"));
        assert_eq!(job.command, "npm run dev");
        assert!(
            Job::from_tool(
                "c2",
                "main",
                Path::new("/w"),
                &json!({"command": "ls"}),
                "a"
            )
            .is_none()
        );
    }

    #[test]
    fn tails_logs() {
        let path = std::env::temp_dir().join(format!("malac-job-{}.log", std::process::id()));
        std::fs::write(&path, "one\ntwo\n\x1b[31mthree\x1b[0m\n").unwrap();
        assert_eq!(tail(&path, 2), ["two", "three"]);
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn sees_and_stops_processes() {
        use std::os::unix::process::CommandExt;
        let child = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(alive(pid));
        stop(pid).unwrap();
        let mut child = child;
        child.wait().unwrap();
        assert!(!alive(pid));
    }
}
