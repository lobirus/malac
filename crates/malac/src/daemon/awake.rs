//! Keeping the machine awake while agents or jobs run, like Amphetamine.
//!
//! On macOS the daemon runs `caffeinate -i -s -w <pid>` as a child: it
//! holds an assertion against idle sleep (and system sleep on AC power)
//! and exits by itself if the daemon dies. On Linux it runs
//! `systemd-inhibit --what=idle:sleep` around a shell that waits for the
//! daemon. Neither stops a MacBook from sleeping when its lid closes,
//! unless it runs in clamshell mode (power, an external display and a
//! keyboard or mouse).

use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use malac_core::config::KeepAwake;

use crate::protocol::AwakeStatus;

/// How long the hold outlasts the work, so back-to-back turns do not let
/// the machine doze off in between.
pub const GRACE: Duration = Duration::from_secs(60);

/// Starts and stops the platform's power assertion.
pub trait Runner: Send + Sync {
    /// The program used, or `None` where nothing is available.
    fn method(&self) -> Option<&'static str>;
    /// Takes the assertion; dropping the hold releases it.
    fn hold(&self) -> Result<Box<dyn Hold>, String>;
}

/// A held assertion.
pub trait Hold: Send {
    /// `Err` with the reason once the assertion went away by itself.
    fn check(&mut self) -> Result<(), String>;

    /// What it does not cover, when it is less than asked for.
    fn limits(&self) -> Option<&str> {
        None
    }
}

/// What is running in the daemon right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Work {
    pub agents: usize,
    pub jobs: usize,
}

impl Work {
    fn busy(self) -> bool {
        self.agents > 0 || self.jobs > 0
    }

    fn describe(self) -> String {
        let plural = |n: usize, one: &str| match n {
            1 => format!("1 {one}"),
            n => format!("{n} {one}s"),
        };
        match (self.agents, self.jobs) {
            (0, 0) => "nothing is running".to_owned(),
            (a, 0) => format!("{} running", plural(a, "agent")),
            (0, j) => format!("{} running", plural(j, "job")),
            (a, j) => format!("{} and {} running", plural(a, "agent"), plural(j, "job")),
        }
    }
}

/// The keep-awake state machine: given the mode and the work, it holds or
/// releases the assertion and says why.
pub struct Awake {
    mode: KeepAwake,
    runner: Arc<dyn Runner>,
    hold: Option<Box<dyn Hold>>,
    /// Why the last attempt failed; not retried until the work stops and starts again.
    failed: Option<String>,
    /// When the work stopped, while the grace period runs.
    idle_since: Option<Instant>,
    grace: Duration,
    status: AwakeStatus,
}

impl Awake {
    pub fn new(mode: KeepAwake, runner: Arc<dyn Runner>, grace: Duration) -> Self {
        let mut awake = Self {
            mode,
            runner,
            hold: None,
            failed: None,
            idle_since: None,
            grace,
            status: AwakeStatus {
                mode,
                ..Default::default()
            },
        };
        awake.update(Work::default(), Instant::now());
        awake
    }

    pub fn status(&self) -> &AwakeStatus {
        &self.status
    }

    pub fn set_mode(&mut self, mode: KeepAwake, work: Work, now: Instant) -> bool {
        self.mode = mode;
        self.failed = None;
        self.update(work, now)
    }

    /// Brings the hold in line with `work`; returns whether the status changed.
    pub fn update(&mut self, work: Work, now: Instant) -> bool {
        if let Some(hold) = &mut self.hold {
            if let Err(err) = hold.check() {
                self.hold = None;
                self.failed = Some(err);
            }
        }
        if !work.busy() && self.mode == KeepAwake::Busy && self.failed.is_some() {
            // Try again when work starts the next time.
            self.failed = None;
        }
        let wanted = match self.mode {
            KeepAwake::Off => false,
            KeepAwake::Always => true,
            KeepAwake::Busy if work.busy() => {
                self.idle_since = None;
                true
            }
            KeepAwake::Busy => {
                let since = *self.idle_since.get_or_insert(now);
                self.hold.is_some() && now.duration_since(since) < self.grace
            }
        };
        if !wanted {
            self.hold = None;
            if self.mode != KeepAwake::Busy || work.busy() {
                self.idle_since = None;
            }
        } else if self.hold.is_none() && self.failed.is_none() {
            match self.runner.hold() {
                Ok(hold) => self.hold = Some(hold),
                Err(err) => self.failed = Some(err),
            }
        }
        let reason = match (self.mode, &self.failed) {
            (KeepAwake::Off, _) => "keep-awake is off".to_owned(),
            (_, Some(_)) if self.runner.method().is_none() => {
                "not supported on this system".to_owned()
            }
            (_, Some(_)) => "could not keep the machine awake".to_owned(),
            (KeepAwake::Always, None) => "always, while the daemon runs".to_owned(),
            (KeepAwake::Busy, None) if work.busy() => {
                match self.hold.as_ref().and_then(|h| h.limits()) {
                    Some(limits) => format!("{} · {limits}", work.describe()),
                    None => work.describe(),
                }
            }
            (KeepAwake::Busy, None) if self.hold.is_some() => {
                "work just finished; releasing shortly".to_owned()
            }
            (KeepAwake::Busy, None) => "nothing is running".to_owned(),
        };
        let awake = self.hold.is_some();
        let next = AwakeStatus {
            mode: self.mode,
            awake,
            reason,
            method: self.runner.method().map(str::to_owned),
            error: self.failed.clone(),
            since: match awake == self.status.awake && self.status.since.is_some() {
                true => self.status.since,
                false => Some(Utc::now()),
            },
        };
        let changed = AwakeStatus {
            since: None,
            ..next.clone()
        } != AwakeStatus {
            since: None,
            ..self.status.clone()
        };
        self.status = next;
        changed
    }
}

/// `caffeinate` on macOS, `systemd-inhibit` on Linux, nothing elsewhere.
pub struct SystemRunner {
    method: Option<&'static str>,
}

impl SystemRunner {
    pub fn detect() -> Self {
        let method = if cfg!(target_os = "macos") {
            Some("caffeinate")
        } else if cfg!(target_os = "linux") && on_path("systemd-inhibit") {
            Some("systemd-inhibit")
        } else {
            None
        };
        Self { method }
    }
}

impl Runner for SystemRunner {
    fn method(&self) -> Option<&'static str> {
        self.method
    }

    fn hold(&self) -> Result<Box<dyn Hold>, String> {
        let pid = std::process::id().to_string();
        match self.method {
            Some("caffeinate") => {
                let mut command = Command::new("caffeinate");
                command.args(["-i", "-s", "-w", &pid]);
                start(command, "caffeinate", None)
            }
            Some(program) => {
                let inhibit = |what: &str| {
                    let mut command = Command::new(program);
                    command.args([
                        &format!("--what={what}"),
                        "--who=malac",
                        "--why=malac agents or jobs are running",
                        "--mode=block",
                        "sh",
                        "-c",
                        // Ends with the daemon, even if it is killed.
                        &format!("while kill -0 {pid} 2>/dev/null; do sleep 10; done"),
                    ]);
                    command
                };
                // logind lets only active local sessions block sleep; a
                // daemon started over SSH may still block idle actions.
                start(inhibit("idle:sleep"), program, None).or_else(|err| {
                    let limits = "idle only: blocking sleep needs an active local session";
                    start(inhibit("idle"), program, Some(limits)).map_err(|_| err)
                })
            }
            None => Err("no caffeinate or systemd-inhibit on this system".into()),
        }
    }
}

fn start(
    mut command: Command,
    program: &'static str,
    limits: Option<&'static str>,
) -> Result<Box<dyn Hold>, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let child = command
        .spawn()
        .map_err(|err| format!("cannot run {program}: {err}"))?;
    let mut hold = ChildHold {
        child,
        program,
        limits,
    };
    // A refusal (polkit, a missing logind) shows within moments.
    std::thread::sleep(Duration::from_millis(150));
    hold.check()?;
    Ok(Box::new(hold))
}

struct ChildHold {
    child: Child,
    program: &'static str,
    limits: Option<&'static str>,
}

impl Hold for ChildHold {
    fn limits(&self) -> Option<&str> {
        self.limits
    }

    fn check(&mut self) -> Result<(), String> {
        match self.child.try_wait() {
            Ok(None) => Ok(()),
            Ok(Some(status)) => {
                let mut error = String::new();
                if let Some(mut stderr) = self.child.stderr.take() {
                    use std::io::Read;
                    let _ = stderr.read_to_string(&mut error);
                }
                let error = error.trim();
                Err(match error.is_empty() {
                    true => format!("{} exited ({status})", self.program),
                    false => format!("{}: {error}", self.program),
                })
            }
            Err(err) => Err(format!("{}: {err}", self.program)),
        }
    }
}

impl Drop for ChildHold {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Records holds instead of running anything.
    #[derive(Default)]
    pub struct MockRunner {
        pub log: Arc<Mutex<Vec<&'static str>>>,
        pub refuse: AtomicBool,
        /// Makes the current hold report that it went away.
        pub lost: Arc<AtomicBool>,
    }

    struct MockHold {
        log: Arc<Mutex<Vec<&'static str>>>,
        lost: Arc<AtomicBool>,
    }

    impl Hold for MockHold {
        fn check(&mut self) -> Result<(), String> {
            match self.lost.swap(false, Ordering::SeqCst) {
                true => Err("mock: lost".into()),
                false => Ok(()),
            }
        }
    }

    impl Drop for MockHold {
        fn drop(&mut self) {
            self.log.lock().unwrap().push("release");
        }
    }

    impl Runner for MockRunner {
        fn method(&self) -> Option<&'static str> {
            Some("mock")
        }

        fn hold(&self) -> Result<Box<dyn Hold>, String> {
            if self.refuse.load(Ordering::SeqCst) {
                self.log.lock().unwrap().push("refused");
                return Err("mock: access denied".into());
            }
            self.log.lock().unwrap().push("hold");
            Ok(Box::new(MockHold {
                log: self.log.clone(),
                lost: self.lost.clone(),
            }))
        }
    }

    fn log(runner: &MockRunner) -> Vec<&'static str> {
        std::mem::take(&mut *runner.log.lock().unwrap())
    }

    const BUSY: Work = Work { agents: 2, jobs: 1 };
    const IDLE: Work = Work { agents: 0, jobs: 0 };

    #[test]
    fn holds_while_busy_and_releases_after_the_grace() {
        let runner = Arc::new(MockRunner::default());
        let start = Instant::now();
        let mut awake = Awake::new(KeepAwake::Busy, runner.clone(), Duration::from_secs(60));
        assert!(!awake.status().awake);
        assert_eq!(awake.status().reason, "nothing is running");
        assert!(log(&runner).is_empty());

        assert!(awake.update(BUSY, start));
        assert!(awake.status().awake);
        assert_eq!(awake.status().reason, "2 agents and 1 job running");
        assert_eq!(awake.status().method.as_deref(), Some("mock"));
        assert!(!awake.update(BUSY, start + Duration::from_secs(5)));
        assert_eq!(log(&runner), ["hold"], "held once");

        let done = start + Duration::from_secs(10);
        assert!(awake.update(IDLE, done));
        assert!(awake.status().awake, "the grace period keeps it");
        assert!(!awake.update(IDLE, done + Duration::from_secs(30)));
        assert!(awake.update(IDLE, done + Duration::from_secs(61)));
        assert!(!awake.status().awake);
        assert_eq!(log(&runner), ["release"]);

        // Work that comes back within the grace keeps the same hold.
        awake.update(Work { agents: 0, jobs: 1 }, done + Duration::from_secs(70));
        awake.update(IDLE, done + Duration::from_secs(80));
        awake.update(BUSY, done + Duration::from_secs(90));
        assert_eq!(log(&runner), ["hold"]);
    }

    #[test]
    fn modes_change_at_runtime() {
        let runner = Arc::new(MockRunner::default());
        let now = Instant::now();
        let mut awake = Awake::new(KeepAwake::Off, runner.clone(), Duration::ZERO);
        assert!(!awake.update(BUSY, now), "off ignores work");
        assert_eq!(awake.status().reason, "keep-awake is off");
        assert!(awake.set_mode(KeepAwake::Always, IDLE, now));
        assert!(awake.status().awake);
        assert_eq!(awake.status().reason, "always, while the daemon runs");
        assert!(awake.set_mode(KeepAwake::Busy, IDLE, now));
        assert!(!awake.status().awake);
        assert!(awake.set_mode(KeepAwake::Off, BUSY, now));
        assert_eq!(log(&runner), ["hold", "release"]);
    }

    #[test]
    fn failures_are_reported_and_retried_on_the_next_work() {
        let runner = Arc::new(MockRunner::default());
        runner.refuse.store(true, Ordering::SeqCst);
        let now = Instant::now();
        let mut awake = Awake::new(KeepAwake::Busy, runner.clone(), Duration::ZERO);
        assert!(awake.update(BUSY, now));
        assert!(!awake.status().awake);
        assert_eq!(awake.status().error.as_deref(), Some("mock: access denied"));
        assert_eq!(awake.status().reason, "could not keep the machine awake");
        awake.update(BUSY, now);
        assert_eq!(
            log(&runner),
            ["refused"],
            "not retried while the work goes on"
        );

        runner.refuse.store(false, Ordering::SeqCst);
        awake.update(IDLE, now);
        assert!(awake.status().error.is_none());
        awake.update(BUSY, now);
        assert!(awake.status().awake);
        assert_eq!(log(&runner), ["hold"]);

        // The assertion process dies: reported, then retried next time.
        runner.lost.store(true, Ordering::SeqCst);
        assert!(awake.update(BUSY, now));
        assert!(!awake.status().awake);
        assert_eq!(awake.status().error.as_deref(), Some("mock: lost"));
        assert_eq!(log(&runner), ["release"]);
    }

    #[test]
    fn a_partial_hold_says_what_it_misses() {
        struct Partial;
        impl Hold for Partial {
            fn check(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn limits(&self) -> Option<&str> {
                Some("idle only")
            }
        }
        struct IdleOnly;
        impl Runner for IdleOnly {
            fn method(&self) -> Option<&'static str> {
                Some("systemd-inhibit")
            }
            fn hold(&self) -> Result<Box<dyn Hold>, String> {
                Ok(Box::new(Partial))
            }
        }
        let mut awake = Awake::new(KeepAwake::Busy, Arc::new(IdleOnly), Duration::ZERO);
        awake.update(Work { agents: 1, jobs: 0 }, Instant::now());
        assert!(awake.status().awake);
        assert_eq!(awake.status().reason, "1 agent running · idle only");
    }

    #[test]
    fn describes_the_work() {
        assert_eq!(Work { agents: 1, jobs: 0 }.describe(), "1 agent running");
        assert_eq!(Work { agents: 0, jobs: 3 }.describe(), "3 jobs running");
    }
}
