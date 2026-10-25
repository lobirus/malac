//! Desktop control: screenshots, mouse and keyboard.

mod geometry;
mod input;
pub mod permissions;
mod screen;
mod tools;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use enigo::{Axis, Coordinate, Direction, Keyboard, Mouse};
use malac_mcp::{CallToolResult, Tool};
use serde_json::Value;

use crate::config::ComputerConfig;
use crate::keys::KeyCombo;
use crate::tool::parse;
use geometry::Frame;
use input::Input;
use tools::{
    Button, ClickArgs, DragArgs, KeyArgs, MAX_WAIT_SECS, MoveArgs, OpenAppArgs, PermissionsArgs,
    ScreenshotArgs, ScrollArgs, TypeArgs, WaitArgs,
};

const DRAG_STEPS: i32 = 12;

pub struct Computer {
    max_width: u32,
    input: Input,
    frames: Mutex<Frames>,
}

/// The latest screenshot geometry of each display, which input coordinates
/// refer to.
#[derive(Default)]
struct Frames {
    latest: Option<usize>,
    by_display: HashMap<usize, Frame>,
}

impl From<Button> for enigo::Button {
    fn from(button: Button) -> Self {
        match button {
            Button::Left => Self::Left,
            Button::Right => Self::Right,
            Button::Middle => Self::Middle,
        }
    }
}

impl Computer {
    pub fn new(config: &ComputerConfig) -> Self {
        Self {
            max_width: config.max_screenshot_width,
            input: Input::default(),
            frames: Mutex::default(),
        }
    }

    pub fn tools() -> Vec<Tool> {
        tools::definitions()
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<CallToolResult> {
        match name {
            "computer_screenshot" => self.screenshot(parse(name, args)?).await,
            "computer_click" => self.click(parse(name, args)?).await,
            "computer_move" => self.move_to(parse(name, args)?).await,
            "computer_drag" => self.drag(parse(name, args)?).await,
            "computer_scroll" => self.scroll(parse(name, args)?).await,
            "computer_type" => self.type_text(parse(name, args)?).await,
            "computer_key" => self.key(parse(name, args)?).await,
            "computer_cursor_position" => self.cursor_position().await,
            "computer_displays" => self.displays().await,
            "computer_open_app" => open_app(&parse::<OpenAppArgs>(name, args)?.name).await,
            "computer_wait" => wait(parse(name, args)?).await,
            "computer_permissions" => check_permissions(parse(name, args)?).await,
            _ => bail!("unknown tool `{name}`"),
        }
    }

    async fn screenshot(&self, args: ScreenshotArgs) -> Result<CallToolResult> {
        let max_width = self.max_width;
        let shot = blocking(move || screen::capture(args.display, max_width, args.format)).await?;
        {
            let mut frames = self.frames.lock().unwrap();
            frames.latest = Some(shot.display.index);
            frames.by_display.insert(shot.display.index, shot.frame);
        }
        let text = format!(
            "{}. The screenshot is {}x{} pixels; computer_* coordinates refer to it.",
            shot.display.describe(),
            shot.image.width,
            shot.image.height,
        );
        Ok(CallToolResult::text(text)
            .with_image(BASE64.encode(&shot.image.data), args.format.mime_type()))
    }

    async fn click(&self, args: ClickArgs) -> Result<CallToolResult> {
        let count = args.count.unwrap_or(1);
        if !(1..=3).contains(&count) {
            bail!("count must be 1, 2 or 3");
        }
        let (x, y) = self.to_logical(args.display, args.x, args.y).await?;
        let button = args.button;
        self.input
            .run(move |enigo| {
                enigo.move_mouse(x, y, Coordinate::Abs)?;
                std::thread::sleep(Duration::from_millis(20));
                for _ in 0..count {
                    enigo.button(button.into(), Direction::Click)?;
                }
                Ok(())
            })
            .await?;
        let verb = match count {
            1 => "Clicked",
            2 => "Double-clicked",
            _ => "Triple-clicked",
        };
        let with = match button {
            Button::Left => "",
            Button::Right => " with the right button",
            Button::Middle => " with the middle button",
        };
        Ok(CallToolResult::text(format!(
            "{verb}{with} at ({}, {})",
            args.x, args.y
        )))
    }

    async fn move_to(&self, args: MoveArgs) -> Result<CallToolResult> {
        let (x, y) = self.to_logical(args.display, args.x, args.y).await?;
        self.input
            .run(move |enigo| enigo.move_mouse(x, y, Coordinate::Abs))
            .await?;
        Ok(CallToolResult::text(format!(
            "Moved the pointer to ({}, {})",
            args.x, args.y
        )))
    }

    async fn drag(&self, args: DragArgs) -> Result<CallToolResult> {
        let frame = self.frame(args.display).await?;
        let from = frame.to_logical(args.from.x, args.from.y)?;
        let to = frame.to_logical(args.to.x, args.to.y)?;
        self.input
            .run(move |enigo| {
                enigo.move_mouse(from.0, from.1, Coordinate::Abs)?;
                enigo.button(enigo::Button::Left, Direction::Press)?;
                let mut moved = Ok(());
                for step in 1..=DRAG_STEPS {
                    let x = from.0 + (to.0 - from.0) * step / DRAG_STEPS;
                    let y = from.1 + (to.1 - from.1) * step / DRAG_STEPS;
                    std::thread::sleep(Duration::from_millis(15));
                    moved = enigo.move_mouse(x, y, Coordinate::Abs);
                    if moved.is_err() {
                        break;
                    }
                }
                let released = enigo.button(enigo::Button::Left, Direction::Release);
                moved.and(released)
            })
            .await?;
        Ok(CallToolResult::text(format!(
            "Dragged from ({}, {}) to ({}, {})",
            args.from.x, args.from.y, args.to.x, args.to.y
        )))
    }

    async fn scroll(&self, args: ScrollArgs) -> Result<CallToolResult> {
        if args.dx == 0 && args.dy == 0 {
            bail!("pass a non-zero dx or dy");
        }
        let at = match (args.x, args.y) {
            (Some(x), Some(y)) => Some(self.to_logical(args.display, x, y).await?),
            (None, None) => None,
            _ => bail!("pass both x and y, or neither"),
        };
        let (dx, dy) = (args.dx, args.dy);
        self.input
            .run(move |enigo| {
                if let Some((x, y)) = at {
                    enigo.move_mouse(x, y, Coordinate::Abs)?;
                }
                if dy != 0 {
                    enigo.scroll(dy, Axis::Vertical)?;
                }
                if dx != 0 {
                    enigo.scroll(dx, Axis::Horizontal)?;
                }
                Ok(())
            })
            .await?;
        Ok(CallToolResult::text(format!("Scrolled by ({dx}, {dy})")))
    }

    async fn type_text(&self, args: TypeArgs) -> Result<CallToolResult> {
        if args.text.is_empty() {
            bail!("text is empty");
        }
        let count = args.text.chars().count();
        self.input.run(move |enigo| enigo.text(&args.text)).await?;
        Ok(CallToolResult::text(format!("Typed {count} characters")))
    }

    async fn key(&self, args: KeyArgs) -> Result<CallToolResult> {
        let combo: KeyCombo = args.key.parse()?;
        let key = input::enigo_key(combo.key)?;
        self.input
            .run(move |enigo| input::press(enigo, combo, key))
            .await?;
        Ok(CallToolResult::text(format!("Pressed {}", args.key.trim())))
    }

    async fn cursor_position(&self) -> Result<CallToolResult> {
        let (x, y) = self.input.run(|enigo| enigo.location()).await?;
        let displays = blocking(screen::displays).await?;
        for display in &displays {
            let frame = self
                .stored(Some(display.index))
                .unwrap_or_else(|| display.frame(self.max_width));
            if let Some((ix, iy)) = frame.to_image(x, y) {
                return Ok(CallToolResult::text(format!(
                    "The pointer is at ({ix}, {iy}) in screenshot pixels of display {}",
                    display.index
                )));
            }
        }
        Ok(CallToolResult::text(format!(
            "The pointer is at logical ({x}, {y}), outside every display"
        )))
    }

    async fn displays(&self) -> Result<CallToolResult> {
        let displays = blocking(screen::displays).await?;
        let lines: Vec<String> = displays
            .iter()
            .map(|display| {
                let (width, height) = display.frame(self.max_width).image;
                format!("{}; screenshots are {width}x{height}", display.describe())
            })
            .collect();
        Ok(CallToolResult::text(lines.join("\n")))
    }

    async fn to_logical(&self, display: Option<usize>, x: f64, y: f64) -> Result<(i32, i32)> {
        self.frame(display).await?.to_logical(x, y)
    }

    /// The frame input coordinates refer to: the latest screenshot of the
    /// display, or what a screenshot of it would look like.
    async fn frame(&self, display: Option<usize>) -> Result<Frame> {
        if let Some(frame) = self.stored(display) {
            return Ok(frame);
        }
        let max_width = self.max_width;
        let displays = blocking(screen::displays).await?;
        Ok(screen::pick(&displays, display)?.frame(max_width))
    }

    fn stored(&self, display: Option<usize>) -> Option<Frame> {
        let frames = self.frames.lock().unwrap();
        let index = display.or(frames.latest)?;
        frames.by_display.get(&index).copied()
    }
}

async fn blocking<T, F>(task: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .context("screen task panicked")?
}

async fn wait(args: WaitArgs) -> Result<CallToolResult> {
    if args.seconds.is_nan() || args.seconds < 0.0 {
        bail!("seconds must be a non-negative number");
    }
    let seconds = args.seconds.min(MAX_WAIT_SECS);
    tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
    Ok(CallToolResult::text(format!("Waited {seconds} seconds")))
}

/// Reports the permission state as JSON, which the malac daemon parses.
async fn check_permissions(args: PermissionsArgs) -> Result<CallToolResult> {
    let report = blocking(move || Ok(permissions::report(args.request))).await?;
    Ok(CallToolResult::text(serde_json::to_string_pretty(&report)?))
}

#[cfg(target_os = "macos")]
async fn open_app(name: &str) -> Result<CallToolResult> {
    let output = tokio::process::Command::new("open")
        .arg("-a")
        .arg(name)
        .output()
        .await
        .context("failed to run `open`")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(CallToolResult::text(format!("Opened {name}")))
}

/// Runs `name` as a program; documents, URLs and other non-executables go to
/// the desktop's default handler.
#[cfg(all(unix, not(target_os = "macos")))]
async fn open_app(name: &str) -> Result<CallToolResult> {
    use std::io::ErrorKind;
    use std::process::Stdio;
    use tokio::process::Command;

    let spawn = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };
    let mut child = match spawn(name, &[]) {
        Ok(child) => child,
        Err(err)
            if matches!(
                err.kind(),
                ErrorKind::NotFound | ErrorKind::PermissionDenied
            ) =>
        {
            spawn("xdg-open", &[name]).context("failed to run xdg-open")?
        }
        Err(err) => return Err(err).with_context(|| format!("failed to start `{name}`")),
    };
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(CallToolResult::text(format!("Opened {name}")))
}

#[cfg(windows)]
async fn open_app(name: &str) -> Result<CallToolResult> {
    let status = tokio::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(name)
        .status()
        .await
        .context("failed to run `start`")?;
    if !status.success() {
        bail!("`start {name}` failed with {status}");
    }
    Ok(CallToolResult::text(format!("Opened {name}")))
}
