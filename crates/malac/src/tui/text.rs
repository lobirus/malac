//! Measuring, cleaning, wrapping and truncating text by display width.

use std::borrow::Cow;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Display width, measured the way ratatui draws: grapheme by grapheme.
pub fn width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_width).sum()
}

fn grapheme_width(g: &str) -> usize {
    if g.contains(char::is_control) {
        0
    } else {
        g.width()
    }
}

/// Removes terminal escape sequences and control characters (tabs become
/// spaces), so tool output cannot move the cursor or change colors.
pub fn sanitize(text: &str) -> Cow<'_, str> {
    if !text.contains(|c: char| c.is_control() && c != '\n') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            '\r' if chars.peek() == Some(&'\n') => {}
            // A bare carriage return redraws the line, as progress bars do.
            '\r' => {
                let start = out.rfind('\n').map_or(0, |i| i + 1);
                out.truncate(start);
            }
            '\x1b' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') | Some('P') | Some('_') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Appends text to a line, merging it into the last span when the style matches.
fn push(line: &mut Vec<Span<'static>>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    match line.last_mut() {
        Some(last) if last.style == style => last.content.to_mut().push_str(text),
        _ => line.push(Span::styled(text.to_owned(), style)),
    }
}

/// How a rendered row relates to the text it shows, for selecting and copying.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RowText {
    /// Leading columns that decorate the row (markers, rules, indentation).
    pub inset: usize,
    /// Set when the row continues the previous one after a soft wrap: the
    /// text the wrap dropped between them, a space or nothing.
    pub join: Option<&'static str>,
}

impl RowText {
    pub fn inset(inset: usize) -> Self {
        Self { inset, join: None }
    }
}

/// Word-wraps styled text to `width` columns, saying how each row joins
/// the one before it. Whitespace at a break is dropped and words longer
/// than a line are split.
pub fn wrap_rows(
    spans: &[Span<'_>],
    width: usize,
) -> Vec<(Vec<Span<'static>>, Option<&'static str>)> {
    let mut wrapper = Wrapper {
        width: width.max(1),
        lines: Vec::new(),
        line: Vec::new(),
        line_width: 0,
        word: Vec::new(),
        word_width: 0,
        space: Vec::new(),
        space_width: 0,
        join: None,
    };
    for span in spans {
        let mut rest = span.content.as_ref();
        while let Some(first) = rest.chars().next() {
            let is_space = first.is_whitespace();
            let end = rest
                .find(|c: char| c.is_whitespace() != is_space)
                .unwrap_or(rest.len());
            let (piece, tail) = rest.split_at(end);
            rest = tail;
            if is_space {
                wrapper.place_word();
                wrapper.space.push((piece.to_owned(), span.style));
                wrapper.space_width += self::width(piece);
            } else {
                wrapper.word.push((piece.to_owned(), span.style));
                wrapper.word_width += self::width(piece);
            }
        }
    }
    wrapper.place_word();
    if !wrapper.line.is_empty() || wrapper.lines.is_empty() {
        wrapper.lines.push((wrapper.line, wrapper.join));
    }
    wrapper.lines
}

struct Wrapper {
    width: usize,
    lines: Vec<(Vec<Span<'static>>, Option<&'static str>)>,
    line: Vec<Span<'static>>,
    line_width: usize,
    word: Vec<(String, Style)>,
    word_width: usize,
    space: Vec<(String, Style)>,
    space_width: usize,
    /// How the line being built joins the previous one.
    join: Option<&'static str>,
}

impl Wrapper {
    /// Ends the line; `dropped` is what the break removes before the next.
    fn break_line(&mut self, dropped: &'static str) {
        let ends_in_space = self
            .line
            .last()
            .is_some_and(|s| s.content.ends_with(char::is_whitespace));
        // Copying trims rows, so a space kept at the end must come back as the join.
        let dropped = if ends_in_space { " " } else { dropped };
        let join = self.join.replace(dropped);
        self.lines.push((std::mem::take(&mut self.line), join));
        self.line_width = 0;
    }

    fn push_space(&mut self) {
        // Leading whitespace is kept only at the very start of the text.
        let keep = self.line_width > 0 || self.lines.is_empty();
        let space = std::mem::take(&mut self.space);
        if keep && self.line_width + self.space_width <= self.width {
            for (text, style) in &space {
                push(&mut self.line, text, *style);
            }
            self.line_width += self.space_width;
        }
        self.space_width = 0;
    }

    fn place_word(&mut self) {
        if self.word.is_empty() {
            return;
        }
        let word = std::mem::take(&mut self.word);
        let word_width = std::mem::replace(&mut self.word_width, 0);
        let fits = self.line_width + self.space_width + word_width <= self.width;
        if !fits && self.line_width > 0 && word_width <= self.width {
            self.break_line(" ");
            self.space.clear();
            self.space_width = 0;
        } else {
            self.push_space();
        }
        for (text, style) in &word {
            if self.line_width + self::width(text) <= self.width {
                push(&mut self.line, text, *style);
                self.line_width += self::width(text);
                continue;
            }
            for g in text.graphemes(true) {
                let w = grapheme_width(g);
                if self.line_width + w > self.width && self.line_width > 0 {
                    self.break_line("");
                }
                push(&mut self.line, g, *style);
                self.line_width += w;
            }
        }
    }
}

/// Splits text into rows of at most `width` columns without touching
/// whitespace, the way a terminal wraps program output.
pub fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut row_width = 0;
    for g in text.graphemes(true) {
        let w = grapheme_width(g);
        if row_width + w > width && row_width > 0 {
            rows.push(std::mem::take(&mut row));
            row_width = 0;
        }
        row.push_str(g);
        row_width += w;
    }
    rows.push(row);
    rows
}

/// How a hard-wrapped row joins the row before it, which copying trims.
pub fn hard_join(previous: &str) -> &'static str {
    if previous.ends_with(char::is_whitespace) {
        " "
    } else {
        ""
    }
}

/// Cuts text to `width` columns, ending in `…` when something was cut.
pub fn truncate(text: &str, width: usize) -> Cow<'_, str> {
    if self::width(text) <= width {
        return Cow::Borrowed(text);
    }
    let mut out = String::new();
    let mut used = 0;
    for g in text.graphemes(true) {
        let w = grapheme_width(g);
        if used + w + 1 > width {
            break;
        }
        out.push_str(g);
        used += w;
    }
    if width > 0 {
        out.push('…');
    }
    Cow::Owned(out)
}

/// Cuts a styled line to `width` columns, ending in `…` when something was cut.
pub fn truncate_line(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    let mut spans = Vec::new();
    let mut used = 0;
    for span in line.spans {
        let w = self::width(&span.content);
        if used + w < width {
            used += w;
            spans.push(span);
            continue;
        }
        let cut = truncate(&span.content, width - used).into_owned();
        spans.push(Span::styled(cut, span.style));
        break;
    }
    Line::from(spans).style(line.style)
}

/// The first non-empty line, for one-line previews.
pub fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
}

/// A URL for display, with the value of a `token` query parameter replaced
/// by `…`, so screenshots and shared screens do not leak it.
pub fn hide_token(url: &str) -> String {
    match url.find("token=") {
        Some(start) => {
            let value = start + "token=".len();
            let end = url[value..].find('&').map_or(url.len(), |i| value + i);
            format!("{}…{}", &url[..value], &url[end..])
        }
        None => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(spans: &[Span<'_>], width: usize) -> Vec<Vec<Span<'static>>> {
        wrap_rows(spans, width)
            .into_iter()
            .map(|(row, _)| row)
            .collect()
    }

    fn plain(lines: &[Vec<Span<'static>>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn wraps_at_word_boundaries() {
        let spans = [Span::raw("the quick brown fox jumps")];
        assert_eq!(
            plain(&wrap(&spans, 10)),
            vec!["the quick", "brown fox", "jumps"]
        );
    }

    #[test]
    fn rows_say_what_the_wrap_dropped() {
        let joins: Vec<_> = wrap_rows(&[Span::raw("the quick abcdefghijkl")], 10)
            .into_iter()
            .map(|(_, join)| join)
            .collect();
        assert_eq!(joins, vec![None, Some(" "), Some("")]);
    }

    #[test]
    fn styled_words_stay_together() {
        let bold = Style::new().add_modifier(ratatui::style::Modifier::BOLD);
        let spans = [
            Span::raw("aaaa "),
            Span::styled("bb", bold),
            Span::raw("cc dd"),
        ];
        let lines = wrap(&spans, 6);
        assert_eq!(plain(&lines), vec!["aaaa", "bbcc", "dd"]);
        assert_eq!(lines[1][0].style, bold);
    }

    #[test]
    fn splits_long_words_and_wide_chars() {
        assert_eq!(
            plain(&wrap(&[Span::raw("abcdefghij")], 4)),
            vec!["abcd", "efgh", "ij"]
        );
        assert_eq!(
            plain(&wrap(&[Span::raw("日本語です")], 4)),
            vec!["日本", "語で", "す"]
        );
    }

    #[test]
    fn hard_wrap_keeps_whitespace() {
        assert_eq!(hard_wrap("  ab  cd", 4), vec!["  ab", "  cd"]);
        assert_eq!(hard_wrap("", 4), vec![""]);
    }

    #[test]
    fn sanitize_strips_escapes() {
        assert_eq!(sanitize("\x1b[31mred\x1b[0m\tx"), "red    x");
        assert_eq!(sanitize("10%\r50%\r100%\ndone\r\n"), "100%\ndone\n");
        assert_eq!(sanitize("\x1b]0;title\x07ok"), "ok");
        assert!(matches!(sanitize("plain\ntext"), Cow::Borrowed(_)));
    }

    #[test]
    fn truncates_with_ellipsis() {
        assert_eq!(truncate("hello world", 8), "hello w…");
        assert_eq!(truncate("short", 8), "short");
        let line = truncate_line(Line::from(vec![Span::raw("abc"), Span::raw("defgh")]), 6);
        assert_eq!(line.to_string(), "abcde…");
    }

    #[test]
    fn hides_tokens() {
        assert_eq!(
            hide_token("http://127.0.0.1:7700/?token=abc123"),
            "http://127.0.0.1:7700/?token=…"
        );
        assert_eq!(
            hide_token("http://h/?token=abc&embed=1"),
            "http://h/?token=…&embed=1"
        );
        assert_eq!(hide_token("http://h/"), "http://h/");
    }
}
