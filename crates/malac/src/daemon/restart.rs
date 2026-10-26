//! What a daemon stopped for a restart leaves for the next one. A restart
//! is for a new version: it keeps the address, remote access and tunnel, so
//! paired phones and open tunnels carry on. `malac daemon stop` and then
//! `start` apply the configuration afresh.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use malac_core::config::{self, Tunnel};
use serde::{Deserialize, Serialize};

use super::hub::Hub;
use crate::protocol::TunnelState;

/// A daemon starting later than this after the restart starts afresh.
const KEEP: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Restart {
    pub listen: SocketAddr,
    pub remote: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<Tunnel>,
    pub saved: DateTime<Utc>,
}

impl Restart {
    /// How `hub` runs now.
    pub fn of(hub: &Hub) -> Option<Self> {
        let remote = hub.remote_status();
        let tunnel = remote
            .tunnel
            .filter(|t| matches!(t.state, TunnelState::Starting | TunnelState::Running))
            .map(|t| t.provider);
        Some(Self {
            listen: remote.listen.parse().ok()?,
            remote: remote.enabled,
            tunnel,
            saved: Utc::now(),
        })
    }

    pub fn save(&self) -> Result<()> {
        save_to(&path(), self)
    }

    /// The state a restart left, once: the file goes as it is read.
    pub fn take() -> Option<Self> {
        take_from(&path(), Utc::now())
    }
}

fn path() -> PathBuf {
    config::home_dir().join("restart.json")
}

fn save_to(path: &Path, restart: &Restart) -> Result<()> {
    let text = serde_json::to_string_pretty(restart)?;
    std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))
}

fn take_from(path: &Path, now: DateTime<Utc>) -> Option<Restart> {
    let text = std::fs::read_to_string(path).ok()?;
    let _ = std::fs::remove_file(path);
    let restart: Restart = serde_json::from_str(&text).ok()?;
    let age = now
        .signed_duration_since(restart.saved)
        .to_std()
        .unwrap_or_default();
    (age < KEEP).then_some(restart)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(saved: DateTime<Utc>) -> Restart {
        Restart {
            listen: "0.0.0.0:7700".parse().unwrap(),
            remote: true,
            tunnel: Some(Tunnel::Tailscale),
            saved,
        }
    }

    #[test]
    fn a_restart_is_taken_once_and_only_while_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("restart.json");
        let now = Utc::now();

        save_to(&file, &sample(now)).unwrap();
        assert_eq!(
            take_from(&file, now + chrono::Duration::seconds(5)),
            Some(sample(now))
        );
        assert!(!file.exists());
        assert_eq!(take_from(&file, now), None);

        save_to(&file, &sample(now)).unwrap();
        assert_eq!(take_from(&file, now + chrono::Duration::minutes(10)), None);
        assert!(!file.exists(), "a stale restart is removed too");

        std::fs::write(&file, "not json").unwrap();
        assert_eq!(take_from(&file, now), None);
    }
}
