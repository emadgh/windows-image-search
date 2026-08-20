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
        Ok(Self { adapter })
    }
}

impl FaceDetector for YuNetProductionDetector {
    fn detector_id(&self) -> &'static str {
        MODEL_ID
    }

    fn detector_version(&self) -> &'static str {
        MODEL_VERSION
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
}
