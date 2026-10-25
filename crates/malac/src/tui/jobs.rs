//! The jobs view: background processes the agents started (`bash` with
//! `background: true`), as the daemon reports them, with the selected
//! job's log tail and a way to stop it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use malac_core::jobs::JobStatus;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::app::{Action, HitMap, Outcome, Target, short_path};
use super::dialog::{frame_box, put};
use super::text::{self, sanitize};
use super::theme::theme;
use super::transcript::duration;
use crate::protocol::JobView;

/// The jobs list over the selected job's log.
#[derive(Default)]
pub struct JobsView {
    selected: usize,
    /// The log tail last fetched, and the job it belongs to.
    pub log: Option<(String, String)>,
    /// Rows of log the view has room for.
    pub rows: usize,
}

impl JobsView {
    pub fn chosen<'a>(&self, jobs: &'a [JobView]) -> Option<&'a JobView> {
        jobs.get(self.selected.min(jobs.len().saturating_sub(1)))
    }

    pub fn click(&mut self, index: usize) -> Outcome {
        self.selected = index;
        Outcome::Stay
    }

    pub fn scroll_by(&mut self, delta: isize, jobs: &[JobView]) {
        let last = jobs.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + delta).clamp(0, last.max(0)) as usize;
    }

    pub fn key(&mut self, key: KeyEvent, jobs: &[JobView]) -> Outcome {
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
                Some(job) if job.job.is_running() => {
                    Outcome::Run(Action::StopJob(job.job.id.clone()))
                }
                _ => Outcome::Stay,
            },
            _ => Outcome::Stay,
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, jobs: &[JobView], hits: &mut HitMap) {
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
        for (row, (index, view)) in jobs
            .iter()
            .enumerate()
            .skip(first)
            .take(list as usize)
            .enumerate()
        {
            let job = &view.job;
            let selected = index == self.selected;
            let style = if selected { t.selected() } else { Style::new() };
            let (mark, mark_style) = match job.status {
                JobStatus::Running => ("●", t.fg(t.accent)),
                JobStatus::Exited if job.exit_code == Some(0) => ("○", t.dim()),
                JobStatus::Exited => ("✗", t.fg(t.error)),
                JobStatus::Killed => ("■", t.fg(t.warning)),
                JobStatus::Unknown => ("?", t.dim()),
            };
            let quiet = if selected { style } else { t.dim() };
            let pid = job.pid.map_or("—".to_owned(), |p| p.to_string());
            let ran = job.finished.unwrap_or_else(chrono::Utc::now) - job.started;
            let ran = duration(ran.to_std().unwrap_or_default());
            let folder = job.cwd.file_name().map_or_else(
                || short_path(&job.cwd),
                |n| n.to_string_lossy().into_owned(),
            );
            let right = format!("{}  {ran} ", text::truncate(&folder, 24));
            let left = format!(" {:<9} {pid:>7}  ", job.describe_status());
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
        let Some(view) = self.chosen(jobs) else {
            return;
        };
        let y = list + 1;
        put(
            frame,
            inner,
            y,
            Line::from(vec![
                Span::styled("log ", t.dim()),
                Span::styled(short_path(&view.job.log), t.faint()),
            ]),
        );
        self.rows = inner.height.saturating_sub(y + 1) as usize;
        let log = match &self.log {
            Some((id, text)) if *id == view.job.id => sanitize(text).into_owned(),
            _ => String::new(),
        };
        let lines: Vec<&str> = log.lines().collect();
        if lines.is_empty() {
            put(
                frame,
                inner,
                y + 1,
                Line::styled("(no output yet)", t.faint()),
            );
        }
        let start = lines.len().saturating_sub(self.rows);
        for (i, line) in lines[start..].iter().enumerate() {
            put(
                frame,
                inner,
                y + 1 + i as u16,
                Line::styled((*line).to_owned(), t.dim()),
            );
        }
    }

    /// The job whose log the view shows.
    pub fn shown_job(&self, jobs: &[JobView]) -> Option<String> {
        self.chosen(jobs).map(|j| j.job.id.clone())
    }
}
