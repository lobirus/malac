//! The MCP handler combining the computer and browser tool sets.

use anyhow::anyhow;
use async_trait::async_trait;
use malac_mcp::{CallToolResult, Handler, Implementation, Tool};
use serde_json::Value;

use crate::browser::Browser;
use crate::computer::Computer;
use crate::config::Config;

const COMPUTER_INSTRUCTIONS: &str = "computer_* tools control the desktop. Take a \
computer_screenshot first: x/y arguments are pixels in the latest screenshot of that display.";

const BROWSER_INSTRUCTIONS: &str = "browser_* tools drive a Chromium browser. Load pages with \
browser_navigate, then call browser_snapshot to read the page; it tags interactive elements \
with refs like [12] that browser_click and browser_type accept as `ref`. Every browser tool \
takes an optional `profile` (default \"default\"); most take an optional `tab`, defaulting to \
the tab used last.";

pub struct Control {
    computer: Option<Computer>,
    browser: Option<Browser>,
}

impl Control {
    pub fn new(config: Config) -> Self {
        Self {
            computer: config
                .computer
                .enabled
                .then(|| Computer::new(&config.computer)),
            browser: config.browser.enabled.then(|| Browser::new(config.browser)),
        }
    }

    /// Closes the browsers this process launched. Browsers attached through a
    /// `cdp_url` keep running.
    pub async fn shutdown(&self) {
        if let Some(browser) = &self.browser {
            browser.shutdown().await;
        }
    }
}

#[async_trait]
impl Handler for Control {
    fn info(&self) -> Implementation {
        Implementation {
            name: "malac-control".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    fn instructions(&self) -> Option<String> {
        let parts: Vec<&str> = [
            self.computer.as_ref().map(|_| COMPUTER_INSTRUCTIONS),
            self.browser.as_ref().map(|_| BROWSER_INSTRUCTIONS),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }

    async fn list_tools(&self) -> Vec<Tool> {
        let mut tools = Vec::new();
        if self.computer.is_some() {
            tools.extend(Computer::tools());
        }
        if self.browser.is_some() {
            tools.extend(Browser::tools());
        }
        tools
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> CallToolResult {
        let outcome = if name.starts_with("computer_") {
            match &self.computer {
                Some(computer) => computer.call(name, arguments).await,
                None => Err(anyhow!("computer control is disabled in the configuration")),
            }
        } else if name.starts_with("browser_") {
            match &self.browser {
                Some(browser) => browser.call(name, arguments).await,
                None => Err(anyhow!("browser control is disabled in the configuration")),
            }
        } else {
            Err(anyhow!("unknown tool `{name}`"))
        };
        outcome.unwrap_or_else(|err| CallToolResult::error(format!("{err:#}")))
    }
}
