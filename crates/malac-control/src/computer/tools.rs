//! The computer tool interface: JSON schemas and the arguments they decode into.

use malac_mcp::Tool;
use serde::Deserialize;
use serde_json::json;

use crate::imaging::Format;
use crate::tool::define;

pub const MAX_WAIT_SECS: f64 = 30.0;

pub fn definitions() -> Vec<Tool> {
    let display = json!({
        "type": "integer",
        "minimum": 0,
        "description": "Display index from computer_displays. Defaults to the display of the latest screenshot, else the primary display."
    });
    let x = json!({ "type": "number", "description": "X in screenshot pixels" });
    let y = json!({ "type": "number", "description": "Y in screenshot pixels" });
    let point = json!({
        "type": "object",
        "properties": { "x": x, "y": y },
        "required": ["x", "y"]
    });
    vec![
        define(
            "computer_screenshot",
            "Capture a display. The x/y arguments of the other computer_* tools are pixels in the most recent screenshot of that display.",
            json!({
                "display": display,
                "format": { "type": "string", "enum": ["jpeg", "png"], "description": "Defaults to jpeg" }
            }),
            &[],
        )
        .read_only(),
        define(
            "computer_click",
            "Click at a point of the latest screenshot.",
            json!({
                "x": x,
                "y": y,
                "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "Defaults to left" },
                "count": { "type": "integer", "minimum": 1, "maximum": 3, "description": "2 for a double click" },
                "display": display
            }),
            &["x", "y"],
        ),
        define(
            "computer_move",
            "Move the mouse pointer to a point of the latest screenshot.",
            json!({ "x": x, "y": y, "display": display }),
            &["x", "y"],
        ),
        define(
            "computer_drag",
            "Press the left button at `from`, move to `to` and release it.",
            json!({ "from": point, "to": point, "display": display }),
            &["from", "to"],
        ),
        define(
            "computer_scroll",
            "Turn the mouse wheel, after moving to (x, y) when given. Units are wheel notches; positive dy scrolls down, positive dx scrolls right.",
            json!({
                "x": x,
                "y": y,
                "dx": { "type": "integer" },
                "dy": { "type": "integer" },
                "display": display
            }),
            &[],
        ),
        define(
            "computer_type",
            "Type text into the focused element.",
            json!({ "text": { "type": "string" } }),
            &["text"],
        ),
        define(
            "computer_key",
            "Press a key or combination, e.g. `enter`, `escape`, `f5`, `ctrl+c` or `cmd+shift+t`. `cmd`, `super`, `meta` and `win` are the same modifier.",
            json!({ "key": { "type": "string" } }),
            &["key"],
        ),
        define(
            "computer_cursor_position",
            "Report the mouse pointer position in screenshot pixels.",
            json!({}),
            &[],
        )
        .read_only(),
        define(
            "computer_displays",
            "List displays with their index, logical bounds and scale factor.",
            json!({}),
            &[],
        )
        .read_only(),
        define(
            "computer_open_app",
            "Open an application by name or path, e.g. `Safari` on macOS or `firefox` on Linux.",
            json!({ "name": { "type": "string" } }),
            &["name"],
        ),
        define(
            "computer_wait",
            "Wait before the next action, e.g. while an app loads.",
            json!({ "seconds": { "type": "number", "minimum": 0, "maximum": MAX_WAIT_SECS } }),
            &["seconds"],
        )
        .read_only(),
    ]
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Deserialize)]
pub struct ScreenshotArgs {
    pub display: Option<usize>,
    #[serde(default)]
    pub format: Format,
}

#[derive(Deserialize)]
pub struct ClickArgs {
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub button: Button,
    pub count: Option<u8>,
    pub display: Option<usize>,
}

#[derive(Deserialize)]
pub struct MoveArgs {
    pub x: f64,
    pub y: f64,
    pub display: Option<usize>,
}

#[derive(Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Deserialize)]
pub struct DragArgs {
    pub from: Point,
    pub to: Point,
    pub display: Option<usize>,
}

#[derive(Deserialize)]
pub struct ScrollArgs {
    pub x: Option<f64>,
    pub y: Option<f64>,
    #[serde(default)]
    pub dx: i32,
    #[serde(default)]
    pub dy: i32,
    pub display: Option<usize>,
}

#[derive(Deserialize)]
pub struct TypeArgs {
    pub text: String,
}

#[derive(Deserialize)]
pub struct KeyArgs {
    pub key: String,
}

#[derive(Deserialize)]
pub struct OpenAppArgs {
    pub name: String,
}

#[derive(Deserialize)]
pub struct WaitArgs {
    pub seconds: f64,
}
