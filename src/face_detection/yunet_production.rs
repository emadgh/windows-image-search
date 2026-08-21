use super::yunet_adapter::YuNetOnnxAdapter;
use super::yunet_settings::FaceDetectorSettings;
use super::{DetectedFace, FaceDetector};
use crate::face_pipeline::{self, FacePipelineEvent, FacePipelineOptions, FacePipelineSummary};
use anyhow::{bail, Result};
use image::DynamicImage;
use std::path::{Path, PathBuf};

pub const MODEL_ID: &str = "opencv-yunet-external";
pub const MODEL_VERSION: &str = "1";

pub struct YuNetProductionDetector {
    adapter: YuNetOnnxAdapter,
    cache_revision: String,
}

impl YuNetProductionDetector {
    pub fn load(settings: &FaceDetectorSettings) -> Result<Self> {
        if !settings.configured() {
            bail!("YuNet model path is not configured");
        }
        let settings = settings.clone().sanitized();
        let adapter = YuNetOnnxAdapter::load(
            &settings.model_path,
            settings.provider,
            settings.score_threshold,
            settings.nms_threshold,
            settings.top_k,
        )?;
        let cache_revision = detector_cache_revision(
            adapter.model_fingerprint(),
            settings.score_threshold,
            settings.nms_threshold,
            settings.top_k,
        );
        Ok(Self {
            adapter,
            cache_revision,
        })
    }
}

impl FaceDetector for YuNetProductionDetector {
    fn detector_id(&self) -> &'static str {
        MODEL_ID
    }

    fn detector_version(&self) -> &'static str {
        MODEL_VERSION
    }

    fn cache_revision(&self) -> String {
        self.cache_revision.clone()
    }

    fn detect(&mut self, image: &DynamicImage) -> Result<Vec<DetectedFace>> {
        self.adapter.detect(image)
    }
}

pub fn run_available_roots<F>(
    session_db_path: &Path,
    roots: &[PathBuf],
    settings: &FaceDetectorSettings,
    options: FacePipelineOptions,
    emit: F,
) -> Result<FacePipelineSummary>
where
    F: FnMut(FacePipelineEvent),
{
    let mut detector = YuNetProductionDetector::load(settings)?;
    face_pipeline::run_available_roots(session_db_path, roots, &mut detector, options, emit)
}

fn detector_cache_revision(
    model_fingerprint: u64,
    score_threshold: f32,
    nms_threshold: f32,
    top_k: usize,
) -> String {
    format!(
        "{MODEL_VERSION}-{:016x}-{:08x}-{:08x}-{top_k}",
        model_fingerprint,
        score_threshold.to_bits(),
        nms_threshold.to_bits()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_external_model_is_rejected_without_download() {
        let err = YuNetProductionDetector::load(&FaceDetectorSettings::default())
            .err()
            .expect("missing model must fail");
        assert!(err.to_string().contains("not configured"));
    }

    #[test]
    fn production_metadata_is_stable() {
        assert_eq!(MODEL_ID, "opencv-yunet-external");
        assert_eq!(MODEL_VERSION, "1");
    }

    #[test]
    fn cache_revision_changes_with_model_or_semantic_detector_settings() {
        let base = detector_cache_revision(0x1234, 0.6, 0.3, 5_000);
        assert_eq!(base, detector_cache_revision(0x1234, 0.6, 0.3, 5_000));
        assert_ne!(base, detector_cache_revision(0x1235, 0.6, 0.3, 5_000));
        assert_ne!(base, detector_cache_revision(0x1234, 0.7, 0.3, 5_000));
        assert_ne!(base, detector_cache_revision(0x1234, 0.6, 0.4, 5_000));
        assert_ne!(base, detector_cache_revision(0x1234, 0.6, 0.3, 2_000));
    }
}
