//! The header (menu bar and tabs), the status line and the function-key bar.
//! Each is laid out into one line plus the clickable ranges on it.

use malac_core::permission::Mode;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::app::{Action, Target};
use super::menu::{TITLES, mnemonic};
use super::text;
use super::theme::theme;
use crate::render::tokens;

/// A clickable column range: start, width and what it does.
pub type Spot = (u16, u16, Target);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Idle,
    Running(&'static str),
    Waiting,
    Done,
    Failed,
}

pub struct TabInfo {
    pub label: String,
    pub mark: Mark,
}

struct Builder {
    spans: Vec<Span<'static>>,
    spots: Vec<Spot>,
    x: u16,
}

impl Builder {
    fn new() -> Self {
        Self {
            spans: Vec::new(),
            spots: Vec::new(),
            x: 0,
        }
    }

    fn push(&mut self, text: impl Into<String>, style: Style) {
        let text = text.into();
        self.x += text::width(&text) as u16;
        self.spans.push(Span::styled(text, style));
    }

    fn extend(&mut self, spans: Vec<Span<'static>>) {
        for span in spans {
            self.x += text::width(&span.content) as u16;
            self.spans.push(span);
        }
    }

    fn spot(&mut self, start: u16, target: Target) {
        self.spots.push((start, self.x - start, target));
    }

    fn pad_to(&mut self, x: u16, style: Style) {
        if x > self.x {
            self.push(" ".repeat((x - self.x) as usize), style);
        }
    }

    fn line(self, style: Style) -> (Line<'static>, Vec<Spot>) {
        (Line::from(self.spans).style(style), self.spots)
    }
}

fn tab_text(index: usize, tab: &TabInfo) -> (String, String) {
    let mark = match tab.mark {
        Mark::Idle => "",
        Mark::Running(frame) => frame,
        Mark::Waiting => "?",
        Mark::Done => "✓",
        Mark::Failed => "✗",
    };
    let label = text::truncate(&tab.label, 18).into_owned();
    let label = if mark.is_empty() {
        format!(" {} {label} ", index + 1)
    } else {
        format!(" {} {label} {mark} ", index + 1)
    };
    (label, mark.to_owned())
}

/// Which tabs fit in `room` columns: a window around the active one.
fn tab_window(widths: &[u16], active: usize, room: u16) -> (usize, usize) {
    let (mut start, mut end) = (active, active + 1);
    let mut used = widths.get(active).copied().unwrap_or(0);
    loop {
        let grew_right = end < widths.len() && used + widths[end] + 2 <= room;
        if grew_right {
            used += widths[end];
            end += 1;
        }
        let grew_left = start > 0 && used + widths[start - 1] + 2 <= room;
        if grew_left {
            start -= 1;
            used += widths[start];
        }
        if !grew_right && !grew_left {
            return (start, end);
        }
    }
}

/// Menu titles, then the tabs, then `right` at the far edge when there is room.
pub fn header(
    highlighted: Option<usize>,
    tabs: &[TabInfo],
    active: usize,
    right: &str,
    width: u16,
) -> (Line<'static>, Vec<Spot>) {
    let t = theme();
    let bar = t.bar();
    let mut b = Builder::new();
    for (index, (title, key)) in TITLES.iter().enumerate() {
        let style = if highlighted == Some(index) {
            t.selected()
        } else {
            bar
        };
        let start = b.x;
        b.push(" ", style);
        b.extend(mnemonic(title, Some(*key), style));
        b.push(" ", style);
        b.spot(start, Target::MenuTitle(index));
    }
    b.push(" │", bar.fg(t.faint));

    let close = tabs.len() > 1;
    let widths: Vec<u16> = tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| {
            let extra = if close && i == active { 2 } else { 0 };
            text::width(&tab_text(i, tab).0) as u16 + extra
        })
        .collect();
    let right_width = text::width(right) as u16 + 1;
    let room = width.saturating_sub(b.x);
    let fits_right = widths.iter().sum::<u16>() + right_width <= room;
    let (start, end) = tab_window(&widths, active, room);
    if start > 0 {
        b.push("‹", bar.fg(t.dim));
    }
    for (index, tab) in tabs.iter().enumerate().take(end).skip(start) {
        let (label, mark) = tab_text(index, tab);
        let style = if index == active {
            Style::new().add_modifier(Modifier::BOLD)
        } else {
            bar.fg(t.dim)
        };
        let mark_style = match tab.mark {
            Mark::Waiting => style.fg(t.warning),
            Mark::Failed => style.fg(t.error),
            Mark::Done if index == active => style.fg(t.accent),
            _ => style,
        };
        let begin = b.x;
        match label.strip_suffix(&format!("{mark} ")) {
            Some(head) if !mark.is_empty() => {
                b.push(head.to_owned(), style);
                b.push(format!("{mark} "), mark_style);
            }
            _ => b.push(label, style),
        }
        b.spot(begin, Target::Tab(index));
        if close && index == active {
            let x = b.x;
            b.push("× ", style.fg(t.dim));
            b.spot(x, Target::CloseTab(index));
        }
    }
    if end < tabs.len() {
        b.push("›", bar.fg(t.dim));
    }
    if fits_right && !right.is_empty() {
        b.pad_to(width.saturating_sub(right_width), bar);
        let start = b.x;
        b.push(format!("{right} "), bar.fg(t.dim));
        b.spot(start, Target::Do(Action::Workspaces));
    }
    b.pad_to(width, bar);
    b.line(bar)
}

/// What the status line reports.
pub struct Status {
    pub left: Vec<Span<'static>>,
    pub model: String,
    pub mode: Mode,
    /// Tokens in context and the window size, when known.
    pub context: Option<(u64, Option<u64>)>,
    pub tokens: u64,
    pub cost: f64,
    pub running: usize,
    /// Subagents under the tab's session, running or not.
    pub subagents: usize,
    /// Background jobs still running.
    pub jobs: usize,
    pub spinner: &'static str,
    pub approvals: usize,
    /// `Some(remote)` while the web UI serves.
    pub web: Option<bool>,
    pub update: Option<String>,
    /// The daemon runs another version than this terminal.
    pub daemon: Option<String>,
    /// What computer control is missing.
    pub control: Option<&'static str>,
    /// The saved connection, when the daemon is on another machine.
    pub connection: Option<String>,
    /// The daemon holds a keep-awake assertion.
    pub awake: bool,
    pub branch: Option<String>,
    pub clock: String,
}

struct Segment {
    spans: Vec<Span<'static>>,
    priority: u8,
    target: Option<Target>,
}

fn segment(spans: Vec<Span<'static>>, priority: u8, target: Option<Action>) -> Segment {
    Segment {
        spans,
        priority,
        target: target.map(Target::Do),
    }
}

fn context_bar(used: u64, window: u64) -> String {
    let fraction = (used as f64 / window.max(1) as f64).clamp(0.0, 1.0);
    let filled = (fraction * 5.0).round() as usize;
    format!(
        "{}{} {:>3}%",
        "▰".repeat(filled),
        "▱".repeat(5 - filled),
        (fraction * 100.0).round() as u32
    )
}

/// `left` takes whatever the segments, collapsed by priority, leave over.
pub fn status_line(status: &Status, width: u16) -> (Line<'static>, Vec<Spot>) {
    let t = theme();
    let bar = t.bar();
    let dim = bar.fg(t.dim);
    let mut segments = Vec::new();
    if let Some(name) = &status.connection {
        segments.push(segment(
            vec![Span::styled(
                format!("@{}", text::truncate(name, 16)),
                bar.fg(t.accent).add_modifier(Modifier::BOLD),
            )],
            1,
            Some(Action::OpenMenu(5)),
        ));
    }
    segments.push(segment(
        vec![Span::styled(status.model.clone(), bar)],
        1,
        Some(Action::PickModel),
    ));
    let mode_style = match status.mode {
        Mode::Autopilot => bar.fg(t.warning).add_modifier(Modifier::BOLD),
        _ => bar,
    };
    segments.push(segment(
        vec![Span::styled(status.mode.as_str(), mode_style)],
        0,
        Some(Action::CycleMode),
    ));
    if let Some((used, window)) = status.context {
        let text = match window {
            Some(window) => context_bar(used, window),
            None => format!("{:>5}", tokens(used)),
        };
        segments.push(segment(
            vec![Span::styled("ctx ", dim), Span::styled(text, bar)],
            5,
            None,
        ));
    }
    if status.tokens > 0 || status.cost > 0.0 {
        let mut spans = vec![Span::styled(format!("{:>5}", tokens(status.tokens)), bar)];
        if status.cost > 0.0 {
            spans.push(Span::styled(format!(" ${:.2}", status.cost), bar));
        }
        segments.push(segment(spans, 6, Some(Action::ShowCost)));
    }
    if status.running > 0 {
        let noun = if status.running == 1 {
            "agent"
        } else {
            "agents"
        };
        segments.push(segment(
            vec![Span::styled(
                format!("{} {} {noun}", status.spinner, status.running),
                bar,
            )],
            3,
            Some(Action::AgentMap),
        ));
    } else if status.subagents > 0 {
        let noun = if status.subagents == 1 {
            "subagent"
        } else {
            "subagents"
        };
        segments.push(segment(
            vec![Span::styled(format!("{} {noun}", status.subagents), dim)],
            6,
            Some(Action::AgentMap),
        ));
    }
    if status.jobs > 0 {
        let noun = if status.jobs == 1 { "job" } else { "jobs" };
        segments.push(segment(
            vec![Span::styled(format!("{} {noun}", status.jobs), bar)],
            3,
            Some(Action::Jobs),
        ));
    }
    if status.approvals > 0 {
        segments.push(segment(
            vec![Span::styled(
                format!("? {} waiting", status.approvals),
                bar.fg(t.warning).add_modifier(Modifier::BOLD),
            )],
            2,
            Some(Action::ShowApprovals),
        ));
    }
    if let Some(remote) = status.web {
        let label = if remote { "web remote" } else { "web" };
        segments.push(segment(
            vec![Span::styled(label, bar)],
            7,
            Some(Action::CopyWebUrl),
        ));
    }
    if status.awake {
        segments.push(segment(
            vec![Span::styled("awake", dim)],
            7,
            Some(Action::OpenMenu(4)),
        ));
    }
    if let Some(update) = &status.update {
        segments.push(segment(
            vec![Span::styled(update.clone(), bar.fg(t.accent))],
            4,
            Some(Action::UpdateDialog),
        ));
    }
    if let Some(daemon) = &status.daemon {
        segments.push(segment(
            vec![Span::styled(daemon.clone(), bar.fg(t.warning))],
            4,
            Some(Action::RestartDaemon),
        ));
    }
    if let Some(control) = status.control {
        segments.push(segment(
            vec![Span::styled(control, bar.fg(t.warning))],
            5,
            Some(Action::RequestPermissions),
        ));
    }
    if let Some(branch) = &status.branch {
        segments.push(segment(
            vec![Span::styled(text::truncate(branch, 20).into_owned(), dim)],
            8,
            None,
        ));
    }
    segments.push(segment(
        vec![Span::styled(status.clock.clone(), dim)],
        9,
        None,
    ));

    let segment_width = |s: &Segment| {
        s.spans
            .iter()
            .map(|x| text::width(&x.content))
            .sum::<usize>()
            + 3
    };
    let min_left = 12.min(width as usize / 3);
    while segments.iter().map(segment_width).sum::<usize>() + min_left > width as usize {
        let Some(lowest) = segments
            .iter()
            .enumerate()
            .max_by_key(|(_, s)| s.priority)
            .map(|(i, _)| i)
        else {
            break;
        };
        segments.remove(lowest);
    }

    let right: usize = segments.iter().map(segment_width).sum::<usize>() + 1;
    let room = (width as usize).saturating_sub(right + 1);
    let mut b = Builder::new();
    b.push(" ", bar);
    let left = text::truncate_line(Line::from(status.left.clone()), room);
    b.extend(left.spans);
    b.pad_to(width.saturating_sub(right as u16), bar);
    for segment in segments {
        b.push(" │ ", bar.fg(t.faint));
        let start = b.x;
        b.extend(segment.spans);
        if let Some(target) = segment.target {
            b.spot(start, target);
        }
    }
    b.pad_to(width, bar);
    b.line(bar)
}

/// The function keys, as in Midnight Commander.
pub const KEYS: [(&str, Action); 10] = [
    ("Help", Action::Help),
    ("Copy", Action::Copy),
    ("Agents", Action::Agents),
    ("Model", Action::PickModel),
    ("Mode", Action::CycleMode),
    ("Tabs", Action::TabMenu),
    ("Resume", Action::Sessions),
    ("Stop", Action::Stop),
    ("Menu", Action::Menu),
    ("Quit", Action::Quit),
];

/// `1Help 2Copy …`: each key gets an equal share of the width, its number
/// in the default colors and its label as a block, as in Midnight Commander.
pub fn key_bar(width: u16) -> (Line<'static>, Vec<Spot>) {
    let t = theme();
    let label_style = Style::new().bg(t.key_bg);
    let mut b = Builder::new();
    let count = KEYS.len() as u16;
    for (index, (label, action)) in KEYS.iter().enumerate() {
        let number = (index + 1).to_string();
        let cell = width * (index as u16 + 1) / count - width * index as u16 / count;
        let room = (cell as usize).saturating_sub(number.len());
        let start = b.x;
        b.push(number, Style::new());
        let label: String = label.chars().take(room).collect();
        let pad = room.saturating_sub(text::width(&label));
        b.push(format!("{label}{}", " ".repeat(pad)), label_style);
        b.spot(start, Target::Do(action.clone()));
    }
    b.pad_to(width, Style::new());
    b.line(Style::new())
}

/// The target under column `x`, if any.
#[cfg(test)]
fn spot_at(spots: &[Spot], x: u16) -> Option<&Target> {
    spots
        .iter()
        .find(|(start, w, _)| x >= *start && x < start + w)
        .map(|(_, _, target)| target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(n: usize) -> Vec<TabInfo> {
        (0..n)
            .map(|i| TabInfo {
                label: format!("session {i}"),
                mark: if i == 0 { Mark::Done } else { Mark::Idle },
            })
            .collect()
    }

    #[test]
    fn header_lists_menus_then_tabs() {
        let (line, spots) = header(None, &tabs(2), 1, "default", 100);
        let text = line.to_string();
        assert!(text.starts_with(
            " Session  Model  Mode  Agents  View  Connection  Help  │ 1 session 0 ✓  2 session 1 × "
        ));
        assert!(text.ends_with("default "), "the workspace's name");
        let x = text.find("default").unwrap() as u16;
        assert_eq!(spot_at(&spots, x), Some(&Target::Do(Action::Workspaces)));
        assert_eq!(text::width(&text), 100);
        let x = text.find("2 session").unwrap() as u16;
        assert_eq!(spot_at(&spots, x), Some(&Target::Tab(1)));
        assert_eq!(spot_at(&spots, 1), Some(&Target::MenuTitle(0)));
        let close = text.chars().position(|c| c == '×').unwrap() as u16;
        assert_eq!(spot_at(&spots, close), Some(&Target::CloseTab(1)));
    }

    #[test]
    fn many_tabs_scroll_around_the_active_one() {
        let (line, _) = header(None, &tabs(9), 6, "", 80);
        let text = line.to_string();
        assert!(text.contains("‹"), "{text}");
        assert!(text.contains(" 7 session 6 × "), "{text}");
        assert_eq!(text::width(&text), 80);
    }

    fn status() -> Status {
        Status {
            left: vec![Span::raw("~/code/malac")],
            model: "lora/smart".into(),
            mode: Mode::Autopilot,
            context: Some((34_000, Some(100_000))),
            tokens: 12_345,
            cost: 0.1234,
            running: 2,
            subagents: 3,
            jobs: 2,
            spinner: "⠋",
            approvals: 1,
            web: Some(false),
            update: None,
            daemon: None,
            control: None,
            connection: None,
            awake: false,
            branch: Some("main".into()),
            clock: "14:05".into(),
        }
    }

    #[test]
    fn status_segments_collapse_by_priority() {
        let (wide, spots) = status_line(&status(), 160);
        let text = wide.to_string();
        assert!(text.contains("lora/smart │ autopilot │ ctx ▰▰▱▱▱  34% │ 12.3k $0.12 │ ⠋ 2 agents │ 2 jobs │ ? 1 waiting │ web │ main │ 14:05"), "{text}");
        let x = text.find("autopilot").unwrap();
        let col = text::width(&text[..x]) as u16;
        assert_eq!(spot_at(&spots, col), Some(&Target::Do(Action::CycleMode)));
        let col = text::width(&text[..text.find("2 jobs").unwrap()]) as u16;
        assert_eq!(spot_at(&spots, col), Some(&Target::Do(Action::Jobs)));
        let col = text::width(&text[..text.find("2 agents").unwrap()]) as u16;
        assert_eq!(spot_at(&spots, col), Some(&Target::Do(Action::AgentMap)));
        let (narrow, _) = status_line(&status(), 60);
        let text = narrow.to_string();
        assert_eq!(text::width(&text), 60);
        assert!(text.contains("autopilot"));
        assert!(text.contains("lora/smart"));
        assert!(!text.contains("14:05"));
        assert!(!text.contains("main"));
    }

    #[test]
    fn remote_daemons_and_keep_awake_show() {
        let status = Status {
            connection: Some("bstation".into()),
            awake: true,
            ..status()
        };
        let (line, spots) = status_line(&status, 160);
        let text = line.to_string();
        assert!(text.contains("@bstation │ lora/smart"), "{text}");
        assert!(text.contains("│ web │ awake │ main"), "{text}");
        let col = text::width(&text[..text.find("@bstation").unwrap()]) as u16;
        assert_eq!(spot_at(&spots, col), Some(&Target::Do(Action::OpenMenu(5))));
        let (narrow, _) = status_line(&status, 60);
        assert!(
            narrow.to_string().contains("@bstation"),
            "the daemon shows first"
        );
    }

    #[test]
    fn key_bar_spreads_ten_keys() {
        let (line, spots) = key_bar(80);
        let text = line.to_string();
        assert_eq!(text::width(&text), 80);
        assert!(
            text.starts_with("1Help   2Copy   3Agents 4Model  5Mode   6Tabs   7Resume 8Stop"),
            "{text}"
        );
        assert!(text.contains("10Quit"));
        assert_eq!(spot_at(&spots, 0), Some(&Target::Do(Action::Help)));
        assert_eq!(spot_at(&spots, 79), Some(&Target::Do(Action::Quit)));
    }
}
