//! Saved connections to daemons on other machines, in
//! `~/.malac/connections.json` (mode 0600: it holds device tokens).
//! `malac --connect <name>` uses one; the full-screen interface also
//! remembers the one it used last for the next plain `malac`.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use malac_core::config;
use serde::{Deserialize, Serialize};

use crate::client::{Client, DaemonFile, Events};
use crate::protocol::Snapshot;

/// The name that always means this machine's daemon.
pub const LOCAL: &str = "local";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub name: String,
    /// The daemon's address, e.g. `http://127.0.0.1:7790` or `https://x.trycloudflare.com`.
    pub url: String,
    /// The device token from pairing.
    pub token: String,
    /// The device id the daemon gave this machine, to revoke it on removal.
    #[serde(default)]
    pub device: Option<String>,
    pub added: DateTime<Utc>,
}

impl Connection {
    pub fn file(&self) -> DaemonFile {
        DaemonFile::remote(&self.name, &self.url, &self.token)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Connections {
    /// The connection the full-screen interface used last; `None` is this machine.
    #[serde(default)]
    pub last: Option<String>,
    #[serde(default)]
    pub connections: Vec<Connection>,
}

impl Connections {
    pub fn path() -> PathBuf {
        config::home_dir().join("connections.json")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("cannot read {}", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        let tmp = path.with_extension("json.tmp");
        config::write_private(&tmp, &serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
    }

    pub fn get(&self, name: &str) -> Option<&Connection> {
        self.connections.iter().find(|c| c.name == name)
    }

    /// Adds a connection or replaces the one with its name.
    pub fn put(&mut self, connection: Connection) {
        match self
            .connections
            .iter_mut()
            .find(|c| c.name == connection.name)
        {
            Some(existing) => *existing = connection,
            None => self.connections.push(connection),
        }
    }

    pub fn remove(&mut self, name: &str) -> Option<Connection> {
        let index = self.connections.iter().position(|c| c.name == name)?;
        if self.last.as_deref() == Some(name) {
            self.last = None;
        }
        Some(self.connections.remove(index))
    }
}

/// Which daemon a client talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Local,
    Saved(Connection),
}

impl Endpoint {
    /// `None` for this machine.
    pub fn name(&self) -> Option<&str> {
        match self {
            Endpoint::Local => None,
            Endpoint::Saved(c) => Some(&c.name),
        }
    }

    pub fn label(&self) -> String {
        self.name().unwrap_or("this computer").to_owned()
    }

    /// Connects, starting the local daemon if needed; a daemon elsewhere
    /// must already run.
    pub async fn connect(&self) -> Result<(Client, Events, Snapshot)> {
        match self {
            Endpoint::Local => Client::connect_or_start(&Default::default()).await,
            Endpoint::Saved(c) => Client::connect(&c.file())
                .await
                .with_context(|| format!("cannot reach `{}` at {}", c.name, c.url)),
        }
    }
}

/// The endpoint a name means: `local` (or none) is this machine.
pub fn resolve(name: Option<&str>) -> Result<Endpoint> {
    match name {
        None | Some(LOCAL) => Ok(Endpoint::Local),
        Some(name) => {
            let saved = Connections::load()?;
            match saved.get(name) {
                Some(c) => Ok(Endpoint::Saved(c.clone())),
                None => {
                    let names: Vec<&str> =
                        saved.connections.iter().map(|c| c.name.as_str()).collect();
                    match names.is_empty() {
                        true => bail!(
                            "no saved connection `{name}`; add one with `malac remote add {name} <url> --pair <code>`"
                        ),
                        false => bail!(
                            "no saved connection `{name}`; there are: {}",
                            names.join(", ")
                        ),
                    }
                }
            }
        }
    }
}

/// The connection the full-screen interface used last, if it still exists.
pub fn remembered() -> Option<Connection> {
    let saved = Connections::load().ok()?;
    saved.get(saved.last.as_deref()?).cloned()
}

/// Remembers the full-screen interface's connection for the next start.
pub fn remember(name: Option<&str>) {
    let result = Connections::load().and_then(|mut saved| {
        if saved.last.as_deref() == name {
            return Ok(());
        }
        saved.last = name.filter(|n| saved.get(n).is_some()).map(str::to_owned);
        saved.save()
    });
    if let Err(err) = result {
        tracing::warn!("cannot remember the connection: {err:#}");
    }
}

/// Names are short and plain, so they work as `-C <name>` and in menus.
pub fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        bail!("a connection name uses letters, digits, `-`, `_` and `.` (up to 40)");
    }
    if name == LOCAL {
        bail!("`{LOCAL}` always means this machine; pick another name");
    }
    Ok(())
}

/// The daemon's base address from what the user typed, and the pairing
/// code when they pasted a whole pairing link (`…/#pair=CODE`).
pub fn parse_address(input: &str) -> Result<(String, Option<String>)> {
    let input = input.trim();
    let with_scheme = match input.contains("://") {
        true => input.to_owned(),
        false => format!("http://{input}"),
    };
    let url =
        reqwest::Url::parse(&with_scheme).with_context(|| format!("`{input}` is not a URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("use an http:// or https:// address");
    }
    let host = url.host_str().context("the address has no host")?;
    let host = match host.contains(':') && !host.starts_with('[') {
        true => format!("[{host}]"),
        false => host.to_owned(),
    };
    let base = match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    };
    let code = url
        .fragment()
        .into_iter()
        .chain(url.query())
        .flat_map(|part| part.split('&'))
        .find_map(|pair| pair.strip_prefix("pair="))
        .map(str::to_owned);
    Ok((base, code))
}

/// This machine's name, for naming its device on another daemon.
pub fn hostname() -> String {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: the buffer is valid for its length; the name is NUL-terminated or cut.
        let ok = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0;
        if ok {
            let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
            let name = String::from_utf8_lossy(&buffer[..end]);
            let name = name.trim_end_matches(".local");
            if !name.is_empty() {
                return name.to_owned();
            }
        }
    }
    "computer".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_addresses_and_pairing_links() {
        assert_eq!(
            parse_address("192.168.1.20:7700").unwrap(),
            ("http://192.168.1.20:7700".into(), None)
        );
        assert_eq!(
            parse_address("https://quiet-river.trycloudflare.com/#pair=ABCDE-FGHJK").unwrap(),
            (
                "https://quiet-river.trycloudflare.com".into(),
                Some("ABCDE-FGHJK".into())
            )
        );
        assert_eq!(
            parse_address("http://[::1]:7790/?pair=X").unwrap(),
            ("http://[::1]:7790".into(), Some("X".into()))
        );
        assert!(parse_address("ftp://host").is_err());
    }

    #[test]
    fn names_are_plain() {
        assert!(check_name("bstation").is_ok());
        assert!(check_name("build-box_2.lan").is_ok());
        assert!(check_name("local").is_err());
        assert!(check_name("has space").is_err());
        assert!(check_name("").is_err());
    }

    #[test]
    fn put_replaces_by_name() {
        let connection = |name: &str, url: &str| Connection {
            name: name.into(),
            url: url.into(),
            token: "t".into(),
            device: None,
            added: Utc::now(),
        };
        let mut saved = Connections::default();
        saved.put(connection("a", "http://one"));
        saved.put(connection("b", "http://two"));
        saved.put(connection("a", "http://three"));
        assert_eq!(saved.connections.len(), 2);
        assert_eq!(saved.get("a").unwrap().url, "http://three");
        saved.last = Some("a".into());
        assert!(saved.remove("a").is_some());
        assert!(saved.last.is_none());
    }
}
