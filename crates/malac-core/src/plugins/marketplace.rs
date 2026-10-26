//! Marketplaces: catalogs of plugins and where to fetch each one. malac
//! reads Claude Code's `.claude-plugin/marketplace.json` and the same shape
//! where other agents keep it (`.agents/plugins/`, `.github/plugin/`,
//! `.cursor-plugin/`). A folder or repository without a catalog is one
//! too: a plugin on its own, or the plugins under its `plugins/`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;

/// Where a catalog may be, first match wins.
const LOCATIONS: &[&str] = &[
    ".claude-plugin/marketplace.json",
    "marketplace.json",
    ".agents/plugins/marketplace.json",
    ".github/plugin/marketplace.json",
    ".cursor-plugin/marketplace.json",
];
/// Names malac uses itself.
pub const RESERVED: &[&str] = &["malac", "builtin", "inline"];
/// What makes a directory a plugin.
const PLUGIN_MARKERS: &[&str] = &[
    "plugin.json",
    ".claude-plugin/plugin.json",
    "skills",
    "SKILL.md",
    "commands",
    "agents",
    "hooks/hooks.json",
    ".mcp.json",
    "mcp.json",
];

#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub plugins: Vec<Entry>,
    /// Entries that could not be read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub source: EntrySource,
    /// The entry as written: it may declare components or stand in for a manifest.
    #[serde(skip)]
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum EntrySource {
    /// A directory inside the marketplace.
    Path { path: String },
    /// A git repository, optionally one directory of it.
    Git {
        url: String,
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        git_ref: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        subdir: Option<String>,
    },
    /// npm, archive and command sources.
    Unsupported { kind: String },
}

pub fn valid_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        && !name.contains("..")
        && name.len() <= 128
}

/// The catalog file inside `root`, if there is one.
pub fn find(root: &Path) -> Option<PathBuf> {
    LOCATIONS
        .iter()
        .map(|l| root.join(l))
        .find(|p| p.is_file() && has_plugins_array(p))
}

fn has_plugins_array(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .is_some_and(|v| v.get("plugins").is_some_and(Value::is_array))
}

/// The catalog of a marketplace directory: its `marketplace.json`, or one
/// made from the plugins it holds, named `name_hint`.
pub fn read_dir(root: &Path, name_hint: &str) -> Result<Catalog> {
    if let Some(file) = find(root) {
        let text = std::fs::read_to_string(&file)?;
        return parse(&text, name_hint).with_context(|| format!("in {}", file.display()));
    }
    implicit(root, name_hint)
}

pub fn is_plugin_dir(dir: &Path) -> bool {
    PLUGIN_MARKERS.iter().any(|m| dir.join(m).exists())
}

/// A folder without a catalog: one plugin, or the plugins in `plugins/`
/// (or at the top level).
fn implicit(root: &Path, name_hint: &str) -> Result<Catalog> {
    let entry = |name: String, path: String| Entry {
        source: EntrySource::Path { path },
        name,
        description: None,
        version: None,
        category: None,
        raw: Value::Null,
    };
    let mut plugins = Vec::new();
    if is_plugin_dir(root) {
        let name = super::manifest::parse(root, root, None)
            .map(|p| p.meta.name)
            .unwrap_or_else(|_| name_hint.to_owned());
        plugins.push(entry(name, ".".into()));
    } else {
        for parent in [root.join("plugins"), root.to_path_buf()] {
            let mut dirs: Vec<PathBuf> = std::fs::read_dir(&parent)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() && is_plugin_dir(p))
                .collect();
            dirs.sort();
            for dir in dirs {
                let rel = dir.strip_prefix(root).unwrap_or(&dir).display().to_string();
                let name = super::manifest::parse(&dir, &dir, None)
                    .map(|p| p.meta.name)
                    .unwrap_or_else(|_| rel.clone());
                plugins.push(entry(name, format!("./{rel}")));
            }
            if !plugins.is_empty() {
                break;
            }
        }
    }
    if plugins.is_empty() {
        bail!("{} has no marketplace.json and no plugins", root.display());
    }
    Ok(Catalog {
        name: name_hint.to_owned(),
        description: None,
        owner: None,
        plugins,
        warnings: Vec::new(),
    })
}

/// Parses a `marketplace.json`. Invalid entries become warnings.
pub fn parse(text: &str, name_hint: &str) -> Result<Catalog> {
    let json: Value = serde_json::from_str(text).context("not valid JSON")?;
    let plugins = json
        .get("plugins")
        .and_then(Value::as_array)
        .context("no `plugins` array")?;
    let name = json
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(name_hint)
        .to_owned();
    if !valid_name(&name) {
        bail!("invalid marketplace name `{name}`");
    }
    let plugin_root = json["metadata"]["pluginRoot"].as_str();
    let mut catalog = Catalog {
        description: json["description"]
            .as_str()
            .or(json["metadata"]["description"].as_str())
            .map(str::to_owned),
        owner: json["owner"]["name"].as_str().map(str::to_owned),
        name,
        plugins: Vec::new(),
        warnings: Vec::new(),
    };
    for (i, raw) in plugins.iter().enumerate() {
        let Some(name) = raw
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| valid_name(n))
        else {
            catalog
                .warnings
                .push(format!("plugins[{i}]: missing or invalid name"));
            continue;
        };
        if catalog.plugins.iter().any(|p| p.name == name) {
            catalog
                .warnings
                .push(format!("plugins[{i}]: duplicate name `{name}`"));
            continue;
        }
        let source = match raw.get("source").map(|s| entry_source(s, plugin_root)) {
            Some(Ok(source)) => source,
            Some(Err(err)) => {
                catalog.warnings.push(format!("plugin `{name}`: {err:#}"));
                continue;
            }
            None => {
                catalog.warnings.push(format!("plugin `{name}`: no source"));
                continue;
            }
        };
        let text = |key: &str| raw.get(key).and_then(Value::as_str).map(str::to_owned);
        catalog.plugins.push(Entry {
            name: name.to_owned(),
            description: text("description"),
            version: text("version"),
            category: text("category"),
            source,
            raw: raw.clone(),
        });
    }
    Ok(catalog)
}

fn entry_source(value: &Value, plugin_root: Option<&str>) -> Result<EntrySource> {
    if let Some(path) = value.as_str() {
        let path = match (path.starts_with("./") || path == ".", plugin_root) {
            (false, Some(root)) if !path.contains('/') => {
                format!("{}/{path}", root.trim_end_matches('/'))
            }
            _ => path.to_owned(),
        };
        if Path::new(&path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            bail!("source `{path}` leaves the marketplace");
        }
        if Path::new(&path).is_absolute() {
            bail!("source `{path}` must be relative to the marketplace");
        }
        return Ok(EntrySource::Path { path });
    }
    let kind = value
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let field = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let github = |repo: &str| format!("https://github.com/{repo}.git");
    let looks_like_shorthand =
        |url: &str| !url.contains("://") && !url.starts_with("git@") && url.split('/').count() == 2;
    match kind {
        "github" => {
            let repo = field("repo").context("a github source needs `repo`")?;
            Ok(EntrySource::Git {
                url: github(&repo),
                git_ref: field("ref"),
                sha: field("sha"),
                subdir: field("path"),
            })
        }
        "url" | "git" | "git-subdir" => {
            let url = field("url").context("a git source needs `url`")?;
            let url = if looks_like_shorthand(&url) {
                github(&url)
            } else {
                url
            };
            Ok(EntrySource::Git {
                url,
                git_ref: field("ref"),
                sha: field("sha"),
                subdir: field("path"),
            })
        }
        other => Ok(EntrySource::Unsupported {
            kind: if other.is_empty() {
                "unknown".into()
            } else {
                other.to_owned()
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_code_catalogs() {
        let text = r#"{
          "name": "team-tools", "owner": {"name": "Ana"}, "metadata": {"pluginRoot": "./plugins", "description": "Ours"},
          "plugins": [
            {"name": "formatter", "source": "formatter", "description": "Formats"},
            {"name": "local", "source": "./tools/local", "version": "1.0.0"},
            {"name": "remote", "source": {"source": "github", "repo": "org/remote", "ref": "v2", "sha": "abc"}},
            {"name": "mono", "source": {"source": "git-subdir", "url": "org/mono", "path": "tools/x"}},
            {"name": "lab", "source": {"source": "url", "url": "https://gitlab.com/g/lab.git"}},
            {"name": "pkg", "source": {"source": "npm", "package": "@org/pkg"}},
            {"name": "escape", "source": "./../outside"},
            {"name": "local", "source": "./again"},
            {"source": "./nameless"}
          ]
        }"#;
        let catalog = parse(text, "hint").unwrap();
        assert_eq!(catalog.name, "team-tools");
        assert_eq!(catalog.description.as_deref(), Some("Ours"));
        assert_eq!(catalog.owner.as_deref(), Some("Ana"));
        let sources: Vec<&EntrySource> = catalog.plugins.iter().map(|p| &p.source).collect();
        assert_eq!(
            sources[0],
            &EntrySource::Path {
                path: "./plugins/formatter".into()
            }
        );
        assert_eq!(
            sources[1],
            &EntrySource::Path {
                path: "./tools/local".into()
            }
        );
        assert_eq!(
            sources[2],
            &EntrySource::Git {
                url: "https://github.com/org/remote.git".into(),
                git_ref: Some("v2".into()),
                sha: Some("abc".into()),
                subdir: None,
            }
        );
        assert!(
            matches!(sources[3], EntrySource::Git { url, subdir: Some(dir), .. } if url == "https://github.com/org/mono.git" && dir == "tools/x")
        );
        assert!(
            matches!(sources[4], EntrySource::Git { url, .. } if url == "https://gitlab.com/g/lab.git")
        );
        assert_eq!(sources[5], &EntrySource::Unsupported { kind: "npm".into() });
        assert_eq!(catalog.plugins.len(), 6);
        assert_eq!(catalog.warnings.len(), 3);
    }

    #[test]
    fn folders_without_a_catalog_are_marketplaces() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("plugins/one/skills/a")).unwrap();
        std::fs::write(
            root.join("plugins/one/skills/a/SKILL.md"),
            "---\nname: a\ndescription: A\n---\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("plugins/two")).unwrap();
        std::fs::write(
            root.join("plugins/two/plugin.json"),
            r#"{"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json", "name": "second"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        // A catalog without plugins (an identity file) is not a catalog.
        std::fs::write(root.join("marketplace.json"), r#"{"name": "x"}"#).unwrap();

        let catalog = read_dir(root, "collection").unwrap();
        assert_eq!(catalog.name, "collection");
        let names: Vec<&str> = catalog.plugins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["one", "second"]);
        assert_eq!(
            catalog.plugins[1].source,
            EntrySource::Path {
                path: "./plugins/two".into()
            }
        );

        let single = read_dir(&root.join("plugins/one"), "one").unwrap();
        assert_eq!(
            single.plugins[0].source,
            EntrySource::Path { path: ".".into() }
        );
        assert!(read_dir(&root.join("docs"), "docs").is_err());
    }
}
