//! The per-user daemon: one long-lived process that holds every workspace,
//! tab runtime and agent, so agents outlive the terminal and several clients
//! (terminals, browsers, the editor) share them.
//!
//! It serves the web UI and the protocol in `docs/protocol.md`, writes
//! `~/.malac/daemon.json` so local clients can find it, holds
//! `~/.malac/daemon.lock` while it runs, and logs to `~/.malac/daemon.log`
//! when started in the background.

pub mod access;
pub mod awake;
pub mod hub;
pub mod push;
pub mod restart;
pub mod store;
pub mod talk;
pub mod tunnel;
pub(crate) mod uploads;
pub mod voice;

use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::style::Paint;
use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Args, Subcommand};
use malac_core::config::{self, Config};
use malac_core::update;
use serde_json::json;

use crate::client::{self, DaemonFile};
use crate::protocol::DaemonInfo;
use crate::web::Server;
pub use hub::{Hub, HubOptions};

/// How often the idle timer looks.
const IDLE_CHECK: Duration = Duration::from_secs(20);
/// How long `stop` waits for the process to exit.
const STOP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Subcommand, Debug, Clone)]
pub enum DaemonAction {
    /// Run in the foreground.
    Run(ServeArgs),
    /// Start in the background unless one is running (the default).
    Start(ServeArgs),
    /// Stop the running daemon. Refused while agents or jobs run, unless --force.
    Stop {
        #[arg(long)]
        force: bool,
    },
    /// Show whether it runs, where, and what it is doing.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Stop and start again, e.g. to use an updated malac. It keeps the
    /// address, remote access and a running tunnel; flags override them,
    /// and `stop` then `start` applies the configuration afresh. Refused
    /// while agents or jobs run, unless --force.
    Restart {
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        serve: ServeArgs,
    },
}

/// How a daemon listens. Defaults come from `[web]`.
#[derive(Args, Debug, Clone, Default)]
pub struct ServeArgs {
    /// Port (default `web.port`, or any free port if it is taken); 0 picks one.
    #[arg(long)]
    pub port: Option<u16>,
    /// Bind address (default `web.host`, else 127.0.0.1, or 0.0.0.0 with --remote).
    #[arg(long)]
    pub host: Option<String>,
    /// Allow paired devices on other machines, directly or through a tunnel.
    /// They need a device token from `malac remote pair`.
    #[arg(long)]
    pub remote: bool,
    /// Use this token instead of the saved one.
    #[arg(long, env = "MALAC_WEB_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
}

pub fn log_path() -> PathBuf {
    config::home_dir().join("daemon.log")
}

fn token_path() -> PathBuf {
    config::home_dir().join("daemon.token")
}

/// A started daemon: listening, its hub loaded, not yet serving.
pub struct Daemon {
    hub: Arc<Hub>,
    server: Server,
    file: DaemonFile,
    /// Held while running when the daemon is published in `~/.malac`.
    lock: Option<File>,
}

impl Daemon {
    /// Binds the server and restores saved workspaces and jobs. With
    /// `publish`, takes the daemon lock and writes `daemon.json`; an
    /// in-process daemon for tests can skip both.
    pub async fn start(args: &ServeArgs, publish: bool) -> Result<Self> {
        let lock = publish.then(acquire_lock).transpose()?;
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let base = Config::load_user()?;
        // After a restart the daemon keeps its address, remote access and
        // tunnel; flags given now still win.
        let restart = publish.then(restart::Restart::take).flatten();
        let remote = args.remote || base.web.remote || restart.as_ref().is_some_and(|r| r.remote);
        let host = match (&args.host, &restart, &base.web.host) {
            (Some(host), _, _) => host.clone(),
            (None, Some(restart), _) => restart.listen.ip().to_string(),
            (None, None, Some(host)) => host.clone(),
            (None, None, None) if remote => "0.0.0.0".into(),
            (None, None, None) => "127.0.0.1".into(),
        };
        let port = args
            .port
            .or(restart.as_ref().map(|r| r.listen.port()))
            .unwrap_or(base.web.port);
        let token = match &args.token {
            Some(token) => token.clone(),
            None => saved_token()?,
        };
        let hub = Hub::daemon(HubOptions {
            remote,
            idle_minutes: base.daemon.idle_minutes,
            browse_root: match &base.daemon.browse_root {
                Some(root) => malac_core::attach::resolve(&root.to_string_lossy(), &home),
                None => home,
            },
            keep_awake: base.daemon.keep_awake,
            public_url: base.remote.public_url.clone(),
        })?;
        let server = Server::bind(hub.clone(), &host, port, true, token.clone()).await?;
        hub.set_listen(server.addr(), remote.then(|| server.lan_url()).flatten());
        if !remote && !server.addr().ip().is_loopback() {
            tracing::warn!(
                "listening on {} without remote access: other machines are refused (use --remote)",
                server.addr()
            );
        }
        let tunnel = base
            .remote
            .tunnel
            .or(restart.as_ref().and_then(|r| r.tunnel));
        if let Some(provider) = tunnel {
            match remote {
                true => {
                    if let Err(err) = hub.start_tunnel(provider).await {
                        tracing::warn!("remote.tunnel: {err:#}");
                    }
                }
                false => tracing::warn!("remote.tunnel is set, but remote access is off"),
            }
        }
        let file = DaemonFile {
            url: server.base_url(),
            token,
            pid: std::process::id(),
            version: update::CURRENT.into(),
            urls: server.urls(),
            started: Some(Utc::now()),
            connection: None,
        };
        if publish {
            file.write()?;
        }
        Ok(Self {
            hub,
            server,
            file,
            lock,
        })
    }

    pub fn hub(&self) -> &Arc<Hub> {
        &self.hub
    }

    pub fn file(&self) -> &DaemonFile {
        &self.file
    }

    /// Serves until asked to stop (by a client, a signal or the idle timer),
    /// then cancels the agents and closes MCP servers.
    pub async fn run(self) -> Result<()> {
        let hub = self.hub.clone();
        let idle = tokio::spawn(idle_timer(hub.clone()));
        let result = tokio::select! {
            result = self.server.serve() => result,
            reason = hub.stopped() => {
                tracing::info!("stopping: {reason}");
                Ok(())
            }
            _ = shutdown_signal() => {
                hub.request_stop("signal");
                Ok(())
            }
        };
        idle.abort();
        hub.close().await;
        if self.lock.is_some() {
            DaemonFile::remove_if_ours(self.file.pid);
        }
        drop(self.lock);
        result
    }
}

/// Stops the daemon after `daemon.idle_minutes` without clients, running
/// agents, jobs or pending questions.
async fn idle_timer(hub: Arc<Hub>) {
    let limit = hub.options().idle_minutes;
    if limit == 0 {
        return;
    }
    let limit = Duration::from_secs(limit * 60);
    let mut idle_since: Option<Instant> = None;
    loop {
        tokio::time::sleep(IDLE_CHECK).await;
        if !hub.is_idle() {
            idle_since = None;
            continue;
        }
        let since = *idle_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= limit {
            hub.request_stop("idle");
            return;
        }
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut terminate) = signal(SignalKind::terminate()) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

/// The token clients use, kept across restarts so open pages reconnect.
/// Delete `~/.malac/daemon.token` to rotate it.
fn saved_token() -> Result<String> {
    let path = token_path();
    if let Ok(token) = std::fs::read_to_string(&path) {
        let token = token.trim();
        if token.len() >= 32 {
            return Ok(token.to_owned());
        }
    }
    let token = crate::web::random_token();
    config::write_private(&path, &token)?;
    Ok(token)
}

/// Takes `~/.malac/daemon.lock`, so two daemons never serve one home.
fn acquire_lock() -> Result<File> {
    let path = config::home_dir().join("daemon.lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: plain syscall on a descriptor this function owns.
        let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if !locked {
            let running = DaemonFile::read()
                .map(|f| format!(" (pid {}, {})", f.pid, f.url))
                .unwrap_or_default();
            bail!("a malac daemon is already running{running}");
        }
    }
    Ok(file)
}

/// Starts `malac daemon run` in its own session, detached from the
/// terminal, with output going to `~/.malac/daemon.log`.
pub fn spawn_detached(args: &ServeArgs) -> Result<u32> {
    let exe = std::env::current_exe().context("cannot find the malac executable")?;
    let log_path = log_path();
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("cannot open {}", log_path.display()))?;
    let mut command = std::process::Command::new(exe);
    command.args(["daemon", "run"]);
    if let Some(port) = args.port {
        command.arg("--port").arg(port.to_string());
    }
    if let Some(host) = &args.host {
        command.arg("--host").arg(host);
    }
    if args.remote {
        command.arg("--remote");
    }
    // Passed in the environment so it does not show up in `ps`.
    match &args.token {
        Some(token) => command.env("MALAC_WEB_TOKEN", token),
        None => command.env_remove("MALAC_WEB_TOKEN"),
    };
    command
        .current_dir(dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")))
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and the closure allocates nothing.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn().context("cannot start the malac daemon")?;
    let pid = child.id();
    // Reaps it if it exits while this process still runs.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

#[cfg(unix)]
pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
pub fn pid_alive(_pid: u32) -> bool {
    true
}

/// `malac daemon [run|start|stop|status|restart]`.
pub async fn command(action: Option<DaemonAction>) -> Result<i32> {
    match action.unwrap_or(DaemonAction::Start(ServeArgs::default())) {
        DaemonAction::Run(args) => {
            let daemon = Daemon::start(&args, true).await?;
            eprintln!(
                "malac daemon {} (pid {}) listening on {}",
                update::CURRENT,
                daemon.file.pid,
                daemon.file.url
            );
            daemon.run().await?;
            Ok(0)
        }
        DaemonAction::Start(args) => {
            if let Some(file) = client::running().await {
                println!(
                    "malac daemon already running (pid {}) at {}",
                    file.pid, file.url
                );
                return Ok(0);
            }
            let file = client::ensure_daemon(&args).await?;
            println!("malac daemon started (pid {}) at {}", file.pid, file.url);
            Ok(0)
        }
        DaemonAction::Stop { force } => match client::running().await {
            None => {
                println!("malac daemon is not running");
                Ok(0)
            }
            Some(file) => {
                stop(&file, force, false).await?;
                println!("malac daemon stopped");
                Ok(0)
            }
        },
        DaemonAction::Status { json } => status(json).await,
        DaemonAction::Restart { force, serve } => {
            if let Some(file) = client::running().await {
                stop(&file, force, true).await?;
            }
            let file = client::ensure_daemon(&serve).await?;
            println!(
                "malac daemon {} running (pid {}) at {}",
                file.version, file.pid, file.url
            );
            Ok(0)
        }
    }
}

async fn status(json: bool) -> Result<i32> {
    let Some(file) = client::running().await else {
        if json {
            println!("{}", json!({ "running": false }));
        } else {
            println!("malac daemon is not running");
        }
        return Ok(1);
    };
    let info: DaemonInfo = client::get(&file, "/api/daemon").await?;
    if json {
        let mut value = serde_json::to_value(&info)?;
        value["running"] = true.into();
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(0);
    }
    let status = &info.status;
    println!(
        "{} {} (pid {}) since {}",
        "malac daemon".bold(),
        status.version,
        status.pid,
        status
            .started
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
    );
    println!("  url        {}", file.web_url());
    let remote = &info.remote_access;
    match remote.enabled {
        true => {
            let others: Vec<String> = remote
                .urls
                .iter()
                .filter(|u| u.kind != crate::protocol::UrlKind::Local)
                .map(|u| format!("{} ({})", u.url, u.kind.label()))
                .collect();
            println!(
                "  remote     allowed · {} paired devices{}",
                remote.devices,
                match others.is_empty() {
                    true => String::new(),
                    false => format!(" · {}", others.join(", ")),
                }
            );
        }
        false => println!(
            "  remote     off (this machine and SSH tunnels only) · {} paired devices",
            remote.devices
        ),
    }
    if let Some(tunnel) = &remote.tunnel {
        println!(
            "  tunnel     {} {:?}{}",
            tunnel.provider.as_str(),
            tunnel.state,
            tunnel
                .url
                .as_deref()
                .map(|u| format!(" {u}"))
                .unwrap_or_default()
        );
    }
    let awake = &info.awake;
    println!(
        "  awake      {} · {} (keep_awake = {})",
        if awake.awake {
            "holding"
        } else {
            "not holding"
        },
        awake.error.as_deref().unwrap_or(&awake.reason),
        awake.mode.as_str(),
    );
    println!("  clients    {}", status.clients);
    println!(
        "  workspaces {} · {} tabs loaded",
        info.workspaces, info.tabs_loaded
    );
    println!(
        "  running    {} agents · {} jobs · {} waiting for approval",
        info.agents_running, info.jobs_running, info.pending_approvals
    );
    let idle = match info.idle_minutes {
        0 => "never".to_owned(),
        1 => "after a minute without clients or work".to_owned(),
        n => format!("after {n} minutes without clients or work"),
    };
    println!("  exits      {idle}");
    println!("  log        {}", log_path().display());
    if status.version != update::CURRENT {
        println!(
            "{}",
            format!(
                "! this malac is {}; run `malac daemon restart` to switch the daemon to it",
                update::CURRENT
            )
            .yellow()
        );
    }
    Ok(0)
}

/// Asks the daemon to stop and waits for its process to exit.
/// Stops the daemon; with `restart`, it leaves how it runs for the next one.
async fn stop(file: &DaemonFile, force: bool, restart: bool) -> Result<()> {
    let info: DaemonInfo = client::get(file, "/api/daemon").await?;
    if info.busy() && !force {
        bail!(
            "the daemon has {} running agents and {} running jobs; stopping would interrupt them \
             (use --force)",
            info.agents_running,
            info.jobs_running
        );
    }
    let body = json!({ "force": force, "restart": restart });
    let _: serde_json::Value = client::post(file, "/api/daemon/stop", &body)
        .await
        .context("cannot ask the daemon to stop")?;
    let deadline = Instant::now() + STOP_TIMEOUT;
    while pid_alive(file.pid) {
        if Instant::now() > deadline {
            bail!("the daemon (pid {}) did not exit in time", file.pid);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}
