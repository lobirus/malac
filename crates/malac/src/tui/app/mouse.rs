//! What the mouse does: clicks on bars, menus and dialogs act on press;
//! in the transcript and the prompt, press, drag and release select text
//! and a press that does not move is a click.

use super::*;

/// Presses this close together count as a double or triple click.
const MULTI_CLICK: Duration = Duration::from_millis(400);

impl App {
    pub fn on_mouse(&mut self, mouse: MouseEvent) -> Option<Action> {
        let (x, y) = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {
                self.wheel(mouse.kind, mouse.modifiers.contains(KeyModifiers::SHIFT));
                None
            }
            MouseEventKind::Moved => {
                self.hover(x, y);
                None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.dirty = true;
                let count = self.count_click(x, y);
                self.pointer = (x, y);
                self.press(x, y, count)
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.drag_to(x, y);
                None
            }
            MouseEventKind::Up(MouseButton::Left) => self.release(),
            MouseEventKind::Down(MouseButton::Right) => {
                self.dirty = true;
                self.context_menu(x, y);
                None
            }
            MouseEventKind::Down(MouseButton::Middle) => match self.hits.at(x, y) {
                Some(Target::Tab(index)) => {
                    self.dirty = true;
                    Some(Action::CloseTab(*index))
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn wheel(&mut self, kind: MouseEventKind, shift: bool) {
        let (dx, dy) = match kind {
            MouseEventKind::ScrollUp if shift => (-6, 0),
            MouseEventKind::ScrollDown if shift => (6, 0),
            MouseEventKind::ScrollUp => (0, -3),
            MouseEventKind::ScrollDown => (0, 3),
            MouseEventKind::ScrollLeft => (-6, 0),
            _ => (6, 0),
        };
        let approval = self.approval_visible();
        let jobs = self.workspace_jobs();
        match &mut self.overlay {
            Some(
                Overlay::Models(p)
                | Overlay::Sessions(p)
                | Overlay::Agents(p)
                | Overlay::Workspaces(p),
            ) => p.scroll_by(dy),
            Some(Overlay::AgentMap(map)) => map.scroll_by(dx, dy),
            Some(Overlay::Folder(browser)) => browser.scroll_by(dy),
            Some(Overlay::Jobs(view)) => view.scroll_by(dy.signum(), &jobs),
            Some(Overlay::Help(help)) => {
                help.scroll = (help.scroll as isize + dy).max(0) as usize;
            }
            Some(_) => {}
            None if !approval && dy != 0 => {
                self.scroll_by(dy);
                // Keep extending a selection that is being dragged.
                if self.drag.is_some() {
                    let (x, y) = self.pointer;
                    self.drag_to(x, y);
                }
            }
            None => {}
        }
        self.dirty = true;
    }

    fn hover(&mut self, x: u16, y: u16) {
        let target = self.hits.at(x, y).cloned();
        if let Some(Overlay::Menu(menu)) = &mut self.overlay {
            match target {
                Some(Target::MenuItem(index)) => self.dirty |= menu.hover(index),
                Some(Target::MenuTitle(index)) if menu.open && menu.place != Place::Bar(index) => {
                    if matches!(menu.place, Place::Bar(_)) {
                        self.open_menu(index, true);
                        self.dirty = true;
                    }
                }
                _ => {}
            }
        }
    }

    fn count_click(&mut self, x: u16, y: u16) -> u8 {
        let count = match self.clicks {
            Some(c) if c.at.elapsed() < MULTI_CLICK && c.y == y && c.x.abs_diff(x) <= 1 => {
                c.count % 3 + 1
            }
            _ => 1,
        };
        self.clicks = Some(Clicks {
            at: Instant::now(),
            x,
            y,
            count,
        });
        count
    }

    /// The transcript cell at a screen position, clamped into the pane.
    fn transcript_point(&self, x: u16, y: u16) -> Point {
        let area = self.geometry.transcript;
        let row = y.clamp(area.y, area.bottom().saturating_sub(1)) - area.y;
        let col = x.clamp(area.x, area.right().saturating_sub(1)) - area.x;
        Point::new(self.transcript_top() + row as usize, col as usize)
    }

    /// The prompt's byte offset at a screen position.
    fn input_offset(&self, x: u16, y: u16) -> usize {
        let area = self.geometry.input;
        let row = y.saturating_sub(area.y) as usize + self.geometry.input_first;
        let col = x.saturating_sub(area.x + 2) as usize;
        self.editor.offset_at(self.input_width(), row, col)
    }

    fn press(&mut self, x: u16, y: u16, count: u8) -> Option<Action> {
        let target = self.hits.at(x, y).cloned();
        if self.approval_visible() {
            return match target {
                Some(Target::Do(action @ Action::Answer(_))) => Some(action),
                _ => None,
            };
        }
        if let Some(overlay) = &mut self.overlay {
            let outcome = match (overlay, &target) {
                (Overlay::Menu(menu), Some(Target::MenuItem(index))) => menu.activate(*index),
                (
                    Overlay::Models(p)
                    | Overlay::Sessions(p)
                    | Overlay::Agents(p)
                    | Overlay::Workspaces(p),
                    Some(Target::Pick(index)),
                ) => p.click(*index),
                (Overlay::AgentMap(map), Some(Target::Pick(index))) => map.click(*index, count),
                (Overlay::Folder(browser), Some(Target::Pick(index))) => {
                    browser.click(*index, count)
                }
                (Overlay::Jobs(view), Some(Target::Pick(index))) => view.click(*index),
                (Overlay::Remote(dialog), Some(Target::Pick(index))) => {
                    dialog.selected = *index;
                    Outcome::Stay
                }
                (_, Some(Target::Do(action))) => Outcome::Run(action.clone()),
                (Overlay::Menu(menu), Some(Target::MenuTitle(index))) => {
                    if menu.place == Place::Bar(*index) {
                        Outcome::Close
                    } else {
                        Outcome::Switch(*index)
                    }
                }
                (_, Some(Target::Overlay)) => Outcome::Stay,
                _ => Outcome::Close,
            };
            if let Outcome::Switch(index) = outcome {
                self.open_menu(index, true);
                return None;
            }
            return self.outcome(outcome);
        }
        // A press anywhere lets go of an earlier selection.
        let had_selection = self.clear_selections();
        match target.clone()? {
            Target::Do(action) => Some(action),
            Target::MenuTitle(index) => {
                self.open_menu(index, true);
                None
            }
            Target::Tab(index) => Some(Action::GoTab(index)),
            Target::CloseTab(index) => Some(Action::CloseTab(index)),
            Target::Entry { .. } | Target::Transcript => {
                let entry = match target {
                    Some(Target::Entry { index, .. }) if !had_selection => Some(index),
                    // Dismissing a selection is all the first click does.
                    _ => None,
                };
                let at = self.transcript_point(x, y);
                match count {
                    1 => {
                        self.drag = Some(Drag::Transcript {
                            start: at,
                            entry,
                            moved: false,
                        })
                    }
                    2 => self.select_word(at),
                    _ => self.select_line(at),
                }
                None
            }
            Target::Completion(index) => {
                let name = self.completions().get(index)?.name.clone();
                self.editor.set_text(&format!("/{name}"));
                self.submit()
            }
            Target::FileMatch(index) => {
                self.choose_file(index);
                None
            }
            Target::Chip(index) => {
                if index < self.attachments.len() {
                    let removed = self.attachments.remove(index);
                    self.flash(format!("removed {}", removed.name));
                }
                None
            }
            Target::Input => {
                self.focus = Focus::Input;
                let at = self.input_offset(x, y);
                let range = match count {
                    1 => {
                        self.editor.set_cursor(at);
                        self.drag = Some(Drag::Input { anchor: at });
                        return None;
                    }
                    2 => self.editor.word_range(at),
                    _ => self.editor.line_range(at),
                };
                self.editor.select(range.start, range.end);
                self.copy_input_selection();
                None
            }
            Target::MenuItem(_) | Target::Pick(_) | Target::Overlay => None,
        }
    }

    fn drag_to(&mut self, x: u16, y: u16) {
        self.pointer = (x, y);
        match self.drag {
            Some(Drag::Transcript {
                start,
                entry,
                moved,
            }) => {
                let area = self.geometry.transcript;
                if y < area.y {
                    self.scroll_by(-1);
                } else if y >= area.bottom() {
                    self.scroll_by(1);
                }
                let head = self.transcript_point(x, y);
                let Some(agent) = self.tab().shown().map(str::to_owned) else {
                    return;
                };
                if head == start && !moved {
                    return;
                }
                self.drag = Some(Drag::Transcript {
                    start,
                    entry,
                    moved: true,
                });
                self.selection = Some(Selection {
                    agent,
                    width: area.width as usize,
                    anchor: start,
                    head,
                });
                self.dirty = true;
            }
            Some(Drag::Input { anchor }) => {
                let at = self.input_offset(x, y);
                self.editor.select(anchor, at);
                self.dirty = true;
            }
            None => {}
        }
    }

    /// Keeps scrolling while a selection is dragged past the transcript's
    /// top or bottom edge; returns whether it did.
    pub(super) fn drag_scroll(&mut self) -> bool {
        let Some(Drag::Transcript { moved: true, .. }) = self.drag else {
            return false;
        };
        let (x, y) = self.pointer;
        let area = self.geometry.transcript;
        if (area.y..area.bottom()).contains(&y) {
            return false;
        }
        self.drag_to(x, y);
        true
    }

    /// Release copies what was dragged over; a press that did not move is
    /// a click, which expands a card or opens a subagent.
    fn release(&mut self) -> Option<Action> {
        match self.drag.take()? {
            Drag::Transcript { moved: true, .. } => {
                self.copy_selection();
                None
            }
            Drag::Transcript { entry, start, .. } => {
                let index = entry?;
                if self.is_file_card(index) {
                    // The card's first row names it; each row after is a file.
                    let line = self.entry_line(index, start)?;
                    return Some(Action::OpenEntry(index, line.saturating_sub(1)));
                }
                match &self.shown()?.transcript.items().get(index)?.entry {
                    Entry::Agent(card) => Some(Action::OpenAgent(card.id.clone())),
                    Entry::Tool(_) | Entry::Thinking { .. } => Some(Action::ToggleEntry(index)),
                    _ => None,
                }
            }
            Drag::Input { .. } => {
                self.copy_input_selection();
                None
            }
        }
    }

    /// The row within entry `index` that a transcript point is on.
    fn entry_line(&self, index: usize, at: Point) -> Option<usize> {
        let transcript = &self.shown()?.transcript;
        let item = transcript.items().get(index)?;
        let first = transcript.row_of(index) + usize::from(item.gap());
        Some(at.row.saturating_sub(first))
    }

    fn select_word(&mut self, at: Point) {
        let Some(view) = self.shown() else {
            return;
        };
        let rows = view.transcript.rows_in(at.row..at.row + 1);
        let Some(Some((line, text))) = rows.first() else {
            return;
        };
        let columns = select::word_at(line, text.inset, at.col);
        self.selection = Some(Selection {
            agent: view.id.clone(),
            width: self.geometry.transcript.width as usize,
            anchor: Point::new(at.row, columns.start),
            head: Point::new(at.row, columns.end.saturating_sub(1)),
        });
        self.copy_selection();
    }

    /// Selects the whole line under the pointer, soft wraps included.
    fn select_line(&mut self, at: Point) {
        const REACH: usize = 200;
        let Some(view) = self.shown() else {
            return;
        };
        let first = at.row.saturating_sub(REACH);
        let rows = view.transcript.rows_in(first..at.row + REACH);
        let Some(Some(_)) = rows.get(at.row - first) else {
            return;
        };
        let range = select::line_rows(&rows, at.row - first);
        let (Some(Some((_, start))), Some(Some((last, _)))) =
            (rows.get(range.start), rows.get(range.end - 1))
        else {
            return;
        };
        let end = select::text_end(last).saturating_sub(1);
        self.selection = Some(Selection {
            agent: view.id.clone(),
            width: self.geometry.transcript.width as usize,
            anchor: Point::new(first + range.start, start.inset),
            head: Point::new(first + range.end - 1, end),
        });
        self.copy_selection();
    }

    /// The transcript text the selection covers.
    pub fn selected_text(&self) -> Option<String> {
        let selection = self.selection.as_ref()?;
        let view = self.agents.get(&selection.agent)?;
        let (start, end) = selection.span();
        let rows = view.transcript.rows_in(start.row..end.row + 1);
        Some(select::copy_text(&rows, start.row, selection)).filter(|t| !t.is_empty())
    }

    pub(super) fn copy_selection(&mut self) {
        if let Some(text) = self.selected_text() {
            self.copy(&text);
        }
    }

    fn copy_input_selection(&mut self) {
        if let Some(text) = self.editor.selected_text().map(str::to_owned) {
            self.copy(&text);
        }
    }

    fn context_menu(&mut self, x: u16, y: u16) {
        if self.approval_visible() {
            return;
        }
        if self.overlay.is_some() {
            self.overlay = None;
            return;
        }
        let Some(Target::Entry { index, line }) = self.hits.at(x, y).cloned() else {
            return;
        };
        let Some(item) = self.shown().and_then(|v| v.transcript.items().get(index)) else {
            return;
        };
        let mut items = Vec::new();
        if self.selection.is_some() {
            items.push(Item::new("Copy selection", 'y', Action::CopySelection));
        }
        items.push(Item::new("Copy", 'c', Action::CopyEntry(index)));
        if item.code_block(Some(line)).is_some() {
            items.push(Item::new(
                "Copy code block",
                'b',
                Action::CopyCode(index, Some(line)),
            ));
        }
        let openable = self.entry_openable(index);
        if let Some(first) = openable.first() {
            let label = match first {
                Openable::Image { .. } => "Open image",
                Openable::Shared(_) => "Open",
            };
            let file = match self.is_file_card(index) {
                true => line.saturating_sub(1).min(openable.len() - 1),
                false => 0,
            };
            items.push(Item::new(label, 'o', Action::OpenEntry(index, file)));
        }
        match &item.entry {
            Entry::Tool(tool) if tool.name == "send_to_user" && !openable.is_empty() => {}
            Entry::Tool(tool) => {
                let label = if tool.expanded { "Collapse" } else { "Expand" };
                items.push(Item::new(label, 'e', Action::ToggleEntry(index)));
            }
            Entry::Thinking { expanded, .. } => {
                let label = if *expanded { "Collapse" } else { "Expand" };
                items.push(Item::new(label, 'e', Action::ToggleEntry(index)));
            }
            Entry::Agent(card) => {
                items.push(Item::new("Open", 'o', Action::OpenAgent(card.id.clone())));
                items.push(Item::new("Stop", 's', Action::StopAgent(card.id.clone())));
            }
            _ => {}
        }
        self.tab_mut().selected = Some(index);
        self.overlay = Some(Overlay::Menu(Menu::new(Place::At(x, y), items, true)));
    }
}
