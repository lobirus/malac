//! Display enumeration and capture through xcap.

use anyhow::{Result, anyhow, bail};
use image::DynamicImage;
use xcap::Monitor;

use super::geometry::Frame;
use crate::imaging::{self, Encoded, Format};

#[derive(Debug, Clone)]
pub struct Display {
    pub index: usize,
    pub name: String,
    pub primary: bool,
    /// Top-left corner in global logical coordinates.
    pub origin: (i32, i32),
    /// Size in logical coordinates.
    pub size: (u32, u32),
    /// Pixels per logical unit.
    pub scale: f32,
}

impl Display {
    /// The frame a screenshot of this display would have.
    pub fn frame(&self, max_width: u32) -> Frame {
        let pixels = |logical: u32| (logical as f32 * self.scale).round() as u32;
        Frame {
            origin: self.origin,
            logical: self.size,
            image: imaging::fit(pixels(self.size.0), pixels(self.size.1), max_width),
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "Display {} \"{}\"{}: {}x{} at ({}, {}), scale {}",
            self.index,
            self.name,
            if self.primary { " (primary)" } else { "" },
            self.size.0,
            self.size.1,
            self.origin.0,
            self.origin.1,
            self.scale,
        )
    }
}

pub struct Screenshot {
    pub display: Display,
    pub frame: Frame,
    pub image: Encoded,
}

pub fn displays() -> Result<Vec<Display>> {
    Ok(monitors()?
        .into_iter()
        .map(|(display, _)| display)
        .collect())
}

/// Picks display `index`, or the primary display.
pub fn pick(displays: &[Display], index: Option<usize>) -> Result<&Display> {
    match index {
        Some(index) => displays.get(index).ok_or_else(|| {
            anyhow!(
                "display {index} does not exist; displays are numbered 0 to {}",
                displays.len().saturating_sub(1)
            )
        }),
        None => displays
            .iter()
            .find(|d| d.primary)
            .or(displays.first())
            .ok_or_else(|| anyhow!("no displays found")),
    }
}

pub fn capture(index: Option<usize>, max_width: u32, format: Format) -> Result<Screenshot> {
    #[cfg(target_os = "macos")]
    super::permissions::require_screen_recording()?;
    let monitors = monitors()?;
    let displays: Vec<Display> = monitors.iter().map(|(d, _)| d.clone()).collect();
    let display = pick(&displays, index)?.clone();
    let monitor = &monitors[display.index].1;
    let pixels = monitor
        .capture_image()
        .map_err(|err| anyhow!("failed to capture display {}: {err}", display.index))?;
    let image = imaging::encode(DynamicImage::ImageRgba8(pixels), max_width, format)?;
    let frame = Frame {
        origin: display.origin,
        logical: display.size,
        image: (image.width, image.height),
    };
    Ok(Screenshot {
        display,
        frame,
        image,
    })
}

fn monitors() -> Result<Vec<(Display, Monitor)>> {
    let monitors = Monitor::all().map_err(|err| anyhow!("failed to list displays: {err}"))?;
    if monitors.is_empty() {
        bail!("no displays found");
    }
    monitors
        .into_iter()
        .enumerate()
        .map(|(index, monitor)| {
            let failed = |err: xcap::XCapError| anyhow!("failed to read display {index}: {err}");
            let display = Display {
                index,
                name: monitor.name().unwrap_or_default(),
                primary: monitor.is_primary().unwrap_or(false),
                origin: (monitor.x().map_err(failed)?, monitor.y().map_err(failed)?),
                size: (
                    monitor.width().map_err(failed)?,
                    monitor.height().map_err(failed)?,
                ),
                scale: monitor.scale_factor().unwrap_or(1.0),
            };
            Ok((display, monitor))
        })
        .collect()
}
