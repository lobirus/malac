//! A connected browser for one profile, and its tabs.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::json;
use tokio::process::Child;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use super::cdp::Cdp;
use super::launch;
use super::page::Page;
use super::profiles::{self, Profile, Source};
use crate::config::BrowserConfig;

const REUSE_TIMEOUT: Duration = Duration::from_secs(3);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
/// Tab ids are shown shortened; any unique prefix selects a tab.
const SHORT_ID_LEN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Started by this process.
    Launched,
    /// A managed browser left running by an earlier process.
    Reused,
    /// A browser started by someone else, reached through `cdp_url`.
    Attached,
}

#[derive(Debug, Clone)]
pub struct Tab {
    pub id: String,
    pub title: String,
    pub url: String,
}

#[derive(Default)]
struct Tabs {
    /// The tab used last, which tools default to.
    active: Option<String>,
    /// CDP session per attached tab.
    sessions: HashMap<String, String>,
}

pub struct Instance {
    cdp: Arc<Cdp>,
    origin: Origin,
    product: String,
    child: tokio::sync::Mutex<Option<Child>>,
    tabs: Arc<Mutex<Tabs>>,
    /// Things that happened outside the current call, reported with its result.
    notices: Arc<Mutex<Vec<String>>>,
    watcher: JoinHandle<()>,
}

impl Instance {
    /// Connects to the profile's browser. A managed profile reuses a browser
    /// still running on its user data dir; otherwise one is launched when
    /// `launch` is set.
    pub async fn connect(profile: &Profile, config: &BrowserConfig, launch: bool) -> Result<Self> {
        let mut notices = Vec::new();
        let (cdp, origin, child) = match &profile.source {
            Source::Attach { cdp_url } => {
                let url = launch::resolve_cdp_url(cdp_url).await?;
                (Cdp::connect(&url).await?, Origin::Attached, None)
            }
            Source::Managed { dir, clone_from } => match reuse(dir).await {
                Some(cdp) => (cdp, Origin::Reused, None),
                None if !launch => bail!(
                    "browser profile `{}` is not open; call browser_open or browser_navigate first",
                    profile.name
                ),
                None => {
                    let setup = {
                        let (dir, clone_from) = (dir.clone(), clone_from.clone());
                        tokio::task::spawn_blocking(move || {
                            profiles::prepare(&dir, clone_from.as_deref())
                        })
                    };
                    notices.extend(setup.await.context("profile setup panicked")??);
                    let executable = launch::find_executable(config.executable.as_deref())?;
                    let launched = launch::launch(&executable, dir, config.headless).await?;
                    let cdp = Cdp::connect(&launched.ws_url).await?;
                    (cdp, Origin::Launched, Some(launched.child))
                }
            },
        };
        let cdp = Arc::new(cdp);
        let version = cdp.call("Browser.getVersion", json!({})).await?;
        let product = version["product"]
            .as_str()
            .unwrap_or("unknown browser")
            .to_owned();
        notices.push(match origin {
            Origin::Launched => format!("Launched {product} for profile `{}`.", profile.name),
            Origin::Reused => format!(
                "Reconnected to {product} already running for profile `{}`.",
                profile.name
            ),
            Origin::Attached => format!("Attached to {product} for profile `{}`.", profile.name),
        });
        let tabs = Arc::default();
        let notices = Arc::new(Mutex::new(notices));
        let watcher = tokio::spawn(watch(cdp.clone(), Arc::clone(&tabs), notices.clone()));
        Ok(Self {
            cdp,
            origin,
            product,
            child: tokio::sync::Mutex::new(child),
            tabs,
            notices,
            watcher,
        })
    }

    pub fn is_alive(&self) -> bool {
        !self.cdp.is_closed()
    }

    pub fn status(&self) -> String {
        let how = match self.origin {
            Origin::Launched => "launched",
            Origin::Reused => "reconnected",
            Origin::Attached => "attached",
        };
        format!("running {}, {how}", self.product)
    }

    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut self.notices.lock().unwrap())
    }

    pub async fn tabs(&self) -> Result<Vec<Tab>> {
        let reply = self.cdp.call("Target.getTargets", json!({})).await?;
        let tabs: Vec<Tab> = reply["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|info| info["type"] == "page")
            .map(|info| {
                let field = |name: &str| info[name].as_str().unwrap_or_default().to_owned();
                Tab {
                    id: field("targetId"),
                    title: field("title"),
                    url: field("url"),
                }
            })
            .filter(|tab| !tab.url.starts_with("devtools://"))
            .collect();
        self.tabs
            .lock()
            .unwrap()
            .sessions
            .retain(|id, _| tabs.iter().any(|tab| &tab.id == id));
        Ok(tabs)
    }

    /// The tab tools use when none is given: the one used last, else the first.
    pub fn default_tab<'a>(&self, tabs: &'a [Tab]) -> Option<&'a Tab> {
        let active = self.tabs.lock().unwrap().active.clone();
        active
            .and_then(|id| tabs.iter().find(|tab| tab.id == id))
            .or(tabs.first())
    }

    /// A session on `tab` (an id or unique id prefix), or on the default tab.
    /// Opens a tab when there is none.
    pub async fn page(&self, tab: Option<&str>) -> Result<Page> {
        let tabs = self.tabs().await?;
        let id = match tab {
            Some(query) => find_tab(&tabs, query)?.id.clone(),
            None => match self.default_tab(&tabs) {
                Some(tab) => tab.id.clone(),
                None => return self.new_tab().await,
            },
        };
        self.attach(&id, tab.is_some()).await
    }

    pub async fn new_tab(&self) -> Result<Page> {
        let reply = self
            .cdp
            .call("Target.createTarget", json!({ "url": "about:blank" }))
            .await?;
        let id = reply["targetId"]
            .as_str()
            .context("Target.createTarget returned no targetId")?
            .to_owned();
        self.attach(&id, false).await
    }

    async fn attach(&self, id: &str, activate: bool) -> Result<Page> {
        let known = self.tabs.lock().unwrap().sessions.get(id).cloned();
        let session = match known {
            Some(session) => session,
            None => {
                let reply = self
                    .cdp
                    .call(
                        "Target.attachToTarget",
                        json!({ "targetId": id, "flatten": true }),
                    )
                    .await?;
                let session = reply["sessionId"]
                    .as_str()
                    .context("Target.attachToTarget returned no sessionId")?
                    .to_owned();
                self.cdp.call_in(&session, "Page.enable", json!({})).await?;
                self.tabs
                    .lock()
                    .unwrap()
                    .sessions
                    .insert(id.to_owned(), session.clone());
                session
            }
        };
        let previous = self.tabs.lock().unwrap().active.replace(id.to_owned());
        if activate && previous.as_deref() != Some(id) {
            self.cdp
                .call("Target.activateTarget", json!({ "targetId": id }))
                .await?;
        }
        Ok(Page::new(self.cdp.clone(), session, id.to_owned()))
    }

    pub async fn close_tab(&self, tab: Option<&str>) -> Result<Tab> {
        let tabs = self.tabs().await?;
        let tab = match tab {
            Some(query) => find_tab(&tabs, query)?,
            None => self.default_tab(&tabs).context("there are no open tabs")?,
        };
        self.cdp
            .call("Target.closeTarget", json!({ "targetId": tab.id }))
            .await?;
        let mut state = self.tabs.lock().unwrap();
        state.sessions.remove(&tab.id);
        if state.active.as_deref() == Some(tab.id.as_str()) {
            state.active = None;
        }
        Ok(tab.clone())
    }

    /// Closes a managed browser. Attached browsers are only disconnected.
    pub async fn close(&self) -> String {
        if self.origin == Origin::Attached {
            return format!("Disconnected from {}; it keeps running.", self.product);
        }
        // The browser may drop the connection before it replies.
        let _ = self.cdp.call("Browser.close", json!({})).await;
        if let Some(mut child) = self.child.lock().await.take() {
            if tokio::time::timeout(CLOSE_TIMEOUT, child.wait())
                .await
                .is_err()
            {
                let _ = child.kill().await;
            }
        }
        format!("Closed {}.", self.product)
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

async fn reuse(dir: &Path) -> Option<Cdp> {
    let url = launch::read_port_file(dir)?;
    match tokio::time::timeout(REUSE_TIMEOUT, Cdp::connect(&url)).await {
        Ok(Ok(cdp)) => Some(cdp),
        _ => {
            launch::remove_port_file(dir);
            None
        }
    }
}

/// Accepts JavaScript dialogs, which would otherwise block the page, and
/// forgets the sessions of detached tabs.
async fn watch(cdp: Arc<Cdp>, tabs: Arc<Mutex<Tabs>>, notices: Arc<Mutex<Vec<String>>>) {
    let mut events = cdp.events();
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return,
        };
        match event.method.as_str() {
            "Page.javascriptDialogOpening" => {
                let Some(session) = event.session else {
                    continue;
                };
                let kind = event.params["type"].as_str().unwrap_or("dialog");
                let message = event.params["message"].as_str().unwrap_or_default();
                let handled = cdp
                    .call_in(
                        &session,
                        "Page.handleJavaScriptDialog",
                        json!({ "accept": true }),
                    )
                    .await;
                let outcome = if handled.is_ok() {
                    "accepted"
                } else {
                    "left open"
                };
                notices.lock().unwrap().push(format!(
                    "The page opened a JavaScript {kind} dialog {message:?}; it was {outcome}."
                ));
            }
            "Target.detachedFromTarget" => {
                if let Some(session) = event.params["sessionId"].as_str() {
                    tabs.lock().unwrap().sessions.retain(|_, s| s != session);
                }
            }
            _ => {}
        }
    }
}

pub fn short_id(id: &str) -> &str {
    id.get(..SHORT_ID_LEN).unwrap_or(id)
}

pub fn find_tab<'a>(tabs: &'a [Tab], query: &str) -> Result<&'a Tab> {
    let query = query.trim();
    if query.is_empty() {
        bail!("the tab id is empty");
    }
    let matches: Vec<&Tab> = tabs
        .iter()
        .filter(|tab| {
            tab.id
                .get(..query.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(query))
        })
        .collect();
    match matches.as_slice() {
        [tab] => Ok(tab),
        [] => {
            let open: Vec<&str> = tabs.iter().map(|tab| short_id(&tab.id)).collect();
            bail!("no tab `{query}`; open tabs: {}", open.join(", "))
        }
        _ => bail!("tab id `{query}` matches several tabs; use more characters"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: &str) -> Tab {
        Tab {
            id: id.into(),
            title: String::new(),
            url: String::new(),
        }
    }

    #[test]
    fn finds_tabs_by_prefix() {
        let tabs = [tab("A1B2C3D4E5F6"), tab("A1FF0000"), tab("0099")];
        assert_eq!(find_tab(&tabs, "a1b2").unwrap().id, "A1B2C3D4E5F6");
        assert_eq!(find_tab(&tabs, "0099").unwrap().id, "0099");
        assert!(
            find_tab(&tabs, "A1")
                .unwrap_err()
                .to_string()
                .contains("several")
        );
        assert!(
            find_tab(&tabs, "ZZ")
                .unwrap_err()
                .to_string()
                .contains("A1B2C3D4")
        );
        assert!(find_tab(&tabs, " ").is_err());
        assert_eq!(short_id("A1B2C3D4E5F6"), "A1B2C3D4");
        assert_eq!(short_id("0099"), "0099");
    }
}
