use crate::face_detection::{FaceBox, FaceLandmark};
use anyhow::{bail, Result};
use image::{imageops::FilterType, DynamicImage, GenericImageView};

pub const SCHEMA_VERSION: i64 = 1;
pub const ALIGNMENT_REVISION: i64 = 1;
const CROP_MARGIN: f32 = 0.18;

/// Replaceable contract for v0.3 face identity embedders.
///
/// The pipeline owns source-state checks and persistence, while each model may
/// override alignment/preprocessing geometry. This keeps a generic default crop
/// for simple embedders without locking production models (for example SFace)
/// to an incompatible alignment contract.
pub trait FaceEmbedder: Send {
    fn model_id(&self) -> &'static str;
    fn model_version(&self) -> &'static str;
    fn input_size(&self) -> u32;
    fn embedding_dimension(&self) -> usize;

    fn alignment_revision(&self) -> i64 {
        ALIGNMENT_REVISION
    }

    fn align_face(
        &self,
        image: &DynamicImage,
        bbox: FaceBox,
        landmarks: &[FaceLandmark],
    ) -> Result<DynamicImage> {
        aligned_face_crop(image, bbox, landmarks, self.input_size())
    }

    fn embed(&mut self, aligned_face: &DynamicImage) -> Result<Vec<f32>>;
}

pub fn aligned_face_crop(
    image: &DynamicImage,
    bbox: FaceBox,
    landmarks: &[FaceLandmark],
    input_size: u32,
) -> Result<DynamicImage> {
    if input_size == 0 {
        bail!("face embedder input size must be non-zero");
    }
    let (image_width, image_height) = image.dimensions();
    if image_width == 0 || image_height == 0 {
        bail!("cannot align a face from an empty image");
    }

    let bbox = bbox.clamped();
    if bbox.width <= 0.0 || bbox.height <= 0.0 {
        bail!("cannot align a face with an empty bounding box");
    }

    let bbox_center_x = bbox.x + bbox.width * 0.5;
    let bbox_center_y = bbox.y + bbox.height * 0.5;
    let valid_landmarks: Vec<FaceLandmark> = landmarks
        .iter()
        .copied()
        .filter(|point| point.x.is_finite() && point.y.is_finite())
        .map(FaceLandmark::clamped)
        .collect();

    let (center_x, center_y) = if valid_landmarks.is_empty() {
        (bbox_center_x, bbox_center_y)
    } else {
        let inv = 1.0 / valid_landmarks.len() as f32;
        let landmark_x = valid_landmarks.iter().map(|point| point.x).sum::<f32>() * inv;
        let landmark_y = valid_landmarks.iter().map(|point| point.y).sum::<f32>() * inv;
        (
            bbox_center_x * 0.75 + landmark_x * 0.25,
            bbox_center_y * 0.75 + landmark_y * 0.25,
        )
    };

    let side = (bbox.width.max(bbox.height) * (1.0 + 2.0 * CROP_MARGIN)).min(1.0);
    let max_left = (1.0 - side).max(0.0);
    let left = (center_x - side * 0.5).clamp(0.0, max_left);
    let top = (center_y - side * 0.5).clamp(0.0, max_left);
    let right = (left + side).min(1.0);
    let bottom = (top + side).min(1.0);

    let x0 = (left * image_width as f32).floor() as u32;
    let y0 = (top * image_height as f32).floor() as u32;
    let x1 = ((right * image_width as f32).ceil() as u32).clamp(x0 + 1, image_width);
    let y1 = ((bottom * image_height as f32).ceil() as u32).clamp(y0 + 1, image_height);

    let crop = image.crop_imm(x0, y0, x1 - x0, y1 - y0);
    Ok(crop.resize_exact(input_size, input_size, FilterType::Triangle))
}

pub fn normalize_embedding(values: Vec<f32>, expected_dimension: usize) -> Result<Vec<f32>> {
    if expected_dimension == 0 {
        bail!("face embedding dimension must be non-zero");
    }
    if values.len() != expected_dimension {
        bail!(
            "face embedder returned dimension {}, expected {}",
            values.len(),
            expected_dimension
        );
    }
    if values.iter().any(|value| !value.is_finite()) {
        bail!("face embedder returned a non-finite value");
    }
    let norm_sq = values
        .iter()
        .map(|value| (*value as f64) * (*value as f64))
        .sum::<f64>();
    if norm_sq <= f64::EPSILON {
        bail!("face embedder returned a zero-length vector");
    }
    let norm = norm_sq.sqrt() as f32;
    Ok(values.into_iter().map(|value| value / norm).collect())
}

pub fn cosine_similarity_normalized(left: &[f32], right: &[f32]) -> Result<f32> {
    if left.is_empty() || left.len() != right.len() {
        bail!("face embedding vectors must be non-empty and have equal dimensions");
    }
    if left
        .iter()
        .chain(right.iter())
        .any(|value| !value.is_finite())
    {
        bail!("face embedding vectors contain non-finite values");
    }
    Ok(left
        .iter()
        .zip(right.iter())
        .map(|(a, b)| a * b)
        .sum::<f32>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    #[test]
    fn alignment_is_square_bounded_and_resized() {
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(200, 100, Rgb([1, 2, 3])));
        let crop = aligned_face_crop(
            &image,
            FaceBox {
                x: 0.85,
                y: 0.05,
                width: 0.14,
                height: 0.35,
            },
            &[FaceLandmark { x: 0.9, y: 0.2 }],
            112,
        )
        .unwrap();
        assert_eq!(crop.dimensions(), (112, 112));
    }

    #[test]
    fn embedding_normalization_rejects_bad_vectors() {
        let normalized = normalize_embedding(vec![3.0, 4.0], 2).unwrap();
        assert!((normalized[0] - 0.6).abs() < 1e-6);
        assert!((normalized[1] - 0.8).abs() < 1e-6);
        assert!(normalize_embedding(vec![0.0, 0.0], 2).is_err());
        assert!(normalize_embedding(vec![1.0], 2).is_err());
        assert!(normalize_embedding(vec![f32::NAN, 1.0], 2).is_err());
    }

    #[test]
    fn normalized_cosine_is_dot_product() {
        let left = normalize_embedding(vec![1.0, 0.0], 2).unwrap();
        let right = normalize_embedding(vec![0.5, 0.5], 2).unwrap();
        let similarity = cosine_similarity_normalized(&left, &right).unwrap();
        assert!((similarity - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }
}
