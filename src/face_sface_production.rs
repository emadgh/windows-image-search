use crate::face_detection::{FaceBox, FaceLandmark};
use crate::face_embedding::{self, FaceEmbedder};
use crate::face_embedding_pipeline::{
    self, FaceEmbeddingPipelineEvent, FaceEmbeddingPipelineOptions, FaceEmbeddingPipelineSummary,
};
use crate::face_settings::FaceEmbeddingSettings;
use crate::face_sface_adapter::{
    align_sface_112, LandmarkPoint, SFaceExecutionProvider, SFaceOnnxAdapter,
    SFACE_EMBEDDING_DIMENSION, SFACE_INPUT_SIZE,
};
use anyhow::{bail, Context, Result};
use image::{DynamicImage, GenericImageView};
use std::path::{Path, PathBuf};

pub const MODEL_ID: &str = "opencv-sface-external";
pub const MODEL_VERSION: &str = "1";
pub const ALIGNMENT_REVISION: i64 = 2;

pub struct SFaceProductionEmbedder {
    adapter: SFaceOnnxAdapter,
    provider: SFaceExecutionProvider,
    cache_revision: &'static str,
}

impl SFaceProductionEmbedder {
    pub fn load(settings: &FaceEmbeddingSettings) -> Result<Self> {
        if !settings.configured() {
            bail!("SFace model path is not configured");
        }
        let model_fingerprint = model_fingerprint_fnv1a64(&settings.model_path)?;
        let adapter = SFaceOnnxAdapter::load(&settings.model_path, settings.provider)?;
        let cache_revision = embedding_cache_revision(model_fingerprint);
        let cache_revision = Box::leak(cache_revision.into_boxed_str());
        Ok(Self {
            adapter,
            provider: settings.provider,
            cache_revision,
        })
    }

    pub fn provider(&self) -> SFaceExecutionProvider {
        self.provider
    }
}

impl FaceEmbedder for SFaceProductionEmbedder {
    fn model_id(&self) -> &'static str {
        MODEL_ID
    }

    fn model_version(&self) -> &'static str {
        self.cache_revision
    }

    fn input_size(&self) -> u32 {
        SFACE_INPUT_SIZE
    }

    fn embedding_dimension(&self) -> usize {
        SFACE_EMBEDDING_DIMENSION
    }

    fn alignment_revision(&self) -> i64 {
        ALIGNMENT_REVISION
    }

    fn align_face(
        &self,
        image: &DynamicImage,
        _bbox: FaceBox,
        landmarks: &[FaceLandmark],
    ) -> Result<DynamicImage> {
        let landmarks = normalized_landmarks_to_pixels(image, landmarks)?;
        align_sface_112(image, landmarks)
    }

    fn embed(&mut self, aligned_face: &DynamicImage) -> Result<Vec<f32>> {
        let (embedding, _) = self.adapter.embed_aligned(aligned_face)?;
        Ok(embedding)
    }
}

pub fn run_available_roots<F>(
    roots: &[PathBuf],
    settings: &FaceEmbeddingSettings,
    options: FaceEmbeddingPipelineOptions,
    emit: F,
) -> Result<FaceEmbeddingPipelineSummary>
where
    F: FnMut(FaceEmbeddingPipelineEvent),
{
    let mut embedder = SFaceProductionEmbedder::load(settings)?;
    face_embedding_pipeline::run_available_roots(roots, &mut embedder, options, emit)
}

fn model_fingerprint_fnv1a64(path: &Path) -> Result<u64> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading SFace model for cache revision {}", path.display()))?;
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Ok(hash)
}

fn embedding_cache_revision(model_fingerprint: u64) -> String {
    format!(
        "{MODEL_VERSION}-{:016x}-align{ALIGNMENT_REVISION}-{}x{}",
        model_fingerprint, SFACE_INPUT_SIZE, SFACE_EMBEDDING_DIMENSION
    )
}

fn normalized_landmarks_to_pixels(
    image: &DynamicImage,
    landmarks: &[FaceLandmark],
) -> Result<[LandmarkPoint; 5]> {
    if landmarks.len() != 5 {
        bail!(
            "SFace production embedding requires exactly 5 landmarks; got {}",
            landmarks.len()
        );
    }
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        bail!("cannot align SFace input from an empty image");
    }

    let max_x = width.saturating_sub(1) as f32;
    let max_y = height.saturating_sub(1) as f32;
    let mut output = [LandmarkPoint { x: 0.0, y: 0.0 }; 5];
    for (index, landmark) in landmarks.iter().enumerate() {
        if !landmark.x.is_finite() || !landmark.y.is_finite() {
            bail!("SFace landmarks must contain finite normalized coordinates");
        }
        if !(0.0..=1.0).contains(&landmark.x) || !(0.0..=1.0).contains(&landmark.y) {
            bail!("SFace landmarks must be normalized to the oriented image");
        }
        output[index] = LandmarkPoint {
            x: landmark.x * max_x,
            y: landmark.y * max_y,
        };
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    fn test_image() -> DynamicImage {
        DynamicImage::ImageRgb8(ImageBuffer::from_pixel(200, 100, Rgb([10, 20, 30])))
    }

    fn five_landmarks() -> [FaceLandmark; 5] {
        [
            FaceLandmark { x: 0.30, y: 0.35 },
            FaceLandmark { x: 0.70, y: 0.35 },
            FaceLandmark { x: 0.50, y: 0.52 },
            FaceLandmark { x: 0.36, y: 0.72 },
            FaceLandmark { x: 0.64, y: 0.72 },
        ]
    }

    #[test]
    fn production_alignment_requires_five_landmarks_without_bbox_fallback() {
        let image = test_image();
        let err = normalized_landmarks_to_pixels(&image, &five_landmarks()[..4]).unwrap_err();
        assert!(err.to_string().contains("exactly 5 landmarks"));
    }

    #[test]
    fn normalized_landmarks_convert_to_oriented_pixel_coordinates() {
        let image = test_image();
        let pixels = normalized_landmarks_to_pixels(&image, &five_landmarks()).unwrap();
        assert!((pixels[0].x - 59.7).abs() < 0.01);
        assert!((pixels[0].y - 34.65).abs() < 0.01);
        assert!((pixels[1].x - 139.3).abs() < 0.01);
    }

    #[test]
    fn production_metadata_is_stable_and_alignment_revision_is_distinct() {
        assert_eq!(MODEL_ID, "opencv-sface-external");
        assert_eq!(MODEL_VERSION, "1");
        assert_eq!(SFACE_INPUT_SIZE, 112);
        assert_eq!(SFACE_EMBEDDING_DIMENSION, 128);
        assert!(ALIGNMENT_REVISION > face_embedding::ALIGNMENT_REVISION);
    }

    #[test]
    fn cache_revision_changes_when_external_model_content_changes() {
        let base = embedding_cache_revision(0x1234);
        assert_eq!(base, embedding_cache_revision(0x1234));
        assert_ne!(base, embedding_cache_revision(0x1235));
        assert!(base.contains(&format!("align{ALIGNMENT_REVISION}")));
    }

    #[test]
    fn missing_external_model_is_rejected_without_download() {
        let settings = FaceEmbeddingSettings::default();
        let err = SFaceProductionEmbedder::load(&settings)
            .err()
            .expect("missing external model must fail");
        assert!(err.to_string().contains("not configured"));
    }
}
