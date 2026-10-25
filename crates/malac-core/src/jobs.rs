//! Background processes started by agents ("jobs").
//!
//! A job is a shell command running in its own process group with stdout and
//! stderr going to a log file. Its record is kept in `~/.malac/jobs/<id>.json`
//! next to the log, so a job outlives the process that started it and a later
//! process (the daemon after a restart) can still list it, read its output
//! and kill it.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::config;
use crate::event::AgentId;

/// How long a killed job gets to exit after SIGTERM before SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(3);
/// How often jobs inherited from an earlier process are checked.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Finished jobs older than this are forgotten, log included.
const RETENTION: chrono::TimeDelta = chrono::TimeDelta::days(7);
/// Bytes read from the end of a log for a tail.
const TAIL_BYTES: u64 = 256 * 1024;
pub const DEFAULT_TAIL_LINES: usize = 100;
pub const MAX_TAIL_LINES: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    /// Exited on its own; `exit_code` is set.
    Exited,
    /// Stopped by `job_kill` or by a signal.
    Killed,
    /// Ended while no malac process was watching, so the exit code is lost.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobInfo {
    pub id: String,
    pub command: String,
    pub cwd: PathBuf,
    pub pid: Option<u32>,
    pub log: PathBuf,
    pub started: DateTime<Utc>,
    pub finished: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    pub status: JobStatus,
    /// The agent that started it.
    pub agent: Option<AgentId>,
}

impl JobInfo {
    pub fn is_running(&self) -> bool {
        self.status == JobStatus::Running
    }

    /// `running`, `exited 0`, `killed`, ...
    pub fn describe_status(&self) -> String {
        match (self.status, self.exit_code) {
            (JobStatus::Exited, Some(code)) => format!("exited {code}"),
            (JobStatus::Running, _) => "running".into(),
            (JobStatus::Exited, None) => "exited".into(),
            (JobStatus::Killed, _) => "killed".into(),
            (JobStatus::Unknown, _) => "finished (exit status unknown)".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    Started { job: JobInfo },
    Finished { job: JobInfo },
}

impl JobEvent {
    pub fn job(&self) -> &JobInfo {
        match self {
            JobEvent::Started { job } | JobEvent::Finished { job } => job,
        }
    }
}

/// The last lines of a job's log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOutput {
    pub job: JobInfo,
    pub text: String,
    /// Size of the whole log in bytes.
    pub size: u64,
    /// Whether `text` starts after the beginning of the log.
    pub truncated: bool,
}

/// The job table. Cheap to share; every runtime of a process can use one.
pub struct Jobs {
    dir: PathBuf,
    table: Mutex<BTreeMap<String, Entry>>,
    events: broadcast::Sender<JobEvent>,
}

struct Entry {
    info: JobInfo,
    kill_requested: bool,
}

impl Jobs {
    pub fn new(dir: PathBuf) -> Arc<Self> {
        let (events, _) = broadcast::channel(256);
        Arc::new(Self {
            dir,
            table: Mutex::new(BTreeMap::new()),
            events,
        })
    }

    /// The table stored in `~/.malac/jobs`.
    pub fn in_home() -> Arc<Self> {
        Self::new(config::home_dir().join("jobs"))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent> {
        self.events.subscribe()
    }

    /// Newest first.
    pub fn list(&self) -> Vec<JobInfo> {
        let mut jobs: Vec<JobInfo> = self
            .table
            .lock()
            .unwrap()
            .values()
            .map(|e| e.info.clone())
            .collect();
        jobs.sort_by(|a, b| b.started.cmp(&a.started));
        jobs
    }

    pub fn get(&self, id: &str) -> Option<JobInfo> {
        self.table.lock().unwrap().get(id).map(|e| e.info.clone())
    }

    pub fn running(&self) -> usize {
        self.table
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.info.is_running())
            .count()
    }

    /// Starts `command` with a shell in `cwd`. Must be called inside a tokio runtime.
    pub fn start(
        self: &Arc<Self>,
        command: &str,
        cwd: &Path,
        agent: Option<&str>,
    ) -> Result<JobInfo> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("cannot create {}", self.dir.display()))?;
        let id = self.new_id();
        let log_path = self.dir.join(format!("{id}.log"));
        let log = fs::File::create(&log_path)
            .with_context(|| format!("cannot create {}", log_path.display()))?;
        let mut child = crate::tools::shell_command(command, cwd)
            .stdout(log.try_clone()?)
            .stderr(log)
            .stdin(Stdio::null())
            .spawn()
            .context("failed to start shell")?;
        let info = JobInfo {
            id: id.clone(),
            command: command.to_owned(),
            cwd: cwd.to_path_buf(),
            pid: child.id(),
            log: log_path,
            started: Utc::now(),
            finished: None,
            exit_code: None,
            status: JobStatus::Running,
            agent: agent.map(str::to_owned),
        };
        self.insert(info.clone());
        let _ = self.events.send(JobEvent::Started { job: info.clone() });

        let jobs = self.clone();
        tokio::spawn(async move {
            let status = child.wait().await.ok();
            jobs.finish(&id, |entry| {
                let code = status.and_then(|s| s.code());
                entry.info.exit_code = code;
                entry.info.status = match code {
                    Some(_) if !entry.kill_requested => JobStatus::Exited,
                    None if status.is_none() => JobStatus::Unknown,
                    _ => JobStatus::Killed,
                };
            });
        });
        Ok(info)
    }

    /// Stops a job's whole process group: SIGTERM, then SIGKILL if it is
    /// still running after a grace period.
    pub fn kill(self: &Arc<Self>, id: &str) -> Result<JobInfo> {
        let info = {
            let mut table = self.table.lock().unwrap();
            let entry = table.get_mut(id).with_context(|| format!("no job {id}"))?;
            if !entry.info.is_running() {
                bail!("job {id} is not running ({})", entry.info.describe_status());
            }
            entry.kill_requested = true;
            entry.info.clone()
        };
        let pid = info.pid.context("the job has no process id")?;
        signal_group(pid, Signal::Terminate);
        let jobs = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(KILL_GRACE).await;
            if jobs.get(&id).is_some_and(|j| j.is_running()) {
                signal_group(pid, Signal::Kill);
            }
        });
        Ok(info)
    }

    /// The last `lines` lines of a job's log.
    pub fn output(&self, id: &str, lines: usize) -> Result<JobOutput> {
        let job = self.get(id).with_context(|| format!("no job {id}"))?;
        let (text, size, truncated) = tail(&job.log, lines.clamp(1, MAX_TAIL_LINES))?;
        Ok(JobOutput {
            job,
            text,
            size,
            truncated,
        })
    }

    /// Loads the records left by earlier processes. Jobs whose process is
    /// gone are marked finished with an unknown status; live ones are
    /// watched until they end. Old finished jobs are deleted.
    pub fn restore(self: &Arc<Self>) -> Result<usize> {
        let Ok(dir) = fs::read_dir(&self.dir) else {
            return Ok(0);
        };
        let mut restored = 0;
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Some(mut info) = fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<JobInfo>(&text).ok())
            else {
                continue;
            };
            if self.table.lock().unwrap().contains_key(&info.id) {
                continue;
            }
            if info.finished.is_some_and(|at| Utc::now() - at > RETENTION) {
                let _ = fs::remove_file(&info.log);
                let _ = fs::remove_file(&path);
                continue;
            }
            let alive = info.is_running() && info.pid.is_some_and(group_alive);
            if info.is_running() && !alive {
                info.status = JobStatus::Unknown;
                info.finished = Some(Utc::now());
            }
            let id = info.id.clone();
            let pid = info.pid;
            self.insert(info);
            restored += 1;
            if let (true, Some(pid)) = (alive, pid) {
                self.watch(id, pid);
            }
        }
        Ok(restored)
    }

    /// Polls a job this process did not start until its process group is gone.
    fn watch(self: &Arc<Self>, id: String, pid: u32) {
        let jobs = self.clone();
        tokio::spawn(async move {
            while group_alive(pid) {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            jobs.finish(&id, |entry| {
                entry.info.status = if entry.kill_requested {
                    JobStatus::Killed
                } else {
                    JobStatus::Unknown
                };
            });
        });
    }

    fn new_id(&self) -> String {
        let table = self.table.lock().unwrap();
        loop {
            let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
            if !table.contains_key(&id) && !self.dir.join(format!("{id}.json")).exists() {
                return id;
            }
        }
    }

    fn insert(&self, info: JobInfo) {
        self.save(&info);
        self.table.lock().unwrap().insert(
            info.id.clone(),
            Entry {
                info,
                kill_requested: false,
            },
        );
    }

    fn finish(&self, id: &str, update: impl FnOnce(&mut Entry)) {
        let info = {
            let mut table = self.table.lock().unwrap();
            let Some(entry) = table.get_mut(id) else {
                return;
            };
            update(entry);
            entry.info.finished = Some(Utc::now());
            entry.info.clone()
        };
        self.save(&info);
        let _ = self.events.send(JobEvent::Finished { job: info });
    }

    /// Persistence failures must not fail the job, so they are only logged.
    fn save(&self, info: &JobInfo) {
        let path = self.dir.join(format!("{}.json", info.id));
        let tmp = path.with_extension("json.tmp");
        let result = serde_json::to_vec_pretty(info)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| Ok(fs::write(&tmp, bytes)?))
            .and_then(|()| Ok(fs::rename(&tmp, &path)?));
        if let Err(err) = result {
            tracing::warn!("cannot save job {}: {err:#}", info.id);
        }
    }
}

/// The last `lines` lines of a file, its size and whether anything before them was left out.
fn tail(path: &Path, lines: usize) -> Result<(String, u64, bool)> {
    let mut file =
        fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    // A partial first line is dropped unless it is all there is.
    let text = match (start > 0, text.find('\n')) {
        (true, Some(newline)) => &text[newline + 1..],
        _ => &text[..],
    };
    let all: Vec<&str> = text.lines().collect();
    let skip = all.len().saturating_sub(lines);
    Ok((all[skip..].join("\n"), size, start > 0 || skip > 0))
}

enum Signal {
    Terminate,
    Kill,
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: Signal) {
    let signal = match signal {
        Signal::Terminate => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    // SAFETY: plain syscall; the job leads its own process group.
    unsafe {
        libc::killpg(pid as libc::pid_t, signal);
    }
}

#[cfg(not(unix))]
fn signal_group(_pid: u32, _signal: Signal) {}

/// Whether a process group led by `pid` still exists. Jobs lead their own
/// group, which also makes a reused pid unlikely to be mistaken for the job.
#[cfg(unix)]
fn group_alive(pid: u32) -> bool {
    let pid = pid as libc::pid_t;
    // SAFETY: plain syscalls that only query process state.
    unsafe { libc::getpgid(pid) == pid && (libc::kill(pid, 0) == 0 || errno_is_eperm()) }
}

#[cfg(unix)]
fn errno_is_eperm() -> bool {
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn group_alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tails_the_end_of_a_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let text: String = (1..=500).map(|i| format!("line {i}\n")).collect();
        fs::write(&path, &text).unwrap();
        let (end, size, truncated) = tail(&path, 3).unwrap();
        assert_eq!(end, "line 498\nline 499\nline 500");
        assert_eq!(size, text.len() as u64);
        assert!(truncated);
        let (all, _, truncated) = tail(&path, 1000).unwrap();
        assert!(all.starts_with("line 1\n"));
        assert!(!truncated);
    }

    #[test]
    fn describes_statuses() {
        let mut job = JobInfo {
            id: "j".into(),
            command: "true".into(),
            cwd: PathBuf::from("/"),
            pid: None,
            log: PathBuf::from("/dev/null"),
            started: Utc::now(),
            finished: None,
            exit_code: None,
            status: JobStatus::Running,
            agent: None,
        };
        assert_eq!(job.describe_status(), "running");
        job.status = JobStatus::Exited;
        job.exit_code = Some(2);
        assert_eq!(job.describe_status(), "exited 2");
    }
}
