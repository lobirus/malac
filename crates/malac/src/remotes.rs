//! `malac remote` and `malac tunnel`: pairing devices with this machine's
//! daemon, saving connections to daemons elsewhere, and tunnels.

use std::io::IsTerminal;
use std::time::{Duration, Instant};

use crate::style::Paint;
use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Args, Subcommand};
use malac_core::config::{Config, Tunnel};
use serde_json::{Value, json};

use crate::client::{self, DaemonFile};
use crate::connections::{self, Connection, Connections};
use crate::daemon::access;
use crate::daemon::{ServeArgs, tunnel};
use crate::protocol::{
    DeviceView, Paired, Pairing, RemoteStatus, TunnelState, TunnelStatus, UrlKind,
};
use crate::qr::QrCode;

#[derive(Subcommand, Debug, Clone)]
pub enum RemoteAction {
    /// Whether other devices may connect, from where, and how (the default).
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Show a one-time code and QR code another device trades for its own token.
    Pair {
        #[arg(long)]
        json: bool,
    },
    /// List the devices paired with this machine's daemon.
    Devices {
        #[arg(long)]
        json: bool,
    },
    /// Revoke a paired device; its connections close at once.
    Revoke { id: String },
    /// Save a connection to a daemon on another machine. With a name that
    /// exists and no --token or --pair, only its address changes.
    Add {
        name: String,
        /// The daemon's address, or a whole pairing link (`…/#pair=CODE`).
        url: String,
        /// A device token you already have.
        #[arg(long, conflicts_with = "pair")]
        token: Option<String>,
        /// A code from `malac remote pair` on the other machine.
        #[arg(long)]
        pair: Option<String>,
        /// What the other daemon calls this machine (default: its host name).
        #[arg(long)]
        device_name: Option<String>,
    },
    /// List saved connections and whether they answer.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Forget a saved connection, revoking its token when the daemon answers.
    Remove { name: String },
}

/// `malac tunnel [cloudflare|ngrok|tailscale|tailscale-funnel|status|stop]`.
#[derive(Args, Debug, Clone)]
pub struct TunnelArgs {
    /// cloudflare, ngrok, tailscale (your tailnet), tailscale-funnel (public),
    /// status or stop. Default: the running tunnel's status, else `remote.tunnel`,
    /// else cloudflare.
    pub what: Option<String>,
    #[arg(long)]
    pub json: bool,
}

pub async fn remote(action: Option<RemoteAction>) -> Result<i32> {
    match action.unwrap_or(RemoteAction::Status { json: false }) {
        RemoteAction::Status { json } => status(json).await,
        RemoteAction::Pair { json } => pair(json).await,
        RemoteAction::Devices { json } => devices(json).await,
        RemoteAction::Revoke { id } => {
            let file = local_daemon().await?;
            let reply: Value = client::delete(&file, &format!("/api/devices/{id}")).await?;
            let name = reply["revoked"]["name"].as_str().unwrap_or(&id).to_owned();
            println!("revoked {name} ({id}); its connections are closed");
            Ok(0)
        }
        RemoteAction::Add {
            name,
            url,
            token,
            pair,
            device_name,
        } => add(&name, &url, token, pair, device_name).await,
        RemoteAction::List { json } => list(json).await,
        RemoteAction::Remove { name } => remove(&name).await,
    }
}

/// This machine's daemon, started if needed (pairing works without remote
/// access too: SSH tunnels look local).
async fn local_daemon() -> Result<DaemonFile> {
    client::ensure_daemon(&ServeArgs::default()).await
}

async fn status(json: bool) -> Result<i32> {
    let file = local_daemon().await?;
    let remote: RemoteStatus = client::get(&file, "/api/remote").await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&remote)?);
        return Ok(0);
    }
    let port = remote
        .listen
        .rsplit(':')
        .next()
        .unwrap_or("7700")
        .to_owned();
    match remote.enabled {
        true => println!(
            "{} allowed · listening on {} · {} paired devices",
            "remote access".bold(),
            remote.listen,
            remote.devices
        ),
        false => println!(
            "{} off · this machine and SSH tunnels only · {} paired devices",
            "remote access".bold(),
            remote.devices
        ),
    }
    for url in &remote.urls {
        println!("  {:<14} {}", url.kind.short(), url.url);
    }
    if let Some(tunnel) = &remote.tunnel {
        print_tunnel(tunnel);
    }
    println!();
    println!("{}", "How to connect".bold());
    for (way, how) in connect_steps(&remote, &port) {
        println!("  {:<14} {how}", way);
    }
    println!();
    println!(
        "{}",
        "`malac remote pair` shows a code to pair a device · `malac remote devices` lists them"
            .dark_grey()
    );
    Ok(0)
}

/// One line per way to reach this daemon.
pub fn connect_steps(remote: &RemoteStatus, port: &str) -> Vec<(&'static str, String)> {
    let lan = remote.urls.iter().find(|u| u.kind == UrlKind::Lan);
    let mut steps = vec![(
        "same network",
        match (remote.enabled, lan) {
            (true, Some(lan)) => format!("malac remote add <name> {} --pair <code>", lan.url),
            (true, None) => {
                "listen on every interface (unset web.host) to be reachable here".into()
            }
            (false, _) => "start the daemon with --remote (or set web.remote = true)".into(),
        },
    )];
    steps.push((
        "SSH tunnel",
        format!(
            "ssh -N -L {port}:127.0.0.1:{port} <this host>, then malac remote add <name> \
             http://127.0.0.1:{port} --pair <code>"
        ),
    ));
    let needs_remote = |what: &str| match remote.enabled {
        true => what.to_owned(),
        false => format!("{what} (needs remote access)"),
    };
    steps.push(("Cloudflare", needs_remote("malac tunnel cloudflare")));
    steps.push(("ngrok", needs_remote("malac tunnel ngrok")));
    steps.push((
        "Tailscale",
        needs_remote("malac tunnel tailscale, or tailscale-funnel to go public"),
    ));
    steps
}

async fn pair(json: bool) -> Result<i32> {
    let file = local_daemon().await?;
    let pairing: Pairing = client::post(&file, "/api/pair/codes", &json!({})).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&pairing)?);
        return Ok(0);
    }
    let remote: RemoteStatus = client::get(&file, "/api/remote").await?;
    let minutes = (pairing.expires - Utc::now()).num_minutes() + 1;
    println!(
        "Pairing code  {}   valid for {minutes} minutes, for one device",
        pairing.code.clone().bold()
    );
    let Some(best) = pairing.links.first() else {
        bail!("the daemon reported no address");
    };
    if std::io::stdout().is_terminal() {
        if let Some(qr) = QrCode::encode(&best.url) {
            println!();
            for line in qr.ansi() {
                println!("  {line}");
            }
        }
    }
    println!();
    println!(
        "  scan it with a phone, or open {}",
        best.url.clone().underlined()
    );
    for link in &pairing.links {
        println!(
            "  {:<14} malac remote add <name> {} --pair {}",
            link.kind.short(),
            link.base,
            pairing.code
        );
    }
    if !remote.enabled {
        println!();
        println!(
            "{}",
            "! remote access is off: only this machine and SSH tunnels can pair and connect; \
             `malac daemon restart --remote` opens it to other devices"
                .yellow()
        );
    } else if best.kind == UrlKind::Local {
        println!();
        println!(
            "{}",
            "! no network address: the daemon listens on loopback only; use an SSH tunnel or \
             `malac tunnel`"
                .yellow()
        );
    }
    Ok(0)
}

async fn devices(json: bool) -> Result<i32> {
    let file = local_daemon().await?;
    let reply: Value = client::get(&file, "/api/devices").await?;
    let devices: Vec<DeviceView> = serde_json::from_value(reply["devices"].clone())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&devices)?);
        return Ok(0);
    }
    if devices.is_empty() {
        println!("no paired devices · `malac remote pair` pairs one");
        return Ok(0);
    }
    for device in devices {
        println!("{}", device_line(&device));
    }
    println!("{}", "`malac remote revoke <id>` revokes one".dark_grey());
    Ok(0)
}

pub fn device_line(device: &DeviceView) -> String {
    let seen = device
        .last_seen
        .map(|t| format!("seen {}", ago(t)))
        .unwrap_or_else(|| "never seen".into());
    let from = device
        .address
        .as_deref()
        .map(|a| format!(" from {a}"))
        .unwrap_or_default();
    format!(
        "{}  {:<24} {:<8} paired {} · {seen}{from}",
        device.id.as_str().dark_grey(),
        device.name,
        device.kind,
        device
            .created
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d"),
    )
}

/// "just now", "5 min ago", "3 h ago", "2 days ago".
pub fn ago(at: chrono::DateTime<Utc>) -> String {
    let seconds = (Utc::now() - at).num_seconds().max(0);
    match seconds {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86400),
    }
}

async fn add(
    name: &str,
    url: &str,
    token: Option<String>,
    code: Option<String>,
    device_name: Option<String>,
) -> Result<i32> {
    connections::check_name(name)?;
    let (base, linked_code) = connections::parse_address(url)?;
    let code = code.or(linked_code);
    let mut saved = Connections::load()?;
    let (token, device) = match (token, code) {
        (Some(token), _) => (token, None),
        (None, Some(code)) => {
            let code = access::check_code(&code)?;
            let name = device_name.unwrap_or_else(connections::hostname);
            let paired = pair_with(&base, &code, &name).await?;
            println!(
                "paired as {} ({}) with malac {} at {base}",
                paired.device.name, paired.device.id, paired.version
            );
            (paired.token, Some(paired.device.id))
        }
        (None, None) => match saved.get(name) {
            Some(existing) => (existing.token.clone(), existing.device.clone()),
            None => bail!(
                "pass --pair <code> (from `malac remote pair` on the other machine) or --token <token>"
            ),
        },
    };
    let connection = Connection {
        name: name.to_owned(),
        url: base.clone(),
        token,
        device,
        added: Utc::now(),
    };
    let info = client::get::<crate::protocol::DaemonInfo>(&connection.file(), "/api/daemon")
        .await
        .with_context(|| format!("{base} did not accept the token"))?;
    saved.put(connection);
    saved.save()?;
    println!(
        "saved `{name}`: malac {} with {} workspaces · `malac -C {name}` connects",
        info.status.version, info.workspaces
    );
    Ok(0)
}

/// Trades a pairing code for a device token.
pub async fn pair_with(base: &str, code: &str, name: &str) -> Result<Paired> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;
    let response = http
        .post(format!("{base}/api/pair"))
        .json(&json!({ "code": code, "name": name, "kind": "cli" }))
        .send()
        .await
        .with_context(|| format!("cannot reach {base}"))?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_owned))
            .unwrap_or(text);
        bail!("pairing failed: {message} ({status})");
    }
    serde_json::from_str(&text).context("unexpected reply to pairing")
}

async fn list(json: bool) -> Result<i32> {
    let saved = Connections::load()?;
    let mut rows = Vec::new();
    for connection in &saved.connections {
        let probe = tokio::time::timeout(
            Duration::from_secs(4),
            client::get::<crate::protocol::DaemonInfo>(&connection.file(), "/api/daemon"),
        )
        .await;
        let state = match probe {
            Ok(Ok(info)) => Ok(format!(
                "malac {} · {} workspaces · {} agents running",
                info.status.version, info.workspaces, info.agents_running
            )),
            Ok(Err(err)) => Err(format!("{err:#}")),
            Err(_) => Err("no answer".to_owned()),
        };
        rows.push((connection, state));
    }
    if json {
        let rows: Vec<Value> = rows
            .iter()
            .map(|(c, state)| {
                json!({
                    "name": c.name, "url": c.url, "device": c.device, "added": c.added,
                    "last": saved.last.as_deref() == Some(c.name.as_str()),
                    "ok": state.is_ok(),
                    "status": match state { Ok(s) | Err(s) => s },
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(0);
    }
    if rows.is_empty() {
        println!("no saved connections · `malac remote add <name> <url> --pair <code>` adds one");
        return Ok(0);
    }
    for (connection, state) in rows {
        let mark = match saved.last.as_deref() == Some(connection.name.as_str()) {
            true => "*",
            false => " ",
        };
        let state = match state {
            Ok(text) => text.green().to_string(),
            Err(err) => err.lines().next().unwrap_or_default().red().to_string(),
        };
        println!(
            "{mark} {:<16} {:<40} {state}",
            connection.name, connection.url
        );
    }
    println!(
        "{}",
        "* the full-screen interface's last connection · `malac -C <name>` connects".dark_grey()
    );
    Ok(0)
}

async fn remove(name: &str) -> Result<i32> {
    let mut saved = Connections::load()?;
    let connection = saved
        .remove(name)
        .with_context(|| format!("no saved connection `{name}`"))?;
    saved.save()?;
    let revoked = match &connection.device {
        Some(id) => tokio::time::timeout(
            Duration::from_secs(5),
            client::delete::<Value>(&connection.file(), &format!("/api/devices/{id}")),
        )
        .await
        .is_ok_and(|r| r.is_ok()),
        None => false,
    };
    match revoked {
        true => println!(
            "removed `{name}` and revoked its token on {}",
            connection.url
        ),
        false => println!(
            "removed `{name}`; its token may still work: `malac remote revoke` on the other machine"
        ),
    }
    Ok(0)
}

pub async fn tunnel(args: TunnelArgs) -> Result<i32> {
    let what = args.what.as_deref();
    if what == Some("stop") {
        let Some(file) = client::running().await else {
            println!("the daemon is not running, so no tunnel is");
            return Ok(0);
        };
        let reply: Value = client::delete(&file, "/api/tunnel").await?;
        match reply["stopped"].as_bool() == Some(true) {
            true => println!("tunnel stopped"),
            false => println!("no tunnel was running"),
        }
        return Ok(0);
    }
    let config = Config::load_user()?;
    let file = match client::running().await {
        Some(file) => file,
        None => {
            // Only for tunnels: remote access on, but nothing on the network.
            let serve = ServeArgs {
                remote: true,
                host: config.web.host.is_none().then(|| "127.0.0.1".to_owned()),
                ..Default::default()
            };
            client::ensure_daemon(&serve).await?
        }
    };
    let remote: RemoteStatus = client::get(&file, "/api/remote").await?;
    let running = remote
        .tunnel
        .clone()
        .filter(|t| matches!(t.state, TunnelState::Starting | TunnelState::Running));
    let provider: Tunnel = match (what, &running) {
        (Some("status"), _) | (None, Some(_)) => {
            match running {
                Some(tunnel) if args.json => println!("{}", serde_json::to_string_pretty(&tunnel)?),
                Some(tunnel) => print_tunnel(&tunnel),
                None => println!("no tunnel is running · `malac tunnel cloudflare` starts one"),
            }
            return Ok(0);
        }
        (Some(name), _) => name.parse()?,
        (None, None) => config.remote.tunnel.unwrap_or(Tunnel::Cloudflare),
    };
    if !remote.enabled {
        bail!(
            "the daemon runs without remote access, so it would refuse everything coming through \
             a tunnel; `malac daemon restart --remote` (or web.remote = true) allows it"
        );
    }
    let port: u16 = remote
        .listen
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .context("the daemon reported no port")?;
    if let Err(hint) = tunnel::Spec::new(provider, port) {
        bail!("{hint}");
    }
    let started: TunnelStatus = client::post(
        &file,
        "/api/tunnel",
        &json!({ "provider": provider.as_str() }),
    )
    .await?;
    eprintln!("starting {}…", provider.as_str());
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut status = started;
    while status.state == TunnelState::Starting && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let remote: RemoteStatus = client::get(&file, "/api/remote").await?;
        if let Some(tunnel) = remote.tunnel {
            status = tunnel;
        }
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        print_tunnel(&status);
        if status.state == TunnelState::Running {
            println!();
            println!(
                "{}",
                "It stops with the daemon, or with `malac tunnel stop`. Pair a device with \
                 `malac remote pair`."
                    .dark_grey()
            );
        }
    }
    Ok(match status.state {
        TunnelState::Running => 0,
        TunnelState::Starting => {
            eprintln!(
                "{}",
                "! no address yet; `malac tunnel status` checks again".yellow()
            );
            0
        }
        TunnelState::Failed | TunnelState::Stopped => 1,
    })
}

fn print_tunnel(tunnel: &TunnelStatus) {
    let state = match tunnel.state {
        TunnelState::Starting => "starting".yellow(),
        TunnelState::Running => "running".green(),
        TunnelState::Failed => "failed".red(),
        TunnelState::Stopped => "stopped".dark_grey(),
    };
    println!(
        "{} {} {state}{}",
        "tunnel".bold(),
        tunnel.provider.as_str(),
        tunnel
            .url
            .as_deref()
            .map(|u| format!(" · {}", u.underlined()))
            .unwrap_or_default()
    );
    if let Some(error) = &tunnel.error {
        for line in error.lines() {
            println!("  {}", line.red());
        }
    }
    for note in &tunnel.notes {
        println!("  {}", format!("! {note}").yellow());
    }
}
