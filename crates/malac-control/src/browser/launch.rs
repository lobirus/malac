//! Finding and launching Chromium-based browsers, and locating the DevTools
//! endpoint of one that is already running.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::Instant;

/// Chrome writes the port it picked for `--remote-debugging-port=0` here.
const PORT_FILE: &str = "DevToolsActivePort";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Launched {
    pub child: Child,
    pub ws_url: String,
}

pub fn find_executable(configured: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = configured {
        if path.is_file() {
            return Ok(path.to_owned());
        }
        bail!("browser.executable {} does not exist", path.display());
    }
    candidates()
        .into_iter()
        .find(|path| path.is_file())
        .context("no Chrome, Chromium, Brave or Edge installation found; set browser.executable in ~/.malac/control.toml")
}

#[cfg(target_os = "macos")]
fn candidates() -> Vec<PathBuf> {
    const APPS: &[&str] = &[
        "Google Chrome.app/Contents/MacOS/Google Chrome",
        "Chromium.app/Contents/MacOS/Chromium",
        "Brave Browser.app/Contents/MacOS/Brave Browser",
        "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ];
    let mut roots = vec![PathBuf::from("/Applications")];
    roots.extend(dirs::home_dir().map(|home| home.join("Applications")));
    APPS.iter()
        .flat_map(|app| roots.iter().map(move |root| root.join(app)))
        .collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn candidates() -> Vec<PathBuf> {
    const NAMES: &[&str] = &[
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "brave-browser",
        "microsoft-edge",
        "microsoft-edge-stable",
    ];
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    NAMES
        .iter()
        .flat_map(|name| dirs.iter().map(move |dir| dir.join(name)))
        .collect()
}

#[cfg(windows)]
fn candidates() -> Vec<PathBuf> {
    const EXES: &[&str] = &[
        r"Google\Chrome\Application\chrome.exe",
        r"Chromium\Application\chrome.exe",
        r"BraveSoftware\Brave-Browser\Application\brave.exe",
        r"Microsoft\Edge\Application\msedge.exe",
    ];
    let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    EXES.iter()
        .flat_map(|exe| roots.iter().map(move |root| root.join(exe)))
        .collect()
}

/// Starts the browser on `user_data_dir` and waits for its DevTools endpoint.
pub async fn launch(executable: &Path, user_data_dir: &Path, headless: bool) -> Result<Launched> {
    let _ = std::fs::remove_file(user_data_dir.join(PORT_FILE));
    let mut command = Command::new(executable);
    command
        .arg(format!("--user-data-dir={}", user_data_dir.display()))
        .args([
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
        ]);
    if headless {
        command.args(["--headless=new", "--window-size=1280,900"]);
    }
    command
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start {}", executable.display()))?;
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "browser", "{line}");
            }
        });
    }

    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Some(ws_url) = read_port_file(user_data_dir) {
            return Ok(Launched { child, ws_url });
        }
        if let Some(status) = child.try_wait()? {
            bail!(
                "{} exited ({status}) before opening its DevTools port; another browser may already be using {}",
                executable.display(),
                user_data_dir.display()
            );
        }
        if Instant::now() >= deadline {
            let _ = child.kill().await;
            bail!(
                "{} did not open its DevTools port within {}s",
                executable.display(),
                STARTUP_TIMEOUT.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The browser WebSocket URL a running browser advertises in its user data dir.
pub fn read_port_file(user_data_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(user_data_dir.join(PORT_FILE)).ok()?;
    let mut lines = text.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.trim();
    path.starts_with('/')
        .then(|| format!("ws://127.0.0.1:{port}{path}"))
}

pub fn remove_port_file(user_data_dir: &Path) {
    let _ = std::fs::remove_file(user_data_dir.join(PORT_FILE));
}

/// Resolves an `http://host:port` DevTools address to the browser's WebSocket
/// URL. WebSocket URLs are returned unchanged.
pub async fn resolve_cdp_url(cdp_url: &str) -> Result<String> {
    if cdp_url.starts_with("ws://") || cdp_url.starts_with("wss://") {
        return Ok(cdp_url.to_owned());
    }
    let version_url = format!("{}/json/version", cdp_url.trim_end_matches('/'));
    let info: Value = reqwest::Client::new()
        .get(&version_url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("no browser is serving DevTools at {cdp_url}"))?
        .json()
        .await
        .with_context(|| format!("unexpected reply from {version_url}"))?;
    info["webSocketDebuggerUrl"]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("{version_url} did not report a webSocketDebuggerUrl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_devtools_port_file() {
        let dir = std::env::temp_dir().join(format!("malac-control-port-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_port_file(&dir), None);
        std::fs::write(dir.join(PORT_FILE), "9333\n").unwrap();
        assert_eq!(read_port_file(&dir), None);
        std::fs::write(dir.join(PORT_FILE), "9333\n/devtools/browser/abc-123\n").unwrap();
        assert_eq!(
            read_port_file(&dir).as_deref(),
            Some("ws://127.0.0.1:9333/devtools/browser/abc-123")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
