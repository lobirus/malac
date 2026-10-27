//! The browser tool interface: JSON schemas and the arguments they decode into.

use anyhow::{Result, bail};
use malac_mcp::Tool;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use super::page::{Button, Locator};
use crate::tool::define;

pub fn definitions() -> Vec<Tool> {
    let profile = json!({
        "type": "string",
        "description": "Browser profile from browser_profiles. Defaults to \"default\"."
    });
    let tab = json!({
        "type": "string",
        "description": "Tab id (or a unique prefix) from browser_tabs. Defaults to the tab used last."
    });
    let at = |extra: Value| {
        let mut props = Map::new();
        props.insert("profile".into(), profile.clone());
        props.insert("tab".into(), tab.clone());
        if let Value::Object(extra) = extra {
            props.extend(extra);
        }
        Value::Object(props)
    };
    let target = |extra: Value| {
        let mut props = json!({
            "ref": { "type": "integer", "description": "Element ref from browser_snapshot, e.g. 12 for [12]" },
            "selector": { "type": "string", "description": "CSS selector" },
            "x": { "type": "number", "description": "Viewport X in CSS pixels, as in browser_screenshot" },
            "y": { "type": "number", "description": "Viewport Y in CSS pixels" }
        });
        if let (Value::Object(props), Value::Object(extra)) = (&mut props, extra) {
            props.extend(extra);
        }
        at(props)
    };
    vec![
        define(
            "browser_profiles",
            "List browser profiles, whether each is running, and the system browser profiles that can be cloned.",
            json!({}),
            &[],
        )
        .read_only(),
        define(
            "browser_open",
            "Launch the profile's browser, or attach to it for profiles with a cdp_url, and optionally load a URL in the current tab. Lists the open tabs.",
            json!({ "profile": profile, "url": { "type": "string" } }),
            &[],
        ),
        define(
            "browser_tabs",
            "List the open tabs of a running profile. The current tab is marked with *.",
            json!({ "profile": profile }),
            &[],
        )
        .read_only(),
        define(
            "browser_new_tab",
            "Open a new tab, optionally loading a URL, and make it the current tab.",
            json!({ "profile": profile, "url": { "type": "string" } }),
            &[],
        ),
        define(
            "browser_navigate",
            "Load a URL in a tab and wait for it. Launches the profile's browser if needed.",
            at(json!({ "url": { "type": "string" } })),
            &["url"],
        ),
        define("browser_back", "Go back one page in a tab's history.", at(json!({})), &[]),
        define(
            "browser_snapshot",
            "Outline the page as text: title, URL, headings, visible text and interactive elements tagged with refs like [12] for browser_click and browser_type.",
            at(json!({})),
            &[],
        )
        .read_only(),
        define(
            "browser_click",
            "Click an element by `ref` or `selector`, or a viewport point, with real mouse events.",
            target(json!({
                "button": { "type": "string", "enum": ["left", "right", "middle"] },
                "count": { "type": "integer", "minimum": 1, "maximum": 3, "description": "2 for a double click" }
            })),
            &[],
        ),
        define(
            "browser_type",
            "Type text into an element (by `ref`, `selector` or point) or into the focused element. On a <select>, picks the option whose label or value matches the text.",
            target(json!({
                "text": { "type": "string" },
                "clear": { "type": "boolean", "description": "Replace the current content" },
                "submit": { "type": "boolean", "description": "Press Enter afterwards" }
            })),
            &["text"],
        ),
        define(
            "browser_press",
            "Press a key or combination in a tab, e.g. `Enter`, `Escape`, `ArrowDown`, `Tab` or `ctrl+a`.",
            at(json!({ "key": { "type": "string" } })),
            &["key"],
        ),
        define(
            "browser_scroll",
            "Scroll with the mouse wheel by dx/dy CSS pixels at an element or point (default: the viewport center). With only `ref` or `selector`, scrolls that element into view. Without arguments, scrolls down most of a viewport.",
            target(json!({ "dx": { "type": "number" }, "dy": { "type": "number" } })),
            &[],
        ),
        define(
            "browser_screenshot",
            "Capture the viewport, or the whole page with full_page, as a JPEG in CSS pixels.",
            at(json!({ "full_page": { "type": "boolean" } })),
            &[],
        )
        .read_only(),
        define(
            "browser_emulate",
            "Emulate a screen size and media preferences in a tab, to check a page at other widths and in dark mode: `width` with `height` (and `mobile` for a phone viewport with touch), `color_scheme`, `reduced_motion`. Size and media are set independently; within each, a value left out goes back to the browser's own. It lasts until `reset` or the tab closes.",
            at(json!({
                "width": { "type": "integer", "minimum": 100, "maximum": 4000, "description": "Viewport width in CSS pixels, e.g. 1440, 1024 or 390" },
                "height": { "type": "integer", "minimum": 100, "maximum": 4000, "description": "Viewport height in CSS pixels, e.g. 900, 768 or 844" },
                "mobile": { "type": "boolean", "description": "Emulate a phone: mobile viewport rules and touch input" },
                "color_scheme": { "type": "string", "enum": ["light", "dark"], "description": "prefers-color-scheme" },
                "reduced_motion": { "type": "boolean", "description": "prefers-reduced-motion: reduce" },
                "reset": { "type": "boolean", "description": "Back to the window's own size and the system's preferences" }
            })),
            &[],
        ),
        define(
            "browser_eval",
            "Evaluate JavaScript in the page like the DevTools console, awaiting promises. Plain values come back as JSON; DOM nodes, functions and other objects as their description.",
            at(json!({ "expression": { "type": "string" } })),
            &["expression"],
        ),
        define(
            "browser_wait",
            "Wait until the page contains `text` or an element matches `selector`, or just wait `ms` milliseconds.",
            at(json!({
                "text": { "type": "string" },
                "selector": { "type": "string" },
                "ms": { "type": "integer", "minimum": 0 },
                "timeout": { "type": "number", "description": "Seconds to wait for text or selector, default 10, at most 60" }
            })),
            &[],
        )
        .read_only(),
        define("browser_close_tab", "Close a tab.", at(json!({})), &[]),
        define(
            "browser_close",
            "Close the profile's browser. Browsers attached through cdp_url are only disconnected.",
            json!({ "profile": profile }),
            &[],
        ),
    ]
}

#[derive(Deserialize)]
pub struct ProfileArgs {
    pub profile: Option<String>,
}

#[derive(Deserialize)]
pub struct OpenArgs {
    pub profile: Option<String>,
    pub url: Option<String>,
}

#[derive(Deserialize)]
pub struct TabArgs {
    pub profile: Option<String>,
    pub tab: Option<String>,
}

/// Where an action happens: an element or a viewport point.
#[derive(Deserialize)]
pub struct TargetArgs {
    #[serde(rename = "ref", default, deserialize_with = "element_ref")]
    pub element: Option<u32>,
    pub selector: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
}

pub enum Aim {
    Element(Locator),
    Point(f64, f64),
}

impl TargetArgs {
    pub fn aim(self) -> Result<Option<Aim>> {
        Ok(match (self.element, self.selector, self.x, self.y) {
            (Some(id), None, None, None) => Some(Aim::Element(Locator::Ref(id))),
            (None, Some(selector), None, None) => Some(Aim::Element(Locator::Selector(selector))),
            (None, None, Some(x), Some(y)) => Some(Aim::Point(x, y)),
            (None, None, None, None) => None,
            _ => bail!("pass one of `ref`, `selector`, or `x` with `y`"),
        })
    }
}

/// Accepts `12`, `"12"` or `"[12]"`.
fn element_ref<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u32>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Number(u32),
        Text(String),
    }
    match Option::<Raw>::deserialize(deserializer)? {
        None => Ok(None),
        Some(Raw::Number(id)) => Ok(Some(id)),
        Some(Raw::Text(text)) => text
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .map(Some)
            .map_err(|_| {
                D::Error::custom(format!("invalid ref `{text}`, expected a number like 12"))
            }),
    }
}

#[derive(Deserialize)]
pub struct NavigateArgs {
    #[serde(flatten)]
    pub at: TabArgs,
    pub url: String,
}

#[derive(Deserialize)]
pub struct NewTabArgs {
    pub profile: Option<String>,
    pub url: Option<String>,
}

#[derive(Deserialize)]
pub struct ClickArgs {
    #[serde(flatten)]
    pub at: TabArgs,
    #[serde(flatten)]
    pub target: TargetArgs,
    #[serde(default)]
    pub button: Button,
    pub count: Option<u8>,
}

#[derive(Deserialize)]
pub struct TypeArgs {
    #[serde(flatten)]
    pub at: TabArgs,
    #[serde(flatten)]
    pub target: TargetArgs,
    pub text: String,
    #[serde(default)]
    pub clear: bool,
    #[serde(default)]
    pub submit: bool,
}

#[derive(Deserialize)]
pub struct PressArgs {
    #[serde(flatten)]
    pub at: TabArgs,
    pub key: String,
}

#[derive(Deserialize)]
pub struct ScrollArgs {
    #[serde(flatten)]
    pub at: TabArgs,
    #[serde(flatten)]
    pub target: TargetArgs,
    pub dx: Option<f64>,
    pub dy: Option<f64>,
}

#[derive(Deserialize)]
pub struct ScreenshotArgs {
    #[serde(flatten)]
    pub at: TabArgs,
    #[serde(default)]
    pub full_page: bool,
}

#[derive(Deserialize)]
pub struct EmulateArgs {
    #[serde(flatten)]
    pub at: TabArgs,
    pub width: Option<u32>,
    pub height: Option<u32>,
    #[serde(default)]
    pub mobile: bool,
    pub color_scheme: Option<ColorScheme>,
    pub reduced_motion: Option<bool>,
    #[serde(default)]
    pub reset: bool,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ColorScheme {
    Light,
    Dark,
}

impl ColorScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// An emulated viewport.
#[derive(Debug, PartialEq)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
    pub mobile: bool,
}

impl EmulateArgs {
    /// The viewport to emulate, if the arguments name one.
    pub fn screen(&self) -> Result<Option<Screen>> {
        let in_range = |side: u32| (100..=4000).contains(&side);
        match (self.width, self.height) {
            (Some(width), Some(height)) if in_range(width) && in_range(height) => {
                Ok(Some(Screen {
                    width,
                    height,
                    mobile: self.mobile,
                }))
            }
            (Some(_), Some(_)) => bail!("width and height must be between 100 and 4000"),
            (None, None) if self.mobile => bail!("`mobile` needs a `width` and `height`"),
            (None, None) => Ok(None),
            _ => bail!("pass `width` and `height` together"),
        }
    }

    pub fn media(&self) -> bool {
        self.color_scheme.is_some() || self.reduced_motion.is_some()
    }
}

#[derive(Deserialize)]
pub struct EvalArgs {
    #[serde(flatten)]
    pub at: TabArgs,
    pub expression: String,
}

#[derive(Deserialize)]
pub struct WaitArgs {
    #[serde(flatten)]
    pub at: TabArgs,
    pub text: Option<String>,
    pub selector: Option<String>,
    pub ms: Option<u64>,
    pub timeout: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::parse;

    #[test]
    fn element_refs_accept_numbers_and_strings() {
        let parse = |value: Value| parse::<TargetArgs>("t", value).map(|args| args.element);
        assert_eq!(parse(json!({ "ref": 12 })).unwrap(), Some(12));
        assert_eq!(parse(json!({ "ref": "12" })).unwrap(), Some(12));
        assert_eq!(parse(json!({ "ref": "[12]" })).unwrap(), Some(12));
        assert_eq!(parse(json!({})).unwrap(), None);
        assert!(parse(json!({ "ref": "button" })).is_err());
    }

    #[test]
    fn flattened_arguments() {
        let args: ClickArgs = parse(
            "browser_click",
            json!({ "profile": "work", "tab": "AB12", "ref": 3, "button": "right" }),
        )
        .unwrap();
        assert_eq!(args.at.profile.as_deref(), Some("work"));
        assert_eq!(args.at.tab.as_deref(), Some("AB12"));
        assert_eq!(args.button, Button::Right);
        assert!(matches!(
            args.target.aim().unwrap(),
            Some(Aim::Element(Locator::Ref(3)))
        ));
    }

    #[test]
    fn emulation_arguments() {
        let args: EmulateArgs = parse(
            "browser_emulate",
            json!({ "width": 390, "height": 844, "mobile": true, "color_scheme": "dark" }),
        )
        .unwrap();
        assert_eq!(
            args.screen().unwrap(),
            Some(Screen {
                width: 390,
                height: 844,
                mobile: true
            })
        );
        assert_eq!(args.color_scheme, Some(ColorScheme::Dark));
        assert!(args.media() && !args.reset);

        let media_only: EmulateArgs =
            parse("browser_emulate", json!({ "reduced_motion": true })).unwrap();
        assert_eq!(media_only.screen().unwrap(), None);
        assert!(media_only.media());

        let screen = |value: Value| {
            parse::<EmulateArgs>("browser_emulate", value)
                .unwrap()
                .screen()
        };
        assert!(screen(json!({ "width": 390 })).is_err());
        assert!(screen(json!({ "mobile": true })).is_err());
        assert!(screen(json!({ "width": 20, "height": 844 })).is_err());
        assert!(
            parse::<EmulateArgs>("browser_emulate", json!({ "color_scheme": "sepia" })).is_err()
        );
    }

    #[test]
    fn target_must_be_unambiguous() {
        let args: TargetArgs = parse("t", json!({ "ref": 1, "x": 3, "y": 4 })).unwrap();
        assert!(args.aim().is_err());
        let args: TargetArgs = parse("t", json!({ "x": 3 })).unwrap();
        assert!(args.aim().is_err());
        let args: TargetArgs = parse("t", json!({ "x": 3, "y": 4.5 })).unwrap();
        assert!(matches!(args.aim().unwrap(), Some(Aim::Point(x, y)) if x == 3.0 && y == 4.5));
    }
}
