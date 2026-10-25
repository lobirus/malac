//! Choosing a folder to work in: the directories under one, recent project
//! folders and the home directory, filtered as you type.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::app::{Action, HitMap, Outcome, Target, short_path};
use super::dialog::{frame_box, put};
use super::files;
use super::text;
use super::theme::theme;

#[derive(Debug, Clone, PartialEq)]
enum Row {
    Header(String),
    /// Work in the folder being shown.
    Choose,
    Up,
    Dir {
        path: PathBuf,
        label: String,
        git: bool,
    },
}

pub struct FolderBrowser {
    pub dir: PathBuf,
    pub filter: String,
    /// Subdirectories of `dir`: name and whether it holds a repository.
    dirs: Vec<(String, bool)>,
    recent: Vec<PathBuf>,
    home: Option<PathBuf>,
    selected: usize,
    scroll: usize,
    height: usize,
    pub error: Option<String>,
}

fn is_repo(path: &Path) -> bool {
    path.join(".git").exists()
}

impl FolderBrowser {
    pub fn new(dir: &Path, recent: Vec<PathBuf>) -> Self {
        let mut browser = Self {
            dir: PathBuf::new(),
            filter: String::new(),
            dirs: Vec::new(),
            recent,
            home: std::env::var_os("HOME").map(PathBuf::from),
            selected: 0,
            scroll: 0,
            height: 10,
            error: None,
        };
        if browser.enter(dir).is_err() {
            if let Some(home) = browser.home.clone() {
                let _ = browser.enter(&home);
            }
        }
        browser
    }

    /// Shows `dir`, keeping the current folder when it cannot be read.
    fn enter(&mut self, dir: &Path) -> std::io::Result<()> {
        let entries = std::fs::read_dir(dir)?;
        let mut dirs: Vec<(String, bool)> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    is_repo(&e.path()),
                )
            })
            .collect();
        dirs.sort_by_key(|(name, _)| name.to_lowercase());
        self.dir = dir.to_owned();
        self.dirs = dirs;
        self.filter.clear();
        self.error = None;
        self.scroll = 0;
        self.selected = 0;
        Ok(())
    }

    fn navigate(&mut self, dir: &Path) {
        if let Err(err) = self.enter(dir) {
            self.error = Some(format!("{}: {err}", short_path(dir)));
        }
    }

    fn up(&mut self) {
        let Some(parent) = self.dir.parent().map(Path::to_owned) else {
            return;
        };
        let from = self
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        self.navigate(&parent);
        let rows = self.rows();
        if let Some(i) = rows
            .iter()
            .position(|r| matches!(r, Row::Dir { label, .. } if Some(label.trim_end_matches('/')) == from.as_deref()))
        {
            self.select(i);
        }
    }

    fn rows(&self) -> Vec<Row> {
        let hidden = self.filter.starts_with('.');
        let mut listing: Vec<(i64, Row)> = self
            .dirs
            .iter()
            .filter(|(name, _)| hidden || !name.starts_with('.'))
            .filter_map(|(name, git)| {
                let (score, _) = files::score(&self.filter, name)?;
                let row = Row::Dir {
                    path: self.dir.join(name),
                    label: format!("{name}/"),
                    git: *git,
                };
                Some((score, row))
            })
            .collect();
        let mut places: Vec<(i64, Row)> = self
            .recent
            .iter()
            .filter(|p| **p != self.dir)
            .chain(self.home.iter().filter(|h| !self.recent.contains(h)))
            .filter_map(|path| {
                let label = short_path(path);
                let (score, _) = files::score(&self.filter, &label)?;
                let row = Row::Dir {
                    path: path.clone(),
                    git: is_repo(path),
                    label,
                };
                Some((score, row))
            })
            .take(8)
            .collect();
        let mut rows = Vec::new();
        if self.filter.is_empty() {
            rows.push(Row::Choose);
            if !places.is_empty() {
                rows.push(Row::Header("Recent and home".into()));
                rows.extend(places.into_iter().map(|(_, r)| r));
            }
            rows.push(Row::Header(short_path(&self.dir)));
            if self.dir.parent().is_some() {
                rows.push(Row::Up);
            }
            rows.extend(listing.into_iter().map(|(_, r)| r));
        } else {
            listing.sort_by(|a, b| b.0.cmp(&a.0));
            places.sort_by(|a, b| b.0.cmp(&a.0));
            rows.extend(listing.into_iter().map(|(_, r)| r));
            if !places.is_empty() {
                rows.push(Row::Header("Recent and home".into()));
                rows.extend(places.into_iter().map(|(_, r)| r));
            }
        }
        rows
    }

    fn select(&mut self, index: usize) {
        let rows = self.rows();
        let Some(last) = rows.iter().rposition(|r| !matches!(r, Row::Header(_))) else {
            self.selected = 0;
            return;
        };
        let mut index = index.min(last);
        while matches!(rows.get(index), Some(Row::Header(_))) && index < last {
            index += 1;
        }
        self.selected = index;
        // Keep a header visible above the first row under it.
        let top = if index > 0 && matches!(rows[index - 1], Row::Header(_)) {
            index - 1
        } else {
            index
        };
        if top < self.scroll {
            self.scroll = top;
        } else if index >= self.scroll + self.height {
            self.scroll = index + 1 - self.height;
        }
    }

    /// Moves over `delta` rows that can be chosen, skipping headers.
    fn step(&mut self, delta: isize) {
        let rows = self.rows();
        let choosable: Vec<usize> = (0..rows.len())
            .filter(|&i| !matches!(rows[i], Row::Header(_)))
            .collect();
        let Some(at) = choosable.iter().position(|&i| i == self.selected) else {
            self.select(0);
            return;
        };
        let next = (at as isize + delta).clamp(0, choosable.len() as isize - 1);
        self.select(choosable[next as usize]);
    }

    /// Enter: work here, go up, or go into the highlighted folder.
    fn activate(&mut self) -> Outcome {
        match self.rows().get(self.selected).cloned() {
            Some(Row::Choose) => Outcome::Run(Action::OpenFolder(self.dir.clone())),
            Some(Row::Up) => {
                self.up();
                Outcome::Stay
            }
            Some(Row::Dir { path, .. }) => {
                self.navigate(&path);
                Outcome::Stay
            }
            Some(Row::Header(_)) | None => Outcome::Stay,
        }
    }

    /// The highlighted folder itself, without going into it.
    fn choose_highlighted(&self) -> Outcome {
        match self.rows().get(self.selected) {
            Some(Row::Dir { path, .. }) => Outcome::Run(Action::OpenFolder(path.clone())),
            _ => Outcome::Run(Action::OpenFolder(self.dir.clone())),
        }
    }

    pub fn click(&mut self, index: usize, count: u8) -> Outcome {
        self.select(index);
        if count >= 2 {
            return self.activate();
        }
        Outcome::Stay
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.step(delta);
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Char('c') if ctrl => return Outcome::Close,
            KeyCode::Char('o') if ctrl => return self.choose_highlighted(),
            KeyCode::Enter if alt => return self.choose_highlighted(),
            KeyCode::Enter => return self.activate(),
            KeyCode::Right => {
                if matches!(self.rows().get(self.selected), Some(Row::Dir { .. })) {
                    return self.activate();
                }
            }
            KeyCode::Left => self.up(),
            KeyCode::Up => self.step(-1),
            KeyCode::Down => self.step(1),
            KeyCode::PageUp => self.step(-(self.height as isize)),
            KeyCode::PageDown => self.step(self.height as isize),
            KeyCode::Home => self.select(0),
            KeyCode::End => self.select(usize::MAX),
            KeyCode::Backspace if self.filter.is_empty() => self.up(),
            KeyCode::Backspace => {
                self.filter.pop();
                self.select(0);
            }
            KeyCode::Char('u') if ctrl => {
                self.filter.clear();
                self.select(0);
            }
            KeyCode::Char('~') if self.filter.is_empty() => {
                if let Some(home) = self.home.clone() {
                    self.navigate(&home);
                }
            }
            KeyCode::Char('/') if self.filter.is_empty() => self.navigate(Path::new("/")),
            KeyCode::Char(c) if !ctrl => {
                self.filter.push(c);
                self.select(0);
            }
            _ => {}
        }
        Outcome::Stay
    }

    pub fn draw(&mut self, frame: &mut Frame, hits: &mut HitMap) {
        let t = theme();
        let rows = self.rows();
        let wanted = rows.len() as u16 + 6;
        let height = frame.area().height.saturating_sub(4).min(wanted.max(14));
        let inner = frame_box(
            frame,
            84,
            height,
            "Open folder",
            "enter open · ⌫ up · ^O use highlighted · esc close",
            hits,
        );
        let mut path = vec![Span::styled(short_path(&self.dir), t.bold())];
        if is_repo(&self.dir) {
            path.push(Span::styled("  git", t.fg(t.accent)));
        }
        if let Some(error) = &self.error {
            path.push(Span::styled(format!("   {error}"), t.fg(t.error)));
        }
        put(frame, inner, 0, Line::from(path));
        let filter = vec![
            Span::styled("› ", t.dim()),
            Span::raw(self.filter.clone()),
            Span::styled("▏", t.fg(t.accent)),
        ];
        put(frame, inner, 1, Line::from(filter));
        put(
            frame,
            inner,
            2,
            Line::styled("─".repeat(inner.width as usize), t.faint()),
        );
        self.height = inner.height.saturating_sub(3) as usize;
        if rows.is_empty() {
            put(frame, inner, 3, Line::styled("  nothing matches", t.dim()));
        }
        self.scroll = self.scroll.min(rows.len().saturating_sub(self.height));
        let width = inner.width as usize;
        for (line, (index, row)) in rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(self.height)
            .enumerate()
        {
            let y = 3 + line as u16;
            let selected = index == self.selected;
            let style = if selected { t.selected() } else { Style::new() };
            let quiet = if selected { style } else { t.dim() };
            let git = if selected { style } else { t.fg(t.accent) };
            let (mark, label, detail, detail_style) = match row {
                Row::Header(title) => {
                    put(
                        frame,
                        inner,
                        y,
                        Line::styled(title.clone(), t.bold().fg(t.dim)),
                    );
                    continue;
                }
                Row::Choose => ("✓ ", "Work in this folder", "enter".into(), quiet),
                Row::Up => ("↑ ", "..", String::new(), quiet),
                Row::Dir {
                    label, git: true, ..
                } => ("▸ ", label.as_str(), "git".into(), git),
                Row::Dir { label, .. } => ("▸ ", label.as_str(), String::new(), quiet),
            };
            let detail = text::truncate(&detail, width / 2).into_owned();
            let room = width.saturating_sub(text::width(&detail) + 4);
            let label = text::truncate(label, room).into_owned();
            let used = 2 + text::width(&label) + text::width(&detail) + 1;
            let line = Line::from(vec![
                Span::styled(mark, quiet),
                Span::styled(label, style),
                Span::styled(" ".repeat(width.saturating_sub(used)), style),
                Span::styled(format!("{detail} "), detail_style),
            ]);
            put(frame, inner, y, line);
            hits.push(
                Rect::new(inner.x, inner.y + y, inner.width, 1),
                Target::Pick(index),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn walks_folders_and_chooses_one() {
        let root = std::env::temp_dir().join(format!("malac-folder-{}", std::process::id()));
        std::fs::create_dir_all(root.join("alpha/.git")).unwrap();
        std::fs::create_dir_all(root.join("beta/inner")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        let mut browser = FolderBrowser::new(&root, Vec::new());
        browser.home = None;
        let labels: Vec<String> = browser
            .rows()
            .iter()
            .filter_map(|r| match r {
                Row::Dir { label, git, .. } => {
                    Some(format!("{label}{}", if *git { " git" } else { "" }))
                }
                _ => None,
            })
            .collect();
        assert_eq!(labels, ["alpha/ git", "beta/"]);
        for c in "bet".chars() {
            browser.key(press(KeyCode::Char(c)));
        }
        assert_eq!(browser.key(press(KeyCode::Enter)), Outcome::Stay);
        assert_eq!(browser.dir, root.join("beta"));
        assert_eq!(
            browser.key(press(KeyCode::Enter)),
            Outcome::Run(Action::OpenFolder(root.join("beta"))),
            "the first row works in the folder shown"
        );
        browser.key(press(KeyCode::Backspace));
        assert_eq!(browser.dir, root);
        assert_eq!(
            browser.rows().get(browser.selected),
            Some(&Row::Dir {
                path: root.join("beta"),
                label: "beta/".into(),
                git: false
            }),
            "going up keeps the folder we came from highlighted"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
