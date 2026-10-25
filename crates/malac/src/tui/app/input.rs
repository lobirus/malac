//! What keys, clicks and the mouse wheel do.

use super::*;

impl App {
    /// Handles a key. `more_input` says other keys arrived in the same read,
    /// which means Enter is part of a paste the terminal did not bracket.
    pub fn on_key(&mut self, key: KeyEvent, more_input: bool) -> Option<Action> {
        if self.approval_visible() {
            return self.approval_key(key);
        }
        if self.overlay.is_some() {
            return self.overlay_key(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if let KeyCode::F(n) = key.code {
            return bars::KEYS
                .get(usize::from(n).wrapping_sub(1))
                .map(|(_, action)| action.clone());
        }
        match key.code {
            KeyCode::Char('t') if ctrl => return Some(Action::NewTab),
            KeyCode::Char('w') if ctrl => return Some(Action::CloseTab(self.active)),
            KeyCode::Char('p') if ctrl => return Some(Action::PickModel),
            KeyCode::Char('o') if ctrl => return Some(Action::Menu),
            KeyCode::Char('g') if ctrl => return Some(Action::ToggleSidebar),
            KeyCode::Char('s') if ctrl => return Some(Action::ToggleMouse),
            KeyCode::Char('y') if ctrl => return Some(Action::Copy),
            KeyCode::Char('l') if ctrl => {
                self.clear_screen = true;
                return None;
            }
            KeyCode::Char('c') if ctrl => return self.ctrl_c(),
            KeyCode::Char('d') if ctrl && self.editor.is_empty() => return Some(Action::Quit),
            KeyCode::PageUp if ctrl => return Some(Action::PrevTab),
            KeyCode::PageDown if ctrl => return Some(Action::NextTab),
            KeyCode::Left if alt => return Some(Action::PrevTab),
            KeyCode::Right if alt => return Some(Action::NextTab),
            KeyCode::Char(c @ '1'..='9') if alt => {
                return Some(Action::GoTab(c as usize - '1' as usize));
            }
            KeyCode::Char('m') if alt => return Some(Action::Menu),
            KeyCode::BackTab => return Some(Action::CycleMode),
            KeyCode::PageUp => {
                self.scroll_page(-1);
                return None;
            }
            KeyCode::PageDown => {
                self.scroll_page(1);
                return None;
            }
            _ => {}
        }
        match self.focus {
            Focus::Transcript => self.transcript_key(key),
            Focus::Input => self.input_key(key, more_input),
        }
    }

    fn ctrl_c(&mut self) -> Option<Action> {
        if self.busy() {
            return Some(Action::Stop);
        }
        if !self.editor.is_empty() {
            self.editor.clear();
            return None;
        }
        if self
            .quit_armed
            .is_some_and(|at| at.elapsed() < DOUBLE_PRESS)
        {
            self.quit = true;
        } else {
            self.quit_armed = Some(Instant::now());
            self.flash("press Ctrl-C again to quit");
        }
        None
    }

    fn approval_key(&mut self, key: KeyEvent) -> Option<Action> {
        let approval = self.approvals.front_mut()?;
        if approval.arrived.elapsed() < ANSWER_GUARD {
            return None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let answer = |d| Some(Action::Answer(d));
        match key.code {
            KeyCode::Char('y' | 'Y') => answer(Decision::Allow),
            KeyCode::Char('a' | 'A') => answer(Decision::AllowSession),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => answer(Decision::Deny),
            KeyCode::Char('c') if ctrl => answer(Decision::Deny),
            KeyCode::Enter => answer(dialog::ANSWERS[approval.focus].1),
            KeyCode::Left | KeyCode::BackTab => {
                approval.focus = (approval.focus + 2) % 3;
                None
            }
            KeyCode::Right | KeyCode::Tab => {
                approval.focus = (approval.focus + 1) % 3;
                None
            }
            KeyCode::Char('d') => {
                approval.details = !approval.details;
                None
            }
            KeyCode::Char('h') => {
                self.approvals_hidden = true;
                None
            }
            _ => None,
        }
    }

    fn overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
        let outcome = match self.overlay.as_mut()? {
            Overlay::Menu(menu) => menu.key(key),
            Overlay::Models(p) | Overlay::Sessions(p) | Overlay::Agents(p) => p.key(key),
            Overlay::Help(help) => help.key(key),
            Overlay::Rename(rename) => rename.key(key),
            Overlay::Update(dialog) => dialog.key(key, &self.update),
        };
        self.outcome(outcome)
    }

    fn outcome(&mut self, outcome: Outcome) -> Option<Action> {
        match outcome {
            Outcome::Stay => None,
            Outcome::Close => {
                self.overlay = None;
                None
            }
            Outcome::Run(action) => {
                let keep = matches!(
                    action,
                    Action::InstallUpdate | Action::CheckUpdate | Action::StopAgent(_)
                );
                if !keep {
                    self.overlay = None;
                }
                Some(action)
            }
            Outcome::Switch(index) => {
                let open = match &self.overlay {
                    Some(Overlay::Menu(menu)) => menu.open,
                    _ => true,
                };
                self.open_menu(index, open);
                None
            }
        }
    }

    pub(super) fn open_menu(&mut self, index: usize, open: bool) {
        let items = self.menu_items(index);
        self.overlay = Some(Overlay::Menu(Menu::new(Place::Bar(index), items, open)));
    }

    fn input_key(&mut self, key: KeyEvent, more_input: bool) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let width = self.input_width();
        let completions = self.completions();
        if !completions.is_empty() {
            let len = completions.len();
            let selected = self.completion.min(len - 1);
            match key.code {
                KeyCode::Up => {
                    self.completion = (selected + len - 1) % len;
                    return None;
                }
                KeyCode::Down => {
                    self.completion = (selected + 1) % len;
                    return None;
                }
                KeyCode::Tab => {
                    let (name, args, _) = completions[selected];
                    let space = if args.is_empty() { "" } else { " " };
                    self.editor.set_text(&format!("/{name}{space}"));
                    return None;
                }
                KeyCode::Enter if !more_input && !shift && !alt => {
                    let (name, ..) = completions[selected];
                    self.editor.set_text(&format!("/{name}"));
                    return self.submit();
                }
                KeyCode::Esc => {
                    self.completion_dismissed = true;
                    return None;
                }
                _ => {}
            }
        }
        let before = self.editor.text().len();
        let e = &mut self.editor;
        match key.code {
            KeyCode::Enter if shift || alt || more_input => e.newline(),
            KeyCode::Enter => return self.submit(),
            KeyCode::Char('j') if ctrl => e.newline(),
            KeyCode::Char('a') if ctrl => e.home(),
            KeyCode::Char('e') if ctrl => e.end(),
            KeyCode::Char('b') if ctrl => e.left(),
            KeyCode::Char('f') if ctrl => e.right(),
            KeyCode::Char('h') if ctrl => e.backspace(),
            KeyCode::Char('u') if ctrl => e.delete_to_line_start(),
            KeyCode::Char('k') if ctrl => e.delete_to_line_end(),
            KeyCode::Char('n') if ctrl => {
                e.history_next();
            }
            KeyCode::Char('b') if alt => e.word_left(),
            KeyCode::Char('f') if alt => e.word_right(),
            KeyCode::Char('d') if alt => e.delete_word_forward(),
            KeyCode::Char('?') if e.is_empty() => return Some(Action::Help),
            KeyCode::Char(_) if ctrl || alt => {}
            KeyCode::Char(c) => e.insert(c.encode_utf8(&mut [0; 4])),
            KeyCode::Backspace if ctrl || alt => e.delete_word_back(),
            KeyCode::Backspace => e.backspace(),
            KeyCode::Delete if ctrl || alt => e.delete_word_forward(),
            KeyCode::Delete => e.delete_forward(),
            KeyCode::Left if ctrl => e.word_left(),
            KeyCode::Right if ctrl => e.word_right(),
            KeyCode::Left => e.left(),
            KeyCode::Right => e.right(),
            KeyCode::Home if e.is_empty() => self.scroll_to_top(),
            KeyCode::End if e.is_empty() => self.tab_mut().scroll = None,
            KeyCode::Home => e.home(),
            KeyCode::End => e.end(),
            KeyCode::Up => {
                if !e.up(width) {
                    e.history_prev();
                }
            }
            KeyCode::Down => {
                if !e.down(width) {
                    e.history_next();
                }
            }
            KeyCode::Tab => self.focus_transcript(),
            KeyCode::Esc => return self.escape(),
            _ => {}
        }
        if self.editor.text().len() != before {
            self.completion = 0;
            self.completion_dismissed = false;
        }
        None
    }

    /// Esc leaves a subagent's view first; on the tab's own view it stops the turn.
    fn escape(&mut self) -> Option<Action> {
        if !self.tab().view.is_empty() {
            return Some(Action::Back);
        }
        if self.busy() {
            return Some(Action::Stop);
        }
        if self.focus == Focus::Transcript {
            self.focus = Focus::Input;
            self.tab_mut().selected = None;
        }
        None
    }

    pub fn input_width(&self) -> usize {
        (self.geometry.input.width as usize)
            .saturating_sub(3)
            .max(1)
    }

    fn submit(&mut self) -> Option<Action> {
        let text = self.editor.text().trim_end().to_owned();
        if text.trim().is_empty() {
            return None;
        }
        if let Some(command) = text.strip_prefix('/') {
            let name = command.split_whitespace().next().unwrap_or_default();
            if !name.contains('/') {
                self.editor.submit();
                editor::append_history(&self.history_path, &text);
                return match Slash::parse(command) {
                    Ok(slash) => self.slash(slash),
                    Err(err) => {
                        self.notice(Level::Error, format!("{err:#}"));
                        None
                    }
                };
            }
        }
        if self.tab().read_only() {
            self.flash("this tab watches a subagent; prompts go to session tabs");
            return None;
        }
        self.editor.submit();
        editor::append_history(&self.history_path, &text);
        self.send(text);
        None
    }

    /// Prompts the active tab's agent, or queues the prompt while it works.
    pub fn send(&mut self, text: String) {
        let tab = self.tab_mut();
        if tab.handle.is_busy() || !tab.queued.is_empty() {
            tab.queued.push_back(text);
        } else {
            tab.handle.send(vec![Block::text(text)]);
        }
        tab.reset_view();
        self.focus = Focus::Input;
    }

    fn slash(&mut self, slash: Slash<'_>) -> Option<Action> {
        Some(match slash {
            Slash::Exit => Action::Quit,
            Slash::Help => Action::Help,
            Slash::Model("") => Action::PickModel,
            Slash::Model(spec) => Action::SetModel(spec.to_owned()),
            Slash::Mode("") => Action::ShowMode,
            Slash::Mode(arg) => match arg.parse::<Mode>() {
                Ok(mode) => Action::SetMode(mode),
                Err(err) => {
                    self.notice(Level::Error, format!("{err:#}"));
                    return None;
                }
            },
            Slash::Agents => Action::Agents,
            Slash::Compact => Action::Compact,
            Slash::Clear => Action::Clear,
            Slash::Resume("") | Slash::Sessions => Action::Sessions,
            Slash::Resume(id) => Action::Resume(id.to_owned()),
            Slash::Cost => Action::ShowCost,
            Slash::Mcp => Action::ShowMcp,
            Slash::Memory => Action::ShowMemory,
            Slash::Web(arg) => {
                Action::Web(arg == "remote" || self.session.runtime.config.web.remote)
            }
        })
    }

    fn visible_entries(&self) -> Vec<usize> {
        self.shown().map_or_else(Vec::new, |v| {
            v.transcript
                .items()
                .iter()
                .enumerate()
                .filter(|(_, i)| !i.lines().is_empty())
                .map(|(n, _)| n)
                .collect()
        })
    }

    fn focus_transcript(&mut self) {
        let last = self.visible_entries().last().copied();
        if last.is_some() {
            self.focus = Focus::Transcript;
            let tab = self.tab_mut();
            tab.selected = tab.selected.or(last);
        }
    }

    fn select_step(&mut self, delta: isize) {
        let entries = self.visible_entries();
        if entries.is_empty() {
            return;
        }
        let current = self
            .tab()
            .selected
            .and_then(|s| entries.iter().position(|&e| e == s))
            .unwrap_or(entries.len() - 1);
        let next = (current as isize + delta).clamp(0, entries.len() as isize - 1) as usize;
        self.tab_mut().selected = Some(entries[next]);
        self.reveal(entries[next]);
    }

    /// Scrolls so an entry is on screen.
    fn reveal(&mut self, index: usize) {
        let Some(view) = self.shown() else {
            return;
        };
        let t = &view.transcript;
        let start = t.row_of(index);
        let end = start + t.items()[index].height();
        let total = t.height();
        let height = self.geometry.transcript.height as usize;
        let max_top = total.saturating_sub(height);
        let top = self.tab().scroll.unwrap_or(max_top);
        let top = if start < top {
            start
        } else if end > top + height {
            end.saturating_sub(height)
        } else {
            top
        };
        self.set_top(top);
    }

    fn set_top(&mut self, top: usize) {
        let total = self.shown().map_or(0, |v| v.transcript.height());
        let max_top = total.saturating_sub(self.geometry.transcript.height as usize);
        let tab = self.tab_mut();
        if top >= max_top {
            tab.scroll = None;
        } else {
            if tab.scroll.is_none() {
                tab.seen = total;
            }
            tab.scroll = Some(top);
        }
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let total = self.shown().map_or(0, |v| v.transcript.height());
        let max_top = total.saturating_sub(self.geometry.transcript.height as usize);
        let top = self.tab().scroll.unwrap_or(max_top) as isize + delta;
        self.set_top(top.max(0) as usize);
    }

    fn scroll_page(&mut self, direction: isize) {
        let page = (self.geometry.transcript.height as isize - 2).max(1);
        self.scroll_by(direction * page);
    }

    fn scroll_to_top(&mut self) {
        self.set_top(0);
    }

    fn transcript_key(&mut self, key: KeyEvent) -> Option<Action> {
        let selected = self.tab().selected;
        let agent = selected.and_then(|i| match &self.shown()?.transcript.items().get(i)?.entry {
            Entry::Agent(card) => Some(card.id.clone()),
            _ => None,
        });
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.select_step(-1),
            KeyCode::Down | KeyCode::Char('j') => self.select_step(1),
            KeyCode::Home | KeyCode::Char('g') => self.select_step(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.select_step(isize::MAX / 2),
            KeyCode::Enter | KeyCode::Char(' ') => {
                return match (agent, selected) {
                    (Some(id), _) => Some(Action::OpenAgent(id)),
                    (None, Some(index)) => Some(Action::ToggleEntry(index)),
                    _ => None,
                };
            }
            KeyCode::Char('o') | KeyCode::Char('t') if agent.is_some() => {
                return agent.map(Action::PinAgent);
            }
            KeyCode::Char('y') | KeyCode::Char('c') => return selected.map(Action::CopyEntry),
            KeyCode::Char('b') => return selected.map(|i| Action::CopyCode(i, None)),
            KeyCode::Tab => {
                self.focus = Focus::Input;
                self.tab_mut().selected = None;
            }
            KeyCode::Esc => return self.escape(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.focus = Focus::Input;
                self.tab_mut().selected = None;
                self.editor.insert(c.encode_utf8(&mut [0; 4]));
            }
            _ => {}
        }
        None
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) -> Option<Action> {
        let (x, y) = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if mouse.kind == MouseEventKind::ScrollUp {
                    -3
                } else {
                    3
                };
                let approval = self.approval_visible();
                match &mut self.overlay {
                    Some(Overlay::Models(p) | Overlay::Sessions(p) | Overlay::Agents(p)) => {
                        p.scroll_by(delta);
                    }
                    Some(Overlay::Help(help)) => {
                        help.scroll = (help.scroll as isize + delta).max(0) as usize;
                    }
                    Some(_) => {}
                    None if !approval => self.scroll_by(delta),
                    None => {}
                }
                self.dirty = true;
                None
            }
            MouseEventKind::Moved => {
                let target = self.hits.at(x, y).cloned();
                if let Some(Overlay::Menu(menu)) = &mut self.overlay {
                    match target {
                        Some(Target::MenuItem(index)) => self.dirty |= menu.hover(index),
                        Some(Target::MenuTitle(index))
                            if menu.open && menu.place != Place::Bar(index) =>
                        {
                            if matches!(menu.place, Place::Bar(_)) {
                                self.open_menu(index, true);
                                self.dirty = true;
                            }
                        }
                        _ => {}
                    }
                }
                None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.dirty = true;
                self.click(x, y)
            }
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

    fn click(&mut self, x: u16, y: u16) -> Option<Action> {
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
                    Overlay::Models(p) | Overlay::Sessions(p) | Overlay::Agents(p),
                    Some(Target::Pick(index)),
                ) => p.click(*index),
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
        match target? {
            Target::Do(action) => Some(action),
            Target::MenuTitle(index) => {
                self.open_menu(index, true);
                None
            }
            Target::Tab(index) => Some(Action::GoTab(index)),
            Target::CloseTab(index) => Some(Action::CloseTab(index)),
            Target::Entry { index, .. } => {
                let entry = &self.shown()?.transcript.items().get(index)?.entry;
                match entry {
                    Entry::Agent(card) => Some(Action::OpenAgent(card.id.clone())),
                    Entry::Tool(_) | Entry::Thinking { .. } => Some(Action::ToggleEntry(index)),
                    _ => None,
                }
            }
            Target::Completion(index) => {
                let (name, ..) = *self.completions().get(index)?;
                self.editor.set_text(&format!("/{name}"));
                self.submit()
            }
            Target::Input => {
                self.focus = Focus::Input;
                let area = self.geometry.input;
                let row = (y.saturating_sub(area.y)) as usize;
                let col = (x.saturating_sub(area.x + 2)) as usize;
                let width = self.input_width();
                self.editor.click(width, row, col);
                None
            }
            Target::MenuItem(_) | Target::Pick(_) | Target::Overlay => None,
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
        let mut items = vec![Item::new("Copy", 'c', Action::CopyEntry(index))];
        if item.code_block(Some(line)).is_some() {
            items.push(Item::new(
                "Copy code block",
                'b',
                Action::CopyCode(index, Some(line)),
            ));
        }
        match &item.entry {
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
                items.push(Item::new(
                    "Open in a new tab",
                    't',
                    Action::PinAgent(card.id.clone()),
                ));
                items.push(Item::new("Stop", 's', Action::StopAgent(card.id.clone())));
            }
            _ => {}
        }
        self.tab_mut().selected = Some(index);
        self.overlay = Some(Overlay::Menu(Menu::new(Place::At(x, y), items, true)));
    }
}
