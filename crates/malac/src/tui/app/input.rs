//! What keys do: in the prompt, the transcript, dialogs and menus.

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
        if key.code == KeyCode::Esc && self.clear_selections() {
            return None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
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
            KeyCode::Char('n') if ctrl && self.editor.is_empty() => return Some(Action::NewAgent),
            KeyCode::Char('r') if ctrl => return Some(Action::Reconnect),
            KeyCode::Up if alt => return Some(Action::StepAgent(-1)),
            KeyCode::Down if alt => return Some(Action::StepAgent(1)),
            KeyCode::PageUp if ctrl && shift => return Some(Action::MoveTab(-1)),
            KeyCode::PageDown if ctrl && shift => return Some(Action::MoveTab(1)),
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

    /// Drops any mouse selection; returns whether there was one.
    pub(super) fn clear_selections(&mut self) -> bool {
        let transcript = self.selection.take().is_some();
        self.editor.clear_selection() || transcript
    }

    fn overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
        // F3 switches between the agent list and the map.
        match (&self.overlay, key.code) {
            (Some(Overlay::Agents(_)), KeyCode::F(3)) => return Some(Action::AgentMap),
            (Some(Overlay::AgentMap(_)), KeyCode::F(3)) => return Some(Action::AgentList),
            _ => {}
        }
        let jobs = self.workspace_jobs();
        let outcome = match self.overlay.as_mut()? {
            Overlay::Menu(menu) => menu.key(key),
            Overlay::Models(p)
            | Overlay::Sessions(p)
            | Overlay::Agents(p)
            | Overlay::Workspaces(p) => p.key(key),
            Overlay::AgentMap(map) => map.key(key),
            Overlay::Folder(browser) => browser.key(key),
            Overlay::Jobs(view) => view.key(key, &jobs),
            Overlay::Help(help) => help.key(key),
            Overlay::Rename(rename) => rename.key(key),
            Overlay::Confirm(confirm) => confirm.key(key),
            Overlay::Update(dialog) => dialog.key(key, &self.update),
            Overlay::Remote(dialog) => dialog.key(key),
        };
        self.outcome(outcome)
    }

    pub(super) fn outcome(&mut self, outcome: Outcome) -> Option<Action> {
        match outcome {
            Outcome::Stay => None,
            Outcome::Close => {
                self.overlay = None;
                None
            }
            Outcome::Run(action) => {
                let keep = matches!(
                    action,
                    Action::InstallUpdate
                        | Action::CheckUpdate
                        | Action::StopAgent(_)
                        | Action::StopJob(_)
                        | Action::PairDevice
                        | Action::RemoteDialog
                        | Action::RevokeDevice(_)
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
        if let Some(action) = self.file_key(key, more_input) {
            return action;
        }
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
            KeyCode::Char('v') if ctrl => return Some(Action::PasteImage),
            KeyCode::Backspace if e.at_start() && !self.attachments.is_empty() => {
                self.attachments.pop();
            }
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
            self.file_choice = 0;
        }
        if self.file_query().is_some() {
            self.index_files();
        }
        None
    }

    /// Keys for the `@` file picker while it is open; `None` passes the
    /// key on to the prompt.
    fn file_key(&mut self, key: KeyEvent, more_input: bool) -> Option<Option<Action>> {
        let (at, _) = self.file_query()?;
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
        let loading = self.files.list.is_none();
        let count = self.file_matches().len();
        match key.code {
            KeyCode::Esc => self.file_dismissed = Some(at),
            KeyCode::Up if count > 0 => self.file_choice = (self.file_choice + count - 1) % count,
            KeyCode::Down if count > 0 => self.file_choice = (self.file_choice + 1) % count,
            KeyCode::Enter | KeyCode::Tab if plain && !more_input && loading => {}
            KeyCode::Enter | KeyCode::Tab if plain && !more_input && count > 0 => {
                self.choose_file(self.file_choice.min(count - 1));
            }
            _ => return None,
        }
        Some(None)
    }

    /// Replaces the `@query` with the chosen file's chip.
    pub(super) fn choose_file(&mut self, index: usize) {
        let Some((at, _)) = self.file_query() else {
            return;
        };
        let Some(chosen) = self.file_matches().into_iter().nth(index) else {
            return;
        };
        let path = self.files.root.join(&chosen.path);
        let cursor = self.editor.cursor_offset();
        self.editor.replace(at..cursor, "");
        self.file_choice = 0;
        self.attach(&path);
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

    pub(super) fn submit(&mut self) -> Option<Action> {
        let text = self.editor.text().trim_end().to_owned();
        if text.trim().is_empty() && self.attachments.is_empty() {
            return None;
        }
        if let Some(command) = text.strip_prefix('/') {
            let name = command.split_whitespace().next().unwrap_or_default();
            if !name.contains('/') {
                self.editor.submit();
                editor::append_history(&self.history_path, &text);
                if let Some(action) = self.tui_command(command) {
                    return action;
                }
                return match Slash::parse(command) {
                    Ok(slash) => self.slash(slash),
                    Err(err) => {
                        self.notice(Level::Error, format!("{err:#}"));
                        None
                    }
                };
            }
        }
        self.editor.submit();
        if !text.is_empty() {
            editor::append_history(&self.history_path, &text);
        }
        let attachments = std::mem::take(&mut self.attachments);
        self.send(Prompt { text, attachments });
        None
    }

    /// The commands only this interface has; `None` when it is another.
    fn tui_command(&mut self, command: &str) -> Option<Option<Action>> {
        let (name, arg) = command
            .trim()
            .split_once(' ')
            .map_or((command.trim(), ""), |(n, a)| (n, a.trim()));
        Some(match name {
            "cd" if arg.is_empty() => Some(Action::FolderDialog),
            "cd" => {
                let home = dirs_home().map(PathBuf::from);
                let remote = self.connection_name().is_some();
                let path = match (arg.strip_prefix('~'), home) {
                    // A daemon elsewhere resolves `~` to its own home.
                    (Some(_), _) if remote => PathBuf::from(arg),
                    (Some(rest), Some(home)) => home.join(rest.trim_start_matches('/')),
                    _ => self.tab_cwd().join(arg),
                };
                Some(Action::OpenFolder(path))
            }
            "jobs" => Some(Action::Jobs),
            _ => return None,
        })
    }

    /// Prompts the active tab's agent, or queues the prompt while it works
    /// or while the tab is still being created.
    pub fn send(&mut self, prompt: Prompt) {
        let index = self.active;
        let wait = self.busy() || !self.tab().queued.is_empty() || self.tab().id.is_none();
        self.tab_mut().reset_view();
        if wait {
            self.tab_mut().queued.push_back(prompt);
            if self.tab().id.is_none() {
                self.create_tab_for(index);
            }
        } else {
            self.dispatch(index, prompt);
        }
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
            Slash::Clear => Action::NewAgent,
            Slash::Resume("") | Slash::Sessions => Action::Sessions,
            Slash::Resume(id) => Action::Resume(id.to_owned()),
            Slash::Cost => Action::ShowCost,
            Slash::Mcp => Action::ShowMcp,
            Slash::Memory => Action::ShowMemory,
            Slash::Web(_) => Action::ShowWeb,
            Slash::Forward(command) => {
                let attachments = std::mem::take(&mut self.attachments);
                self.send(Prompt {
                    text: format!("/{command}"),
                    attachments,
                });
                return None;
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

    pub(super) fn focus_transcript(&mut self) {
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

    /// Selects the previous or next turn's answer.
    fn jump_answer(&mut self, direction: isize) {
        let Some(view) = self.shown() else {
            return;
        };
        let answers = view.transcript.answers();
        let current = self.tab().selected.unwrap_or(usize::MAX);
        let next = if direction < 0 {
            answers
                .iter()
                .rev()
                .find(|&&i| i < current)
                .or(answers.first())
        } else {
            answers
                .iter()
                .find(|&&i| i > current && current != usize::MAX)
                .or(answers.last())
        };
        if let Some(&index) = next {
            self.tab_mut().selected = Some(index);
            self.reveal(index);
        }
    }

    /// Scrolls so an entry is on screen.
    pub(super) fn reveal(&mut self, index: usize) {
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
        let top = self.transcript_top() as isize + delta;
        self.set_top(top.max(0) as usize);
    }

    /// The transcript row at the top of its pane.
    pub(super) fn transcript_top(&self) -> usize {
        let total = self.shown().map_or(0, |v| v.transcript.height());
        let max_top = total.saturating_sub(self.geometry.transcript.height as usize);
        self.tab().scroll.map_or(max_top, |top| top.min(max_top))
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
            KeyCode::Char('y') | KeyCode::Char('c') => return selected.map(Action::CopyEntry),
            KeyCode::Char('b') => return selected.map(|i| Action::CopyCode(i, None)),
            KeyCode::Char('[') => self.jump_answer(-1),
            KeyCode::Char(']') => self.jump_answer(1),
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
}
