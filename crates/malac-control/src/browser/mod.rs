//! Browser control over the Chrome DevTools Protocol.

mod cdp;
mod instance;
mod keyboard;
mod launch;
mod page;
mod profiles;
mod tools;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use malac_mcp::{CallToolResult, Tool};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::config::{BrowserConfig, DEFAULT_PROFILE};
use crate::keys::{Key, KeyCombo};
use crate::tool::{parse, truncate};
use instance::{Instance, short_id};
use page::{Button, Locator, Page};
use tools::{
    Aim, ClickArgs, EmulateArgs, EvalArgs, NavigateArgs, NewTabArgs, OpenArgs, PressArgs,
    ProfileArgs, ScreenshotArgs, ScrollArgs, TabArgs, TypeArgs, WaitArgs,
};

const SNAPSHOT_LIMIT: usize = 20_000;
const EVAL_LIMIT: usize = 20_000;
const DEFAULT_WAIT_SECS: f64 = 10.0;
const MAX_WAIT_SECS: f64 = 60.0;
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const SCROLL_SETTLE: Duration = Duration::from_millis(300);
/// Time for layout and media-query listeners to catch up with an emulation.
const EMULATE_SETTLE: Duration = Duration::from_millis(200);

/// Lists the profiles in `config` and the system profiles they can clone.
pub fn list_profiles(config: &BrowserConfig) -> String {
    profiles::report(config, &BTreeMap::new())
}

pub struct Browser {
    config: BrowserConfig,
    instances: Mutex<HashMap<String, Arc<Instance>>>,
}

impl Browser {
    pub fn new(config: BrowserConfig) -> Self {
        Self {
            config,
            instances: Mutex::default(),
        }
    }

    pub fn tools() -> Vec<Tool> {
        tools::definitions()
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<CallToolResult> {
        match name {
            "browser_profiles" => self.profiles().await,
            "browser_open" => self.open(parse(name, args)?).await,
            "browser_tabs" => self.tabs(parse(name, args)?).await,
            "browser_new_tab" => self.new_tab(parse(name, args)?).await,
            "browser_navigate" => self.navigate(parse(name, args)?).await,
            "browser_back" => self.back(parse(name, args)?).await,
            "browser_snapshot" => self.snapshot(parse(name, args)?).await,
            "browser_click" => self.click(parse(name, args)?).await,
            "browser_type" => self.type_text(parse(name, args)?).await,
            "browser_press" => self.press(parse(name, args)?).await,
            "browser_scroll" => self.scroll(parse(name, args)?).await,
            "browser_screenshot" => self.screenshot(parse(name, args)?).await,
            "browser_emulate" => self.emulate(parse(name, args)?).await,
            "browser_eval" => self.eval(parse(name, args)?).await,
            "browser_wait" => self.wait(parse(name, args)?).await,
            "browser_close_tab" => self.close_tab(parse(name, args)?).await,
            "browser_close" => self.close(parse(name, args)?).await,
            _ => bail!("unknown tool `{name}`"),
        }
    }

    /// Closes every browser this process manages.
    pub async fn shutdown(&self) {
        let instances: Vec<Arc<Instance>> = self
            .instances
            .lock()
            .await
            .drain()
            .map(|(_, i)| i)
            .collect();
        for instance in instances {
            instance.close().await;
        }
    }

    /// The running browser of `profile`. With `launch`, a managed profile's
    /// browser is started when it is not running.
    async fn instance(&self, profile: Option<&str>, launch: bool) -> Result<Arc<Instance>> {
        let name = profile.unwrap_or(DEFAULT_PROFILE);
        let mut instances = self.instances.lock().await;
        if let Some(instance) = instances.get(name) {
            if instance.is_alive() {
                return Ok(instance.clone());
            }
            instances.remove(name);
        }
        let profile = profiles::resolve(&self.config, name)?;
        let instance = Arc::new(Instance::connect(&profile, &self.config, launch).await?);
        instances.insert(name.to_owned(), instance.clone());
        Ok(instance)
    }

    async fn page(&self, at: &TabArgs, launch: bool) -> Result<(Arc<Instance>, Page)> {
        let instance = self.instance(at.profile.as_deref(), launch).await?;
        let page = instance.page(at.tab.as_deref()).await?;
        Ok((instance, page))
    }

    async fn profiles(&self) -> Result<CallToolResult> {
        let running: BTreeMap<String, String> = self
            .instances
            .lock()
            .await
            .iter()
            .filter(|(_, instance)| instance.is_alive())
            .map(|(name, instance)| (name.clone(), instance.status()))
            .collect();
        Ok(CallToolResult::text(profiles::report(
            &self.config,
            &running,
        )))
    }

    async fn open(&self, args: OpenArgs) -> Result<CallToolResult> {
        let instance = self.instance(args.profile.as_deref(), true).await?;
        let mut parts = Vec::new();
        if let Some(url) = &args.url {
            let page = instance.page(None).await?;
            let loaded = page.navigate(&normalize_url(url)).await?;
            parts.push(load_summary(&page, loaded).await?);
        }
        parts.push(tab_list(&instance).await?);
        Ok(reply(&instance, parts.join("\n\n")))
    }

    async fn tabs(&self, args: ProfileArgs) -> Result<CallToolResult> {
        let instance = self.instance(args.profile.as_deref(), false).await?;
        let list = tab_list(&instance).await?;
        Ok(reply(&instance, list))
    }

    async fn new_tab(&self, args: NewTabArgs) -> Result<CallToolResult> {
        let instance = self.instance(args.profile.as_deref(), true).await?;
        let page = instance.new_tab().await?;
        let mut text = format!("Opened tab {}.", short_id(page.id()));
        if let Some(url) = &args.url {
            let loaded = page.navigate(&normalize_url(url)).await?;
            text = format!("{text} {}", load_summary(&page, loaded).await?);
        }
        Ok(reply(&instance, text))
    }

    async fn navigate(&self, args: NavigateArgs) -> Result<CallToolResult> {
        let (instance, page) = self.page(&args.at, true).await?;
        let loaded = page.navigate(&normalize_url(&args.url)).await?;
        let summary = load_summary(&page, loaded).await?;
        Ok(reply(&instance, summary))
    }

    async fn back(&self, args: TabArgs) -> Result<CallToolResult> {
        let (instance, page) = self.page(&args, false).await?;
        page.back().await?;
        let summary = load_summary(&page, true).await?;
        Ok(reply(&instance, summary))
    }

    async fn snapshot(&self, args: TabArgs) -> Result<CallToolResult> {
        let (instance, page) = self.page(&args, false).await?;
        let text = truncate(page.snapshot().await?, SNAPSHOT_LIMIT);
        Ok(reply(&instance, text))
    }

    async fn click(&self, args: ClickArgs) -> Result<CallToolResult> {
        let count = args.count.unwrap_or(1);
        if !(1..=3).contains(&count) {
            bail!("count must be 1, 2 or 3");
        }
        let (instance, page) = self.page(&args.at, false).await?;
        let (x, y, what, covered) = match args.target.aim()? {
            Some(Aim::Element(locator)) => {
                let found = page.locate(&locator, false, false).await?;
                (found.x, found.y, found.label, found.covered)
            }
            Some(Aim::Point(x, y)) => {
                ensure_in_viewport(x, y, page.viewport().await?)?;
                (x, y, format!("({x}, {y})"), None)
            }
            None => bail!("pass `ref`, `selector`, or `x` and `y`"),
        };
        let mut events = page.events();
        page.click(x, y, args.button, count).await?;
        page.settle(&mut events).await;
        let mut text = format!("Clicked {what}.");
        if let Some(covered) = covered {
            text.push_str(&format!(" {covered} was on top of it at the click point."));
        }
        text.push_str(&format!("\nPage: {}", page_summary(&page).await?));
        Ok(reply(&instance, text))
    }

    async fn type_text(&self, args: TypeArgs) -> Result<CallToolResult> {
        let (instance, page) = self.page(&args.at, false).await?;
        let locator = match args.target.aim()? {
            Some(Aim::Element(locator)) => locator,
            Some(Aim::Point(x, y)) => {
                ensure_in_viewport(x, y, page.viewport().await?)?;
                page.click(x, y, Button::Left, 1).await?;
                Locator::Focused
            }
            None => Locator::Focused,
        };
        let found = page.locate(&locator, true, args.clear).await?;
        if found.select {
            let (label, option) = page.select_option(&locator, &args.text).await?;
            return Ok(reply(&instance, format!("Selected {option:?} in {label}.")));
        }
        let mut text = if args.text.is_empty() {
            if args.clear {
                page.press(&Key::Backspace.into()).await?;
            }
            format!("Cleared {}.", found.label)
        } else {
            page.insert_text(&args.text).await?;
            format!(
                "Typed {} characters into {}.",
                args.text.chars().count(),
                found.label
            )
        };
        if args.submit {
            let mut events = page.events();
            page.press(&Key::Enter.into()).await?;
            page.settle(&mut events).await;
            text.push_str(&format!(
                " Pressed Enter.\nPage: {}",
                page_summary(&page).await?
            ));
        }
        Ok(reply(&instance, text))
    }

    async fn press(&self, args: PressArgs) -> Result<CallToolResult> {
        let combo: KeyCombo = args.key.parse()?;
        let (instance, page) = self.page(&args.at, false).await?;
        let mut events = page.events();
        page.press(&combo).await?;
        page.settle(&mut events).await;
        let text = format!(
            "Pressed {}.\nPage: {}",
            args.key.trim(),
            page_summary(&page).await?
        );
        Ok(reply(&instance, text))
    }

    async fn scroll(&self, args: ScrollArgs) -> Result<CallToolResult> {
        let (instance, page) = self.page(&args.at, false).await?;
        let delta = (args.dx.unwrap_or(0.0), args.dy.unwrap_or(0.0));
        let viewport = page.viewport().await?;
        let (x, y) = match args.target.aim()? {
            Some(Aim::Element(locator)) => {
                let found = page.locate(&locator, false, false).await?;
                if delta == (0.0, 0.0) {
                    return Ok(reply(
                        &instance,
                        format!("Scrolled {} into view.", found.label),
                    ));
                }
                (found.x, found.y)
            }
            Some(Aim::Point(x, y)) => {
                ensure_in_viewport(x, y, viewport)?;
                (x, y)
            }
            None => (viewport.0 / 2.0, viewport.1 / 2.0),
        };
        let (dx, dy) = if delta == (0.0, 0.0) {
            (0.0, (viewport.1 * 0.8).round())
        } else {
            delta
        };
        page.wheel(x, y, dx, dy).await?;
        tokio::time::sleep(SCROLL_SETTLE).await;
        let position = page
            .evaluate("[scrollX, scrollY, document.documentElement.scrollHeight - innerHeight]")
            .await?;
        let number = |i: usize| position[i].as_f64().unwrap_or(0.0).round();
        Ok(reply(
            &instance,
            format!(
                "Scrolled by ({dx}, {dy}). The page is scrolled to ({}, {}) of at most {} vertically.",
                number(0),
                number(1),
                number(2).max(0.0)
            ),
        ))
    }

    async fn screenshot(&self, args: ScreenshotArgs) -> Result<CallToolResult> {
        let (instance, page) = self.page(&args.at, false).await?;
        let shot = page.screenshot(args.full_page).await?;
        let (width, height) = (shot.image.width, shot.image.height);
        let mut text = if args.full_page {
            format!(
                "Full-page screenshot, {width}x{height} CSS pixels; its coordinates are page coordinates, not viewport coordinates."
            )
        } else {
            format!(
                "Viewport screenshot, {width}x{height} CSS pixels; x/y for browser_click, browser_type and browser_scroll are pixels in this image."
            )
        };
        if let Some(full) = shot.cut_from {
            text.push_str(&format!(
                " The page is {full}px tall; only the top {height}px are shown."
            ));
        }
        text.push_str(&format!("\nPage: {}", page_summary(&page).await?));
        Ok(reply(&instance, text).with_image(BASE64.encode(&shot.image.data), "image/jpeg"))
    }

    async fn emulate(&self, args: EmulateArgs) -> Result<CallToolResult> {
        let screen = args.screen()?;
        if args.reset && (screen.is_some() || args.media()) {
            bail!("`reset` takes no other settings");
        }
        if !args.reset && screen.is_none() && !args.media() {
            bail!("pass `width` and `height`, `color_scheme`, `reduced_motion`, or `reset`");
        }
        let (instance, page) = self.page(&args.at, false).await?;
        if args.reset || screen.is_some() {
            page.emulate_screen(screen.as_ref()).await?;
        }
        if args.reset || args.media() {
            page.emulate_media(args.color_scheme, args.reduced_motion)
                .await?;
        }
        tokio::time::sleep(EMULATE_SETTLE).await;

        let (width, height) = page.viewport().await?;
        let media = page
            .evaluate(
                "['(prefers-color-scheme: dark)', '(prefers-reduced-motion: reduce)', '(pointer: coarse)'].map(q => matchMedia(q).matches)",
            )
            .await?;
        let on = |i: usize| media[i].as_bool().unwrap_or(false);
        let mut text = format!(
            "Viewport {width}x{height}{}, prefers-color-scheme: {}, prefers-reduced-motion: {}.",
            if on(2) { " (touch)" } else { "" },
            if on(0) { "dark" } else { "light" },
            if on(1) { "reduce" } else { "no-preference" },
        );
        if let Some(screen) = screen.as_ref().filter(|s| s.mobile) {
            if width > f64::from(screen.width) + 1.0 {
                text.push_str(&format!(
                    " The page lays out {width} px wide on a {} px screen, so something overflows and a phone zooms out to fit it.",
                    screen.width
                ));
            }
        }
        if !args.reset {
            text.push_str(" It stays for this tab until `reset: true`.");
        }
        text.push_str(&format!("\nPage: {}", page_summary(&page).await?));
        Ok(reply(&instance, text))
    }

    async fn eval(&self, args: EvalArgs) -> Result<CallToolResult> {
        let (instance, page) = self.page(&args.at, false).await?;
        let text = page.evaluate_for_display(&args.expression).await?;
        Ok(reply(&instance, truncate(text, EVAL_LIMIT)))
    }

    async fn wait(&self, args: WaitArgs) -> Result<CallToolResult> {
        let mut checks = Vec::new();
        let mut wanted = Vec::new();
        if let Some(text) = &args.text {
            checks.push(format!(
                "(document.body?.innerText ?? '').includes({})",
                json!(text)
            ));
            wanted.push(format!("text {text:?}"));
        }
        if let Some(selector) = &args.selector {
            checks.push(format!(
                "document.querySelector({}) !== null",
                json!(selector)
            ));
            wanted.push(format!("selector {selector:?}"));
        }
        if checks.is_empty() {
            let Some(ms) = args.ms else {
                bail!("pass `text`, `selector` or `ms`");
            };
            let ms = ms.min((MAX_WAIT_SECS * 1000.0) as u64);
            tokio::time::sleep(Duration::from_millis(ms)).await;
            return Ok(CallToolResult::text(format!("Waited {ms} ms.")));
        }
        let timeout = args.timeout.unwrap_or(DEFAULT_WAIT_SECS);
        if !(0.0..=MAX_WAIT_SECS).contains(&timeout) {
            bail!("timeout must be between 0 and {MAX_WAIT_SECS} seconds");
        }
        let (instance, page) = self.page(&args.at, false).await?;
        // Exceptions (such as an invalid selector) come back as strings, so
        // they end the wait instead of being retried like navigation errors.
        let expression = format!(
            "(() => {{ try {{ return {}; }} catch (e) {{ return String(e); }} }})()",
            checks.join(" && ")
        );
        let what = wanted.join(" and ");
        let started = Instant::now();
        let deadline = started + Duration::from_secs_f64(timeout);
        loop {
            match page.evaluate(&expression).await {
                Ok(Value::Bool(true)) => {
                    let elapsed = started.elapsed().as_millis();
                    return Ok(reply(
                        &instance,
                        format!("Found {what} after {elapsed} ms."),
                    ));
                }
                Ok(Value::String(error)) => bail!("{error}"),
                Ok(_) | Err(_) => {}
            }
            if Instant::now() >= deadline {
                bail!("timed out after {timeout} s waiting for {what}");
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    async fn close_tab(&self, args: TabArgs) -> Result<CallToolResult> {
        let instance = self.instance(args.profile.as_deref(), false).await?;
        let tab = instance.close_tab(args.tab.as_deref()).await?;
        Ok(reply(
            &instance,
            format!("Closed tab {} ({}).", short_id(&tab.id), tab.url),
        ))
    }

    async fn close(&self, args: ProfileArgs) -> Result<CallToolResult> {
        let name = args.profile.as_deref().unwrap_or(DEFAULT_PROFILE);
        let open = self.instances.lock().await.remove(name);
        let instance = match open {
            Some(instance) => instance,
            // A browser left running by an earlier process can be closed too.
            None => {
                let profile = profiles::resolve(&self.config, name)?;
                match Instance::connect(&profile, &self.config, false).await {
                    Ok(instance) => Arc::new(instance),
                    Err(_) => {
                        let text = format!("Profile `{name}` is not open.");
                        return Ok(CallToolResult::text(text));
                    }
                }
            }
        };
        Ok(CallToolResult::text(instance.close().await))
    }
}

/// The browser silently drops input outside the viewport.
fn ensure_in_viewport(x: f64, y: f64, (width, height): (f64, f64)) -> Result<()> {
    if (0.0..width).contains(&x) && (0.0..height).contains(&y) {
        Ok(())
    } else {
        bail!("({x}, {y}) is outside the {width}x{height} viewport")
    }
}

/// `Title (url)` of the tab.
async fn page_summary(page: &Page) -> Result<String> {
    let (title, url) = page.info().await?;
    let title = if title.is_empty() {
        "(untitled)"
    } else {
        &title
    };
    Ok(format!("{title} ({url})"))
}

/// The tool result text, preceded by anything the browser reported meanwhile.
fn reply(instance: &Instance, text: String) -> CallToolResult {
    let notices = instance.take_notices();
    if notices.is_empty() {
        CallToolResult::text(text)
    } else {
        CallToolResult::text(format!("{}\n\n{text}", notices.join("\n")))
    }
}

async fn load_summary(page: &Page, loaded: bool) -> Result<String> {
    let summary = page_summary(page).await?;
    Ok(if loaded {
        format!("Loaded {summary}")
    } else {
        format!("Still loading {summary}; the load event has not fired yet")
    })
}

async fn tab_list(instance: &Instance) -> Result<String> {
    let tabs = instance.tabs().await?;
    if tabs.is_empty() {
        return Ok("No open tabs.".into());
    }
    let current = instance.default_tab(&tabs).map(|tab| tab.id.clone());
    let lines: Vec<String> = tabs
        .iter()
        .map(|tab| {
            let marker = if Some(&tab.id) == current.as_ref() {
                '*'
            } else {
                ' '
            };
            let title = if tab.title.is_empty() {
                "(untitled)"
            } else {
                &tab.title
            };
            format!("{marker} {}  {title}  {}", short_id(&tab.id), tab.url)
        })
        .collect();
    Ok(format!("Tabs:\n{}", lines.join("\n")))
}

/// Adds a scheme to bare addresses: `http://` for local hosts, `https://`
/// otherwise.
fn normalize_url(url: &str) -> String {
    const OPAQUE: &[&str] = &[
        "about:",
        "data:",
        "javascript:",
        "blob:",
        "file:",
        "view-source:",
    ];
    const LOCAL: &[&str] = &["localhost", "127.0.0.1", "[::1]"];
    let url = url.trim();
    if url.contains("://") || OPAQUE.iter().any(|scheme| url.starts_with(scheme)) {
        return url.to_owned();
    }
    let local = LOCAL.iter().any(|host| {
        url.strip_prefix(host)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
    });
    let scheme = if local { "http" } else { "https" };
    format!("{scheme}://{url}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_bounds() {
        assert!(ensure_in_viewport(0.0, 0.0, (1280.0, 800.0)).is_ok());
        assert!(ensure_in_viewport(1279.5, 799.0, (1280.0, 800.0)).is_ok());
        assert!(ensure_in_viewport(1280.0, 10.0, (1280.0, 800.0)).is_err());
        assert!(ensure_in_viewport(10.0, -1.0, (1280.0, 800.0)).is_err());
    }

    #[test]
    fn normalizes_urls() {
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(
            normalize_url("example.com/a?b=c"),
            "https://example.com/a?b=c"
        );
        assert_eq!(normalize_url("https://example.com"), "https://example.com");
        assert_eq!(normalize_url("about:blank"), "about:blank");
        assert_eq!(
            normalize_url("data:text/html,<p>hi</p>"),
            "data:text/html,<p>hi</p>"
        );
        assert_eq!(normalize_url("chrome://version"), "chrome://version");
        assert_eq!(
            normalize_url("example.com:8080/x"),
            "https://example.com:8080/x"
        );
        assert_eq!(
            normalize_url("localhost:3000/app"),
            "http://localhost:3000/app"
        );
        assert_eq!(normalize_url("127.0.0.1"), "http://127.0.0.1");
        assert_eq!(
            normalize_url("localhost.example.com"),
            "https://localhost.example.com"
        );
    }
}
