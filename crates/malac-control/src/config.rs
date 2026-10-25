//! `control.toml` in the malac home directory (`$MALAC_HOME`, by default
//! `~/.malac`). Every key is optional.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The name of the managed profile that exists even when not configured.
pub const DEFAULT_PROFILE: &str = "default";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub computer: ComputerConfig,
    pub browser: BrowserConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ComputerConfig {
    pub enabled: bool,
    /// Screenshots wider than this are scaled down before they are returned.
    pub max_screenshot_width: u32,
}

impl Default for ComputerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_screenshot_width: 1366,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserConfig {
    pub enabled: bool,
    /// Chrome, Chromium, Brave or Edge binary. Detected when unset.
    pub executable: Option<PathBuf>,
    pub headless: bool,
    pub profiles: BTreeMap<String, ProfileConfig>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            executable: None,
            headless: false,
            profiles: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProfileConfig {
    /// System browser profile (directory name, display name or path) copied
    /// into the managed profile on its first launch.
    pub clone_from: Option<String>,
    /// DevTools endpoint of an already running browser to attach to.
    pub cdp_url: Option<String>,
}

impl Config {
    /// `control.toml` in the malac home directory.
    pub fn default_path() -> Result<PathBuf> {
        Ok(malac_dir()?.join("control.toml"))
    }

    /// Loads the default config file, or the defaults when it does not exist.
    pub fn load() -> Result<Self> {
        let path = Self::default_path()?;
        if path.exists() {
            Self::from_file(&path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid config {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Self = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.computer.max_screenshot_width < 320 {
            bail!("computer.max_screenshot_width must be at least 320");
        }
        for (name, profile) in &self.browser.profiles {
            validate_profile_name(name)?;
            if profile.clone_from.is_some() && profile.cdp_url.is_some() {
                bail!("browser profile `{name}`: `clone_from` and `cdp_url` cannot be combined");
            }
        }
        Ok(())
    }
}

/// `$MALAC_HOME`, or `~/.malac`: where malac keeps its state.
pub fn malac_dir() -> Result<PathBuf> {
    match std::env::var_os("MALAC_HOME").filter(|dir| !dir.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => dirs::home_dir()
            .map(|home| home.join(".malac"))
            .context("cannot determine the home directory"),
    }
}

/// Profile names become directory names, so keep them to a safe alphabet.
pub fn validate_profile_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        bail!("invalid browser profile name `{name}`: use letters, digits, `-` and `_`");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_defaults() {
        let config = Config::parse("").unwrap();
        assert!(config.computer.enabled);
        assert_eq!(config.computer.max_screenshot_width, 1366);
        assert!(config.browser.enabled);
        assert!(!config.browser.headless);
        assert!(config.browser.profiles.is_empty());
    }

    #[test]
    fn parses_full_config() {
        let config = Config::parse(
            r#"
            [computer]
            enabled = false
            max_screenshot_width = 1024

            [browser]
            executable = "/opt/chrome"
            headless = true

            [browser.profiles.work]
            [browser.profiles.personal]
            clone_from = "Profile 1"
            [browser.profiles.debug]
            cdp_url = "http://127.0.0.1:9222"
            "#,
        )
        .unwrap();
        assert!(!config.computer.enabled);
        assert_eq!(config.computer.max_screenshot_width, 1024);
        assert_eq!(
            config.browser.executable,
            Some(PathBuf::from("/opt/chrome"))
        );
        assert!(config.browser.headless);
        let profiles = &config.browser.profiles;
        assert_eq!(profiles["work"], ProfileConfig::default());
        assert_eq!(
            profiles["personal"].clone_from.as_deref(),
            Some("Profile 1")
        );
        assert_eq!(
            profiles["debug"].cdp_url.as_deref(),
            Some("http://127.0.0.1:9222")
        );
    }

    #[test]
    fn rejects_unknown_keys() {
        let err = Config::parse("[browser]\nheadles = true").unwrap_err();
        assert!(format!("{err:#}").contains("headles"));
    }

    #[test]
    fn rejects_conflicting_profile_sources() {
        let err = Config::parse(
            "[browser.profiles.x]\nclone_from = \"Default\"\ncdp_url = \"http://localhost:9222\"",
        )
        .unwrap_err();
        assert!(err.to_string().contains("cannot be combined"));
    }

    #[test]
    fn rejects_unsafe_profile_names() {
        assert!(Config::parse("[browser.profiles.\"../x\"]").is_err());
        assert!(Config::parse("[browser.profiles.\"a b\"]").is_err());
        assert!(Config::parse("[browser.profiles.ok-name_2]").is_ok());
    }
}
