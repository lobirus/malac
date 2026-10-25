//! Self-update from GitHub releases.
//!
//! New binaries are unpacked next to the installed ones and moved into place
//! with `rename`, which never disturbs a running process: agents keep running
//! on the old binary and the next start picks up the new one.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config;

pub const RELEASES: &str = "https://github.com/lobirus/malac/releases";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const CHECK_INTERVAL: chrono::TimeDelta = chrono::TimeDelta::hours(24);
const BINARIES: [&str; 2] = ["malac-control", "malac"];
/// A lock older than this belongs to an update that died.
const STALE_LOCK: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// The binaries can be replaced in place.
    SelfUpdate,
    /// Installed by Homebrew; `brew upgrade` owns the files.
    Homebrew,
    /// The install directory is not writable, e.g. `/usr/local/bin` from the .pkg.
    ReadOnly,
    /// No release is built for this platform.
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub current: String,
    pub latest: Option<String>,
    /// A newer release exists and is not installed yet.
    pub available: bool,
    /// A newer version is already installed; restart malac to use it.
    pub installed: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
    pub method: Method,
    pub release_url: String,
    /// What to run when `method` is not `SelfUpdate`.
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    checked_at: Option<DateTime<Utc>>,
    latest: Option<String>,
    installed: Option<String>,
}

fn state_path() -> PathBuf {
    config::home_dir().join("update.json")
}

fn load_state() -> State {
    fs::read_to_string(state_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_state(state: &State) -> Result<()> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(state)?)?;
    Ok(())
}

fn releases_base() -> String {
    std::env::var("MALAC_RELEASES").unwrap_or_else(|_| RELEASES.into())
}

/// Status from the last check, without touching the network.
pub fn status() -> Status {
    let state = load_state();
    let method = method();
    let newer = |v: &Option<String>| {
        v.as_deref()
            .filter(|v| is_newer(v, CURRENT))
            .map(str::to_owned)
    };
    let installed = newer(&state.installed);
    let latest = state.latest.clone();
    let available =
        newer(&latest).is_some_and(|l| installed.as_deref().is_none_or(|i| is_newer(&l, i)));
    let hint = match method {
        Method::SelfUpdate => None,
        Method::Homebrew => Some("brew upgrade lobirus/malac/malac".into()),
        Method::ReadOnly => Some(format!(
            "download the latest installer from {}/latest, or run `sudo malac update`",
            releases_base()
        )),
        Method::Unsupported => Some("build from source: MALAC_FROM_SOURCE=1 install.sh".into()),
    };
    Status {
        current: CURRENT.into(),
        latest,
        available,
        installed,
        checked_at: state.checked_at,
        method,
        release_url: format!("{}/latest", releases_base()),
        hint,
    }
}

/// Asks GitHub for the latest release unless the last check is recent.
pub async fn check(force: bool) -> Result<Status> {
    let mut state = load_state();
    let fresh = state
        .checked_at
        .is_some_and(|t| Utc::now() - t < CHECK_INTERVAL);
    if force || !fresh {
        let sums = fetch_sums().await?;
        state.latest = sums.as_deref().and_then(latest_version);
        state.checked_at = Some(Utc::now());
        save_state(&state)?;
    }
    Ok(status())
}

/// Downloads the latest release for this machine and swaps the binaries in.
/// Returns the installed version.
pub async fn install() -> Result<String> {
    if method() != Method::SelfUpdate {
        bail!(
            status()
                .hint
                .unwrap_or_else(|| "this installation cannot update itself".into())
        );
    }
    let dir = install_dir()?;
    let _lock = Lock::acquire(&dir)?;
    let target = target_triple().context("no release is built for this platform")?;
    let sums = fetch_sums()
        .await?
        .context("no release has been published yet")?;
    let (archive, expected) =
        archive_for(&sums, target).context("the latest release has no build for this machine")?;
    let version = version_of(&archive).context("cannot read the version from the archive name")?;
    if !is_newer(&version, CURRENT) {
        bail!("malac {CURRENT} is already the latest version");
    }

    let url = format!("{}/latest/download/{archive}", releases_base());
    let bytes = crate::provider::http_client()
        .get(&url)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("download failed: {url}"))?
        .bytes()
        .await?;
    let actual = hex(&Sha256::digest(&bytes));
    if actual != expected {
        bail!("checksum mismatch for {archive}");
    }

    let staging = dir.join(format!(".malac-update-{}", std::process::id()));
    let result = unpack_and_swap(&bytes, &staging, &dir);
    let _ = fs::remove_dir_all(&staging);
    result?;

    let mut state = load_state();
    state.installed = Some(version.clone());
    state.latest = Some(version.clone());
    save_state(&state)?;
    Ok(version)
}

fn unpack_and_swap(archive: &[u8], staging: &Path, dir: &Path) -> Result<()> {
    fs::create_dir_all(staging)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if BINARIES.contains(&name) {
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            fs::write(staging.join(name), data)?;
        }
    }
    for name in BINARIES {
        let staged = staging.join(name);
        if !staged.exists() {
            bail!("the archive does not contain {name}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
        }
    }
    // malac-control first, so a malac that starts mid-swap finds a control
    // service at least as new as itself.
    for name in BINARIES {
        fs::rename(staging.join(name), dir.join(name))
            .with_context(|| format!("cannot replace {}", dir.join(name).display()))?;
    }
    Ok(())
}

pub fn method() -> Method {
    if target_triple().is_none() {
        return Method::Unsupported;
    }
    let Ok(dir) = install_dir() else {
        return Method::Unsupported;
    };
    let path = dir.to_string_lossy();
    if path.contains("/Cellar/") || path.contains("/homebrew/") || path.contains("/linuxbrew/") {
        return Method::Homebrew;
    }
    let probe = dir.join(format!(".malac-write-test-{}", std::process::id()));
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(probe);
            Method::SelfUpdate
        }
        Err(_) => Method::ReadOnly,
    }
}

fn install_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe()?.canonicalize()?;
    exe.parent()
        .map(Path::to_path_buf)
        .context("cannot find the install directory")
}

fn target_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        _ => None,
    }
}

/// `SHA256SUMS` of the latest release, or `None` when there is no release.
async fn fetch_sums() -> Result<Option<String>> {
    let url = format!("{}/latest/download/SHA256SUMS", releases_base());
    let resp = crate::provider::http_client()
        .get(&url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .context("cannot reach the release server")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    Ok(Some(resp.error_for_status()?.text().await?))
}

/// `(file name, sha256)` of the archive for `target`.
fn archive_for(sums: &str, target: &str) -> Option<(String, String)> {
    let suffix = format!("-{target}.tar.gz");
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        (name.starts_with("malac-") && name.ends_with(&suffix))
            .then(|| (name.to_owned(), hash.to_lowercase()))
    })
}

fn latest_version(sums: &str) -> Option<String> {
    let target = target_triple()?;
    archive_for(sums, target).and_then(|(name, _)| version_of(&name))
}

/// `malac-0.2.0-aarch64-apple-darwin.tar.gz` → `0.2.0`.
fn version_of(archive: &str) -> Option<String> {
    let rest = archive.strip_prefix("malac-")?;
    let version = rest.split('-').next()?;
    parse(version).map(|_| version.to_owned())
}

fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
    ))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Keeps two malac processes from updating the same directory at once.
struct Lock(PathBuf);

impl Lock {
    fn acquire(dir: &Path) -> Result<Self> {
        let path = dir.join(".malac-update.lock");
        let stale = fs::metadata(&path)
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().unwrap_or_default() > STALE_LOCK);
        if stale {
            let _ = fs::remove_file(&path);
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| anyhow::anyhow!("another malac is already updating"))?;
        Ok(Self(path))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v1.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0-rc1", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn finds_archives() {
        let sums = "abc123  malac-0.3.1-aarch64-apple-darwin.tar.gz\n\
                    def456  malac-0.3.1-x86_64-unknown-linux-gnu.tar.gz\n\
                    999999  malac-0.3.1-macos.pkg\n";
        assert_eq!(
            archive_for(sums, "x86_64-unknown-linux-gnu"),
            Some((
                "malac-0.3.1-x86_64-unknown-linux-gnu.tar.gz".into(),
                "def456".into()
            ))
        );
        assert_eq!(archive_for(sums, "x86_64-apple-darwin"), None);
        assert_eq!(
            version_of("malac-0.3.1-aarch64-apple-darwin.tar.gz").as_deref(),
            Some("0.3.1")
        );
    }

    #[test]
    fn swaps_binaries_from_an_archive() {
        let dir = tempfile::tempdir().unwrap();
        for name in BINARIES {
            fs::write(dir.path().join(name), b"old").unwrap();
        }
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for name in BINARIES {
            let mut header = tar::Header::new_gnu();
            header.set_size(3);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("malac-9.9.9-x/{name}"), &b"new"[..])
                .unwrap();
        }
        let archive = builder.into_inner().unwrap().finish().unwrap();
        let staging = dir.path().join(".staging");
        unpack_and_swap(&archive, &staging, dir.path()).unwrap();
        for name in BINARIES {
            assert_eq!(fs::read(dir.path().join(name)).unwrap(), b"new");
        }
    }
}
