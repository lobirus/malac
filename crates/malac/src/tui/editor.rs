//! The prompt editor: a multi-line buffer with a cursor, word motions,
//! soft wrapping and history.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::text;

/// One visual row: the byte range of the text it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub start: usize,
    pub end: usize,
}

#[derive(Default)]
pub struct Editor {
    text: String,
    cursor: usize,
    history: Vec<String>,
    browsing: Option<usize>,
    draft: String,
    /// Column kept while moving up and down through shorter rows.
    goal: Option<usize>,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    pub fn new(history: Vec<String>) -> Self {
        Self {
            history,
            ..Default::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    #[cfg(test)]
    fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_owned();
        self.cursor = self.text.len();
        self.goal = None;
    }

    pub fn clear(&mut self) {
        self.set_text("");
        self.browsing = None;
    }

    /// Takes the text for sending and remembers it in the history.
    pub fn submit(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.clear();
        if !text.trim().is_empty() && self.history.last() != Some(&text) {
            self.history.push(text.clone());
        }
        text
    }

    fn edited(&mut self) {
        self.goal = None;
        self.browsing = None;
    }

    /// Inserts typed or pasted text. Line endings are normalized and tabs
    /// become spaces, so a multi-line paste stays one prompt.
    pub fn insert(&mut self, input: &str) {
        let mut clean = String::with_capacity(input.len());
        for c in input.replace("\r\n", "\n").chars() {
            match c {
                '\r' | '\n' => clean.push('\n'),
                '\t' => clean.push_str("    "),
                c if c.is_control() => {}
                c => clean.push(c),
            }
        }
        self.text.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
        self.edited();
    }

    pub fn newline(&mut self) {
        self.insert("\n");
    }

    fn prev_boundary(&self, at: usize) -> usize {
        self.text[..at]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self, at: usize) -> usize {
        self.text[at..]
            .graphemes(true)
            .next()
            .map_or(at, |g| at + g.len())
    }

    fn word_start(&self) -> usize {
        let mut at = self.cursor;
        let mut chars = self.text[..self.cursor].char_indices().rev().peekable();
        while let Some((i, _)) = chars.next_if(|(_, c)| !is_word(*c)) {
            at = i;
        }
        while let Some((i, _)) = chars.next_if(|(_, c)| is_word(*c)) {
            at = i;
        }
        at
    }

    fn word_end(&self) -> usize {
        let mut at = self.cursor;
        let mut chars = self.text[self.cursor..].char_indices().peekable();
        while let Some((i, c)) = chars.next_if(|(_, c)| !is_word(*c)) {
            at = self.cursor + i + c.len_utf8();
        }
        while let Some((i, c)) = chars.next_if(|(_, c)| is_word(*c)) {
            at = self.cursor + i + c.len_utf8();
        }
        at
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i)
    }

    fn move_to(&mut self, at: usize) {
        self.cursor = at;
        self.goal = None;
    }

    fn delete(&mut self, from: usize, to: usize) {
        if from < to {
            self.text.replace_range(from..to, "");
            self.cursor = from;
        }
        self.edited();
    }

    pub fn left(&mut self) {
        self.move_to(self.prev_boundary(self.cursor));
    }

    pub fn right(&mut self) {
        self.move_to(self.next_boundary(self.cursor));
    }

    pub fn word_left(&mut self) {
        self.move_to(self.word_start());
    }

    pub fn word_right(&mut self) {
        self.move_to(self.word_end());
    }

    pub fn home(&mut self) {
        self.move_to(self.line_start());
    }

    pub fn end(&mut self) {
        self.move_to(self.line_end());
    }

    pub fn backspace(&mut self) {
        self.delete(self.prev_boundary(self.cursor), self.cursor);
    }

    pub fn delete_forward(&mut self) {
        self.delete(self.cursor, self.next_boundary(self.cursor));
    }

    pub fn delete_word_back(&mut self) {
        self.delete(self.word_start(), self.cursor);
    }

    pub fn delete_word_forward(&mut self) {
        self.delete(self.cursor, self.word_end());
    }

    /// Deletes to the start of the line, or joins with the previous line
    /// when already there.
    pub fn delete_to_line_start(&mut self) {
        let start = self.line_start();
        if start == self.cursor {
            self.backspace();
        } else {
            self.delete(start, self.cursor);
        }
    }

    /// Deletes to the end of the line, or joins with the next line when
    /// already there.
    pub fn delete_to_line_end(&mut self) {
        let end = self.line_end();
        if end == self.cursor {
            self.delete_forward();
        } else {
            self.delete(self.cursor, end);
        }
    }

    /// Soft-wraps the text into rows of at most `width` columns, breaking
    /// after spaces where possible.
    pub fn rows(&self, width: usize) -> Vec<Row> {
        let width = width.max(1);
        let mut rows = Vec::new();
        let mut base = 0;
        for line in self.text.split('\n') {
            let mut start = 0;
            let mut col = 0;
            let mut after_space = None;
            for (i, g) in line.grapheme_indices(true) {
                let w = g.width();
                while col + w > width && col > 0 {
                    let at = match after_space {
                        Some(at) if at > start => at,
                        _ => i,
                    };
                    rows.push(Row {
                        start: base + start,
                        end: base + at,
                    });
                    col = text::width(&line[at..i]);
                    start = at;
                    after_space = None;
                }
                col += w;
                if g == " " {
                    after_space = Some(i + 1);
                }
            }
            rows.push(Row {
                start: base + start,
                end: base + line.len(),
            });
            base += line.len() + 1;
        }
        rows
    }

    fn row_of(rows: &[Row], at: usize) -> usize {
        rows.iter().rposition(|r| r.start <= at).unwrap_or(0)
    }

    /// The cursor as (row, column) in the wrapped layout.
    pub fn cursor_position(&self, width: usize) -> (usize, usize) {
        let rows = self.rows(width);
        let row = Self::row_of(&rows, self.cursor);
        let col = text::width(&self.text[rows[row].start..self.cursor]);
        (row, col)
    }

    /// The byte offset shown at `col` in `row`; a wrapped row's last
    /// position belongs to the row below, so it stops one short.
    fn offset_in(&self, rows: &[Row], row: usize, col: usize) -> usize {
        let Row { start, end } = rows[row];
        let wrapped = rows.get(row + 1).is_some_and(|next| next.start == end);
        let mut at = start;
        let mut used = 0;
        for (i, g) in self.text[start..end].grapheme_indices(true) {
            let w = g.width();
            if used + w > col || (wrapped && start + i + g.len() == end) {
                break;
            }
            used += w;
            at = start + i + g.len();
        }
        at
    }

    /// Moves up one visual row; returns false on the first row.
    pub fn up(&mut self, width: usize) -> bool {
        self.vertical(width, -1)
    }

    /// Moves down one visual row; returns false on the last row.
    pub fn down(&mut self, width: usize) -> bool {
        self.vertical(width, 1)
    }

    fn vertical(&mut self, width: usize, step: isize) -> bool {
        let rows = self.rows(width);
        let (row, col) = self.cursor_position(width);
        let Some(target) = row.checked_add_signed(step).filter(|r| *r < rows.len()) else {
            return false;
        };
        let goal = *self.goal.get_or_insert(col);
        self.cursor = self.offset_in(&rows, target, goal);
        true
    }

    /// Puts the cursor where the user clicked.
    pub fn click(&mut self, width: usize, row: usize, col: usize) {
        let rows = self.rows(width);
        let row = row.min(rows.len() - 1);
        self.move_to(self.offset_in(&rows, row, col));
    }

    pub fn history_prev(&mut self) -> bool {
        let index = match self.browsing {
            None if self.history.is_empty() => return false,
            None => {
                self.draft = self.text.clone();
                self.history.len() - 1
            }
            Some(0) => return false,
            Some(i) => i - 1,
        };
        self.browsing = Some(index);
        self.text = self.history[index].clone();
        self.cursor = self.text.len();
        self.goal = None;
        true
    }

    pub fn history_next(&mut self) -> bool {
        let Some(index) = self.browsing else {
            return false;
        };
        if index + 1 < self.history.len() {
            self.browsing = Some(index + 1);
            self.text = self.history[index + 1].clone();
        } else {
            self.browsing = None;
            self.text = std::mem::take(&mut self.draft);
        }
        self.cursor = self.text.len();
        self.goal = None;
        true
    }
}

/// Entries kept when the history file is rewritten.
const HISTORY_KEEP: usize = 1000;

/// Reads the line REPL's history file (rustyline's `#V2` format, where
/// newlines and backslashes are escaped), so both modes share history.
pub fn load_history(path: &Path) -> Vec<String> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut lines = BufReader::new(file).lines().map_while(Result::ok);
    let mut entries = Vec::new();
    let v2 = match lines.next() {
        Some(first) if first == "#V2" => true,
        Some(first) => {
            entries.push(first);
            false
        }
        None => false,
    };
    for line in lines.filter(|l| !l.is_empty()) {
        entries.push(if v2 { unescape(&line) } else { line });
    }
    if entries.len() > 2 * HISTORY_KEEP {
        entries.drain(..entries.len() - HISTORY_KEEP);
        let mut out = String::from("#V2\n");
        for entry in &entries {
            out.push_str(&escape(entry));
            out.push('\n');
        }
        let _ = std::fs::write(path, out);
    }
    entries
}

pub fn append_history(path: &Path, entry: &str) {
    let fresh = !path.exists();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
    if let Ok(mut file) = file {
        let header = if fresh { "#V2\n" } else { "" };
        let _ = writeln!(file, "{header}{}", escape(entry));
    }
}

fn escape(entry: &str) -> String {
    entry.replace('\\', r"\\").replace('\n', r"\n")
}

fn unescape(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('\\', Some('n')) => {
                out.push('\n');
                chars.next();
            }
            ('\\', Some('\\')) => {
                out.push('\\');
                chars.next();
            }
            (c, _) => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> Editor {
        let mut e = Editor::default();
        e.set_text(text);
        e
    }

    #[test]
    fn editing_and_grapheme_motion() {
        let mut e = editor("héllo");
        e.left();
        e.left();
        e.insert("X");
        assert_eq!(e.text(), "hélXlo");
        e.backspace();
        e.backspace();
        assert_eq!(e.text(), "hélo");
        e.home();
        e.right();
        e.delete_forward();
        assert_eq!(e.text(), "hlo");
    }

    #[test]
    fn word_motions() {
        let mut e = editor("cargo test --all");
        e.word_left();
        assert_eq!(e.cursor(), 13);
        e.word_left();
        assert_eq!(e.cursor(), 6);
        e.word_right();
        assert_eq!(e.cursor(), 10);
        e.end();
        e.delete_word_back();
        assert_eq!(e.text(), "cargo test --");
        e.home();
        e.delete_word_forward();
        assert_eq!(e.text(), " test --");
    }

    #[test]
    fn line_kills_join_lines_at_the_edges() {
        let mut e = editor("one\ntwo");
        e.delete_to_line_start();
        assert_eq!(e.text(), "one\n");
        e.delete_to_line_start();
        assert_eq!(e.text(), "one");
        e.home();
        e.delete_to_line_end();
        assert_eq!(e.text(), "");
    }

    #[test]
    fn paste_normalizes_line_endings() {
        let mut e = Editor::default();
        e.insert("a\r\nb\rc\td\x07");
        assert_eq!(e.text(), "a\nb\nc    d");
    }

    #[test]
    fn rows_wrap_after_spaces() {
        let e = editor("hello world foo\nbar");
        let rows: Vec<&str> = e
            .rows(8)
            .iter()
            .map(|r| &e.text()[r.start..r.end])
            .collect();
        assert_eq!(rows, vec!["hello ", "world ", "foo", "bar"]);
        assert_eq!(e.cursor_position(8), (3, 3));
        let e = editor("abcdefghij");
        assert_eq!(e.rows(4).len(), 3);
        assert_eq!(e.cursor_position(4), (2, 2));
    }

    #[test]
    fn vertical_motion_keeps_the_column() {
        let mut e = editor("first line\nab\nthird line");
        assert!(e.up(40));
        assert_eq!(e.cursor_position(40), (1, 2));
        assert!(e.up(40));
        assert_eq!(e.cursor_position(40), (0, 10));
        assert!(!e.up(40));
        assert!(e.down(40));
        assert!(e.down(40));
        assert_eq!(e.cursor_position(40), (2, 10));
        assert!(!e.down(40));
    }

    #[test]
    fn wrapped_rows_do_not_skip_rows() {
        let mut e = editor("aaaa bbbb cccc");
        e.home();
        e.end();
        assert!(e.up(5));
        assert_eq!(e.cursor_position(5), (1, 4));
        e.click(5, 0, 99);
        assert_eq!(e.cursor_position(5), (0, 4));
    }

    #[test]
    fn history_keeps_the_draft() {
        let mut e = Editor::new(vec!["old".into(), "newer".into()]);
        e.insert("draft");
        assert!(e.history_prev());
        assert_eq!(e.text(), "newer");
        assert!(e.history_prev());
        assert_eq!(e.text(), "old");
        assert!(!e.history_prev());
        assert!(e.history_next());
        assert!(e.history_next());
        assert_eq!(e.text(), "draft");
        assert!(!e.history_next());
        assert_eq!(e.submit(), "draft");
        assert!(e.is_empty());
        assert!(e.history_prev());
        assert_eq!(e.text(), "draft");
    }

    #[test]
    fn history_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("malac-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.txt");
        let _ = std::fs::remove_file(&path);
        append_history(&path, "one");
        append_history(&path, "two\nlines \\ slash");
        assert_eq!(
            load_history(&path),
            vec!["one".to_owned(), "two\nlines \\ slash".to_owned()]
        );
        assert!(std::fs::read_to_string(&path).unwrap().starts_with("#V2\n"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
