//! Selecting rendered text with the mouse: points in rows and columns,
//! words and lines under the pointer, and the text a selection copies.

use std::ops::Range;

use malac_core::event::AgentId;
use ratatui::text::Line;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::text::RowText;

/// A cell: a transcript row and a display column.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub row: usize,
    pub col: usize,
}

impl Point {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// Selected transcript text, from `anchor` to `head` with both cells included.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub agent: AgentId,
    /// The width the transcript was laid out at; rows mean nothing at another.
    pub width: usize,
    pub anchor: Point,
    pub head: Point,
}

impl Selection {
    /// Start and end in reading order.
    pub fn span(&self) -> (Point, Point) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    /// The columns selected in `row`, end excluded.
    pub fn columns(&self, row: usize) -> Option<Range<usize>> {
        let (start, end) = self.span();
        if row < start.row || row > end.row {
            return None;
        }
        let from = if row == start.row { start.col } else { 0 };
        let to = if row == end.row {
            end.col + 1
        } else {
            usize::MAX
        };
        Some(from..to)
    }
}

/// The graphemes of a line with the column each starts at.
pub fn cells<'a>(line: &'a Line<'_>) -> Vec<(usize, &'a str)> {
    let mut col = 0;
    let mut out = Vec::new();
    for span in &line.spans {
        for g in span.content.graphemes(true) {
            out.push((col, g));
            col += g.width();
        }
    }
    out
}

/// Where the line's text ends, trailing spaces left out.
pub fn text_end(line: &Line<'_>) -> usize {
    cells(line)
        .iter()
        .rev()
        .find(|(_, g)| !g.trim().is_empty())
        .map_or(0, |(col, g)| col + g.width())
}

/// The text of the cells that start within `columns`, trailing spaces left out.
pub fn slice(line: &Line<'_>, columns: Range<usize>) -> String {
    let text: String = cells(line)
        .into_iter()
        .filter(|(col, _)| columns.contains(col))
        .map(|(_, g)| g)
        .collect();
    text.trim_end().to_owned()
}

fn is_word(g: &str) -> bool {
    g.chars()
        .all(|c| c.is_alphanumeric() || "_-./~:@%+#=?&".contains(c))
}

/// The columns of the word at `col`, or of the single cell when it is not
/// part of one. Text before `inset` is decoration and never selected.
pub fn word_at(line: &Line<'_>, inset: usize, col: usize) -> Range<usize> {
    let cells: Vec<(usize, &str)> = cells(line)
        .into_iter()
        .filter(|(c, _)| *c >= inset)
        .collect();
    let Some(at) = cells
        .iter()
        .rposition(|(c, _)| *c <= col)
        .filter(|&i| cells[i].0 + cells[i].1.width().max(1) > col)
    else {
        return col..col + 1;
    };
    if !is_word(cells[at].1) {
        return cells[at].0..cells[at].0 + cells[at].1.width().max(1);
    }
    let mut first = at;
    while first > 0 && is_word(cells[first - 1].1) {
        first -= 1;
    }
    let mut last = at;
    while last + 1 < cells.len() && is_word(cells[last + 1].1) {
        last += 1;
    }
    // A sentence's full stop or colon is not part of its last word.
    while last > at && matches!(cells[last].1, "." | ":" | "," | "?") {
        last -= 1;
    }
    cells[first].0..cells[last].0 + cells[last].1.width()
}

/// The rows of the logical line through `row`: back to where it starts
/// and on while the rows after continue it.
pub fn line_rows(rows: &[Option<(&Line<'_>, RowText)>], row: usize) -> Range<usize> {
    let continues = |r: usize| matches!(rows.get(r), Some(Some((_, t))) if t.join.is_some());
    let mut first = row;
    while first > 0 && continues(first) {
        first -= 1;
    }
    let mut last = row;
    while continues(last + 1) {
        last += 1;
    }
    first..last + 1
}

/// The selected text. `rows` starts at row `first`; soft-wrapped rows are
/// joined as they were written, decoration and padding left out.
pub fn copy_text(
    rows: &[Option<(&Line<'_>, RowText)>],
    first: usize,
    selection: &Selection,
) -> String {
    let mut out = String::new();
    let mut started = false;
    for (i, row) in rows.iter().enumerate() {
        let Some(columns) = selection.columns(first + i) else {
            continue;
        };
        match row {
            Some((line, text)) => {
                if started {
                    out.push_str(text.join.unwrap_or("\n"));
                }
                out.push_str(&slice(line, columns.start.max(text.inset)..columns.end));
                started = true;
            }
            // A blank row between entries.
            None if started => out.push('\n'),
            None => {}
        }
    }
    out.trim_end_matches('\n').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;

    fn line(text: &str) -> Line<'static> {
        Line::from(vec![Span::raw("┃ "), Span::raw(text.to_owned())])
    }

    fn selection(anchor: (usize, usize), head: (usize, usize)) -> Selection {
        Selection {
            agent: "a".into(),
            width: 40,
            anchor: Point::new(anchor.0, anchor.1),
            head: Point::new(head.0, head.1),
        }
    }

    #[test]
    fn words_stop_at_decoration_and_punctuation() {
        let l = line("run cargo-test now.");
        assert_eq!(word_at(&l, 2, 8), 6..16);
        assert_eq!(word_at(&l, 2, 18), 17..20);
        assert_eq!(word_at(&l, 2, 0), 0..1, "the rule is not a word");
        assert_eq!(word_at(&l, 2, 5), 5..6, "a space selects itself");
        assert_eq!(word_at(&l, 2, 30), 30..31, "past the end");
    }

    #[test]
    fn copies_across_soft_and_hard_breaks() {
        let lines = [line("The quick"), line("brown fox."), line("Next")];
        let rows = vec![
            Some((&lines[0], RowText::inset(2))),
            Some((
                &lines[1],
                RowText {
                    inset: 2,
                    join: Some(" "),
                },
            )),
            None,
            Some((&lines[2], RowText::inset(2))),
        ];
        let all = selection((10, 0), (13, 99));
        assert_eq!(copy_text(&rows, 10, &all), "The quick brown fox.\n\nNext");
        let part = selection((11, 8), (10, 6));
        assert_eq!(copy_text(&rows, 10, &part), "quick brown f");
        assert_eq!(line_rows(&rows, 1), 0..2);
        assert_eq!(line_rows(&rows, 3), 3..4);
        assert_eq!(text_end(&lines[0]), 11);
    }
}
