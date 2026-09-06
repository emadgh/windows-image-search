use anyhow::{bail, Result};
use image::DynamicImage;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QueryRegion {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Default for QueryRegion {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }
    }
}

impl QueryRegion {
    pub fn crop(self, image: &DynamicImage) -> Result<DynamicImage> {
        if image.width() == 0 || image.height() == 0 {
            bail!("Query image is empty");
        }
        if ![self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            || self.x < 0.0
            || self.y < 0.0
            || self.width <= 0.0
            || self.height <= 0.0
            || self.x >= 1.0
            || self.y >= 1.0
            || self.x + self.width > 1.00001
            || self.y + self.height > 1.00001
        {
            bail!("Query region must be a non-empty rectangle inside the image");
        }
        let x = (self.x * image.width() as f32).floor() as u32;
        let y = (self.y * image.height() as f32).floor() as u32;
        let width = ((self.width * image.width() as f32).ceil() as u32)
            .max(1)
            .min(image.width() - x);
        let height = ((self.height * image.height() as f32).ceil() as u32)
            .max(1)
            .min(image.height() - y);
        Ok(image.crop_imm(x, y, width, height))
    }
}

#[derive(Clone, Debug, Default)]
pub struct VisualQueryOptions {
    pub region: Option<QueryRegion>,
    pub description: Option<String>,
    pub eligible_paths: Option<HashSet<PathBuf>>,
}

#[derive(Clone, Debug)]
pub struct ScoreBreakdown {
    pub values: [Option<f32>; 4],
    pub weights: [f32; 4],
    pub exact_source: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SearchTimings {
    pub decode_ms: f64,
    pub inference_ms: f64,
    pub retrieval_ms: f64,
    pub ranking_ms: f64,
    pub metadata_ms: f64,
    pub descriptor_cache_hit: bool,
    pub descriptor_cache_bytes: usize,
    pub total_ms: f64,
}

pub struct TemporaryQueryImage(pub PathBuf);
impl Drop for TemporaryQueryImage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crop_selects_pixels_and_rejects_invalid_rectangles() {
        let image = DynamicImage::new_rgb8(100, 80);
        assert!(QueryRegion::default()
            .crop(&DynamicImage::new_rgb8(0, 0))
            .is_err());
        let crop = QueryRegion {
            x: 0.25,
            y: 0.25,
            width: 0.5,
            height: 0.5,
        }
        .crop(&image)
        .unwrap();
        assert_eq!((crop.width(), crop.height()), (50, 40));
        assert!(QueryRegion {
            x: f32::NAN,
            ..Default::default()
        }
        .crop(&image)
        .is_err());
        assert!(QueryRegion {
            width: 0.0,
            ..Default::default()
        }
        .crop(&image)
        .is_err());
    }
}
