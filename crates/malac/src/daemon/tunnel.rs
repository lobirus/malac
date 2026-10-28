//! Tunnels that give the daemon a public address: a Cloudflare quick
//! tunnel, ngrok, or Tailscale Serve and Funnel. The daemon runs the tool
//! as a child, reads the address from its output and stops it when it
//! stops itself. Tunnels forward to the loopback listener, so requests
//! through them are remote (see `access::reach`) and need a device token
//! or a pairing code; the master token never works through one.
//!
//! A malac.io relay is planned as another [`Tunnel`] kind: the daemon
//! would dial out to the relay, which forwards requests the same way, so
//! pairing, device tokens and these status messages stay as they are.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use malac_core::config::Tunnel;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::protocol::{TunnelState, TunnelStatus};

/// Lines of output kept to explain a failure.
const KEEP_LINES: usize = 6;

/// The program and arguments for a tunnel to `port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub provider: Tunnel,
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl Spec {
    /// Finds the provider's tool; `Err` carries how to install it.
    pub fn new(provider: Tunnel, port: u16) -> Result<Self, String> {
        let name = program_name(provider);
        let program = find_program(name).ok_or_else(|| {
            format!(
                "{name} is not installed. {}",
                install_hint(provider, cfg!(target_os = "macos"))
            )
        })?;
        Ok(Self::with_program(provider, program, port))
    }

    /// The same with a given program, e.g. a stand-in for tests.
    pub fn with_program(provider: Tunnel, program: PathBuf, port: u16) -> Self {
        let target = format!("http://127.0.0.1:{port}");
        let args: Vec<String> = match provider {
            Tunnel::Cloudflare => vec![
                "tunnel".into(),
                "--no-autoupdate".into(),
                "--url".into(),
                target,
            ],
            Tunnel::Ngrok => vec![
                "http".into(),
                port.to_string(),
                "--log=stdout".into(),
                "--log-format=json".into(),
            ],
            Tunnel::Tailscale => vec!["serve".into(), target],
            Tunnel::TailscaleFunnel => vec!["funnel".into(), target],
        };
        Self {
            provider,
            program,
            args,
        }
    }
}

fn program_name(provider: Tunnel) -> &'static str {
    match provider {
        Tunnel::Cloudflare => "cloudflared",
        Tunnel::Ngrok => "ngrok",
        Tunnel::Tailscale | Tunnel::TailscaleFunnel => "tailscale",
    }
}

/// `PATH` first, then where installers put these tools when a daemon
/// started by an editor or launchd has a short `PATH`.
fn find_program(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join("bin"));
        dirs.push(home.join(".local/bin"));
    }
    dirs.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/snap/bin",
        ]
        .map(PathBuf::from),
    );
    let found = dirs.iter().map(|d| d.join(name)).find(|p| p.is_file());
    let app = Path::new("/Applications/Tailscale.app/Contents/MacOS/Tailscale");
    found.or_else(|| (name == "tailscale" && app.is_file()).then(|| app.to_owned()))
}

/// How to install a provider's tool; malac never installs it itself.
pub fn install_hint(provider: Tunnel, macos: bool) -> String {
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    };
    match (provider, macos) {
        (Tunnel::Cloudflare, true) => "Install it with `brew install cloudflared`.".into(),
        (Tunnel::Cloudflare, false) => format!(
            "Install the static binary in ~/bin: `mkdir -p ~/bin && curl -fsSL -o ~/bin/cloudflared \
             https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-{arch} \
             && chmod +x ~/bin/cloudflared`."
        ),
        (Tunnel::Ngrok, true) => {
            "Install it with `brew install ngrok`, then `ngrok config add-authtoken <token>` \
             (a free account at ngrok.com)."
                .into()
        }
        (Tunnel::Ngrok, false) => {
            "Install it from https://ngrok.com/download (or `snap install ngrok`), then \
             `ngrok config add-authtoken <token>` (a free account at ngrok.com)."
                .into()
        }
        (_, true) => "Install the Tailscale app from https://tailscale.com/download, or \
                      `brew install tailscale`, and sign in."
            .into(),
        (_, false) => "Install it with `curl -fsSL https://tailscale.com/install.sh | sh`, then \
                       `sudo tailscale up`."
            .into(),
    }
}

/// What to know about a provider, shown with its address.
pub fn notes(provider: Tunnel) -> Vec<String> {
    let mut notes = vec![
        "Anyone with the address reaches the pairing page; using malac still takes a pairing \
         code or a device token."
            .to_owned(),
    ];
    notes.push(
        match provider {
            Tunnel::Cloudflare => {
                "A quick tunnel gets a new address each time it starts; update saved connections \
                 with `malac remote add <name> <new address>`."
            }
            Tunnel::Ngrok => {
                "Free ngrok addresses show a warning page in browsers first; choose Visit Site once."
            }
            Tunnel::Tailscale => "Reachable only from devices in your tailnet.",
            Tunnel::TailscaleFunnel => "Public on the internet through Tailscale Funnel.",
        }
        .to_owned(),
    );
    notes
}

/// The public address in a line of the tool's output, if it has one.
pub fn parse_url(provider: Tunnel, line: &str) -> Option<String> {
    if provider == Tunnel::Ngrok {
        let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        if value["msg"] == "started tunnel" {
            return value["url"]
                .as_str()
                .map(|u| u.trim_end_matches('/').to_owned());
        }
        return None;
    }
    let suffixes: &[&str] = match provider {
        Tunnel::Cloudflare => &[".trycloudflare.com"],
        _ => &[".ts.net"],
    };
    line.match_indices("https://").find_map(|(start, _)| {
        let rest = &line[start..];
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '|' | '"' | '\'' | '<' | '>'))
            .unwrap_or(rest.len());
        let url = rest[..end].trim_end_matches('/');
        let host = url["https://".len()..].split(['/', ':']).next()?;
        let ours = suffixes.iter().any(|s| host.ends_with(s)) && !host.starts_with("api.");
        ours.then(|| url.to_owned())
    })
}

/// A line worth showing when the tunnel fails: ngrok's JSON errors become
/// their message.
fn readable(provider: Tunnel, line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if provider == Tunnel::Ngrok {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            let level = value["lvl"].as_str().unwrap_or_default();
            if !matches!(level, "eror" | "crit" | "warn") {
                return None;
            }
            let err = value["err"].as_str().or(value["msg"].as_str())?;
            return Some(err.lines().next().unwrap_or(err).to_owned());
        }
    }
    Some(line.to_owned())
}

/// A tunnel the daemon runs.
pub struct Running {
    status: Arc<Mutex<TunnelStatus>>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl Running {
    /// Starts `spec`; `changed` hears every status change.
    pub fn start(
        spec: Spec,
        changed: impl Fn(TunnelStatus) + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        let provider = spec.provider;
        let status = Arc::new(Mutex::new(TunnelStatus {
            provider,
            state: TunnelState::Starting,
            url: None,
            error: None,
            waiting: None,
            notes: notes(provider),
            started: Utc::now(),
        }));
        let (lines_tx, mut lines) = mpsc::unbounded_channel();
        if let Some(stdout) = child.stdout.take() {
            forward_lines(stdout, lines_tx.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            forward_lines(stderr, lines_tx);
        }
        let (stop, mut stopped) = oneshot::channel::<()>();
        let shared = status.clone();
        let task = tokio::spawn(async move {
            let mut recent: Vec<String> = Vec::new();
            let update = |edit: &dyn Fn(&mut TunnelStatus)| {
                let snapshot = {
                    let mut status = shared.lock().unwrap();
                    edit(&mut status);
                    status.clone()
                };
                changed(snapshot);
            };
            let exit = loop {
                tokio::select! {
                    line = lines.recv() => {
                        let Some(line) = line else {
                            break child.wait().await.ok();
                        };
                        tracing::debug!("{}: {line}", program_name(provider));
                        if let Some(url) = parse_url(provider, &line) {
                            if shared.lock().unwrap().url.is_none() {
                                update(&|s| {
                                    s.state = TunnelState::Running;
                                    s.url = Some(url.clone());
                                    s.waiting = None;
                                });
                            }
                        } else if let Some(text) = readable(provider, &line) {
                            recent.push(text);
                            if recent.len() > KEEP_LINES {
                                recent.remove(0);
                            }
                            // A tool that waits for the user, such as
                            // `tailscale serve` on a tailnet without Serve,
                            // says so here and nowhere else.
                            if shared.lock().unwrap().state == TunnelState::Starting {
                                let said = recent.join("\n");
                                update(&|s| s.waiting = Some(said.clone()));
                            }
                        }
                    }
                    status = child.wait() => {
                        // What it wrote just before exiting may still be on its way.
                        let drain = Duration::from_secs(1);
                        while let Ok(Some(line)) = tokio::time::timeout(drain, lines.recv()).await {
                            if let Some(text) = readable(provider, &line) {
                                recent.push(text);
                            }
                        }
                        recent.drain(..recent.len().saturating_sub(KEEP_LINES));
                        break status.ok();
                    }
                    _ = &mut stopped => {
                        terminate(&mut child).await;
                        update(&|s| {
                            s.state = TunnelState::Stopped;
                            s.waiting = None;
                        });
                        return;
                    }
                }
            };
            let why = match recent.is_empty() {
                true => format!(
                    "{} exited ({})",
                    program_name(provider),
                    exit.map_or("no status".into(), |s| s.to_string())
                ),
                false => recent.join("\n"),
            };
            update(&|s| {
                s.state = TunnelState::Failed;
                s.error = Some(why.clone());
                s.waiting = None;
            });
        });
        Ok(Self {
            status,
            stop: Some(stop),
            task,
        })
    }

    pub fn status(&self) -> TunnelStatus {
        self.status.lock().unwrap().clone()
    }

    /// Stops the tool: SIGTERM, then SIGKILL after a few seconds.
    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut self.task).await;
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        // Aborting drops the child, which kills it.
        self.task.abort();
    }
}

fn forward_lines(
    stream: impl AsyncRead + Unpin + Send + 'static,
    out: mpsc::UnboundedSender<String>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if out.send(line).is_err() {
                break;
            }
        }
    });
}

async fn terminate(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // SAFETY: signals a child this process started and still owns.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .is_ok()
        {
            return;
        }
    }
    let _ = child.kill().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Starts a fake tool written just now. On Linux, another test thread
    /// that forks while the script is still open makes running it fail with
    /// "Text file busy" for a moment, so this tries again.
    #[cfg(unix)]
    fn start_fake(spec: Spec, tx: &mpsc::UnboundedSender<TunnelStatus>) -> Running {
        for _ in 0..50 {
            let tx = tx.clone();
            match Running::start(spec.clone(), move |s| {
                let _ = tx.send(s);
            }) {
                Err(err) if err.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                started => return started.unwrap(),
            }
        }
        panic!("the fake tool stayed busy");
    }

    #[test]
    fn finds_addresses_in_tool_output() {
        let cloudflare = [
            "2026-10-26T08:00:00Z INF Requesting new quick Tunnel on trycloudflare.com...",
            "2026-10-26T08:00:01Z INF |  https://quiet-river-maple.trycloudflare.com                                  |",
        ];
        assert_eq!(parse_url(Tunnel::Cloudflare, cloudflare[0]), None);
        assert_eq!(
            parse_url(Tunnel::Cloudflare, cloudflare[1]).as_deref(),
            Some("https://quiet-river-maple.trycloudflare.com")
        );
        assert_eq!(
            parse_url(
                Tunnel::Cloudflare,
                r#"ERR failed to request quick Tunnel: Post "https://api.trycloudflare.com/tunnel""#
            ),
            None
        );
        let ngrok = r#"{"addr":"http://localhost:7700","lvl":"info","msg":"started tunnel","name":"command_line","obj":"tunnels","t":"2026-10-26T08:00:00Z","url":"https://ab12-1-2-3-4.ngrok-free.app"}"#;
        assert_eq!(
            parse_url(Tunnel::Ngrok, ngrok).as_deref(),
            Some("https://ab12-1-2-3-4.ngrok-free.app")
        );
        let failed = r#"{"err":"authentication failed: Usage of ngrok requires a verified account and authtoken.\n\nSign up","lvl":"eror","msg":"session closing","obj":"tunnels.session"}"#;
        assert_eq!(parse_url(Tunnel::Ngrok, failed), None);
        assert_eq!(
            readable(Tunnel::Ngrok, failed).as_deref(),
            Some(
                "authentication failed: Usage of ngrok requires a verified account and authtoken."
            )
        );
        assert_eq!(
            readable(Tunnel::Ngrok, r#"{"lvl":"info","msg":"open config file"}"#),
            None
        );
        assert_eq!(
            parse_url(Tunnel::Tailscale, "https://bstation.tail1234.ts.net/").as_deref(),
            Some("https://bstation.tail1234.ts.net")
        );
        assert_eq!(
            parse_url(
                Tunnel::TailscaleFunnel,
                "To enable, visit: https://login.tailscale.com/f/funnel?node=abc"
            ),
            None
        );
    }

    #[test]
    fn builds_commands_for_the_loopback_port() {
        let spec = Spec::with_program(Tunnel::Cloudflare, "cloudflared".into(), 7790);
        assert_eq!(
            spec.args,
            [
                "tunnel",
                "--no-autoupdate",
                "--url",
                "http://127.0.0.1:7790"
            ]
        );
        let spec = Spec::with_program(Tunnel::TailscaleFunnel, "tailscale".into(), 7790);
        assert_eq!(spec.args, ["funnel", "http://127.0.0.1:7790"]);
        assert!(install_hint(Tunnel::Cloudflare, true).contains("brew install cloudflared"));
        assert!(install_hint(Tunnel::Cloudflare, false).contains("~/bin/cloudflared"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_reports_and_stops_a_tool() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("cloudflared");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho 'INF |  https://fake-tunnel.trycloudflare.com  |' >&2\nexec sleep 30\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let spec = Spec::with_program(Tunnel::Cloudflare, fake.clone(), 7790);
        let running = start_fake(spec, &tx);
        let up = rx.recv().await.unwrap();
        assert_eq!(up.state, TunnelState::Running);
        assert_eq!(
            up.url.as_deref(),
            Some("https://fake-tunnel.trycloudflare.com")
        );
        assert_eq!(running.status().url, up.url);
        running.stop().await;
        assert_eq!(rx.recv().await.unwrap().state, TunnelState::Stopped);

        std::fs::write(
            &fake,
            "#!/bin/sh\necho 'ERR cannot reach the edge' >&2\nexit 1\n",
        )
        .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let spec = Spec::with_program(Tunnel::Cloudflare, fake, 7790);
        let _running = start_fake(spec, &tx);
        // What it printed shows as `waiting` first, then as the error.
        let failed = loop {
            let status = rx.recv().await.unwrap();
            if status.state != TunnelState::Starting {
                break status;
            }
        };
        assert_eq!(failed.state, TunnelState::Failed);
        assert_eq!(failed.error.as_deref(), Some("ERR cannot reach the edge"));
        assert_eq!(failed.waiting, None);
    }

    /// `tailscale serve` on a tailnet without Serve prints a link and waits
    /// until the user turns Serve on; the status shows the link meanwhile.
    #[cfg(unix)]
    #[tokio::test]
    async fn shows_what_a_waiting_tool_asks_for() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("tailscale");
        std::fs::write(
            &fake,
            "#!/bin/sh\n\
             echo 'Serve is not enabled on your tailnet.'\n\
             echo 'To enable, visit:'\n\
             echo ''\n\
             echo '         https://login.tailscale.com/f/serve?node=abc'\n\
             sleep 1\n\
             echo 'https://laptop.tail1234.ts.net/'\n\
             exec sleep 30\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let spec = Spec::with_program(Tunnel::Tailscale, fake, 7790);
        let running = start_fake(spec, &tx);
        let mut waiting = None;
        let up = loop {
            let status = rx.recv().await.unwrap();
            if status.state != TunnelState::Starting {
                break status;
            }
            waiting = status.waiting;
        };
        assert_eq!(
            waiting.as_deref(),
            Some(
                "Serve is not enabled on your tailnet.\nTo enable, visit:\n\
                 https://login.tailscale.com/f/serve?node=abc"
            )
        );
        assert_eq!(up.state, TunnelState::Running);
        assert_eq!(up.url.as_deref(), Some("https://laptop.tail1234.ts.net"));
        assert_eq!(up.waiting, None);
        running.stop().await;
    }
}
