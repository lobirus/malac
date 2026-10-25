//! What the new views ask of the agents, gathered in one place so the
//! daemon's client API can take the runtime's place: the agent tree for the
//! map, opening a folder, background jobs, the `@` file index and turning a
//! prompt into content.

use super::*;

impl App {
    /// The working directory of the active tab.
    pub fn tab_cwd(&self) -> PathBuf {
        self.tab().handle.info().cwd.clone()
    }

    /// Every session's agents, depth first, as the map shows them. Pinned
    /// subagents appear under their session unless it has no tab.
    pub fn agent_nodes(&self) -> Vec<Node> {
        let mut nodes: Vec<Node> = Vec::new();
        let sessions = self.tabs.iter().enumerate().filter(|(_, t)| !t.read_only());
        let pinned = self.tabs.iter().enumerate().filter(|(_, t)| t.read_only());
        for (index, tab) in sessions.chain(pinned) {
            if nodes.iter().any(|n| n.id == tab.root()) {
                continue;
            }
            for (_, view) in self.agents.tree(tab.root()) {
                let snapshot = self.snapshots.get(&view.id);
                let waiting = self.approvals.iter().any(|a| a.request.agent == view.id);
                let state = match snapshot.map(|s| s.status) {
                    _ if waiting => State::Waiting,
                    Some(AgentStatus::Running) => State::Running,
                    Some(AgentStatus::Completed) => State::Done,
                    Some(AgentStatus::Failed) => State::Failed,
                    Some(AgentStatus::Cancelled) => State::Cancelled,
                    Some(AgentStatus::Idle) | None => State::Idle,
                };
                let activity = match (state, snapshot) {
                    (State::Running | State::Waiting, Some(s)) => s.activity.clone(),
                    (_, Some(s)) => s.report.clone().unwrap_or_default(),
                    (_, None) => String::new(),
                };
                let usage = view.total;
                nodes.push(Node {
                    id: view.id.clone(),
                    parent: view.parent.clone(),
                    label: view.label.clone(),
                    model: view.model.clone(),
                    state,
                    activity,
                    tokens: usage.input_tokens
                        + usage.cache_read_tokens
                        + usage.cache_write_tokens
                        + usage.output_tokens,
                    cost: usage.cost,
                    depth: snapshot.map_or(0, |s| s.info.depth),
                    tab: (view.id == tab.root()).then_some(index),
                });
            }
        }
        nodes
    }

    /// Works in `dir`: today a new tab with an agent there.
    pub fn open_folder(&mut self, dir: &Path) {
        if !dir.is_dir() {
            self.flash(format!("{} is not a folder", short_path(dir)));
            return;
        }
        let spec = AgentSpec {
            cwd: Some(dir.to_owned()),
            ..Default::default()
        };
        match self.session.runtime.spawn_agent(spec) {
            Ok(handle) => {
                let index = self.open_tab(handle);
                self.switch_tab(index);
                self.flash_ok(format!("working in {}", short_path(dir)));
            }
            Err(err) => self.notice(Level::Error, format!("{err:#}")),
        }
    }

    /// Folders sessions were started in, newest first.
    pub fn recent_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        for summary in session::list(false, 200).unwrap_or_default() {
            let dir = summary.meta.cwd;
            if !dirs.contains(&dir) && dir.is_dir() {
                dirs.push(dir);
            }
            if dirs.len() == 8 {
                break;
            }
        }
        dirs
    }

    /// Notes the background job a finished `bash` call started.
    pub(super) fn track_job(&mut self, agent: &str, call_id: &str) {
        let Some(view) = self.agents.get(agent) else {
            return;
        };
        let tool = view
            .transcript
            .items()
            .iter()
            .rev()
            .find_map(|i| match &i.entry {
                Entry::Tool(tool) if tool.call_id == call_id => Some(tool),
                _ => None,
            });
        let cwd = self
            .snapshots
            .get(agent)
            .map_or_else(|| self.tab_cwd(), |s| s.info.cwd.clone());
        let job = tool.and_then(|t| Job::from_tool(call_id, agent, &cwd, &t.input, &t.output));
        if let Some(job) = job {
            if !self.jobs.iter().any(|j| j.id == job.id) {
                self.jobs.push(job);
            }
        }
    }

    /// Marks jobs whose process is gone; returns whether any changed.
    pub(super) fn poll_jobs(&mut self) -> bool {
        let mut changed = false;
        for job in self.jobs.iter_mut().filter(|j| j.is_running()) {
            if !job.pid.is_some_and(jobs::alive) {
                job.status = JobStatus::Exited;
                changed = true;
            }
        }
        changed
    }

    pub fn running_jobs(&self) -> usize {
        self.jobs.iter().filter(|j| j.is_running()).count()
    }

    pub(super) fn stop_job(&mut self, id: &str) {
        let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) else {
            return;
        };
        let Some(pid) = job.pid.filter(|_| job.is_running()) else {
            return;
        };
        match jobs::stop(pid) {
            Ok(()) => {
                job.status = JobStatus::Stopped;
                self.flash_ok(format!("stopped job {pid}"));
            }
            Err(err) => self.flash(format!("could not stop {pid}: {err}")),
        }
    }

    /// Indexes the tab's folder for the `@` picker in the background.
    pub(super) fn index_files(&mut self) {
        let root = self.tab_cwd();
        if self.files.root == root && (self.files.loading || self.files.list.is_some()) {
            return;
        }
        self.files = FileIndex {
            root: root.clone(),
            list: None,
            loading: true,
        };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let list = files::index(&root);
            let _ = tx.send(Update::Files { root, list });
        });
    }

    /// Saves the clipboard's image off the interface thread and attaches it.
    pub(super) fn paste_image(&mut self) {
        let tx = self.tx.clone();
        let dir = std::env::temp_dir().join("malac");
        tokio::task::spawn_blocking(move || {
            let _ = tx.send(Update::Pasted(attach::clipboard_image(&dir)));
        });
    }

    /// What a prompt sends to its agent.
    pub(super) fn prompt_blocks(prompt: &Prompt) -> Vec<Block> {
        attach::to_blocks(&prompt.text, &prompt.attachments)
    }
}
