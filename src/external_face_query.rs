use crate::face_detection::{
    self, yunet_production::YuNetProductionDetector, yunet_settings::FaceDetectorSettings, FaceBox,
    FaceDetector,
};
use crate::face_embedding::{self, FaceEmbedder};
use crate::face_settings::FaceEmbeddingSettings;
use crate::face_sface_production::SFaceProductionEmbedder;
use crate::face_similarity::{FaceEmbeddingRevision, FaceSimilarityQuery};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug)]
pub struct ExternalFaceChoice {
    pub image_path: PathBuf,
    pub ordinal: usize,
    pub confidence: f32,
    pub bbox: FaceBox,
    pub query: FaceSimilarityQuery,
}

pub fn prepare_external_faces(
    path: &Path,
    detector_settings: FaceDetectorSettings,
    embedding_settings: FaceEmbeddingSettings,
    request: &crate::search_request::SearchRequest,
) -> Result<Vec<ExternalFaceChoice>> {
    request.check()?;
    if !path.is_file() {
        bail!("external query image is unavailable: {}", path.display());
    }
    if !detector_settings.configured() || !detector_settings.model_path.is_file() {
        bail!("configure an available YuNet model in Settings before using Face from file");
    }
    if !embedding_settings.configured() || !embedding_settings.model_path.is_file() {
        bail!("configure an available SFace model in Settings before using Face from file");
    }

    let image = face_detection::decode_oriented(path)
        .with_context(|| format!("decoding external face query {}", path.display()))?;
    let mut detector = YuNetProductionDetector::load(&detector_settings)
        .context("loading YuNet for external face query")?;
    request.check()?;
    let detections = detector
        .detect(&image)
        .context("detecting faces in external query image")?;
    request.check()?;
    if detections.is_empty() {
        return Ok(Vec::new());
    }

    let mut embedder = SFaceProductionEmbedder::load(&embedding_settings)
        .context("loading SFace for external face query")?;
    let revision = FaceEmbeddingRevision {
        model_id: embedder.model_id().to_owned(),
        model_version: embedder.model_version().to_owned(),
        model_cache_revision: embedder.cache_revision(),
        schema_version: face_embedding::SCHEMA_VERSION,
        alignment_revision: embedder.alignment_revision(),
        dimension: embedder.embedding_dimension(),
    };

    let detected_count = detections.len();
    let mut output = Vec::with_capacity(detected_count);
    for (ordinal, face) in detections.into_iter().enumerate() {
        request.check()?;
        let aligned = match embedder.align_face(&image, face.bbox, &face.landmarks) {
            Ok(aligned) => aligned,
            Err(_) => continue,
        };
        let raw = match embedder.embed(&aligned) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        let values = match face_embedding::normalize_embedding(raw, revision.dimension) {
            Ok(values) => values,
            Err(_) => continue,
        };
        let query = FaceSimilarityQuery {
            root: PathBuf::new(),
            library_id: "external-query".to_owned(),
            face_id: format!("external-face-{ordinal}"),
            relative_image_path: path.to_path_buf(),
            revision: revision.clone(),
            values,
        };
        output.push(ExternalFaceChoice {
            image_path: path.to_path_buf(),
            ordinal,
            confidence: face.confidence,
            bbox: face.bbox,
            query,
        });
    }

    if output.is_empty() {
        bail!(
            "YuNet detected {detected_count} face(s), but SFace could not create a searchable embedding for any of them"
        );
    }
    Ok(output)
}
