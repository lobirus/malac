//! The agent map: each session agent and its subagents as boxes, children
//! to the right of their parent and joined to it, scrolled when large.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use malac_core::event::AgentId;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::app::{Action, HitMap, Outcome, Target};
use super::dialog::frame_box;
use super::markdown;
use super::text;
use super::theme::theme;
use crate::render::tokens;

const BOX_W: u16 = 30;
const BOX_H: u16 = 6;
const GAP_X: u16 = 6;
const GAP_Y: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Running,
    /// Waiting for the user to approve a tool call.
    Waiting,
    Done,
    Failed,
    Cancelled,
}

/// One agent as the map shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: AgentId,
    pub parent: Option<AgentId>,
    pub label: String,
    pub model: String,
    pub state: State,
    pub activity: String,
    pub tokens: u64,
    pub cost: f64,
    /// How deep the agent is nested; session agents are 0.
    pub depth: u32,
    /// The tab a session agent is shown in.
    pub tab: Option<usize>,
}

pub struct AgentMap {
    /// Depth first, parents before their children; refreshed every frame.
    nodes: Vec<Node>,
    selected: Option<AgentId>,
    scroll: (u16, u16),
    /// Keep the selected box in view; off once the wheel moves the map.
    follow: bool,
    spinner: &'static str,
}

impl AgentMap {
    pub fn new(nodes: Vec<Node>, selected: Option<AgentId>) -> Self {
        let mut map = Self {
            nodes: Vec::new(),
            selected,
            scroll: (0, 0),
            follow: true,
            spinner: "",
        };
        map.set_nodes(nodes, "");
        map
    }

    pub fn set_nodes(&mut self, nodes: Vec<Node>, spinner: &'static str) {
        self.nodes = nodes;
        self.spinner = spinner;
        if self.index().is_none() {
            self.selected = self.nodes.first().map(|n| n.id.clone());
        }
    }

    fn index(&self) -> Option<usize> {
        let id = self.selected.as_ref()?;
        self.nodes.iter().position(|n| &n.id == id)
    }

    pub fn selected(&self) -> Option<&Node> {
        self.index().map(|i| &self.nodes[i])
    }

    fn parent_of(&self, index: usize) -> Option<usize> {
        let parent = self.nodes[index].parent.as_ref()?;
        self.nodes[..index].iter().position(|n| &n.id == parent)
    }

    fn select(&mut self, index: usize) {
        if let Some(node) = self.nodes.get(index) {
            self.selected = Some(node.id.clone());
            self.follow = true;
        }
    }

    /// Selects a box by its row in the hit map; a second click opens it.
    pub fn click(&mut self, index: usize, count: u8) -> Outcome {
        self.select(index);
        if count >= 2 {
            return self.run(Action::OpenAgent);
        }
        Outcome::Stay
    }

    pub fn scroll_by(&mut self, dx: isize, dy: isize) {
        self.follow = false;
        self.scroll.0 = (self.scroll.0 as isize + dx).max(0) as u16;
        self.scroll.1 = (self.scroll.1 as isize + dy).max(0) as u16;
    }

    fn run(&self, action: fn(AgentId) -> Action) -> Outcome {
        match self.selected() {
            Some(node) => Outcome::Run(action(node.id.clone())),
            None => Outcome::Stay,
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let Some(at) = self.index() else {
            return match key.code {
                KeyCode::Esc => Outcome::Close,
                _ => Outcome::Stay,
            };
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Close,
            KeyCode::Char('c') if ctrl => return Outcome::Close,
            KeyCode::Enter | KeyCode::Char(' ') => return self.run(Action::OpenAgent),
            KeyCode::Tab | KeyCode::Char('t') => return self.run(Action::PinAgent),
            KeyCode::Char('x') => return self.run(Action::StopAgent),
            KeyCode::Up | KeyCode::Char('k') => self.select(at.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.select(at + 1),
            KeyCode::Left | KeyCode::Char('h') => {
                if let Some(parent) = self.parent_of(at) {
                    self.select(parent);
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let id = &self.nodes[at].id;
                if let Some(child) = self
                    .nodes
                    .iter()
                    .position(|n| n.parent.as_ref() == Some(id))
                {
                    self.select(child);
                }
            }
            KeyCode::Home => self.select(0),
            KeyCode::End => self.select(self.nodes.len().saturating_sub(1)),
            _ => {}
        }
        Outcome::Stay
    }

    /// Where each box goes, in columns and rows of the whole map: a parent
    /// level with its first child, later children below the subtrees before.
    fn layout(&self) -> Vec<(u16, u16)> {
        let mut out: Vec<(u16, u16)> = Vec::with_capacity(self.nodes.len());
        let mut slots: Vec<u16> = Vec::with_capacity(self.nodes.len());
        let mut columns: Vec<u16> = Vec::with_capacity(self.nodes.len());
        let mut used: Option<u16> = None;
        for i in 0..self.nodes.len() {
            let parent = self.parent_of(i);
            let first_child = parent.is_some_and(|p| {
                !self.nodes[p + 1..i]
                    .iter()
                    .any(|n| n.parent == self.nodes[i].parent)
            });
            let slot = match parent {
                Some(p) if first_child => slots[p],
                _ => used.map_or(0, |u| u + 1),
            };
            let column = parent.map_or(0, |p| columns[p] + 1);
            used = Some(used.map_or(slot, |u| u.max(slot)));
            slots.push(slot);
            columns.push(column);
            out.push((column * (BOX_W + GAP_X), slot * (BOX_H + GAP_Y)));
        }
        out
    }

    pub fn draw(&mut self, frame: &mut Frame, hits: &mut HitMap) {
        let t = theme();
        let screen = frame.area();
        let hint = "←↑↓→ move · enter open · t tab · x stop · F3 list · esc close";
        let area = frame_box(
            frame,
            screen.width.saturating_sub(4),
            screen.height.saturating_sub(2),
            "Agent map",
            hint,
            hits,
        );
        if self.nodes.is_empty() {
            let line = Line::styled("no agents", t.dim());
            frame.render_widget(Paragraph::new(line), area);
            return;
        }
        let places = self.layout();
        let width = places.iter().map(|p| p.0 + BOX_W).max().unwrap_or(0);
        let height = places.iter().map(|p| p.1 + BOX_H).max().unwrap_or(0);
        if self.follow {
            if let Some(i) = self.index() {
                let (x, y) = places[i];
                let (sx, sy) = &mut self.scroll;
                if x < *sx {
                    *sx = x;
                } else if x + BOX_W > *sx + area.width {
                    *sx = (x + BOX_W).saturating_sub(area.width);
                }
                if y < *sy {
                    *sy = y;
                } else if y + BOX_H > *sy + area.height {
                    *sy = (y + BOX_H).saturating_sub(area.height);
                }
            }
        }
        self.scroll.0 = self.scroll.0.min(width.saturating_sub(area.width));
        self.scroll.1 = self.scroll.1.min(height.saturating_sub(area.height));
        let canvas = Canvas {
            area,
            scroll: self.scroll,
        };
        let selected = self.index();
        for (i, node) in self.nodes.iter().enumerate() {
            let (x, y) = places[i];
            self.draw_box(
                frame.buffer_mut(),
                &canvas,
                node,
                (x, y),
                selected == Some(i),
            );
            if let Some(rect) = canvas.visible(Rect::new(x, y, BOX_W, BOX_H)) {
                hits.push(rect, Target::Pick(i));
            }
        }
        for i in 0..self.nodes.len() {
            self.connect(frame.buffer_mut(), &canvas, &places, i);
        }
        if width > area.width || height > area.height {
            let label = format!(
                " {}–{} of {} cols · {}–{} of {} rows ",
                self.scroll.0 + 1,
                (self.scroll.0 + area.width).min(width),
                width,
                self.scroll.1 + 1,
                (self.scroll.1 + area.height).min(height),
                height
            );
            let w = text::width(&label) as u16;
            let rect = Rect::new(area.right().saturating_sub(w), area.bottom(), w, 1);
            frame.render_widget(
                Paragraph::new(Line::styled(label, t.faint())),
                rect.intersection(screen),
            );
        }
    }

    /// The line from a box's parent to it, along a bus shared by siblings.
    fn connect(&self, buf: &mut Buffer, canvas: &Canvas, places: &[(u16, u16)], i: usize) {
        let Some(p) = self.parent_of(i) else {
            return;
        };
        let t = theme();
        let style = if self.nodes[i].state == State::Running {
            t.dim()
        } else {
            t.faint()
        };
        let mid = BOX_H / 2 - 1;
        let (px, py) = places[p];
        let (cx, cy) = places[i];
        let bus = px + BOX_W + GAP_X / 2 - 1;
        let siblings: Vec<usize> = (0..self.nodes.len())
            .filter(|&j| self.parent_of(j) == Some(p))
            .collect();
        let Some(order) = siblings.iter().position(|&j| j == i) else {
            return;
        };
        let first = order == 0;
        let last = order + 1 == siblings.len();
        if first {
            for x in px + BOX_W..bus {
                canvas.put(buf, x, py + mid, "─", style);
            }
        } else {
            let previous = places[siblings[order - 1]].1;
            for y in previous + mid + 1..cy + mid {
                canvas.put(buf, bus, y, "│", style);
            }
        }
        let junction = match (first, last) {
            (true, true) => "─",
            (true, false) => "┬",
            (false, false) => "├",
            (false, true) => "└",
        };
        canvas.put(buf, bus, cy + mid, junction, style);
        for x in bus + 1..cx {
            canvas.put(buf, x, cy + mid, "─", style);
        }
        canvas.put(buf, cx.saturating_sub(1), cy + mid, "▸", style);
    }

    fn draw_box(
        &self,
        buf: &mut Buffer,
        canvas: &Canvas,
        node: &Node,
        at: (u16, u16),
        selected: bool,
    ) {
        let t = theme();
        let (x, y) = at;
        let (border, mark, status, status_style) = match node.state {
            State::Running => (Style::new(), self.spinner, "running", t.fg(t.accent)),
            State::Waiting => (
                t.fg(t.warning),
                "?",
                "awaiting approval",
                t.fg(t.warning).add_modifier(Modifier::BOLD),
            ),
            State::Done => (t.faint(), "✓", "done", t.fg(t.accent)),
            State::Failed => (t.fg(t.error), "✗", "failed", t.fg(t.error)),
            State::Cancelled => (t.faint(), "■", "cancelled", t.fg(t.warning)),
            State::Idle => (t.faint(), "○", "idle", t.dim()),
        };
        // The selected box is drawn heavy, and bright when its state is quiet.
        let border = match selected {
            true if border == t.faint() => Style::new().add_modifier(Modifier::BOLD),
            true => border.add_modifier(Modifier::BOLD),
            false => border,
        };
        let [tl, tr, bl, br, h, v] = if selected {
            ["┏", "┓", "┗", "┛", "━", "┃"]
        } else {
            ["╭", "╮", "╰", "╯", "─", "│"]
        };
        let right = x + BOX_W - 1;
        let bottom = y + BOX_H - 1;
        for cx in x + 1..right {
            canvas.put(buf, cx, y, h, border);
            canvas.put(buf, cx, bottom, h, border);
        }
        for cy in y + 1..bottom {
            canvas.put(buf, x, cy, v, border);
            canvas.put(buf, right, cy, v, border);
            for cx in x + 1..right {
                canvas.put(buf, cx, cy, " ", Style::new());
            }
        }
        canvas.put(buf, x, y, tl, border);
        canvas.put(buf, right, y, tr, border);
        canvas.put(buf, x, bottom, bl, border);
        canvas.put(buf, right, bottom, br, border);

        let room = (BOX_W - 4) as usize;
        let tag = match node.tab {
            Some(tab) => format!(" tab {} ", tab + 1),
            None => format!(" d{} ", node.depth),
        };
        let label_room = room.saturating_sub(text::width(&tag) + 1);
        let label = format!(
            " {} ",
            text::truncate(&node.label, label_room.saturating_sub(2))
        );
        let label_style = if selected { t.selected() } else { t.bold() };
        canvas.line(buf, x + 1, y, Line::styled(label, label_style), room);
        let tag_x = right.saturating_sub(text::width(&tag) as u16);
        canvas.line(buf, tag_x, y, Line::styled(tag, t.faint()), BOX_W as usize);

        // Reports are markdown; their first line is shown with its inline styles.
        let activity = match text::first_line(&node.activity) {
            "" => Line::styled("—", t.faint()),
            first => markdown::render(first, room)
                .lines
                .into_iter()
                .next()
                .unwrap_or_default(),
        };
        let usage = if node.cost > 0.0 {
            format!("{} tokens · ${:.4}", tokens(node.tokens), node.cost)
        } else {
            format!("{} tokens", tokens(node.tokens))
        };
        let rows = [
            Line::from(vec![
                Span::styled(format!("{mark} "), status_style),
                Span::styled(status, status_style),
            ]),
            activity,
            Line::styled(node.model.clone(), t.dim()),
            Line::styled(usage, t.dim()),
        ];
        for (i, row) in rows.into_iter().enumerate() {
            canvas.line(buf, x + 2, y + 1 + i as u16, row, room);
        }
    }
}

/// The visible part of the map: an area of the screen and the map
/// coordinates at its top left.
struct Canvas {
    area: Rect,
    scroll: (u16, u16),
}

impl Canvas {
    fn screen(&self, x: u16, y: u16) -> Option<(u16, u16)> {
        let sx = x.checked_sub(self.scroll.0)? + self.area.x;
        let sy = y.checked_sub(self.scroll.1)? + self.area.y;
        self.area
            .contains(Position { x: sx, y: sy })
            .then_some((sx, sy))
    }

    fn visible(&self, rect: Rect) -> Option<Rect> {
        let x = rect.x.max(self.scroll.0);
        let y = rect.y.max(self.scroll.1);
        let right = rect.right().min(self.scroll.0 + self.area.width);
        let bottom = rect.bottom().min(self.scroll.1 + self.area.height);
        if x >= right || y >= bottom {
            return None;
        }
        let (sx, sy) = self.screen(x, y)?;
        Some(Rect::new(sx, sy, right - x, bottom - y))
    }

    fn put(&self, buf: &mut Buffer, x: u16, y: u16, symbol: &str, style: Style) {
        if let Some((sx, sy)) = self.screen(x, y) {
            buf[(sx, sy)].set_symbol(symbol).set_style(style);
        }
    }

    /// Writes a line cut to `room` columns, clipped to the visible part.
    fn line(&self, buf: &mut Buffer, x: u16, y: u16, line: Line<'static>, room: usize) {
        let mut col = x;
        let line = text::truncate_line(line, room);
        for span in &line.spans {
            let style = line.style.patch(span.style);
            for g in span.content.graphemes(true) {
                self.put(buf, col, y, g, style);
                col += g.width().max(1) as u16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, parent: Option<&str>, state: State) -> Node {
        Node {
            id: id.into(),
            parent: parent.map(Into::into),
            label: id.into(),
            model: "lora/smart".into(),
            state,
            activity: String::new(),
            tokens: 1200,
            cost: 0.0,
            depth: 0,
            tab: None,
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn lays_children_out_to_the_right() {
        let map = AgentMap::new(
            vec![
                node("main", None, State::Running),
                node("a", Some("main"), State::Done),
                node("a1", Some("a"), State::Running),
                node("b", Some("main"), State::Failed),
            ],
            None,
        );
        assert_eq!(map.layout(), vec![(0, 0), (36, 0), (72, 0), (36, 7)]);
    }

    #[test]
    fn arrows_follow_the_tree() {
        let mut map = AgentMap::new(
            vec![
                node("main", None, State::Running),
                node("a", Some("main"), State::Done),
                node("a1", Some("a"), State::Running),
                node("b", Some("main"), State::Failed),
            ],
            Some("a1".into()),
        );
        map.key(press(KeyCode::Left));
        assert_eq!(map.selected().unwrap().id, "a");
        map.key(press(KeyCode::Down));
        map.key(press(KeyCode::Down));
        assert_eq!(map.selected().unwrap().id, "b");
        map.key(press(KeyCode::Left));
        map.key(press(KeyCode::Right));
        assert_eq!(map.selected().unwrap().id, "a");
        assert_eq!(
            map.key(press(KeyCode::Enter)),
            Outcome::Run(Action::OpenAgent("a".into()))
        );
        assert_eq!(
            map.key(press(KeyCode::Char('t'))),
            Outcome::Run(Action::PinAgent("a".into()))
        );
        assert_eq!(map.click(3, 2), Outcome::Run(Action::OpenAgent("b".into())));
    }
}
