//! Downscaling and encoding screenshots before they reach a model.

use std::io::Cursor;

use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, ImageReader};
use serde::Deserialize;

const JPEG_QUALITY: u8 = 75;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Jpeg,
    Png,
}

impl Format {
    pub fn mime_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
        }
    }
}

/// An encoded image and its pixel size.
pub struct Encoded {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// The size of a `width`x`height` image scaled down to at most `max_width`,
/// keeping the aspect ratio.
pub fn fit(width: u32, height: u32, max_width: u32) -> (u32, u32) {
    if width <= max_width || width == 0 {
        return (width, height);
    }
    let scaled =
        (u64::from(height) * u64::from(max_width) + u64::from(width) / 2) / u64::from(width);
    (max_width, scaled.max(1) as u32)
}

pub fn encode(image: DynamicImage, max_width: u32, format: Format) -> Result<Encoded> {
    let (width, height) = fit(image.width(), image.height(), max_width);
    let image = if (width, height) == (image.width(), image.height()) {
        image
    } else {
        image.resize_exact(width, height, FilterType::Triangle)
    };
    let mut data = Vec::new();
    match format {
        Format::Jpeg => DynamicImage::ImageRgb8(image.to_rgb8())
            .write_with_encoder(JpegEncoder::new_with_quality(&mut data, JPEG_QUALITY)),
        Format::Png => image.write_to(&mut Cursor::new(&mut data), ImageFormat::Png),
    }
    .context("failed to encode the screenshot")?;
    Ok(Encoded {
        data,
        width,
        height,
    })
}

/// Scales a JPEG down by `divisor`, e.g. the device pixel ratio, so that
/// its pixels match CSS pixels. Returns it unchanged when `divisor` is at most 1.
pub fn shrink_jpeg(data: Vec<u8>, divisor: f64) -> Result<Encoded> {
    let (width, height) = ImageReader::with_format(Cursor::new(&data), ImageFormat::Jpeg)
        .into_dimensions()
        .context("the browser returned an unreadable screenshot")?;
    let target = (f64::from(width) / divisor).round() as u32;
    if divisor <= 1.0 || target >= width {
        return Ok(Encoded {
            data,
            width,
            height,
        });
    }
    let image = image::load_from_memory_with_format(&data, ImageFormat::Jpeg)?;
    encode(image, target.max(1), Format::Jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_ratio() {
        assert_eq!(fit(2880, 1800, 1366), (1366, 854));
        assert_eq!(fit(1280, 800, 1366), (1280, 800));
        assert_eq!(fit(1366, 768, 1366), (1366, 768));
        assert_eq!(fit(10000, 1, 1000), (1000, 1));
    }

    #[test]
    fn encodes_scaled_jpeg_and_png() {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::new(400, 200));
        let jpeg = encode(image.clone(), 320, Format::Jpeg).unwrap();
        assert_eq!((jpeg.width, jpeg.height), (320, 160));
        assert_eq!(&jpeg.data[..2], &[0xFF, 0xD8]);

        let png = encode(image, 1000, Format::Png).unwrap();
        assert_eq!((png.width, png.height), (400, 200));
        assert_eq!(&png.data[1..4], b"PNG");

        let same = shrink_jpeg(jpeg.data.clone(), 1.0).unwrap();
        assert_eq!((same.width, same.height), (320, 160));
        let shrunk = shrink_jpeg(jpeg.data, 2.0).unwrap();
        assert_eq!((shrunk.width, shrunk.height), (160, 80));
    }
}
