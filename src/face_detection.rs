use anyhow::{Context, Result};
use image::DynamicImage;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl FaceBox {
    pub fn clamped(self) -> Self {
        let x = self.x.clamp(0.0, 1.0);
        let y = self.y.clamp(0.0, 1.0);
        let width = self.width.max(0.0).min(1.0 - x);
        let height = self.height.max(0.0).min(1.0 - y);
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceLandmark {
    pub x: f32,
    pub y: f32,
}

impl FaceLandmark {
    pub fn clamped(self) -> Self {
        Self {
            x: self.x.clamp(0.0, 1.0),
            y: self.y.clamp(0.0, 1.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DetectedFace {
    pub confidence: f32,
    pub bbox: FaceBox,
    pub landmarks: Vec<FaceLandmark>,
}

impl DetectedFace {
    pub fn normalized(mut self) -> Self {
        self.confidence = self.confidence.clamp(0.0, 1.0);
        self.bbox = self.bbox.clamped();
        for landmark in &mut self.landmarks {
            *landmark = landmark.clamped();
        }
        self
    }
}

/// Replaceable contract for v0.3 face detectors.
///
/// Detector implementations receive an image after EXIF orientation has been
/// applied. Bounding boxes and landmarks must be returned in normalized
/// oriented-image coordinates so stored geometry stays independent of source
/// resolution and detector input scaling.
pub trait FaceDetector: Send {
    fn detector_id(&self) -> &'static str;
    fn detector_version(&self) -> &'static str;
    fn detect(&mut self, image: &DynamicImage) -> Result<Vec<DetectedFace>>;
}

pub fn decode_oriented(path: &Path) -> Result<DynamicImage> {
    decode_oriented_with_orientation(path).map(|(image, _)| image)
}

pub fn decode_oriented_with_orientation(path: &Path) -> Result<(DynamicImage, u32)> {
    let orientation = read_exif_orientation(path);
    let image = image::ImageReader::open(path)
        .with_context(|| format!("opening image for face detection {}", path.display()))?
        .with_guessed_format()
        .with_context(|| {
            format!(
                "guessing image format for face detection {}",
                path.display()
            )
        })?
        .decode()
        .with_context(|| format!("decoding image for face detection {}", path.display()))?;
    Ok((apply_orientation(image, orientation), orientation))
}

pub fn read_exif_orientation(path: &Path) -> u32 {
    let Ok(file) = File::open(path) else {
        return 1;
    };
    let mut reader = BufReader::new(file);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) else {
        return 1;
    };
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        .filter(|orientation| (1..=8).contains(orientation))
        .unwrap_or(1)
}

pub fn apply_orientation(image: DynamicImage, orientation: u32) -> DynamicImage {
    match orientation {
        2 => image.fliph(),
        3 => image.rotate180(),
        4 => image.flipv(),
        // EXIF 5 is a transpose across the top-left/bottom-right diagonal.
        5 => image.rotate90().fliph(),
        6 => image.rotate90(),
        // EXIF 7 is a transverse flip across the opposite diagonal.
        7 => image.rotate90().flipv(),
        8 => image.rotate270(),
        _ => image,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, ImageBuffer, Rgba};

    fn marker_image() -> DynamicImage {
        let mut image = ImageBuffer::from_pixel(2, 3, Rgba([0, 0, 0, 255]));
        image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        image.put_pixel(1, 2, Rgba([0, 255, 0, 255]));
        DynamicImage::ImageRgba8(image)
    }

    #[test]
    fn orientation_rotations_swap_dimensions_when_required() {
        let image = marker_image();
        assert_eq!(apply_orientation(image.clone(), 1).dimensions(), (2, 3));
        assert_eq!(apply_orientation(image.clone(), 3).dimensions(), (2, 3));
        assert_eq!(apply_orientation(image.clone(), 6).dimensions(), (3, 2));
        assert_eq!(apply_orientation(image, 8).dimensions(), (3, 2));
    }

    #[test]
    fn mirrored_orientation_cases_preserve_expected_corner_mapping() {
        let image = marker_image();
        let horizontal = apply_orientation(image.clone(), 2);
        assert_eq!(horizontal.get_pixel(1, 0), Rgba([255, 0, 0, 255]));

        let transpose = apply_orientation(image.clone(), 5);
        assert_eq!(transpose.dimensions(), (3, 2));
        assert_eq!(transpose.get_pixel(0, 0), Rgba([255, 0, 0, 255]));

        let transverse = apply_orientation(image, 7);
        assert_eq!(transverse.dimensions(), (3, 2));
        assert_eq!(transverse.get_pixel(2, 1), Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn detector_geometry_is_clamped_to_normalized_space() {
        let face = DetectedFace {
            confidence: 1.4,
            bbox: FaceBox {
                x: -0.1,
                y: 0.8,
                width: 1.2,
                height: 0.5,
            },
            landmarks: vec![FaceLandmark { x: -1.0, y: 2.0 }],
        }
        .normalized();

        assert_eq!(face.confidence, 1.0);
        assert_eq!(face.bbox.x, 0.0);
        assert_eq!(face.bbox.y, 0.8);
        assert!((face.bbox.height - 0.2).abs() < 1e-6);
        assert_eq!(face.landmarks[0], FaceLandmark { x: 0.0, y: 1.0 });
    }
}
