//! Operations on one tab through its CDP session.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::cdp::{Cdp, Event, wait_for};
use super::keyboard;
use crate::imaging::{self, Encoded};
use crate::keys::KeyCombo;

const SNAPSHOT_JS: &str = include_str!("snapshot.js");
const ELEMENT_JS: &str = include_str!("element.js");

const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// How soon after input a navigation must start to be waited for.
const NAVIGATION_GRACE: Duration = Duration::from_millis(500);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);
const JPEG_QUALITY: u8 = 75;
/// Taller pages are cut off in full-page screenshots.
const MAX_FULL_PAGE_HEIGHT: f64 = 12_000.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

impl Button {
    fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
        }
    }

    fn mask(self) -> u8 {
        match self {
            Self::Left => 1,
            Self::Right => 2,
            Self::Middle => 4,
        }
    }
}

pub enum Locator {
    Ref(u32),
    Selector(String),
    Focused,
}

/// An element found by element.js, scrolled into view.
#[derive(Debug, Deserialize)]
pub struct Located {
    pub x: f64,
    pub y: f64,
    pub label: String,
    /// The element on top at the center point, when it is not the target.
    pub covered: Option<String>,
    #[serde(default)]
    pub select: bool,
}

pub struct Screenshot {
    pub image: Encoded,
    /// Full height of a page that was too tall for a full-page screenshot.
    pub cut_from: Option<f64>,
}

pub struct Page {
    cdp: Arc<Cdp>,
    session: String,
    target: String,
}

impl Page {
    pub fn new(cdp: Arc<Cdp>, session: String, target: String) -> Self {
        Self {
            cdp,
            session,
            target,
        }
    }

    pub fn id(&self) -> &str {
        &self.target
    }

    /// Subscribe before acting to see the events the action causes.
    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.cdp.events()
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.cdp.call_in(&self.session, method, params).await
    }

    /// The tab's title and URL.
    pub async fn info(&self) -> Result<(String, String)> {
        let reply = self
            .cdp
            .call("Target.getTargetInfo", json!({ "targetId": self.target }))
            .await?;
        let info = &reply["targetInfo"];
        let field = |name: &str| info[name].as_str().unwrap_or_default().to_owned();
        Ok((field("title"), field("url")))
    }

    /// Evaluates `expression` like the DevTools console, awaiting promises,
    /// and returns the resulting CDP `RemoteObject`.
    async fn run(&self, expression: &str, by_value: bool) -> Result<Value> {
        let mut reply = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": by_value,
                    "awaitPromise": true,
                    "userGesture": true,
                    "replMode": true,
                }),
            )
            .await?;
        if let Some(details) = reply.get("exceptionDetails") {
            let message = details["exception"]["description"]
                .as_str()
                .or(details["text"].as_str())
                .unwrap_or("the script threw an exception");
            bail!("{message}");
        }
        Ok(reply["result"].take())
    }

    /// Evaluates `expression` and returns its value as JSON.
    pub async fn evaluate(&self, expression: &str) -> Result<Value> {
        Ok(self.run(expression, true).await?["value"].take())
    }

    /// Evaluates `expression` for display: plain objects, arrays and
    /// primitives as pretty JSON; DOM nodes, functions, maps, dates, errors
    /// and other objects that do not serialize faithfully by their description.
    pub async fn evaluate_for_display(&self, expression: &str) -> Result<String> {
        let object = self.run(expression, false).await?;
        if object["type"] == "undefined" {
            return Ok("undefined".into());
        }
        if let Some(value) = object.get("value") {
            return Ok(serde_json::to_string_pretty(value)?);
        }
        if let Some(special) = object["unserializableValue"].as_str() {
            return Ok(special.into());
        }
        let description = object["description"]
            .as_str()
            .unwrap_or("object")
            .to_owned();
        let Some(id) = object["objectId"].as_str() else {
            return Ok(description);
        };
        let plain = object["type"] == "object"
            && matches!(object["subtype"].as_str(), None | Some("array"));
        let serialized = if plain {
            self.call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": id,
                    "functionDeclaration": "function () { return this; }",
                    "returnByValue": true,
                }),
            )
            .await
            .ok()
            .filter(|reply| reply.get("exceptionDetails").is_none())
            .map(|reply| serde_json::to_string_pretty(&reply["result"]["value"]))
            .transpose()?
        } else {
            None
        };
        let _ = self
            .call("Runtime.releaseObject", json!({ "objectId": id }))
            .await;
        Ok(serialized.unwrap_or(description))
    }

    /// The viewport size in CSS pixels.
    pub async fn viewport(&self) -> Result<(f64, f64)> {
        let size = self.evaluate("[innerWidth, innerHeight]").await?;
        match (size[0].as_f64(), size[1].as_f64()) {
            (Some(width), Some(height)) => Ok((width, height)),
            _ => bail!("the page did not report its viewport size"),
        }
    }

    pub async fn snapshot(&self) -> Result<String> {
        let value = self.evaluate(SNAPSHOT_JS).await?;
        value
            .as_str()
            .map(str::to_owned)
            .context("the snapshot script returned no text")
    }

    /// Navigates and waits for the load event. Returns `false` when the page
    /// was still loading at the timeout.
    pub async fn navigate(&self, url: &str) -> Result<bool> {
        let mut events = self.events();
        let reply = self.call("Page.navigate", json!({ "url": url })).await?;
        if let Some(error) = reply["errorText"].as_str().filter(|e| !e.is_empty()) {
            bail!("navigation to {url} failed: {error}");
        }
        if reply.get("loaderId").is_none() {
            return Ok(true);
        }
        let session = self.session.as_str();
        let loaded = wait_for(&mut events, LOAD_TIMEOUT, |event| {
            event.method == "Page.loadEventFired" && event.session.as_deref() == Some(session)
        })
        .await;
        Ok(loaded.is_some())
    }

    pub async fn back(&self) -> Result<()> {
        let history = self.call("Page.getNavigationHistory", json!({})).await?;
        let current = history["currentIndex"].as_u64().unwrap_or(0) as usize;
        let Some(previous) = current.checked_sub(1) else {
            bail!("there is no previous page in this tab's history");
        };
        let mut events = self.events();
        self.call(
            "Page.navigateToHistoryEntry",
            json!({ "entryId": history["entries"][previous]["id"] }),
        )
        .await?;
        self.settle(&mut events).await;
        Ok(())
    }

    /// After input, waits for the navigation it started, if any. `events`
    /// must have been subscribed before the input was sent.
    pub async fn settle(&self, events: &mut broadcast::Receiver<Event>) {
        let main_frame = |event: &Event, method: &str| {
            event.method == method
                && event.session.as_deref() == Some(self.session.as_str())
                && event.params["frameId"].as_str() == Some(self.target.as_str())
        };
        let started = wait_for(events, NAVIGATION_GRACE, |e| {
            main_frame(e, "Page.frameStartedLoading")
        })
        .await;
        if started.is_some() {
            wait_for(events, SETTLE_TIMEOUT, |e| {
                main_frame(e, "Page.frameStoppedLoading")
            })
            .await;
        }
    }

    /// Scrolls the element into view and returns where it is. With `focus`,
    /// focuses it, selecting its contents when `clear` is set.
    pub async fn locate(&self, locator: &Locator, focus: bool, clear: bool) -> Result<Located> {
        let mut args = locator_args(locator);
        args["focus"] = focus.into();
        args["clear"] = clear.into();
        serde_json::from_value(self.element(args).await?)
            .context("unexpected reply from the element script")
    }

    /// Picks the `<select>` option matching `option`; returns the element
    /// label and the chosen option.
    pub async fn select_option(&self, locator: &Locator, option: &str) -> Result<(String, String)> {
        let mut args = locator_args(locator);
        args["option"] = option.into();
        let reply = self.element(args).await?;
        let field = |name: &str| reply[name].as_str().unwrap_or_default().to_owned();
        Ok((field("label"), field("selected")))
    }

    async fn element(&self, args: Value) -> Result<Value> {
        let reply = self.evaluate(&format!("{ELEMENT_JS}({args})")).await?;
        if let Some(error) = reply["error"].as_str() {
            bail!("{error}");
        }
        Ok(reply)
    }

    pub async fn click(&self, x: f64, y: f64, button: Button, count: u8) -> Result<()> {
        self.mouse(json!({ "type": "mouseMoved", "x": x, "y": y }))
            .await?;
        for click_count in 1..=count {
            let event = |kind: &str, buttons: u8| {
                json!({
                    "type": kind,
                    "x": x,
                    "y": y,
                    "button": button.name(),
                    "buttons": buttons,
                    "clickCount": click_count,
                })
            };
            self.mouse(event("mousePressed", button.mask())).await?;
            self.mouse(event("mouseReleased", 0)).await?;
        }
        Ok(())
    }

    pub async fn wheel(&self, x: f64, y: f64, dx: f64, dy: f64) -> Result<()> {
        self.mouse(json!({
            "type": "mouseWheel",
            "x": x,
            "y": y,
            "deltaX": dx,
            "deltaY": dy,
        }))
        .await
    }

    async fn mouse(&self, params: Value) -> Result<()> {
        self.call("Input.dispatchMouseEvent", params).await?;
        Ok(())
    }

    pub async fn insert_text(&self, text: &str) -> Result<()> {
        self.call("Input.insertText", json!({ "text": text }))
            .await?;
        Ok(())
    }

    pub async fn press(&self, combo: &KeyCombo) -> Result<()> {
        for event in keyboard::key_events(combo) {
            self.call("Input.dispatchKeyEvent", event).await?;
        }
        Ok(())
    }

    /// A JPEG of the viewport or the whole page, in CSS pixels so that its
    /// coordinates match the ones input events take.
    pub async fn screenshot(&self, full_page: bool) -> Result<Screenshot> {
        let ratio = self
            .evaluate("devicePixelRatio")
            .await?
            .as_f64()
            .filter(|ratio| *ratio > 0.0)
            .unwrap_or(1.0);
        let mut params = json!({ "format": "jpeg", "quality": JPEG_QUALITY });
        let mut cut_from = None;
        if full_page {
            let metrics = self.call("Page.getLayoutMetrics", json!({})).await?;
            let size = &metrics["cssContentSize"];
            let (Some(width), Some(height)) = (size["width"].as_f64(), size["height"].as_f64())
            else {
                bail!("the browser did not report the page size");
            };
            let shown = height.min(MAX_FULL_PAGE_HEIGHT);
            if shown < height {
                cut_from = Some(height);
            }
            params["captureBeyondViewport"] = true.into();
            params["clip"] = json!({ "x": 0, "y": 0, "width": width, "height": shown, "scale": 1 });
        }
        let reply = self.call("Page.captureScreenshot", params).await?;
        let data = BASE64
            .decode(reply["data"].as_str().unwrap_or_default())
            .context("the browser returned malformed screenshot data")?;
        let image = tokio::task::spawn_blocking(move || imaging::shrink_jpeg(data, ratio))
            .await
            .context("screenshot task panicked")??;
        Ok(Screenshot { image, cut_from })
    }
}

fn locator_args(locator: &Locator) -> Value {
    match locator {
        Locator::Ref(id) => json!({ "ref": id }),
        Locator::Selector(selector) => json!({ "selector": selector }),
        Locator::Focused => json!({ "focused": true }),
    }
}
