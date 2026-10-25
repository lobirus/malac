//! Markdown to styled, wrapped terminal lines.

use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::text::{self, RowText, sanitize};
use super::theme::theme;

/// Rendered markdown, with where its code blocks ended up so they can be copied.
pub struct Markdown {
    pub lines: Vec<Line<'static>>,
    /// One per line: what of it is text, and how it joins the line before.
    pub rows: Vec<RowText>,
    pub code: Vec<CodeBlock>,
}

pub struct CodeBlock {
    pub lines: Range<usize>,
    pub text: String,
}

/// Renders markdown into lines at most `width` columns wide.
pub fn render(source: &str, width: usize) -> Markdown {
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    let source = sanitize(source);
    let mut writer = Writer::new(width.max(8));
    for event in Parser::new_ext(&source, options) {
        writer.event(event);
    }
    writer.finish()
}

/// A line prefix contributed by an enclosing block: list markers hang on
/// the first line and indent the rest; quotes repeat on every line.
struct Prefix {
    first: Span<'static>,
    rest: Span<'static>,
    used: bool,
}

struct Table {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<Vec<Span<'static>>>>,
    row: Vec<Vec<Span<'static>>>,
}

struct Writer {
    width: usize,
    lines: Vec<Line<'static>>,
    rows: Vec<RowText>,
    spans: Vec<Span<'static>>,
    styles: Vec<Style>,
    prefixes: Vec<Prefix>,
    lists: Vec<Option<u64>>,
    code: Option<(String, String)>,
    links: Vec<(String, usize)>,
    table: Option<Table>,
    gap: bool,
    blocks: Vec<CodeBlock>,
}

impl Writer {
    fn new(width: usize) -> Self {
        Self {
            width,
            lines: Vec::new(),
            rows: Vec::new(),
            spans: Vec::new(),
            styles: Vec::new(),
            prefixes: Vec::new(),
            lists: Vec::new(),
            code: None,
            links: Vec::new(),
            table: None,
            gap: false,
            blocks: Vec::new(),
        }
    }

    fn style(&self) -> Style {
        self.styles
            .iter()
            .fold(Style::new(), |acc, style| acc.patch(*style))
    }

    fn text(&mut self, text: &str) {
        if let Some((_, code)) = &mut self.code {
            code.push_str(text);
            return;
        }
        let style = self.style();
        self.spans.push(Span::styled(text.to_owned(), style));
    }

    fn event(&mut self, event: Event<'_>) {
        let t = theme();
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                let style = self.style().bg(t.code_bg);
                self.spans.push(Span::styled(code.into_string(), style));
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                let style = self.style().fg(t.dim);
                for (i, line) in html.lines().enumerate() {
                    if i > 0 {
                        self.flush();
                    }
                    self.spans.push(Span::styled(line.to_owned(), style));
                }
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.block();
                let width = self.width.saturating_sub(self.prefix_width());
                let rule = Span::styled("─".repeat(width), t.faint());
                self.emit(vec![rule]);
                self.gap = true;
            }
            Event::TaskListMarker(done) => {
                let marker = if done {
                    Span::styled("✓ ", t.fg(t.accent))
                } else {
                    Span::styled("○ ", t.dim())
                };
                self.spans.push(marker);
            }
            Event::FootnoteReference(name) => self.text(&format!("[{name}]")),
            Event::InlineMath(math) | Event::DisplayMath(math) => self.text(&math),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        let t = theme();
        match tag {
            Tag::Paragraph => self.block(),
            Tag::Heading { level, .. } => {
                self.block();
                let style = match level {
                    HeadingLevel::H1 => t.bold().add_modifier(Modifier::UNDERLINED),
                    HeadingLevel::H2 | HeadingLevel::H3 => t.bold(),
                    _ => t.bold().fg(t.dim),
                };
                self.styles.push(style);
            }
            Tag::BlockQuote(_) => {
                self.block();
                let bar = Span::styled("│ ", t.faint());
                self.prefixes.push(Prefix {
                    first: bar.clone(),
                    rest: bar,
                    used: false,
                });
                self.styles.push(t.dim().add_modifier(Modifier::ITALIC));
            }
            Tag::CodeBlock(kind) => {
                self.block();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split([',', ' ']).next().unwrap_or_default().to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.block();
                } else {
                    self.flush();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.block();
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => format!("{} ", ["•", "◦", "▪"][depth % 3]),
                };
                let indent = " ".repeat(text::width(&marker));
                self.prefixes.push(Prefix {
                    first: Span::styled(marker, t.dim()),
                    rest: Span::raw(indent),
                    used: false,
                });
            }
            Tag::Emphasis => self
                .styles
                .push(Style::new().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.styles.push(t.bold()),
            Tag::Strikethrough => self
                .styles
                .push(Style::new().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.links.push((dest_url.into_string(), self.spans.len()));
                self.styles
                    .push(t.fg(t.accent).add_modifier(Modifier::UNDERLINED));
            }
            Tag::Image { dest_url, .. } => {
                self.links.push((dest_url.into_string(), self.spans.len()));
                self.text("[image: ");
                self.styles.push(t.dim());
            }
            Tag::Table(alignments) => {
                self.block();
                self.table = Some(Table {
                    alignments,
                    rows: Vec::new(),
                    row: Vec::new(),
                });
            }
            Tag::TableHead | Tag::TableRow | Tag::TableCell => self.spans.clear(),
            Tag::HtmlBlock => self.block(),
            Tag::FootnoteDefinition(_)
            | Tag::Superscript
            | Tag::Subscript
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                self.gap = true;
            }
            TagEnd::Heading(_) => {
                self.flush();
                self.styles.pop();
                self.gap = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.prefixes.pop();
                self.styles.pop();
                self.gap = true;
            }
            TagEnd::CodeBlock => {
                if let Some((lang, code)) = self.code.take() {
                    self.code_block(&lang, &code);
                }
                self.gap = true;
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.gap = true;
                }
            }
            TagEnd::Item => {
                self.flush();
                self.prefixes.pop();
            }
            TagEnd::HtmlBlock => {
                self.flush();
                self.gap = true;
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                if let Some((url, start)) = self.links.pop() {
                    let label: String = self.spans[start.min(self.spans.len())..]
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect();
                    let shown = url.strip_prefix("mailto:").unwrap_or(&url);
                    if !url.is_empty() && label != shown {
                        self.spans
                            .push(Span::styled(format!(" ({url})"), theme().dim()));
                    }
                }
            }
            TagEnd::Image => {
                self.styles.pop();
                self.links.pop();
                self.text("]");
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.spans);
                if let Some(table) = &mut self.table {
                    table.row.push(cell);
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.render_table(table);
                }
                self.gap = true;
            }
            _ => {}
        }
    }

    /// Starts a block: flushes pending inline text and separates it from
    /// the previous block with a blank line.
    fn block(&mut self) {
        self.flush();
        if self.gap && !self.lines.is_empty() {
            let spans = self
                .prefixes
                .iter()
                .filter(|p| p.first.content == p.rest.content)
                .map(|p| p.rest.clone())
                .collect::<Vec<_>>();
            let line = Line::from(spans);
            self.rows.push(RowText::inset(line.width()));
            self.lines.push(line);
        }
        self.gap = false;
    }

    fn prefix_width(&self) -> usize {
        self.prefixes
            .iter()
            .map(|p| text::width(&p.rest.content))
            .sum()
    }

    /// Emits one finished line, prefixed by the enclosing blocks.
    fn emit(&mut self, spans: Vec<Span<'static>>) {
        self.emit_row(spans, None, 0);
    }

    /// Emits a line that may continue the previous one. Leading quote bars
    /// and hanging indents are decoration; list markers are text. `pad`
    /// more columns of decoration follow the prefixes.
    fn emit_row(&mut self, spans: Vec<Span<'static>>, join: Option<&'static str>, pad: usize) {
        let mut line = Vec::with_capacity(spans.len() + self.prefixes.len());
        let mut inset = 0;
        let mut decorating = true;
        for prefix in &mut self.prefixes {
            let quote = prefix.first.content == prefix.rest.content;
            let span = if prefix.used {
                prefix.rest.clone()
            } else {
                prefix.used = true;
                prefix.first.clone()
            };
            decorating &= quote || span.content == prefix.rest.content;
            if decorating {
                inset += text::width(&span.content);
            }
            line.push(span);
        }
        if decorating {
            inset += pad;
        }
        line.extend(spans);
        self.lines.push(Line::from(line));
        self.rows.push(RowText { inset, join });
    }

    /// Wraps the pending inline text into lines.
    fn flush(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        let width = self.width.saturating_sub(self.prefix_width()).max(4);
        for (line, join) in text::wrap_rows(&spans, width) {
            self.emit_row(line, join, 0);
        }
    }

    fn code_block(&mut self, lang: &str, code: &str) {
        let t = theme();
        let bg = Style::new().bg(t.code_bg);
        let width = self.width.saturating_sub(self.prefix_width()).max(4);
        let inner = width.saturating_sub(2).max(1);
        let start = self.lines.len();
        if !lang.is_empty() {
            let label = text::truncate(lang, inner).into_owned();
            let pad = width.saturating_sub(text::width(&label) + 1);
            self.emit_row(
                vec![
                    Span::styled(" ", bg),
                    Span::styled(label, bg.fg(t.dim).add_modifier(Modifier::ITALIC)),
                    Span::styled(" ".repeat(pad), bg),
                ],
                None,
                1,
            );
        }
        for source in code.strip_suffix('\n').unwrap_or(code).split('\n') {
            let mut join = None;
            for row in text::hard_wrap(source, inner) {
                let pad = inner.saturating_sub(text::width(&row));
                let next = text::hard_join(&row);
                self.emit_row(
                    vec![
                        Span::styled(" ", bg),
                        Span::styled(row, bg),
                        Span::styled(" ".repeat(pad + 1), bg),
                    ],
                    join.replace(next),
                    1,
                );
            }
        }
        self.blocks.push(CodeBlock {
            lines: start..self.lines.len(),
            text: code.strip_suffix('\n').unwrap_or(code).to_owned(),
        });
    }

    fn render_table(&mut self, table: Table) {
        let t = theme();
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let cell_width = |cell: &Vec<Span<'static>>| -> usize {
            cell.iter().map(|s| text::width(&s.content)).sum()
        };
        let mut widths = vec![1; columns];
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell_width(cell));
            }
        }
        let available = self
            .width
            .saturating_sub(self.prefix_width() + (columns - 1) * 3);
        while widths.iter().sum::<usize>() > available {
            let (widest, &w) = widths
                .iter()
                .enumerate()
                .max_by_key(|(_, w)| **w)
                .expect("at least one column");
            if w <= 3 {
                break;
            }
            widths[widest] = w - 1;
        }
        let separator = Span::styled(" │ ", t.faint());
        for (r, row) in table.rows.iter().enumerate() {
            let mut line = Vec::new();
            for (i, width) in widths.iter().enumerate() {
                if i > 0 {
                    line.push(separator.clone());
                }
                let cell = row.get(i).cloned().unwrap_or_default();
                let mut cell = text::truncate_line(Line::from(cell), *width).spans;
                if r == 0 {
                    for span in &mut cell {
                        span.style = span.style.add_modifier(Modifier::BOLD);
                    }
                }
                let pad = width.saturating_sub(cell_width(&cell));
                let (left, right) = match table.alignments.get(i) {
                    Some(Alignment::Right) => (pad, 0),
                    Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                line.push(Span::raw(" ".repeat(left)));
                line.extend(cell);
                line.push(Span::raw(" ".repeat(right)));
            }
            self.emit(line);
            if r == 0 {
                let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                self.emit(vec![Span::styled(rule.join("─┼─"), t.faint())]);
            }
        }
    }

    fn finish(mut self) -> Markdown {
        if let Some((lang, code)) = self.code.take() {
            self.code_block(&lang, &code);
        }
        self.flush();
        Markdown {
            lines: self.lines,
            rows: self.rows,
            code: self.blocks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(source: &str, width: usize) -> Vec<String> {
        render(source, width)
            .lines
            .iter()
            .map(|l| l.to_string().trim_end().to_owned())
            .collect()
    }

    #[test]
    fn paragraphs_and_headings() {
        let lines = plain("# Title\n\nSome **bold** and `code` text.\n\nNext.", 40);
        assert_eq!(
            lines,
            vec!["Title", "", "Some bold and code text.", "", "Next."]
        );
        let rendered = render("Some **bold** and `code`", 40).lines;
        let bold = rendered[0]
            .spans
            .iter()
            .find(|s| s.content == "bold")
            .unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        let code = rendered[0]
            .spans
            .iter()
            .find(|s| s.content == "code")
            .unwrap();
        assert_eq!(code.style.bg, Some(theme().code_bg));
    }

    #[test]
    fn lists_hang_their_markers() {
        let lines = plain(
            "- one two three four\n- second\n  - nested\n\n1. first\n2. next",
            14,
        );
        assert_eq!(
            lines,
            vec![
                "• one two",
                "  three four",
                "• second",
                "  ◦ nested",
                "",
                "1. first",
                "2. next"
            ]
        );
    }

    #[test]
    fn code_blocks_keep_indentation() {
        let lines = plain("```rust\nfn main() {\n    run();\n}\n```\nafter", 30);
        assert_eq!(
            lines,
            vec![" rust", " fn main() {", "     run();", " }", "", "after"]
        );
        let rendered = render("```\nx\n```", 10);
        assert_eq!(rendered.lines[0].width(), 10);
        let rendered = render("intro\n\n```sh\nls\npwd\n```", 20);
        assert_eq!(rendered.code[0].lines, 2..5);
        assert_eq!(rendered.code[0].text, "ls\npwd");
    }

    #[test]
    fn links_show_their_target() {
        assert_eq!(
            plain("see [the docs](https://x.dev) or <https://y.dev>", 60),
            vec!["see the docs (https://x.dev) or https://y.dev"]
        );
    }

    #[test]
    fn tables_align_columns() {
        let lines = plain("| a | long header |\n|---|---:|\n| 1 | 2 |", 40);
        assert_eq!(
            lines,
            vec!["a │ long header", "──┼────────────", "1 │           2"]
        );
    }

    #[test]
    fn quotes_and_tasks() {
        let lines = plain("> quoted text\n\n- [x] done\n- [ ] todo", 30);
        assert_eq!(lines, vec!["│ quoted text", "", "• ✓ done", "• ○ todo"]);
    }

    #[test]
    fn rows_mark_decoration_and_soft_wraps() {
        let rendered = render("- one two three four\n\n> quoted\n\n```\nx\n```", 14);
        let rows: Vec<(usize, Option<&str>)> =
            rendered.rows.iter().map(|r| (r.inset, r.join)).collect();
        assert_eq!(
            rows,
            vec![
                (0, None),
                (2, Some(" ")),
                (0, None),
                (2, None),
                (0, None),
                (1, None)
            ]
        );
    }

    #[test]
    fn unterminated_fence_while_streaming() {
        let lines = plain("text\n```py\nprint(1)", 20);
        assert_eq!(lines, vec!["text", "", " py", " print(1)"]);
    }
}
