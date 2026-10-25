//! Drop-down and pop-up menus: the menu bar in the header, the tab menu
//! and the context menu on transcript entries.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use super::app::{Action, HitMap, Outcome, Target};
use super::text;
use super::theme::theme;

/// Menu bar titles and their mnemonics.
pub const TITLES: [(&str, char); 7] = [
    ("Session", 's'),
    ("Model", 'm'),
    ("Mode", 'o'),
    ("Agents", 'a'),
    ("View", 'v'),
    ("Web", 'w'),
    ("Help", 'h'),
];

#[derive(Debug, Clone)]
pub struct Item {
    pub label: String,
    /// The underlined letter that activates the item.
    pub key: Option<char>,
    /// The shortcut shown on the right.
    pub hint: String,
    /// `None` for separators and notes, which cannot be chosen.
    pub action: Option<Action>,
    pub checked: Option<bool>,
}

impl Item {
    pub fn new(label: impl Into<String>, key: char, action: Action) -> Self {
        Self {
            label: label.into(),
            key: Some(key),
            hint: String::new(),
            action: Some(action),
            checked: None,
        }
    }

    /// An item without a mnemonic.
    pub fn plain(label: impl Into<String>, action: Action) -> Self {
        Self {
            key: None,
            ..Self::new(label, ' ', action)
        }
    }

    /// Text that cannot be chosen, such as a URL or "nothing here".
    pub fn note(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            key: None,
            hint: String::new(),
            action: None,
            checked: None,
        }
    }

    pub fn separator() -> Self {
        Self::note("")
    }

    pub fn hint(mut self, hint: &str) -> Self {
        self.hint = hint.to_owned();
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// Disabled items keep their label but cannot be chosen.
    pub fn enabled(mut self, enabled: bool) -> Self {
        if !enabled {
            self.action = None;
        }
        self
    }

    fn selectable(&self) -> bool {
        self.action.is_some()
    }

    fn is_separator(&self) -> bool {
        self.label.is_empty() && self.action.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// Below a menu bar title.
    Bar(usize),
    /// Below the tab strip.
    Tabs,
    /// At the mouse pointer.
    At(u16, u16),
}

pub struct Menu {
    pub place: Place,
    /// For the menu bar: whether the drop-down is shown or only the title is highlighted.
    pub open: bool,
    pub items: Vec<Item>,
    pub selected: usize,
}

impl Menu {
    pub fn new(place: Place, items: Vec<Item>, open: bool) -> Self {
        let mut menu = Self {
            place,
            open,
            items,
            selected: 0,
        };
        menu.selected = menu.items.iter().position(Item::selectable).unwrap_or(0);
        menu
    }

    fn step(&mut self, delta: isize) {
        let len = self.items.len() as isize;
        if !self.items.iter().any(Item::selectable) {
            return;
        }
        let mut index = self.selected as isize;
        loop {
            index = (index + delta).rem_euclid(len);
            if self.items[index as usize].selectable() {
                break;
            }
        }
        self.selected = index as usize;
    }

    pub fn hover(&mut self, index: usize) -> bool {
        let changed = self.selected != index;
        if self.items.get(index).is_some_and(Item::selectable) {
            self.selected = index;
        }
        changed
    }

    pub fn activate(&self, index: usize) -> Outcome {
        match self.items.get(index).and_then(|i| i.action.clone()) {
            Some(action) => Outcome::Run(action),
            None => Outcome::Stay,
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let bar = match self.place {
            Place::Bar(index) => Some(index),
            _ => None,
        };
        match key.code {
            KeyCode::Esc | KeyCode::F(9) | KeyCode::F(10) => Outcome::Close,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Outcome::Close,
            KeyCode::Left => match bar {
                Some(i) => Outcome::Switch((i + TITLES.len() - 1) % TITLES.len()),
                None => Outcome::Close,
            },
            KeyCode::Right => match bar {
                Some(i) => Outcome::Switch((i + 1) % TITLES.len()),
                None => Outcome::Stay,
            },
            KeyCode::Down | KeyCode::Enter if bar.is_some() && !self.open => {
                self.open = true;
                Outcome::Stay
            }
            KeyCode::Up if self.open => {
                self.step(-1);
                Outcome::Stay
            }
            KeyCode::Down | KeyCode::Tab if self.open => {
                self.step(1);
                Outcome::Stay
            }
            KeyCode::Home if self.open => {
                self.selected = self.items.len() - 1;
                self.step(1);
                Outcome::Stay
            }
            KeyCode::End if self.open => {
                self.selected = 0;
                self.step(-1);
                Outcome::Stay
            }
            KeyCode::Enter | KeyCode::Char(' ') if self.open => self.activate(self.selected),
            KeyCode::Char(c) if !self.open => {
                let c = c.to_ascii_lowercase();
                match TITLES.iter().position(|(_, key)| *key == c) {
                    Some(index) => {
                        self.open = true;
                        Outcome::Switch(index)
                    }
                    None => Outcome::Stay,
                }
            }
            KeyCode::Char(c) => {
                let c = c.to_ascii_lowercase();
                match self
                    .items
                    .iter()
                    .position(|i| i.selectable() && i.key == Some(c))
                {
                    Some(index) => self.activate(index),
                    None => Outcome::Stay,
                }
            }
            _ => Outcome::Stay,
        }
    }

    /// Size of the drop-down box, borders included.
    fn size(&self) -> (u16, u16) {
        let widest = self
            .items
            .iter()
            .map(|i| {
                let hint = if i.hint.is_empty() {
                    0
                } else {
                    text::width(&i.hint) + 3
                };
                text::width(&i.label) + hint + 4
            })
            .max()
            .unwrap_or(10)
            .max(16);
        (widest as u16 + 2, self.items.len() as u16 + 2)
    }
}

/// The label with its mnemonic underlined.
pub fn mnemonic(label: &str, key: Option<char>, style: Style) -> Vec<Span<'static>> {
    let at = key.and_then(|k| {
        label
            .char_indices()
            .find(|(_, c)| c.to_ascii_lowercase() == k)
            .map(|(i, c)| (i, c.len_utf8()))
    });
    match at {
        Some((i, len)) => vec![
            Span::styled(label[..i].to_owned(), style),
            Span::styled(
                label[i..i + len].to_owned(),
                style.add_modifier(Modifier::UNDERLINED),
            ),
            Span::styled(label[i + len..].to_owned(), style),
        ],
        None => vec![Span::styled(label.to_owned(), style)],
    }
}

/// Draws an open menu. `anchor` is where the drop-down hangs from.
pub fn draw(frame: &mut Frame, menu: &Menu, anchor: (u16, u16), hits: &mut HitMap) {
    if !menu.open {
        return;
    }
    let t = theme();
    let screen = frame.area();
    let (w, h) = menu.size();
    let w = w.min(screen.width);
    let h = h.min(screen.height);
    let x = anchor.0.min(screen.width.saturating_sub(w));
    let mut y = anchor.1;
    if y + h > screen.height {
        y = anchor
            .1
            .saturating_sub(h + 1)
            .max(1)
            .min(screen.height.saturating_sub(h));
    }
    let area = Rect::new(x, y, w, h);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(t.faint());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    hits.push(area, Target::Overlay);
    let width = inner.width as usize;
    for (index, item) in menu.items.iter().enumerate().take(inner.height as usize) {
        let row = Rect::new(inner.x, inner.y + index as u16, inner.width, 1);
        if item.is_separator() {
            let rule = Line::styled("─".repeat(width), t.faint());
            frame.render_widget(Paragraph::new(rule), row);
            continue;
        }
        let selected = index == menu.selected && item.selectable();
        let base = if selected {
            t.selected()
        } else if item.selectable() {
            Style::new()
        } else {
            t.dim()
        };
        let mark = match item.checked {
            Some(true) => "✓ ",
            Some(false) | None => "  ",
        };
        let mut spans = vec![Span::styled(mark, base)];
        spans.extend(mnemonic(&item.label, item.key, base));
        let used: usize = spans.iter().map(|s| text::width(&s.content)).sum();
        let hint_width = text::width(&item.hint);
        let gap = width.saturating_sub(used + hint_width + 1);
        spans.push(Span::styled(" ".repeat(gap), base));
        let hint_style = if selected { base } else { t.dim() };
        spans.push(Span::styled(format!("{} ", item.hint), hint_style));
        let line = text::truncate_line(Line::from(spans), width);
        frame.render_widget(Paragraph::new(line).style(base), row);
        hits.push(row, Target::MenuItem(index));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn items() -> Vec<Item> {
        vec![
            Item::new("New tab", 'n', Action::NewTab).hint("^T"),
            Item::separator(),
            Item::note("nothing"),
            Item::new("Quit", 'q', Action::Quit),
        ]
    }

    #[test]
    fn arrows_skip_separators_and_notes() {
        let mut menu = Menu::new(Place::Bar(0), items(), true);
        assert_eq!(menu.selected, 0);
        menu.key(press(KeyCode::Down));
        assert_eq!(menu.selected, 3);
        menu.key(press(KeyCode::Down));
        assert_eq!(menu.selected, 0);
        menu.key(press(KeyCode::Up));
        assert_eq!(menu.selected, 3);
        assert_eq!(menu.key(press(KeyCode::Enter)), Outcome::Run(Action::Quit));
    }

    #[test]
    fn mnemonics_and_the_bar() {
        let mut menu = Menu::new(Place::Bar(0), items(), false);
        assert_eq!(menu.key(press(KeyCode::Char('h'))), Outcome::Switch(6));
        assert!(menu.open, "a title mnemonic opens its drop-down");
        let mut menu = Menu::new(Place::Bar(0), items(), false);
        assert_eq!(menu.key(press(KeyCode::Left)), Outcome::Switch(6));
        assert_eq!(menu.key(press(KeyCode::Enter)), Outcome::Stay);
        assert!(menu.open);
        assert_eq!(
            menu.key(press(KeyCode::Char('N'))),
            Outcome::Run(Action::NewTab)
        );
        assert_eq!(menu.key(press(KeyCode::Esc)), Outcome::Close);
    }

    #[test]
    fn underlines_the_mnemonic() {
        let spans = mnemonic("Mode", Some('o'), Style::new());
        assert_eq!(spans[1].content, "o");
        assert!(spans[1].style.add_modifier.contains(Modifier::UNDERLINED));
    }
}
