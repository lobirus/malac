//! Adding marketplaces and installing, enabling, updating and removing
//! plugins. Installing only copies files; nothing a plugin ships runs
//! until it is enabled, and enabling one with hooks, MCP servers or
//! scripts needs the user's approval ([`NeedsApproval`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Serialize;

use super::marketplace::{self, Catalog, Entry, EntrySource};
use super::source::{self, Source};
use super::store::{PluginsFile, Record, Scope, Store};
use super::{EXECUTABLES, Plugin, PluginInfo, builtin};

/// Enabling this plugin needs the user's explicit approval because it runs
/// code on this machine. The CLI asks; agents ask through the approver.
#[derive(Debug, Clone)]
pub struct NeedsApproval {
    pub id: String,
    /// The parts that run code and are not approved yet.
    pub parts: Vec<&'static str>,
    /// What the plugin contains, from [`Plugin::describe`].
    pub summary: String,
}

impl std::fmt::Display for NeedsApproval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} runs code on this machine ({}); enabling it needs your approval:\n{}",
            self.id,
            self.parts.join(", "),
            self.summary
        )
    }
}

impl std::error::Error for NeedsApproval {}

#[derive(Debug, Clone, Serialize)]
pub struct MarketplaceInfo {
    pub name: String,
    pub source: String,
    pub scope: Scope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub plugins: Vec<Entry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Whether a plugin is on, and which file says so.
#[derive(Debug, Clone)]
pub struct State {
    pub id: String,
    pub enabled: bool,
    pub scope: Scope,
}

/// Where a marketplace's files are.
#[derive(Debug, Clone)]
enum Location {
    /// A folder (a local marketplace, or a clone).
    Dir(PathBuf),
    /// Only its `marketplace.json`, downloaded from a URL.
    File(PathBuf),
}

pub struct Manager {
    store: Store,
    project_root: Option<PathBuf>,
    cwd: PathBuf,
    defaults: bool,
}

/// `name@marketplace` into its parts.
fn split_id(spec: &str) -> (&str, Option<&str>) {
    match spec.rsplit_once('@') {
        Some((name, marketplace)) if !name.is_empty() => (name, Some(marketplace)),
        _ => (spec, None),
    }
}

/// A source rather than a plugin name: a path, a URL or `owner/repo`.
fn looks_like_source(spec: &str) -> bool {
    spec.contains('/') || spec.starts_with(['.', '~']) || spec.contains("://")
}

impl Manager {
    /// `defaults` enables the built-in plugins unless a file disables them.
    pub fn new(malac_home: &Path, project_root: Option<&Path>, cwd: &Path, defaults: bool) -> Self {
        Self {
            store: Store::new(malac_home),
            project_root: project_root.map(Path::to_path_buf),
            cwd: cwd.to_path_buf(),
            defaults,
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn file(&self, scope: Scope) -> Result<PathBuf> {
        match scope {
            Scope::User => Ok(self.store.user_file()),
            Scope::Project => self
                .project_root
                .as_deref()
                .map(super::store::project_file)
                .context("not in a project"),
            Scope::Builtin => bail!("built-in plugins have no file"),
        }
    }

    fn read(&self, scope: Scope) -> Result<PluginsFile> {
        match self.file(scope) {
            Ok(path) => PluginsFile::read(&path),
            Err(_) => Ok(PluginsFile::default()),
        }
    }

    /// Marketplaces from both files, `(name, source, scope)`; the project's
    /// replace the user's of the same name.
    pub fn marketplaces(&self) -> Result<Vec<(String, String, Scope)>> {
        let mut all: BTreeMap<String, (String, Scope)> = BTreeMap::new();
        for scope in [Scope::User, Scope::Project] {
            for (name, spec) in self.read(scope)?.marketplaces {
                all.insert(name, (spec, scope));
            }
        }
        Ok(all
            .into_iter()
            .map(|(name, (spec, scope))| (name, spec, scope))
            .collect())
    }

    /// Folders in the user file are absolute; the project's are relative to the project.
    fn base(&self, scope: Scope) -> &Path {
        match (scope, &self.project_root) {
            (Scope::Project, Some(root)) => root,
            _ => &self.cwd,
        }
    }

    fn location(&self, name: &str, source: &Source) -> Location {
        match source {
            Source::Local(path) if path.is_file() => {
                // `<root>/.claude-plugin/marketplace.json` belongs to `<root>`.
                let parent = path.parent().unwrap_or(path);
                let hidden = parent
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'));
                let root = if hidden {
                    parent.parent().unwrap_or(parent)
                } else {
                    parent
                };
                Location::Dir(root.to_path_buf())
            }
            Source::Local(path) => Location::Dir(path.clone()),
            Source::Git { .. } => Location::Dir(self.store.marketplace_dir(name)),
            Source::Url(_) => {
                Location::File(self.store.marketplace_dir(name).join("marketplace.json"))
            }
        }
    }

    fn read_catalog(location: &Location, hint: &str) -> Result<Catalog> {
        match location {
            Location::Dir(dir) => marketplace::read_dir(dir, hint),
            Location::File(file) => {
                let text = std::fs::read_to_string(file).with_context(|| {
                    format!(
                        "{} is missing; run `malac plugin marketplace update`",
                        file.display()
                    )
                })?;
                marketplace::parse(&text, hint)
            }
        }
    }

    /// A known marketplace's catalog.
    fn catalog(&self, name: &str) -> Result<(Catalog, Location, Source, String)> {
        let (spec, scope) = self
            .marketplaces()?
            .into_iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, spec, scope)| (spec, scope))
            .with_context(|| format!("no marketplace named `{name}`; add it with `malac plugin marketplace add <source>`"))?;
        let source = Source::parse(&spec, self.base(scope))?;
        let location = self.location(name, &source);
        let catalog = Self::read_catalog(&location, name)?;
        Ok((catalog, location, source, spec))
    }

    fn temp_dir(&self) -> PathBuf {
        self.store
            .dir
            .join("marketplaces")
            .join(format!(".tmp-{}", uuid::Uuid::new_v4().simple()))
    }

    /// Fetches a marketplace and records it in `scope`'s file.
    pub async fn add_marketplace(&self, spec: &str, scope: Scope) -> Result<Catalog> {
        let source = Source::parse(spec, &self.cwd)?;
        let hint = source.name_hint();
        let (catalog, stored) = match &source {
            Source::Local(path) => {
                let path = std::fs::canonicalize(path)
                    .with_context(|| format!("{} not found", path.display()))?;
                let location = self.location(&hint, &Source::Local(path.clone()));
                let catalog = Self::read_catalog(&location, &hint)?;
                let stored = match (&self.project_root, scope) {
                    (Some(root), Scope::Project) => match path.strip_prefix(root) {
                        Ok(rel) => format!("./{}", rel.display()),
                        Err(_) => path.display().to_string(),
                    },
                    _ => path.display().to_string(),
                };
                self.check_new_name(&catalog.name, &stored)?;
                (catalog, stored)
            }
            Source::Git { .. } => {
                let tmp = self.temp_dir();
                let fetched = async {
                    source::clone_source(&source, &tmp).await?;
                    marketplace::read_dir(&tmp, &hint)
                };
                let catalog = match fetched.await {
                    Ok(catalog) => catalog,
                    Err(err) => {
                        let _ = std::fs::remove_dir_all(&tmp);
                        return Err(err);
                    }
                };
                self.check_new_name(&catalog.name, spec)?;
                let dest = self.store.marketplace_dir(&catalog.name);
                if dest.exists() {
                    std::fs::remove_dir_all(&dest)?;
                }
                std::fs::rename(&tmp, &dest)?;
                (catalog, spec.trim().to_owned())
            }
            Source::Url(url) => {
                let bytes = source::download(url).await?;
                let text = String::from_utf8(bytes).context("the marketplace file is not UTF-8")?;
                let catalog = marketplace::parse(&text, &hint)?;
                self.check_new_name(&catalog.name, spec)?;
                let dir = self.store.marketplace_dir(&catalog.name);
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join("marketplace.json"), text)?;
                (catalog, spec.trim().to_owned())
            }
        };
        if marketplace::RESERVED.contains(&catalog.name.as_str()) {
            bail!("the marketplace name `{}` is reserved", catalog.name);
        }
        PluginsFile::edit(&self.file(scope)?, |file| {
            file.marketplaces.insert(catalog.name.clone(), stored);
        })?;
        Ok(catalog)
    }

    /// Refuses a name that another source already uses.
    fn check_new_name(&self, name: &str, spec: &str) -> Result<()> {
        if marketplace::RESERVED.contains(&name) {
            bail!("the marketplace name `{name}` is reserved");
        }
        if let Some((_, existing, _)) = self.marketplaces()?.into_iter().find(|(n, _, _)| n == name)
        {
            if existing.trim() != spec.trim() {
                bail!(
                    "a marketplace named `{name}` already comes from {existing}; remove it first"
                );
            }
        }
        Ok(())
    }

    /// Fetches the latest catalogs (all, or `name`). Returns a line per marketplace.
    pub async fn update_marketplaces(&self, name: Option<&str>) -> Result<Vec<String>> {
        let mut report = Vec::new();
        let known = self.marketplaces()?;
        if let Some(name) = name {
            if !known.iter().any(|(n, _, _)| n == name) {
                bail!("no marketplace named `{name}`");
            }
        }
        for (mkt, spec, scope) in known
            .into_iter()
            .filter(|(n, _, _)| name.is_none_or(|x| n.as_str() == x))
        {
            let source = match Source::parse(&spec, self.base(scope)) {
                Ok(source) => source,
                Err(err) => {
                    report.push(format!("{mkt}: {err:#}"));
                    continue;
                }
            };
            let outcome = match (&source, self.location(&mkt, &source)) {
                (Source::Local(_), _) => Ok("local folder, read in place".to_owned()),
                (Source::Git { git_ref, .. }, Location::Dir(dir)) if dir.join(".git").exists() => {
                    source::pull(&dir, git_ref.as_deref())
                        .await
                        .map(|c| format!("at {}", &c[..c.len().min(12)]))
                }
                (Source::Git { .. }, Location::Dir(dir)) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    source::clone_source(&source, &dir)
                        .await
                        .map(|c| format!("fetched {}", &c[..c.len().min(12)]))
                }
                (Source::Url(url), Location::File(file)) => match source::download(url).await {
                    Ok(bytes) => std::fs::create_dir_all(file.parent().unwrap_or(&file))
                        .and_then(|_| std::fs::write(&file, bytes))
                        .map(|_| "downloaded".to_owned())
                        .map_err(Into::into),
                    Err(err) => Err(err),
                },
                _ => Ok(String::new()),
            };
            report.push(match outcome {
                Ok(text) => format!("{mkt}: {text}"),
                Err(err) => format!("{mkt}: {err:#}"),
            });
        }
        Ok(report)
    }

    /// Removes a marketplace from `scope`'s file, and its clone when no
    /// file lists it any more. Plugins installed from it stay installed.
    pub fn remove_marketplace(&self, name: &str, scope: Scope) -> Result<()> {
        let path = self.file(scope)?;
        let mut file = PluginsFile::read(&path)?;
        if file.marketplaces.remove(name).is_none() {
            bail!("{} lists no marketplace named `{name}`", path.display());
        }
        file.write(&path)?;
        if !self.marketplaces()?.iter().any(|(n, _, _)| n == name) {
            let dir = self.store.marketplace_dir(name);
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
        }
        Ok(())
    }

    pub fn list_marketplaces(&self) -> Result<Vec<MarketplaceInfo>> {
        let mut out = Vec::new();
        for (name, spec, scope) in self.marketplaces()? {
            let mut info = MarketplaceInfo {
                name: name.clone(),
                source: spec,
                scope,
                description: None,
                plugins: Vec::new(),
                warnings: Vec::new(),
                error: None,
            };
            match self.catalog(&name) {
                Ok((catalog, ..)) => {
                    info.description = catalog.description;
                    info.plugins = catalog.plugins;
                    info.warnings = catalog.warnings;
                }
                Err(err) => info.error = Some(format!("{err:#}")),
            }
            out.push(info);
        }
        Ok(out)
    }

    /// Whether each known plugin is enabled: the user's file, then the
    /// project's, then the built-in defaults. A built-in plugin steps aside
    /// for an enabled plugin of the same name.
    pub fn states(&self) -> Result<Vec<State>> {
        let mut map: BTreeMap<String, (bool, Scope)> = BTreeMap::new();
        for scope in [Scope::User, Scope::Project] {
            for (id, enabled) in self.read(scope)?.plugins {
                map.insert(id, (enabled, scope));
            }
        }
        if self.defaults {
            for builtin in builtin::all() {
                map.entry(builtin.id()).or_insert((true, Scope::Builtin));
            }
        }
        let enabled_names: Vec<String> = map
            .iter()
            .filter(|(id, (on, _))| *on && !id.ends_with(&format!("@{}", builtin::MARKETPLACE)))
            .map(|(id, _)| split_id(id).0.to_owned())
            .collect();
        for builtin in builtin::all() {
            if let Some((on, Scope::Builtin)) = map.get_mut(&builtin.id()) {
                if enabled_names.iter().any(|n| n == builtin.name) {
                    *on = false;
                }
            }
        }
        Ok(map
            .into_iter()
            .map(|(id, (enabled, scope))| State { id, enabled, scope })
            .collect())
    }

    /// An installed (or built-in) plugin and the executable parts approved for it.
    pub fn load_installed(&self, id: &str) -> Result<(Plugin, Vec<String>)> {
        let data = self.store.data_dir(id);
        let (name, marketplace) = split_id(id);
        if marketplace == Some(builtin::MARKETPLACE) {
            let builtin =
                builtin::get(name).with_context(|| format!("no built-in plugin `{name}`"))?;
            let root = builtin::materialize(builtin, &self.store.builtin_dir())?;
            let approved = EXECUTABLES.iter().map(|e| e.to_string()).collect();
            return Ok((Plugin::load(id, &root, &data, None)?, approved));
        }
        let installed = self.store.installed()?;
        let record = installed.plugins.get(id).with_context(|| {
            format!("enabled but not installed here; run `malac plugin install {id}`")
        })?;
        let entry = Some(&record.entry).filter(|e| !e.is_null());
        let plugin = Plugin::load(id, &record.path, &data, entry)?;
        Ok((plugin, record.approved.clone()))
    }

    /// The id of an installed or built-in plugin given by name or id.
    pub fn resolve(&self, spec: &str) -> Result<String> {
        let installed = self.store.installed()?;
        let builtins = builtin::all().iter().map(|b| b.id());
        let mut ids: Vec<String> = installed.plugins.keys().cloned().chain(builtins).collect();
        ids.dedup();
        if spec.contains('@') {
            return ids.into_iter().find(|id| id == spec).with_context(|| {
                format!("`{spec}` is not installed; `malac plugin install {spec}` installs it")
            });
        }
        let matches: Vec<String> = ids
            .into_iter()
            .filter(|id| split_id(id).0 == spec)
            .collect();
        match matches.as_slice() {
            [one] => Ok(one.clone()),
            [] => bail!("no installed plugin named `{spec}`"),
            many => bail!("`{spec}` is ambiguous: {}", many.join(", ")),
        }
    }

    /// Installs a plugin (`name`, `name@marketplace`, or a source that holds
    /// one plugin) without enabling it, and returns it. Nothing in it runs.
    pub async fn install(&self, spec: &str) -> Result<Plugin> {
        if looks_like_source(spec) {
            let catalog = self.add_marketplace(spec, Scope::User).await?;
            return match catalog.plugins.as_slice() {
                [one] => {
                    let id = format!("{}@{}", one.name, catalog.name);
                    Box::pin(self.install(&id)).await
                }
                plugins => bail!(
                    "added the marketplace `{}`, which lists {} plugins ({}); install one with `malac plugin install <plugin>@{}`",
                    catalog.name,
                    plugins.len(),
                    plugins
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    catalog.name
                ),
            };
        }
        let (name, marketplace) = split_id(spec);
        if marketplace == Some(builtin::MARKETPLACE) {
            bail!("{spec} is built into malac; `malac plugin enable {spec}` turns it on");
        }
        let mut found = Vec::new();
        for (mkt, _, _) in self.marketplaces()? {
            if marketplace.is_some_and(|m| m != mkt) {
                continue;
            }
            match self.catalog(&mkt) {
                Ok((catalog, location, source, spec)) => {
                    if let Some(entry) = catalog.plugins.into_iter().find(|p| p.name == name) {
                        found.push((mkt, entry, location, source, spec));
                    }
                }
                Err(err) if marketplace.is_some() => return Err(err),
                Err(_) => {}
            }
        }
        match found.len() {
            0 => match marketplace {
                Some(m) => bail!("the marketplace `{m}` lists no plugin named `{name}`"),
                None => bail!(
                    "no marketplace lists a plugin named `{name}`; add one with `malac plugin marketplace add <source>`"
                ),
            },
            1 => {
                let (mkt, entry, location, source, spec) = found.remove(0);
                self.install_entry(&mkt, &entry, &location, &source, &spec)
                    .await
            }
            _ => bail!(
                "`{name}` is in several marketplaces; pick one: {}",
                found
                    .iter()
                    .map(|f| format!("{name}@{}", f.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    async fn install_entry(
        &self,
        marketplace: &str,
        entry: &Entry,
        location: &Location,
        source: &Source,
        spec: &str,
    ) -> Result<Plugin> {
        let id = format!("{}@{marketplace}", entry.name);
        let cache = self.store.cache_dir(marketplace, &entry.name);
        let (path, commit, origin) = match &entry.source {
            EntrySource::Path { path } => {
                let Location::Dir(dir) = location else {
                    bail!(
                        "`{}` lives inside its marketplace, but only the catalog of `{marketplace}` was downloaded",
                        entry.name
                    );
                };
                let dir = std::fs::canonicalize(dir)?;
                let plugin_dir =
                    super::manifest::contained(&dir, &dir.join(path)).with_context(|| {
                        format!("{path} not found in the marketplace `{marketplace}`")
                    })?;
                if matches!(source, Source::Local(_)) {
                    // Local marketplaces are used in place, so edits show up on the next session.
                    (plugin_dir.clone(), None, plugin_dir.display().to_string())
                } else {
                    replace_dir(&cache, |dest| source::copy_tree(&plugin_dir, dest))?;
                    let commit = source::commit(&dir).await.ok();
                    (cache.clone(), commit, format!("{spec} {path}"))
                }
            }
            EntrySource::Git {
                url,
                git_ref,
                sha,
                subdir,
            } => {
                let tmp = self.temp_dir();
                let fetched =
                    match source::clone(url, git_ref.as_deref(), sha.as_deref(), &tmp).await {
                        Ok(commit) => Ok(commit),
                        Err(err) => match url.strip_prefix("https://github.com/") {
                            Some(repo) => {
                                let _ = std::fs::remove_dir_all(&tmp);
                                source::clone(
                                    &format!("git@github.com:{repo}"),
                                    git_ref.as_deref(),
                                    sha.as_deref(),
                                    &tmp,
                                )
                                .await
                                .with_context(|| format!("{err:#}; over SSH too"))
                            }
                            None => Err(err),
                        },
                    };
                let commit = match fetched {
                    Ok(commit) => commit,
                    Err(err) => {
                        let _ = std::fs::remove_dir_all(&tmp);
                        return Err(err);
                    }
                };
                let from = match subdir {
                    Some(dir) => tmp.join(dir),
                    None => tmp.clone(),
                };
                let copied = replace_dir(&cache, |dest| source::copy_tree(&from, dest));
                let _ = std::fs::remove_dir_all(&tmp);
                copied?;
                let at = git_ref
                    .as_deref()
                    .map(|r| format!("#{r}"))
                    .unwrap_or_default();
                (cache.clone(), Some(commit), format!("{url}{at}"))
            }
            EntrySource::Unsupported { kind } => {
                bail!(
                    "`{}` comes from a {kind} source, which malac does not install; install it from its git repository instead",
                    entry.name
                )
            }
        };
        let data = self.store.data_dir(&id);
        let raw = Some(&entry.raw).filter(|e| !e.is_null());
        let plugin = Plugin::load(&id, &path, &data, raw)?;
        let mut installed = self.store.installed()?;
        let now = Utc::now();
        let previous = installed.plugins.get(&id);
        let record = Record {
            name: entry.name.clone(),
            marketplace: marketplace.to_owned(),
            version: plugin.meta.version.clone().or(entry.version.clone()),
            path,
            source: origin,
            commit,
            entry: entry.raw.clone(),
            installed: previous.map_or(now, |p| p.installed),
            updated: now,
            approved: previous.map(|p| p.approved.clone()).unwrap_or_default(),
        };
        installed.plugins.insert(id, record);
        self.store.save_installed(&installed)?;
        Ok(plugin)
    }

    /// Enables a plugin in `scope`'s file. A plugin with hooks, MCP servers
    /// or scripts not approved yet fails with [`NeedsApproval`] unless
    /// `approve`, which records the approval.
    pub fn enable(&self, spec: &str, scope: Scope, approve: bool) -> Result<String> {
        let id = self.resolve(spec)?;
        let (plugin, approved) = self.load_installed(&id)?;
        let missing: Vec<&'static str> = plugin
            .executables()
            .into_iter()
            .filter(|e| !approved.iter().any(|a| a == e))
            .collect();
        if !missing.is_empty() {
            if !approve {
                return Err(NeedsApproval {
                    id,
                    parts: missing,
                    summary: plugin.describe(),
                }
                .into());
            }
            let mut installed = self.store.installed()?;
            if let Some(record) = installed.plugins.get_mut(&id) {
                record
                    .approved
                    .extend(missing.iter().map(|m| m.to_string()));
            }
            self.store.save_installed(&installed)?;
        }
        let user_source = self
            .read(Scope::User)?
            .marketplaces
            .get(&plugin.marketplace)
            .cloned();
        PluginsFile::edit(&self.file(scope)?, |file| {
            file.plugins.insert(id.clone(), true);
            // Teammates need the marketplace to install it.
            if let (Scope::Project, Some(source)) = (scope, user_source) {
                file.marketplaces
                    .entry(plugin.marketplace.clone())
                    .or_insert(source);
            }
        })?;
        Ok(id)
    }

    pub fn disable(&self, spec: &str, scope: Scope) -> Result<String> {
        let id = if spec.contains('@') {
            spec.to_owned()
        } else {
            self.resolve(spec)?
        };
        PluginsFile::edit(&self.file(scope)?, |file| {
            file.plugins.insert(id.clone(), false);
        })?;
        Ok(id)
    }

    /// Removes an installed plugin, its cached copy and its entry in the
    /// user's file. Its data directory is kept.
    pub fn uninstall(&self, spec: &str) -> Result<String> {
        let id = self.resolve(spec)?;
        if id.ends_with(&format!("@{}", builtin::MARKETPLACE)) {
            bail!("{id} is built into malac; `malac plugin disable {id}` turns it off");
        }
        let mut installed = self.store.installed()?;
        let record = installed.plugins.remove(&id).context("not installed")?;
        let cache = self.store.dir.join("cache");
        if record.path.starts_with(&cache) && record.path.exists() {
            std::fs::remove_dir_all(&record.path)?;
        }
        self.store.save_installed(&installed)?;
        PluginsFile::edit(&self.file(Scope::User)?, |file| {
            file.plugins.remove(&id);
        })?;
        Ok(id)
    }

    /// Fetches the latest marketplaces and reinstalls the installed plugins
    /// (or one). Hooks or MCP servers an update adds stay off until approved.
    pub async fn update(&self, spec: Option<&str>) -> Result<Vec<String>> {
        let only = spec.map(|s| self.resolve(s)).transpose()?;
        let mut report = self.update_marketplaces(None).await?;
        let installed = self.store.installed()?;
        for (id, record) in installed.plugins {
            if only.as_ref().is_some_and(|o| *o != id) {
                continue;
            }
            let before = (record.version.clone(), record.commit.clone());
            match self.install(&id).await {
                Ok(plugin) => {
                    let after = self
                        .store
                        .installed()?
                        .plugins
                        .get(&id)
                        .map(|r| (r.version.clone(), r.commit.clone()));
                    let new_parts: Vec<&str> = plugin
                        .executables()
                        .into_iter()
                        .filter(|e| *e != "scripts" && !record.approved.iter().any(|a| a == e))
                        .collect();
                    let mut line = if after.as_ref() == Some(&before) {
                        format!("{id}: up to date")
                    } else {
                        format!(
                            "{id}: {} → {}",
                            before.0.as_deref().unwrap_or("?"),
                            plugin.meta.version.as_deref().unwrap_or("new commit")
                        )
                    };
                    if !new_parts.is_empty() {
                        line.push_str(&format!(
                            " (now has {} that will not run until `malac plugin enable {id}` approves them)",
                            new_parts.join(", ")
                        ));
                    }
                    report.push(line);
                }
                Err(err) => report.push(format!("{id}: {err:#}")),
            }
        }
        Ok(report)
    }

    /// Every installed, built-in or listed plugin.
    pub fn list(&self) -> Result<Vec<PluginInfo>> {
        let states = self.states()?;
        let state = |id: &str| states.iter().find(|s| s.id == id);
        let installed = self.store.installed()?;
        let mut ids: Vec<String> = installed.plugins.keys().cloned().collect();
        ids.extend(builtin::all().iter().map(|b| b.id()));
        ids.extend(states.iter().map(|s| s.id.clone()));
        ids.sort();
        ids.dedup();
        let mut out = Vec::new();
        for id in ids {
            let enabled = state(&id).is_some_and(|s| s.enabled);
            let scope = state(&id).map(|s| s.scope);
            let info = match self.load_installed(&id) {
                Ok((plugin, approved)) => plugin.info(enabled, scope, &approved),
                Err(err) => {
                    let (name, marketplace) = split_id(&id);
                    PluginInfo {
                        name: name.to_owned(),
                        marketplace: marketplace.unwrap_or_default().to_owned(),
                        id,
                        version: None,
                        description: None,
                        enabled,
                        scope,
                        installed: false,
                        path: None,
                        formats: Vec::new(),
                        skills: Vec::new(),
                        commands: Vec::new(),
                        agents: Vec::new(),
                        mcp: Vec::new(),
                        hooks: Vec::new(),
                        scripts: false,
                        approved: Vec::new(),
                        pending: Vec::new(),
                        unsupported: Vec::new(),
                        warnings: Vec::new(),
                        error: Some(format!("{err:#}")),
                    }
                }
            };
            out.push(info);
        }
        Ok(out)
    }
}

/// Fills a fresh directory and swaps it in for `dest`.
fn replace_dir(dest: &Path, fill: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let parent = dest.parent().context("no parent directory")?;
    std::fs::create_dir_all(parent)?;
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let fresh = parent.join(format!(".{name}.new"));
    if fresh.exists() {
        std::fs::remove_dir_all(&fresh)?;
    }
    if let Err(err) = fill(&fresh) {
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(err);
    }
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    std::fs::rename(&fresh, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A marketplace with a skills-only plugin and one with a hook.
    fn marketplace(root: &Path) {
        write(
            root,
            ".claude-plugin/marketplace.json",
            r#"{"name": "fixtures", "owner": {"name": "Tests"}, "plugins": [
                {"name": "notes", "source": "./plugins/notes", "description": "Note skills"},
                {"name": "guard", "source": "./plugins/guard"}
            ]}"#,
        );
        write(
            root,
            "plugins/notes/.claude-plugin/plugin.json",
            r#"{"name": "notes", "version": "1.0.0"}"#,
        );
        write(
            root,
            "plugins/notes/skills/jot/SKILL.md",
            "---\nname: jot\ndescription: Jot notes\n---\nWrite it down.",
        );
        write(
            root,
            "plugins/notes/commands/today.md",
            "---\ndescription: Today\n---\nSummarize today: $ARGUMENTS",
        );
        write(
            root,
            "plugins/guard/.claude-plugin/plugin.json",
            r#"{"name": "guard", "version": "0.1.0"}"#,
        );
        write(
            root,
            "plugins/guard/hooks/hooks.json",
            r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "exit 0"}]}]}}"#,
        );
    }

    #[tokio::test]
    async fn installs_from_a_local_marketplace() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let market = tempfile::tempdir().unwrap();
        marketplace(market.path());
        let manager = Manager::new(home.path(), Some(project.path()), project.path(), true);

        let catalog = manager
            .add_marketplace(&market.path().display().to_string(), Scope::User)
            .await
            .unwrap();
        assert_eq!(catalog.name, "fixtures");
        assert_eq!(catalog.plugins.len(), 2);

        let notes = manager.install("notes").await.unwrap();
        assert_eq!(notes.id, "notes@fixtures");
        assert!(notes.executables().is_empty());
        // Installing runs nothing and enables nothing.
        assert!(
            !manager
                .states()
                .unwrap()
                .iter()
                .any(|s| s.id == "notes@fixtures")
        );
        manager.enable("notes", Scope::User, false).unwrap();

        manager.install("guard@fixtures").await.unwrap();
        let err = manager.enable("guard", Scope::Project, false).unwrap_err();
        let needs = err.downcast_ref::<NeedsApproval>().expect("needs approval");
        assert_eq!(needs.parts, vec!["hooks"]);
        assert!(needs.summary.contains("PreToolUse: exit 0"));
        manager.enable("guard", Scope::Project, true).unwrap();
        let project_file =
            PluginsFile::read(&super::super::store::project_file(project.path())).unwrap();
        assert_eq!(project_file.plugins.get("guard@fixtures"), Some(&true));
        assert!(
            project_file.marketplaces.contains_key("fixtures"),
            "teammates get the marketplace"
        );

        let loaded = super::super::load(home.path(), project.path(), &Default::default());
        let ids: Vec<&str> = loaded.plugins.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["guard@fixtures", "notes@fixtures", "ponytail@malac"]
        );
        assert!(loaded.hooks.has("PreToolUse"));

        manager.disable("ponytail", Scope::User).unwrap();
        let list = manager.list().unwrap();
        let ponytail = list.iter().find(|p| p.id == "ponytail@malac").unwrap();
        assert!(!ponytail.enabled);
        assert_eq!(ponytail.skills.len(), 6);

        manager.uninstall("notes").unwrap();
        assert!(
            market.path().join("plugins/notes").exists(),
            "local plugins are used in place, never deleted"
        );
        assert!(manager.resolve("notes").is_err());
    }

    #[tokio::test]
    async fn hooks_added_by_a_project_file_need_approval() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let market = tempfile::tempdir().unwrap();
        marketplace(market.path());
        let manager = Manager::new(home.path(), Some(project.path()), project.path(), false);
        manager
            .add_marketplace(&market.path().display().to_string(), Scope::User)
            .await
            .unwrap();
        manager.install("guard").await.unwrap();
        // A cloned repository enables it without the user's approval.
        PluginsFile::edit(&super::super::store::project_file(project.path()), |f| {
            f.plugins.insert("guard@fixtures".into(), true);
        })
        .unwrap();
        let config = crate::config::PluginsConfig {
            enabled: true,
            defaults: false,
        };
        let loaded = super::super::load(home.path(), project.path(), &config);
        assert_eq!(loaded.plugins.len(), 1);
        assert!(loaded.hooks.is_empty());
        assert!(
            loaded
                .warnings
                .iter()
                .any(|w| w.contains("hooks not approved"))
        );
    }
}
