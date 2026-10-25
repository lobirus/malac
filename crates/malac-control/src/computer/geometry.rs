//! Mapping between screenshot pixels and the logical coordinates the OS uses
//! for input (points on macOS, which differ from pixels on Retina displays).

use anyhow::{Result, bail};

/// The geometry of one screenshot of one display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// Top-left corner of the display in global logical coordinates.
    pub origin: (i32, i32),
    /// Display size in logical coordinates.
    pub logical: (u32, u32),
    /// Screenshot size in pixels.
    pub image: (u32, u32),
}

impl Frame {
    /// Converts a point in the screenshot to global logical coordinates.
    pub fn to_logical(self, x: f64, y: f64) -> Result<(i32, i32)> {
        let (width, height) = self.image;
        if !(0.0..=f64::from(width)).contains(&x) || !(0.0..=f64::from(height)).contains(&y) {
            bail!("({x}, {y}) is outside the {width}x{height} screenshot");
        }
        let axis = |value: f64, image: u32, logical: u32, origin: i32| {
            let scaled = (value * f64::from(logical) / f64::from(image)).round() as i64;
            origin + scaled.clamp(0, i64::from(logical.max(1) - 1)) as i32
        };
        Ok((
            axis(x, width, self.logical.0, self.origin.0),
            axis(y, height, self.logical.1, self.origin.1),
        ))
    }

    /// Converts a global logical point to screenshot pixels, or `None` when
    /// the point is on another display.
    pub fn to_image(self, x: i32, y: i32) -> Option<(u32, u32)> {
        let axis = |value: i32, origin: i32, logical: u32, image: u32| {
            let offset = i64::from(value) - i64::from(origin);
            (0..i64::from(logical))
                .contains(&offset)
                .then(|| (offset as f64 * f64::from(image) / f64::from(logical)) as u32)
        };
        Some((
            axis(x, self.origin.0, self.logical.0, self.image.0)?,
            axis(y, self.origin.1, self.logical.1, self.image.1)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RETINA: Frame = Frame {
        origin: (0, 0),
        logical: (1440, 900),
        image: (1366, 854),
    };

    #[test]
    fn scales_retina_screenshot_to_points() {
        assert_eq!(RETINA.to_logical(0.0, 0.0).unwrap(), (0, 0));
        assert_eq!(RETINA.to_logical(683.0, 427.0).unwrap(), (720, 450));
        assert_eq!(RETINA.to_logical(1366.0, 854.0).unwrap(), (1439, 899));
    }

    #[test]
    fn offsets_secondary_display() {
        let frame = Frame {
            origin: (1440, -200),
            logical: (1920, 1080),
            image: (1366, 768),
        };
        assert_eq!(frame.to_logical(0.0, 0.0).unwrap(), (1440, -200));
        assert_eq!(
            frame.to_logical(683.0, 384.0).unwrap(),
            (1440 + 960, -200 + 540)
        );
        assert_eq!(frame.to_image(1440 + 960, 340), Some((683, 384)));
        assert_eq!(frame.to_image(100, 100), None);
    }

    #[test]
    fn identity_without_scaling() {
        let frame = Frame {
            origin: (0, 0),
            logical: (1280, 800),
            image: (1280, 800),
        };
        assert_eq!(frame.to_logical(640.0, 400.0).unwrap(), (640, 400));
        assert_eq!(frame.to_image(640, 400), Some((640, 400)));
    }

    #[test]
    fn round_trips_through_image_space() {
        let (x, y) = RETINA.to_logical(500.0, 300.0).unwrap();
        let (ix, iy) = RETINA.to_image(x, y).unwrap();
        assert!(ix.abs_diff(500) <= 1 && iy.abs_diff(300) <= 1);
    }

    #[test]
    fn rejects_points_outside_the_screenshot() {
        assert!(RETINA.to_logical(-1.0, 10.0).is_err());
        assert!(RETINA.to_logical(10.0, 900.0).is_err());
        assert!(RETINA.to_logical(f64::NAN, 10.0).is_err());
    }
}
