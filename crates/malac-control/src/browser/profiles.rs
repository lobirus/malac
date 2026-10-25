//! Browser profiles: managed user data dirs under `~/.malac/browser`,
//! optionally seeded from a system browser profile, or attached browsers.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::config::{BrowserConfig, DEFAULT_PROFILE, ProfileConfig, malac_dir};

/// Paths, relative to a profile directory, that only hold caches.
const CACHE_DIRS: &[&str] = &[
    "Cache",
    "Code Cache",
    "GPUCache",
    "DawnCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "GrShaderCache",
    "GraphiteDawnCache",
    "ShaderCache",
    "Media Cache",
    "Application Cache",
    "Service Worker/CacheStorage",
    "Service Worker/ScriptCache",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A user data dir owned by malac-control.
    Managed {
        dir: PathBuf,
        clone_from: Option<String>,
    },
    /// A browser someone else started with remote debugging enabled.
    Attach { cdp_url: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub source: Source,
}

pub fn resolve(config: &BrowserConfig, name: &str) -> Result<Profile> {
    let profile = match config.profiles.get(name) {
        Some(profile) => profile.clone(),
        None if name == DEFAULT_PROFILE => ProfileConfig::default(),
        None => bail!(
            "unknown browser profile `{name}`; available: {}",
            names(config).join(", ")
        ),
    };
    let source = match profile.cdp_url {
        Some(cdp_url) => Source::Attach { cdp_url },
        None => Source::Managed {
            dir: malac_dir()?.join("browser").join(name),
            clone_from: profile.clone_from,
        },
    };
    Ok(Profile {
        name: name.to_owned(),
        source,
    })
}

/// Configured profile names, with the implicit `default` first.
pub fn names(config: &BrowserConfig) -> Vec<String> {
    let mut names = vec![DEFAULT_PROFILE.to_owned()];
    names.extend(
        config
            .profiles
            .keys()
            .filter(|name| *name != DEFAULT_PROFILE)
            .cloned(),
    );
    names
}

/// A profile of a browser installed on the system, usable as `clone_from`.
#[derive(Debug, Clone)]
pub struct SystemProfile {
    pub browser: &'static str,
    /// Directory name inside the browser's user data dir, e.g. `Profile 1`.
    pub id: String,
    /// The name shown in the browser's profile picker.
    pub name: String,
    pub dir: PathBuf,
}

pub fn system_profiles() -> Vec<SystemProfile> {
    let mut found = Vec::new();
    for (browser, root) in user_data_roots() {
        let Ok(text) = fs::read_to_string(root.join("Local State")) else {
            continue;
        };
        let Ok(state) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(cache) = state["profile"]["info_cache"].as_object() else {
            continue;
        };
        let mut profiles: Vec<SystemProfile> = cache
            .iter()
            .map(|(id, info)| SystemProfile {
                browser,
                id: id.clone(),
                name: info["name"].as_str().unwrap_or(id).to_owned(),
                dir: root.join(id),
            })
            .filter(|profile| profile.dir.is_dir())
            .collect();
        profiles.sort_by(|a, b| a.id.cmp(&b.id));
        found.extend(profiles);
    }
    found
}

fn user_data_roots() -> Vec<(&'static str, PathBuf)> {
    #[cfg(target_os = "macos")]
    let (base, roots) = (
        dirs::data_dir(),
        [
            ("Google Chrome", "Google/Chrome"),
            ("Chromium", "Chromium"),
            ("Brave", "BraveSoftware/Brave-Browser"),
            ("Microsoft Edge", "Microsoft Edge"),
        ],
    );
    #[cfg(all(unix, not(target_os = "macos")))]
    let (base, roots) = (
        dirs::config_dir(),
        [
            ("Google Chrome", "google-chrome"),
            ("Chromium", "chromium"),
            ("Brave", "BraveSoftware/Brave-Browser"),
            ("Microsoft Edge", "microsoft-edge"),
        ],
    );
    #[cfg(windows)]
    let (base, roots) = (
        dirs::data_local_dir(),
        [
            ("Google Chrome", r"Google\Chrome\User Data"),
            ("Chromium", r"Chromium\User Data"),
            ("Brave", r"BraveSoftware\Brave-Browser\User Data"),
            ("Microsoft Edge", r"Microsoft\Edge\User Data"),
        ],
    );
    let Some(base) = base else {
        return Vec::new();
    };
    roots
        .into_iter()
        .map(|(browser, rel)| (browser, base.join(rel)))
        .collect()
}

/// Creates a managed user data dir. On first use with `clone_from`, the
/// system profile is copied in as its `Default` profile. Returns a note
/// describing the copy.
pub fn prepare(dir: &Path, clone_from: Option<&str>) -> Result<Option<String>> {
    if dir.exists() {
        return Ok(None);
    }
    let Some(spec) = clone_from else {
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
        return Ok(None);
    };
    let source = clone_source(spec)?;
    // Copy into a staging dir so an interrupted copy is retried next time.
    let staging = dir.with_extension("partial");
    let _ = fs::remove_dir_all(&staging);
    let bytes = copy_profile(&source, &staging.join("Default"))
        .with_context(|| format!("failed to copy {}", source.display()))?;
    fs::rename(&staging, dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(Some(format!(
        "Copied {} ({}, caches skipped) into {}.",
        source.display(),
        human_size(bytes),
        dir.display()
    )))
}

fn human_size(bytes: u64) -> String {
    match bytes {
        0..1_000_000 => format!("{} KB", bytes.div_ceil(1000)),
        _ => format!("{} MB", bytes / 1_000_000),
    }
}

fn clone_source(spec: &str) -> Result<PathBuf> {
    let path = Path::new(spec);
    if path.is_absolute() {
        if path.is_dir() {
            return Ok(path.to_owned());
        }
        bail!("clone_from directory {spec} does not exist");
    }
    let profiles = system_profiles();
    profiles
        .iter()
        .find(|p| p.id == spec)
        .or_else(|| profiles.iter().find(|p| p.name.eq_ignore_ascii_case(spec)))
        .map(|p| p.dir.clone())
        .with_context(|| {
            let available: Vec<String> = profiles
                .iter()
                .map(|p| format!("{} \"{}\" ({})", p.browser, p.id, p.name))
                .collect();
            if available.is_empty() {
                format!("no system browser profile `{spec}`: no browser profiles were found")
            } else {
                format!(
                    "no system browser profile `{spec}`; available: {}",
                    available.join(", ")
                )
            }
        })
}

/// Copies a browser profile directory, skipping caches, symlinks and sockets.
/// Returns the number of bytes copied.
pub fn copy_profile(src: &Path, dst: &Path) -> Result<u64> {
    copy_tree(src, dst, Path::new(""))
}

fn copy_tree(src: &Path, dst: &Path, rel: &Path) -> Result<u64> {
    fs::create_dir_all(dst).with_context(|| format!("failed to create {}", dst.display()))?;
    let mut bytes = 0;
    for entry in fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))? {
        let entry = entry?;
        let rel = rel.join(entry.file_name());
        if CACHE_DIRS.iter().any(|cache| rel == Path::new(cache)) {
            continue;
        }
        let file_type = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            bytes += copy_tree(&entry.path(), &target, &rel)?;
        } else if file_type.is_file() {
            bytes += fs::copy(entry.path(), &target)
                .with_context(|| format!("failed to copy {}", entry.path().display()))?;
        }
    }
    Ok(bytes)
}

/// A listing of configured and system profiles; `running` describes the
/// profiles that are currently connected.
pub fn report(config: &BrowserConfig, running: &BTreeMap<String, String>) -> String {
    let mut out = String::from("Profiles:\n");
    for name in names(config) {
        let line = match resolve(config, &name) {
            Ok(Profile {
                source: Source::Managed { dir, clone_from },
                ..
            }) => match clone_from {
                Some(spec) => format!(
                    "managed, {} (cloned from \"{spec}\" on first launch)",
                    dir.display()
                ),
                None => format!("managed, {}", dir.display()),
            },
            Ok(Profile {
                source: Source::Attach { cdp_url },
                ..
            }) => format!("attaches to {cdp_url}"),
            Err(err) => format!("{err:#}"),
        };
        out.push_str(&format!("- {name}: {line}"));
        if let Some(status) = running.get(&name) {
            out.push_str(&format!(" [{status}]"));
        }
        out.push('\n');
    }
    let system = system_profiles();
    if system.is_empty() {
        out.push_str("\nNo system browser profiles found.\n");
    } else {
        out.push_str("\nSystem browser profiles (usable as clone_from):\n");
        for profile in system {
            out.push_str(&format!(
                "- {} \"{}\" ({})\n",
                profile.browser, profile.id, profile.name
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("malac-control-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn copy_skips_caches() {
        let tmp = TempDir::new("copy");
        let src = tmp.0.join("src");
        write(&src.join("Cookies"), "cookies");
        write(&src.join("Preferences"), "{}");
        write(&src.join("Local Storage/leveldb/000003.log"), "data");
        write(&src.join("Service Worker/Database/MANIFEST"), "sw");
        write(&src.join("Service Worker/CacheStorage/abc/index"), "cached");
        write(&src.join("Cache/Cache_Data/f_000001"), "cached");
        write(&src.join("Code Cache/js/index"), "cached");
        write(&src.join("GPUCache/data_0"), "cached");
        write(
            &src.join("Extensions/id/1.0/Cache/kept"),
            "nested caches are kept",
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink("/nonexistent", src.join("SingletonLock")).unwrap();

        let dst = tmp.0.join("dst");
        let bytes = copy_profile(&src, &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("Cookies")).unwrap(), "cookies");
        assert!(dst.join("Local Storage/leveldb/000003.log").is_file());
        assert!(dst.join("Service Worker/Database/MANIFEST").is_file());
        assert!(dst.join("Extensions/id/1.0/Cache/kept").is_file());
        for skipped in [
            "Cache",
            "Code Cache",
            "GPUCache",
            "Service Worker/CacheStorage",
            "SingletonLock",
        ] {
            assert!(!dst.join(skipped).exists(), "{skipped} was copied");
        }
        let expected = ["cookies", "{}", "data", "sw", "nested caches are kept"]
            .iter()
            .map(|s| s.len() as u64)
            .sum::<u64>();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn prepare_clones_once_from_a_directory() {
        let tmp = TempDir::new("prepare");
        let source = tmp.0.join("Profile 1");
        write(&source.join("Bookmarks"), "{}");
        write(&source.join("Cache/x"), "cached");
        let dir = tmp.0.join("managed").join("work");

        let note = prepare(&dir, Some(source.to_str().unwrap())).unwrap();
        assert!(note.unwrap().contains("Copied"));
        assert!(dir.join("Default/Bookmarks").is_file());
        assert!(!dir.join("Default/Cache").exists());
        assert!(!dir.with_extension("partial").exists());

        fs::write(source.join("Bookmarks"), "changed").unwrap();
        assert_eq!(prepare(&dir, Some(source.to_str().unwrap())).unwrap(), None);
        assert_eq!(
            fs::read_to_string(dir.join("Default/Bookmarks")).unwrap(),
            "{}"
        );
    }

    #[test]
    fn prepare_without_clone_creates_empty_dir() {
        let tmp = TempDir::new("empty");
        let dir = tmp.0.join("default");
        assert_eq!(prepare(&dir, None).unwrap(), None);
        assert!(dir.is_dir());
    }

    #[test]
    fn missing_clone_source_is_an_error() {
        let tmp = TempDir::new("missing");
        let dir = tmp.0.join("x");
        let missing = tmp.0.join("nope");
        assert!(prepare(&dir, Some(missing.to_str().unwrap())).is_err());
        assert!(!dir.exists());
    }

    #[test]
    fn resolves_profiles() {
        let config = crate::config::Config::parse(
            "[browser.profiles.debug]\ncdp_url = \"http://127.0.0.1:9222\"\n[browser.profiles.work]",
        )
        .unwrap()
        .browser;
        assert_eq!(names(&config), ["default", "debug", "work"]);
        let default = resolve(&config, "default").unwrap();
        assert!(matches!(
            default.source,
            Source::Managed { ref dir, clone_from: None } if dir.ends_with("browser/default")
        ));
        assert_eq!(
            resolve(&config, "debug").unwrap().source,
            Source::Attach {
                cdp_url: "http://127.0.0.1:9222".into()
            }
        );
        let err = resolve(&config, "nope").unwrap_err().to_string();
        assert!(err.contains("default, debug, work"), "{err}");
    }
}
