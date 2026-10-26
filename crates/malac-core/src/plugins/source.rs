//! Where marketplaces and plugins come from, and fetching them. Git
//! repositories are cloned with the `git` command, so SSH keys, credential
//! helpers and private repositories work as they do in a terminal.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// A source as the user writes it: a folder, a git repository (URL,
/// `owner/repo` for GitHub, `#ref` or `@ref` to pin a branch or tag), or
/// the URL of a `marketplace.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local(PathBuf),
    Git {
        url: String,
        git_ref: Option<String>,
        /// `owner/repo` when given as GitHub shorthand, to retry over SSH.
        github: Option<String>,
    },
    /// A `marketplace.json` served over HTTP(S).
    Url(String),
}

impl Source {
    /// Parses `spec`; relative folders resolve against `base`.
    pub fn parse(spec: &str, base: &Path) -> Result<Self> {
        let spec = spec.trim();
        if spec.is_empty() {
            bail!("empty source");
        }
        if let Some(rest) = spec.strip_prefix("~/") {
            let home = dirs::home_dir().context("no home directory")?;
            return Ok(Source::Local(home.join(rest)));
        }
        if spec.starts_with(['.', '/']) || (cfg!(windows) && spec.chars().nth(1) == Some(':')) {
            return Ok(Source::Local(base.join(spec)));
        }
        let (body, git_ref) = split_ref(spec);
        if body.starts_with("git@")
            || body.starts_with("ssh://")
            || body.starts_with("git://")
            || body.starts_with("file://")
        {
            return Ok(Source::Git {
                url: body.to_owned(),
                git_ref,
                github: None,
            });
        }
        if body.starts_with("http://") || body.starts_with("https://") {
            if is_git_url(body) {
                return Ok(Source::Git {
                    url: body.to_owned(),
                    git_ref,
                    github: None,
                });
            }
            return Ok(Source::Url(spec.to_owned()));
        }
        if let Some(repo) = body
            .strip_prefix("github:")
            .or(Some(body))
            .filter(|b| is_shorthand(b))
        {
            if !base.join(spec).exists() {
                return Ok(Source::Git {
                    url: format!("https://github.com/{repo}.git"),
                    git_ref,
                    github: Some(repo.to_owned()),
                });
            }
        }
        if base.join(spec).exists() {
            return Ok(Source::Local(base.join(spec)));
        }
        bail!(
            "cannot tell what `{spec}` is: use a folder (./dir), owner/repo for GitHub, a git URL (https://… or git@…), or the URL of a marketplace.json"
        )
    }

    /// A name for the source when it has none of its own: the repository or folder name.
    pub fn name_hint(&self) -> String {
        let text = match self {
            Source::Local(path) => path
                .canonicalize()
                .unwrap_or_else(|_| path.clone())
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Source::Git { url, .. } => url
                .trim_end_matches('/')
                .trim_end_matches(".git")
                .to_owned(),
            Source::Url(url) => url.trim_end_matches("/marketplace.json").to_owned(),
        };
        let last = text
            .rsplit(['/', ':'])
            .next()
            .unwrap_or(&text)
            .trim_end_matches(".git");
        sanitize(last)
    }
}

/// `owner/repo#ref` or `owner/repo@ref`; an `@` before a `:` belongs to `git@host:`.
fn split_ref(spec: &str) -> (&str, Option<String>) {
    if let Some((body, r)) = spec.rsplit_once('#') {
        return (body, Some(r.to_owned()).filter(|r| !r.is_empty()));
    }
    if let Some((body, r)) = spec.rsplit_once('@') {
        let in_user_part = body.is_empty() || r.contains(':') || r.contains('/');
        if !in_user_part {
            return (body, Some(r.to_owned()));
        }
    }
    (spec, None)
}

fn is_shorthand(text: &str) -> bool {
    let mut parts = text.split('/');
    let valid = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None) if valid(a) && valid(b))
}

/// Git hosting URLs, as opposed to a link to a JSON file.
fn is_git_url(url: &str) -> bool {
    let path = url.split('?').next().unwrap_or(url);
    if path.ends_with(".json") {
        return false;
    }
    let host = path
        .split("://")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or_default();
    path.ends_with(".git")
        || path.contains("/_git/")
        || matches!(
            host,
            "github.com" | "gitlab.com" | "bitbucket.org" | "codeberg.org"
        )
        || host.starts_with("gitlab.")
}

/// Letters, digits, `.`, `_` and `-`, starting with a letter or digit.
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    cleaned.trim_start_matches(['-', '_', '.']).to_owned()
}

fn git(dir: Option<&Path>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    // Fail instead of waiting for a password nobody will type; credential
    // helpers and SSH agents still work.
    command.env("GIT_TERMINAL_PROMPT", "0");
    if !std::io::stdin().is_terminal() && std::env::var_os("GIT_SSH_COMMAND").is_none() {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    command.stdin(std::process::Stdio::null());
    command
}

async fn run(mut command: tokio::process::Command, what: &str) -> Result<String> {
    let output = command
        .output()
        .await
        .context("cannot run git; is it installed?")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{what} failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Clones `url` (at `git_ref` or `sha`) into `dest` and returns the commit.
pub async fn clone(
    url: &str,
    git_ref: Option<&str>,
    sha: Option<&str>,
    dest: &Path,
) -> Result<String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Some(sha) = sha {
        std::fs::create_dir_all(dest)?;
        let mut init = git(Some(dest));
        init.args(["init", "--quiet"]);
        run(init, "git init").await?;
        let mut add = git(Some(dest));
        add.args(["remote", "add", "origin", url]);
        run(add, "git remote add").await?;
        let mut fetch = git(Some(dest));
        fetch.args(["fetch", "--quiet", "--depth", "1", "origin", sha]);
        run(fetch, &format!("fetching {sha} from {url}")).await?;
        let mut checkout = git(Some(dest));
        checkout.args(["checkout", "--quiet", "FETCH_HEAD"]);
        run(checkout, "git checkout").await?;
    } else {
        let mut command = git(None);
        command.args(["clone", "--quiet", "--depth", "1"]);
        if let Some(git_ref) = git_ref {
            command.args(["--branch", git_ref]);
        }
        command.arg("--").arg(url).arg(dest);
        run(command, &format!("cloning {url}")).await?;
    }
    commit(dest).await
}

/// Clones a parsed source, retrying GitHub shorthand over SSH when HTTPS
/// is refused (a private repository without HTTPS credentials).
pub async fn clone_source(source: &Source, dest: &Path) -> Result<String> {
    let Source::Git {
        url,
        git_ref,
        github,
    } = source
    else {
        bail!("not a git source");
    };
    match clone(url, git_ref.as_deref(), None, dest).await {
        Ok(commit) => Ok(commit),
        Err(err) => {
            let Some(repo) = github else {
                return Err(err);
            };
            let _ = std::fs::remove_dir_all(dest);
            let ssh = format!("git@github.com:{repo}.git");
            clone(&ssh, git_ref.as_deref(), None, dest)
                .await
                .with_context(|| format!("{err:#}; over SSH too"))
        }
    }
}

/// Fetches the latest `git_ref` (or the default branch) into a clone.
pub async fn pull(dir: &Path, git_ref: Option<&str>) -> Result<String> {
    let mut fetch = git(Some(dir));
    fetch.args([
        "fetch",
        "--quiet",
        "--depth",
        "1",
        "origin",
        git_ref.unwrap_or("HEAD"),
    ]);
    run(fetch, &format!("updating {}", dir.display())).await?;
    let mut reset = git(Some(dir));
    reset.args(["reset", "--quiet", "--hard", "FETCH_HEAD"]);
    run(reset, "git reset").await?;
    commit(dir).await
}

pub async fn commit(dir: &Path) -> Result<String> {
    let mut command = git(Some(dir));
    command.args(["rev-parse", "HEAD"]);
    run(command, "git rev-parse").await
}

/// Downloads a marketplace file.
pub async fn download(url: &str) -> Result<Vec<u8>> {
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("cannot fetch {url}"))?
        .error_for_status()
        .with_context(|| format!("cannot fetch {url}"))?;
    Ok(response.bytes().await?.to_vec())
}

/// Copies a directory without its `.git`, hard-linking files where it can
/// so a large plugin costs no extra space. Symlinks that leave `src` are
/// skipped.
pub fn copy_tree(src: &Path, dest: &Path) -> Result<()> {
    let src = std::fs::canonicalize(src).with_context(|| format!("{} not found", src.display()))?;
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(&src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = dest.join(&name);
        let Ok(resolved) = std::fs::canonicalize(&from) else {
            continue;
        };
        if !resolved.starts_with(&src) {
            continue;
        }
        if resolved.is_dir() {
            copy_tree(&resolved, &to)?;
        } else if std::fs::hard_link(&resolved, &to).is_err() {
            std::fs::copy(&resolved, &to)
                .with_context(|| format!("cannot copy {}", from.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sources() {
        let base = Path::new("/work");
        assert_eq!(
            Source::parse("./plugins", base).unwrap(),
            Source::Local("/work/./plugins".into())
        );
        assert_eq!(
            Source::parse("lobirus/claude-plugins", base).unwrap(),
            Source::Git {
                url: "https://github.com/lobirus/claude-plugins.git".into(),
                git_ref: None,
                github: Some("lobirus/claude-plugins".into()),
            }
        );
        assert_eq!(
            Source::parse("owner/repo@v2", base).unwrap(),
            Source::Git {
                url: "https://github.com/owner/repo.git".into(),
                git_ref: Some("v2".into()),
                github: Some("owner/repo".into()),
            }
        );
        assert_eq!(
            Source::parse("git@gitlab.com:group/sub/repo.git#main", base).unwrap(),
            Source::Git {
                url: "git@gitlab.com:group/sub/repo.git".into(),
                git_ref: Some("main".into()),
                github: None,
            }
        );
        assert!(matches!(
            Source::parse("https://gitlab.example.com/team/plugins", base).unwrap(),
            Source::Git { .. }
        ));
        assert_eq!(
            Source::parse("https://example.com/plugins/marketplace.json", base).unwrap(),
            Source::Url("https://example.com/plugins/marketplace.json".into())
        );
        assert!(Source::parse("not a source", base).is_err());
    }

    #[test]
    fn names_sources() {
        let base = Path::new("/work");
        assert_eq!(
            Source::parse("lobirus/claude-plugins", base)
                .unwrap()
                .name_hint(),
            "claude-plugins"
        );
        assert_eq!(
            Source::parse("git@host:a/b.git", base).unwrap().name_hint(),
            "b"
        );
        assert_eq!(
            Source::parse("https://example.com/x/marketplace.json", base)
                .unwrap()
                .name_hint(),
            "x"
        );
    }
}
