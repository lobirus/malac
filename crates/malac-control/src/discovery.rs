//! `~/.malac/control.json` advertises a running `malac-control serve` so
//! clients can find its URL and token without configuration.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::malac_dir;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// WebSocket URL, e.g. `ws://127.0.0.1:7878`.
    pub url: String,
    /// Sent as a bearer token or `?token=` query parameter.
    pub token: String,
    pub pid: u32,
}

pub fn path() -> Result<PathBuf> {
    Ok(malac_dir()?.join("control.json"))
}

/// Reads the advertised service, if any. The process may have died since.
pub fn read() -> Result<Option<ServiceInfo>> {
    let path = path()?;
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .with_context(|| format!("malformed {}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

impl ServiceInfo {
    /// Writes the discovery file, readable only by the current user. The
    /// returned guard removes it again when dropped.
    pub fn publish(&self) -> Result<Publication> {
        let path = path()?;
        let dir = path.parent().context("discovery path has no parent")?;
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let tmp = path.with_extension("json.tmp");
        let _ = fs::remove_file(&tmp);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        file.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
        drop(file);
        fs::rename(&tmp, &path).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(Publication {
            path,
            pid: self.pid,
        })
    }
}

pub struct Publication {
    path: PathBuf,
    pid: u32,
}

impl Drop for Publication {
    fn drop(&mut self) {
        // Leave the file alone if another instance has replaced it.
        let ours = fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str::<ServiceInfo>(&text).ok())
            .is_some_and(|info| info.pid == self.pid);
        if ours {
            let _ = fs::remove_file(&self.path);
        }
    }
}
