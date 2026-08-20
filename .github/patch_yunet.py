from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def replace_once(path: str, old: str, new: str) -> None:
    target = ROOT / path
    text = target.read_text(encoding="utf-8")
    if old not in text:
        raise SystemExit(f"anchor not found in {path}: {old[:120]!r}")
    target.write_text(text.replace(old, new, 1), encoding="utf-8")


adapter = r'''use crate::face_detection::{DetectedFace, FaceBox, FaceLandmark};
use anyhow::{bail, Context, Result};
use image::{DynamicImage, GenericImageView};
use ort::{ep::DirectML, session::Session, value::TensorRef};
use std::path::Path;

pub const YUNET_DIVISOR: u32 = 32;
pub const YUNET_STRIDES: [u32; 3] = [8, 16, 32];
pub const DEFAULT_SCORE_THRESHOLD: f32 = 0.6;
pub const DEFAULT_NMS_THRESHOLD: f32 = 0.3;
pub const DEFAULT_TOP_K: usize = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YuNetExecutionProvider {
    Cpu,
    DirectMl,
}

impl YuNetExecutionProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::DirectMl => "directml",
        }
    }
}

#[derive(Clone, Debug)]
struct YuNetInput {
    values: Vec<f32>,
    width: u32,
    height: u32,
    padded_width: u32,
    padded_height: u32,
}

#[derive(Clone, Debug, PartialEq)]
struct Candidate {
    score: f32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    landmarks: [(f32, f32); 5],
}

pub struct YuNetOnnxAdapter {
    session: Session,
    provider: YuNetExecutionProvider,
    score_threshold: f32,
    nms_threshold: f32,
    top_k: usize,
    model_fingerprint: u64,
}

impl YuNetOnnxAdapter {
    pub fn load(
        model_path: &Path,
        provider: YuNetExecutionProvider,
        score_threshold: f32,
        nms_threshold: f32,
        top_k: usize,
    ) -> Result<Self> {
        if !model_path.is_file() {
            bail!("YuNet ONNX file does not exist: {}", model_path.display());
        }
        validate_thresholds(score_threshold, nms_threshold, top_k)?;
        let model_fingerprint = file_fingerprint_fnv1a64(model_path)?;
        let builder = Session::builder().context("creating YuNet ONNX session builder")?;
        let mut builder = match provider {
            YuNetExecutionProvider::Cpu => builder,
            YuNetExecutionProvider::DirectMl => builder
                .with_execution_providers([DirectML::default().build()])
                .map_err(|err| anyhow::anyhow!("configuring DirectML for YuNet: {err}"))?,
        };
        let session = builder
            .commit_from_file(model_path)
            .with_context(|| format!("loading external YuNet ONNX {}", model_path.display()))?;
        Ok(Self {
            session,
            provider,
            score_threshold,
            nms_threshold,
            top_k,
            model_fingerprint,
        })
    }

    pub fn provider(&self) -> YuNetExecutionProvider {
        self.provider
    }

    pub fn model_fingerprint(&self) -> u64 {
        self.model_fingerprint
    }

    pub fn detect(&mut self, image: &DynamicImage) -> Result<Vec<DetectedFace>> {
        let input = yunet_nchw_bgr_f32(image)?;
        let tensor = TensorRef::from_array_view((
            [
                1usize,
                3,
                input.padded_height as usize,
                input.padded_width as usize,
            ],
            input.values.as_slice(),
        ))
        .context("creating YuNet ONNX input tensor")?;
        let outputs = self
            .session
            .run(ort::inputs![tensor])
            .context("running YuNet ONNX inference")?;

        let extract = |name: &str| -> Result<Vec<f32>> {
            let output = outputs
                .get(name)
                .with_context(|| format!("YuNet ONNX output `{name}` is missing"))?;
            let (_shape, values) = output
                .try_extract_tensor::<f32>()
                .with_context(|| format!("extracting YuNet ONNX output `{name}`"))?;
            Ok(values.to_vec())
        };

        let mut candidates = Vec::new();
        for stride in YUNET_STRIDES {
            let cls = extract(&format!("cls_{stride}"))?;
            let obj = extract(&format!("obj_{stride}"))?;
            let bbox = extract(&format!("bbox_{stride}"))?;
            let kps = extract(&format!("kps_{stride}"))?;
            candidates.extend(decode_stride(
                input.padded_width,
                input.padded_height,
                stride,
                &cls,
                &obj,
                &bbox,
                &kps,
                self.score_threshold,
            )?);
        }

        let kept = apply_nms(candidates, self.nms_threshold, self.top_k);
        let width = input.width as f32;
        let height = input.height as f32;
        let landmark_max_x = input.width.saturating_sub(1).max(1) as f32;
        let landmark_max_y = input.height.saturating_sub(1).max(1) as f32;
        Ok(kept
            .into_iter()
            .map(|candidate| {
                DetectedFace {
                    confidence: candidate.score,
                    bbox: FaceBox {
                        x: candidate.x / width,
                        y: candidate.y / height,
                        width: candidate.width / width,
                        height: candidate.height / height,
                    },
                    landmarks: candidate
                        .landmarks
                        .into_iter()
                        .map(|(x, y)| FaceLandmark {
                            x: x / landmark_max_x,
                            y: y / landmark_max_y,
                        })
                        .collect(),
                }
                .normalized()
            })
            .collect())
    }
}

fn validate_thresholds(score_threshold: f32, nms_threshold: f32, top_k: usize) -> Result<()> {
    if !score_threshold.is_finite() || !(0.0..=1.0).contains(&score_threshold) {
        bail!("YuNet score threshold must be between 0 and 1");
    }
    if !nms_threshold.is_finite() || !(0.0..=1.0).contains(&nms_threshold) {
        bail!("YuNet NMS threshold must be between 0 and 1");
    }
    if top_k == 0 {
        bail!("YuNet Top-K must be greater than zero");
    }
    Ok(())
}

fn file_fingerprint_fnv1a64(path: &Path) -> Result<u64> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading YuNet model {}", path.display()))?;
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Ok(hash)
}

fn padded_dimension(value: u32) -> u32 {
    value.div_ceil(YUNET_DIVISOR) * YUNET_DIVISOR
}

fn yunet_nchw_bgr_f32(image: &DynamicImage) -> Result<YuNetInput> {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        bail!("cannot run YuNet on an empty image");
    }
    let padded_width = padded_dimension(width);
    let padded_height = padded_dimension(height);
    let plane = (padded_width as usize)
        .checked_mul(padded_height as usize)
        .context("YuNet padded image is too large")?;
    let mut values = vec![0.0f32; plane * 3];
    let rgb = image.to_rgb8();
    for y in 0..height {
        for x in 0..width {
            let pixel = rgb.get_pixel(x, y);
            let index = y as usize * padded_width as usize + x as usize;
            values[index] = pixel[2] as f32;
            values[plane + index] = pixel[1] as f32;
            values[2 * plane + index] = pixel[0] as f32;
        }
    }
    Ok(YuNetInput {
        values,
        width,
        height,
        padded_width,
        padded_height,
    })
}

#[allow(clippy::too_many_arguments)]
fn decode_stride(
    padded_width: u32,
    padded_height: u32,
    stride: u32,
    cls: &[f32],
    obj: &[f32],
    bbox: &[f32],
    kps: &[f32],
    score_threshold: f32,
) -> Result<Vec<Candidate>> {
    let cols = (padded_width / stride) as usize;
    let rows = (padded_height / stride) as usize;
    let cells = rows
        .checked_mul(cols)
        .context("YuNet output grid is too large")?;
    if cls.len() != cells || obj.len() != cells || bbox.len() != cells * 4 || kps.len() != cells * 10 {
        bail!(
            "YuNet stride {stride} output dimensions are incompatible: cells={cells}, cls={}, obj={}, bbox={}, kps={}",
            cls.len(), obj.len(), bbox.len(), kps.len()
        );
    }

    let mut candidates = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let index = row * cols + col;
            let cls_score = cls[index].clamp(0.0, 1.0);
            let obj_score = obj[index].clamp(0.0, 1.0);
            let score = (cls_score * obj_score).sqrt();
            if !score.is_finite() || score < score_threshold {
                continue;
            }
            let base = index * 4;
            let cx = (col as f32 + bbox[base]) * stride as f32;
            let cy = (row as f32 + bbox[base + 1]) * stride as f32;
            let width = bbox[base + 2].exp() * stride as f32;
            let height = bbox[base + 3].exp() * stride as f32;
            if !cx.is_finite()
                || !cy.is_finite()
                || !width.is_finite()
                || !height.is_finite()
                || width <= 0.0
                || height <= 0.0
            {
                continue;
            }
            let mut landmarks = [(0.0f32, 0.0f32); 5];
            let kps_base = index * 10;
            let mut valid = true;
            for landmark in 0..5 {
                let x = (kps[kps_base + landmark * 2] + col as f32) * stride as f32;
                let y = (kps[kps_base + landmark * 2 + 1] + row as f32) * stride as f32;
                if !x.is_finite() || !y.is_finite() {
                    valid = false;
                    break;
                }
                landmarks[landmark] = (x, y);
            }
            if !valid {
                continue;
            }
            candidates.push(Candidate {
                score,
                x: cx - width / 2.0,
                y: cy - height / 2.0,
                width,
                height,
                landmarks,
            });
        }
    }
    Ok(candidates)
}

fn apply_nms(mut candidates: Vec<Candidate>, threshold: f32, top_k: usize) -> Vec<Candidate> {
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.x.total_cmp(&b.x))
            .then_with(|| a.y.total_cmp(&b.y))
            .then_with(|| a.width.total_cmp(&b.width))
            .then_with(|| a.height.total_cmp(&b.height))
    });
    candidates.truncate(top_k.min(candidates.len()));
    let mut kept = Vec::with_capacity(candidates.len());
    'candidate: for candidate in candidates {
        for previous in &kept {
            if intersection_over_union(&candidate, previous) > threshold {
                continue 'candidate;
            }
        }
        kept.push(candidate);
    }
    kept
}

fn intersection_over_union(a: &Candidate, b: &Candidate) -> f32 {
    let left = a.x.max(b.x);
    let top = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    let intersection = (right - left).max(0.0) * (bottom - top).max(0.0);
    if intersection <= 0.0 {
        return 0.0;
    }
    let union = a.width * a.height + b.width * b.height - intersection;
    if union <= f32::EPSILON {
        0.0
    } else {
        intersection / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    #[test]
    fn preprocessing_pads_to_divisor_and_preserves_bgr_planes() {
        let mut image = ImageBuffer::from_pixel(33, 17, Rgb([10u8, 20, 30]));
        image.put_pixel(1, 0, Rgb([40, 50, 60]));
        let input = yunet_nchw_bgr_f32(&DynamicImage::ImageRgb8(image)).unwrap();
        assert_eq!((input.padded_width, input.padded_height), (64, 32));
        let plane = 64usize * 32usize;
        assert_eq!(input.values[0], 30.0);
        assert_eq!(input.values[1], 60.0);
        assert_eq!(input.values[plane], 20.0);
        assert_eq!(input.values[plane + 1], 50.0);
        assert_eq!(input.values[2 * plane], 10.0);
        assert_eq!(input.values[2 * plane + 1], 40.0);
        assert_eq!(input.values[63], 0.0);
    }

    #[test]
    fn stride_decode_matches_opencv_geometry() {
        let cls = [0.81];
        let obj = [0.64];
        let bbox = [0.5, 0.5, 0.0, 0.0];
        let kps = [0.25, 0.25, 0.75, 0.25, 0.5, 0.5, 0.3, 0.75, 0.7, 0.75];
        let decoded = decode_stride(8, 8, 8, &cls, &obj, &bbox, &kps, 0.6).unwrap();
        assert_eq!(decoded.len(), 1);
        let face = &decoded[0];
        assert!((face.score - 0.72).abs() < 1e-6);
        assert!((face.x - 0.0).abs() < 1e-6);
        assert!((face.y - 0.0).abs() < 1e-6);
        assert!((face.width - 8.0).abs() < 1e-6);
        assert_eq!(face.landmarks[0], (2.0, 2.0));
        assert_eq!(face.landmarks[4], (5.6, 6.0));
    }

    #[test]
    fn nms_keeps_best_overlap_and_separate_face() {
        let face = |score: f32, x: f32| Candidate {
            score,
            x,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            landmarks: [(0.0, 0.0); 5],
        };
        let kept = apply_nms(vec![face(0.8, 1.0), face(0.9, 0.0), face(0.7, 30.0)], 0.3, 10);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].score, 0.9);
        assert_eq!(kept[1].score, 0.7);
    }

    #[test]
    fn invalid_thresholds_are_rejected() {
        assert!(validate_thresholds(-0.1, 0.3, 10).is_err());
        assert!(validate_thresholds(0.6, 1.1, 10).is_err());
        assert!(validate_thresholds(0.6, 0.3, 0).is_err());
    }
}
'''

settings = r'''use crate::face_yunet_adapter::{
    YuNetExecutionProvider, DEFAULT_NMS_THRESHOLD, DEFAULT_SCORE_THRESHOLD, DEFAULT_TOP_K,
};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct FaceDetectorSettings {
    pub model_path: PathBuf,
    pub provider: YuNetExecutionProvider,
    pub score_threshold: f32,
    pub nms_threshold: f32,
    pub top_k: usize,
}

impl Default for FaceDetectorSettings {
    fn default() -> Self {
        Self {
            model_path: PathBuf::new(),
            provider: YuNetExecutionProvider::Cpu,
            score_threshold: DEFAULT_SCORE_THRESHOLD,
            nms_threshold: DEFAULT_NMS_THRESHOLD,
            top_k: DEFAULT_TOP_K,
        }
    }
}

impl FaceDetectorSettings {
    pub fn configured(&self) -> bool {
        !self.model_path.as_os_str().is_empty()
    }

    pub fn sanitized(mut self) -> Self {
        self.score_threshold = if self.score_threshold.is_finite() {
            self.score_threshold.clamp(0.0, 1.0)
        } else {
            DEFAULT_SCORE_THRESHOLD
        };
        self.nms_threshold = if self.nms_threshold.is_finite() {
            self.nms_threshold.clamp(0.0, 1.0)
        } else {
            DEFAULT_NMS_THRESHOLD
        };
        self.top_k = self.top_k.clamp(1, 100_000);
        self
    }

    pub fn provider_label(&self) -> &'static str {
        match self.provider {
            YuNetExecutionProvider::Cpu => "CPU",
            YuNetExecutionProvider::DirectMl => "DirectML (GPU)",
        }
    }
}

pub fn load(path: &Path) -> FaceDetectorSettings {
    let mut settings = FaceDetectorSettings::default();
    let Ok(content) = std::fs::read_to_string(path) else {
        return settings;
    };
    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "model_path" => settings.model_path = PathBuf::from(value.trim()),
            "provider" => {
                settings.provider = match value.trim().to_ascii_lowercase().as_str() {
                    "directml" | "dml" | "gpu" => YuNetExecutionProvider::DirectMl,
                    _ => YuNetExecutionProvider::Cpu,
                }
            }
            "score_threshold" => {
                if let Ok(parsed) = value.trim().parse::<f32>() {
                    settings.score_threshold = parsed;
                }
            }
            "nms_threshold" => {
                if let Ok(parsed) = value.trim().parse::<f32>() {
                    settings.nms_threshold = parsed;
                }
            }
            "top_k" => {
                if let Ok(parsed) = value.trim().parse::<usize>() {
                    settings.top_k = parsed;
                }
            }
            _ => {}
        }
    }
    settings.sanitized()
}

pub fn save(path: &Path, settings: &FaceDetectorSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating face detector settings directory {}", parent.display()))?;
    }
    let settings = settings.clone().sanitized();
    let content = format!(
        "model_path={}\nprovider={}\nscore_threshold={}\nnms_threshold={}\ntop_k={}\n",
        settings.model_path.display(),
        settings.provider.as_str(),
        settings.score_threshold,
        settings.nms_threshold,
        settings.top_k
    );
    std::fs::write(path, content)
        .with_context(|| format!("writing face detector settings {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "windows-image-search-yunet-settings-{label}-{}-{nonce}.ini",
            std::process::id()
        ))
    }

    #[test]
    fn settings_round_trip_without_touching_sface_configuration() {
        let path = temp_path("roundtrip");
        let expected = FaceDetectorSettings {
            model_path: PathBuf::from(r"D:\models\face\yunet.onnx"),
            provider: YuNetExecutionProvider::DirectMl,
            score_threshold: 0.72,
            nms_threshold: 0.28,
            top_k: 2048,
        };
        save(&path, &expected).unwrap();
        assert_eq!(load(&path), expected);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn malformed_numeric_values_fall_back_to_safe_bounds() {
        let path = temp_path("sanitize");
        std::fs::write(
            &path,
            "score_threshold=NaN\nnms_threshold=2\ntop_k=0\nprovider=unknown\n",
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.score_threshold, DEFAULT_SCORE_THRESHOLD);
        assert_eq!(loaded.nms_threshold, 1.0);
        assert_eq!(loaded.top_k, 1);
        assert_eq!(loaded.provider, YuNetExecutionProvider::Cpu);
        let _ = std::fs::remove_file(path);
    }
}
'''

production = r'''use crate::face_detection::{DetectedFace, FaceDetector};
use crate::face_pipeline::{self, FacePipelineEvent, FacePipelineOptions, FacePipelineSummary};
use crate::face_yunet_adapter::YuNetOnnxAdapter;
use crate::face_yunet_settings::FaceDetectorSettings;
use anyhow::{bail, Result};
use image::DynamicImage;
use std::path::{Path, PathBuf};

pub const MODEL_ID: &str = "opencv-yunet-external";
pub const ADAPTER_VERSION: &str = "1";

pub struct YuNetProductionDetector {
    adapter: YuNetOnnxAdapter,
    revision: String,
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
        let revision = format!(
            "{}-{:016x}-{:08x}-{:08x}-{}",
            ADAPTER_VERSION,
            adapter.model_fingerprint(),
            settings.score_threshold.to_bits(),
            settings.nms_threshold.to_bits(),
            settings.top_k
        );
        Ok(Self { adapter, revision })
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }
}

impl FaceDetector for YuNetProductionDetector {
    fn detector_id(&self) -> &str {
        MODEL_ID
    }

    fn detector_version(&self) -> &str {
        &self.revision
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
        let settings = FaceDetectorSettings::default();
        let err = YuNetProductionDetector::load(&settings)
            .err()
            .expect("missing external model must fail");
        assert!(err.to_string().contains("not configured"));
    }

    #[test]
    fn production_detector_id_is_stable() {
        assert_eq!(MODEL_ID, "opencv-yunet-external");
        assert_eq!(ADAPTER_VERSION, "1");
    }
}
'''

(ROOT / "src/face_yunet_adapter.rs").write_text(adapter, encoding="utf-8")
(ROOT / "src/face_yunet_settings.rs").write_text(settings, encoding="utf-8")
(ROOT / "src/face_yunet_production.rs").write_text(production, encoding="utf-8")

replace_once(
    "src/main.rs",
    "mod face_sface_production;\nmod face_similarity;",
    "mod face_sface_production;\nmod face_similarity;\nmod face_yunet_adapter;\nmod face_yunet_production;\nmod face_yunet_settings;",
)

replace_once(
    "src/face_detection.rs",
    "    fn detector_id(&self) -> &'static str;\n    fn detector_version(&self) -> &'static str;",
    "    fn detector_id(&self) -> &str;\n    fn detector_version(&self) -> &str;",
)

replace_once(
    "src/face_pipeline.rs",
    "    let detector_id = detector.detector_id();\n    let detector_version = detector.detector_version();",
    "    let detector_id = detector.detector_id().to_owned();\n    let detector_version = detector.detector_version().to_owned();",
)
replace_once(
    "src/face_pipeline.rs",
    "                    detector_id,\n                    detector_version,",
    "                    &detector_id,\n                    &detector_version,",
)
replace_once(
    "src/face_pipeline.rs",
    "                        detector_id,\n                        detector_version,",
    "                        &detector_id,\n                        &detector_version,",
)
replace_once(
    "src/face_pipeline.rs",
    "        fn detector_id(&self) -> &'static str {\n            \"fake-detector\"\n        }\n\n        fn detector_version(&self) -> &'static str {",
    "        fn detector_id(&self) -> &str {\n            \"fake-detector\"\n        }\n\n        fn detector_version(&self) -> &str {",
)

replace_once(
    "src/ui/mod.rs",
    "use crate::face_sface_adapter::SFaceExecutionProvider;",
    "use crate::face_sface_adapter::SFaceExecutionProvider;\nuse crate::face_yunet_adapter::YuNetExecutionProvider;\nuse crate::face_yunet_settings::{self, FaceDetectorSettings};",
)
replace_once(
    "src/ui/mod.rs",
    "    face_embedding_settings: FaceEmbeddingSettings,\n    face_settings_path: PathBuf,",
    "    face_embedding_settings: FaceEmbeddingSettings,\n    face_settings_path: PathBuf,\n    face_detector_settings: FaceDetectorSettings,\n    face_detector_settings_path: PathBuf,",
)
replace_once(
    "src/ui/mod.rs",
    "        let face_settings_path = app_data_dir.join(\"face-embedding-settings.ini\");\n        let face_embedding_settings = face_settings::load(&face_settings_path);",
    "        let face_settings_path = app_data_dir.join(\"face-embedding-settings.ini\");\n        let face_embedding_settings = face_settings::load(&face_settings_path);\n        let face_detector_settings_path = app_data_dir.join(\"face-detector-settings.ini\");\n        let face_detector_settings = face_yunet_settings::load(&face_detector_settings_path);",
)
replace_once(
    "src/ui/mod.rs",
    "            face_embedding_settings,\n            face_settings_path,",
    "            face_embedding_settings,\n            face_settings_path,\n            face_detector_settings,\n            face_detector_settings_path,",
)

face_method = r'''    fn start_face_indexing(&mut self) {
        if self.busy {
            return;
        }
        if !self.face_detector_settings.configured()
            || !self.face_detector_settings.model_path.is_file()
        {
            self.last_error = Some(
                "Configure an available external YuNet ONNX model before face indexing."
                    .to_owned(),
            );
            return;
        }

        let db_path = self.db_path.clone();
        let roots = self.roots.clone();
        let detector_settings = self.face_detector_settings.clone().sanitized();
        let embedding_settings = self.face_embedding_settings.clone();
        let batch_size = self.indexing_settings.batch_size;
        let tx = self.tx.clone();

        self.busy = true;
        self.indexing = true;
        self.index_paused = false;
        self.index_control = None;
        self.current_file = None;
        self.progress = None;
        self.status = "Starting collection-scoped face detection…".to_owned();

        std::thread::spawn(move || {
            let detector_result = crate::face_yunet_production::run_available_roots(
                &db_path,
                &roots,
                &detector_settings,
                crate::face_pipeline::FacePipelineOptions { batch_size },
                |event| match event {
                    crate::face_pipeline::FacePipelineEvent::RootStarted { root, eligible } => {
                        let _ = tx.send(WorkerMessage::Status(format!(
                            "Face detection: {eligible} eligible image(s) in {}",
                            root.display()
                        )));
                        let _ = tx.send(WorkerMessage::Progress {
                            done: 0,
                            total: eligible,
                        });
                    }
                    crate::face_pipeline::FacePipelineEvent::Progress {
                        root,
                        visited,
                        eligible,
                        processed,
                        faces,
                        failures,
                    } => {
                        let _ = tx.send(WorkerMessage::Status(format!(
                            "Face detection {}: {processed} processed, {faces} faces, {failures} failure(s)",
                            root.display()
                        )));
                        let _ = tx.send(WorkerMessage::Progress {
                            done: visited,
                            total: eligible,
                        });
                    }
                    crate::face_pipeline::FacePipelineEvent::ImageFailed { image, error, .. } => {
                        let name = image
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or("image");
                        let _ = tx.send(WorkerMessage::Status(format!(
                            "Face detection skipped {name}: {error}"
                        )));
                    }
                    crate::face_pipeline::FacePipelineEvent::RootUnavailable { root } => {
                        let _ = tx.send(WorkerMessage::Status(format!(
                            "Face detection skipped unavailable root {}",
                            root.display()
                        )));
                    }
                    crate::face_pipeline::FacePipelineEvent::RootFinished { .. } => {}
                },
            );

            let detector_summary = match detector_result {
                Ok(summary) => summary,
                Err(err) => {
                    let _ = tx.send(WorkerMessage::Error(format!(
                        "Face detection failed: {err:#}"
                    )));
                    let _ = tx.send(WorkerMessage::Idle);
                    return;
                }
            };

            if !embedding_settings.configured() {
                let _ = tx.send(WorkerMessage::Status(format!(
                    "Face detection complete: {} image(s), {} face(s). Configure SFace to generate identity embeddings.",
                    detector_summary.images_processed, detector_summary.faces_detected
                )));
                let _ = tx.send(WorkerMessage::Idle);
                return;
            }

            let embedding_result = crate::face_sface_production::run_available_roots(
                &roots,
                &embedding_settings,
                crate::face_embedding_pipeline::FaceEmbeddingPipelineOptions { batch_size },
                |event| match event {
                    crate::face_embedding_pipeline::FaceEmbeddingPipelineEvent::RootStarted {
                        root,
                        pending,
                    } => {
                        let _ = tx.send(WorkerMessage::Status(format!(
                            "Face embeddings: {pending} pending face(s) in {}",
                            root.display()
                        )));
                        let _ = tx.send(WorkerMessage::Progress {
                            done: 0,
                            total: pending,
                        });
                    }
                    crate::face_embedding_pipeline::FaceEmbeddingPipelineEvent::Progress {
                        root,
                        visited,
                        pending,
                        embedded,
                        failures,
                    } => {
                        let _ = tx.send(WorkerMessage::Status(format!(
                            "Face embeddings {}: {embedded} embedded, {failures} failure(s)",
                            root.display()
                        )));
                        let _ = tx.send(WorkerMessage::Progress {
                            done: visited,
                            total: pending,
                        });
                    }
                    crate::face_embedding_pipeline::FaceEmbeddingPipelineEvent::FaceFailed {
                        image,
                        error,
                        ..
                    } => {
                        let name = image
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or("image");
                        let _ = tx.send(WorkerMessage::Status(format!(
                            "Face embedding skipped {name}: {error}"
                        )));
                    }
                    crate::face_embedding_pipeline::FaceEmbeddingPipelineEvent::RootUnavailable {
                        root,
                    } => {
                        let _ = tx.send(WorkerMessage::Status(format!(
                            "Face embedding skipped unavailable root {}",
                            root.display()
                        )));
                    }
                    crate::face_embedding_pipeline::FaceEmbeddingPipelineEvent::RootFinished {
                        ..
                    } => {}
                },
            );

            match embedding_result {
                Ok(summary) => {
                    let _ = tx.send(WorkerMessage::Status(format!(
                        "Face indexing complete: {} image(s) detected, {} face(s) found, {} identity embedding(s) generated",
                        detector_summary.images_processed,
                        detector_summary.faces_detected,
                        summary.faces_embedded
                    )));
                }
                Err(err) => {
                    let _ = tx.send(WorkerMessage::Error(format!(
                        "Face embedding failed after detection: {err:#}"
                    )));
                }
            }
            let _ = tx.send(WorkerMessage::Idle);
        });
    }

'''
replace_once(
    "src/ui/mod.rs",
    "    fn show_settings_window(&mut self, ctx: &egui::Context) {",
    face_method + "    fn show_settings_window(&mut self, ctx: &egui::Context) {",
)
replace_once(
    "src/ui/mod.rs",
    "        let mut save_performance_settings = false;\n        let mut save_face_settings = false;",
    "        let mut save_performance_settings = false;\n        let mut save_face_settings = false;\n        let mut save_face_detector_settings = false;\n        let mut run_face_index = false;",
)

yunet_ui = r'''                ui.heading("Face detection (YuNet)");
                ui.label("Use an external OpenCV YuNet-compatible ONNX detector. The application does not download or bundle detector weights.");
                ui.add_enabled_ui(!self.busy, |ui| {
                    ui.horizontal(|ui| {
                        let mut model_path = self.face_detector_settings.model_path.to_string_lossy().into_owned();
                        if ui.add(egui::TextEdit::singleline(&mut model_path).hint_text("Path to external YuNet .onnx").desired_width(560.0)).changed() {
                            self.face_detector_settings.model_path = PathBuf::from(model_path.trim());
                            save_face_detector_settings = true;
                        }
                        if ui.button("Browse…").clicked() {
                            if let Some(path) = rfd::FileDialog::new().add_filter("ONNX model", &["onnx"]).pick_file() {
                                self.face_detector_settings.model_path = path;
                                save_face_detector_settings = true;
                            }
                        }
                    });
                    let provider_before = self.face_detector_settings.provider;
                    egui::ComboBox::from_label("YuNet execution provider")
                        .selected_text(self.face_detector_settings.provider_label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.face_detector_settings.provider, YuNetExecutionProvider::Cpu, "CPU");
                            ui.selectable_value(&mut self.face_detector_settings.provider, YuNetExecutionProvider::DirectMl, "DirectML (Windows GPU)");
                        });
                    if provider_before != self.face_detector_settings.provider {
                        save_face_detector_settings = true;
                    }
                    let score_changed = ui.add(egui::Slider::new(&mut self.face_detector_settings.score_threshold, 0.05..=0.99).text("Face confidence threshold")).changed();
                    let nms_changed = ui.add(egui::Slider::new(&mut self.face_detector_settings.nms_threshold, 0.05..=0.90).text("NMS overlap threshold")).changed();
                    let top_k_changed = ui.add(egui::Slider::new(&mut self.face_detector_settings.top_k, 100..=10_000).text("Detector Top-K")).changed();
                    if score_changed || nms_changed || top_k_changed {
                        self.face_detector_settings = self.face_detector_settings.clone().sanitized();
                        save_face_detector_settings = true;
                    }
                });
                if !self.face_detector_settings.configured() {
                    ui.small("No YuNet model configured. Collection-scoped face detection remains disabled.");
                } else if self.face_detector_settings.model_path.is_file() {
                    ui.small(format!("Configured: {} on {}", self.face_detector_settings.model_path.display(), self.face_detector_settings.provider_label()));
                } else {
                    ui.colored_label(egui::Color32::LIGHT_RED, "Configured YuNet model path is not currently available.");
                }
                if ui.add_enabled(
                    !self.busy
                        && !self.roots.is_empty()
                        && self.face_detector_settings.configured()
                        && self.face_detector_settings.model_path.is_file(),
                    egui::Button::new("Run face indexing"),
                ).on_hover_text("Runs YuNet only for members of collections with Detect faces enabled, then generates SFace embeddings when SFace is configured.").clicked() {
                    run_face_index = true;
                }
                if self.face_embedding_settings.configured() {
                    ui.small("Face indexing will run YuNet detection followed by SFace identity embeddings.");
                } else {
                    ui.small("SFace is not configured; Run face indexing will persist detections only.");
                }

                ui.add_space(12.0);
                ui.separator();
'''
replace_once(
    "src/ui/mod.rs",
    "                ui.heading(\"Face identity (SFace)\");",
    yunet_ui + "                ui.heading(\"Face identity (SFace)\");",
)

before_search = r'''        if save_face_detector_settings {
            self.face_detector_settings = self.face_detector_settings.clone().sanitized();
            match face_yunet_settings::save(
                &self.face_detector_settings_path,
                &self.face_detector_settings,
            ) {
                Ok(()) => {
                    self.status = if self.face_detector_settings.configured() {
                        format!(
                            "YuNet settings saved: {} on {}",
                            self.face_detector_settings.model_path.display(),
                            self.face_detector_settings.provider_label()
                        )
                    } else {
                        "YuNet settings cleared".to_owned()
                    };
                }
                Err(err) => {
                    self.last_error = Some(format!("Cannot save YuNet settings: {err:#}"));
                }
            }
        }
        if run_face_index {
            self.start_face_indexing();
        }
    }

    fn show_search_sidebar'''
replace_once(
    "src/ui/mod.rs",
    "    }\n\n    fn show_search_sidebar",
    before_search,
)

# Temporary wiring removes itself so the PR diff contains source changes only.
(ROOT / ".github/patch_yunet.py").unlink(missing_ok=True)
(ROOT / ".github/workflows/yunet-production-patch.yml").unlink(missing_ok=True)
