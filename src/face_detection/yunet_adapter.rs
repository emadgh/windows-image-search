use crate::face_detection::{DetectedFace, FaceBox, FaceLandmark};
use anyhow::{bail, Context, Result};
use image::{DynamicImage, GenericImageView};
use ort::{ep::DirectML, session::Session, value::TensorRef};
use std::path::Path;
use std::time::{Duration, Instant};

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

struct YuNetInput {
    values: Vec<f32>,
    width: u32,
    height: u32,
    padded_width: u32,
    padded_height: u32,
}

#[derive(Clone, Debug)]
pub struct YuNetBatchInferenceStats {
    pub provider: YuNetExecutionProvider,
    pub inference: Duration,
    pub batch_size: usize,
    pub padded_width: u32,
    pub padded_height: u32,
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
        let model_fingerprint = fnv1a64_file(model_path)?;
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

    pub fn benchmark_batch_inference(
        &mut self,
        image: &DynamicImage,
        batch_size: usize,
    ) -> Result<YuNetBatchInferenceStats> {
        let input = preprocess(image)?;
        let values = repeat_input_for_batch(&input, batch_size)?;
        let tensor = TensorRef::from_array_view((
            [
                batch_size,
                3usize,
                input.padded_height as usize,
                input.padded_width as usize,
            ],
            values.as_slice(),
        ))
        .context("creating batched YuNet input tensor")?;
        let started = Instant::now();
        let outputs = self
            .session
            .run(ort::inputs![tensor])
            .context("running batched YuNet ONNX inference")?;
        let inference = started.elapsed();

        for stride in YUNET_STRIDES {
            let cells =
                (input.padded_width / stride) as usize * (input.padded_height / stride) as usize;
            for (prefix, values_per_cell) in [
                ("cls", 1usize),
                ("obj", 1usize),
                ("bbox", 4usize),
                ("kps", 10usize),
            ] {
                let name = format!("{prefix}_{stride}");
                let value = outputs
                    .get(name.as_str())
                    .with_context(|| format!("YuNet output `{name}` is missing"))?;
                let (_shape, data) = value
                    .try_extract_tensor::<f32>()
                    .with_context(|| format!("extracting batched YuNet output `{name}`"))?;
                let expected = batch_size
                    .checked_mul(cells)
                    .and_then(|count| count.checked_mul(values_per_cell))
                    .context("YuNet batched output size overflow")?;
                if data.len() != expected {
                    bail!(
                        "batched YuNet output `{name}` returned {} values; expected {} for batch size {}",
                        data.len(),
                        expected,
                        batch_size
                    );
                }
            }
        }

        Ok(YuNetBatchInferenceStats {
            provider: self.provider,
            inference,
            batch_size,
            padded_width: input.padded_width,
            padded_height: input.padded_height,
        })
    }

    pub fn detect(&mut self, image: &DynamicImage) -> Result<Vec<DetectedFace>> {
        let input = preprocess(image)?;
        let tensor = TensorRef::from_array_view((
            [
                1usize,
                3,
                input.padded_height as usize,
                input.padded_width as usize,
            ],
            input.values.as_slice(),
        ))
        .context("creating YuNet input tensor")?;
        let outputs = self
            .session
            .run(ort::inputs![tensor])
            .context("running YuNet ONNX inference")?;

        let output = |name: &str| -> Result<Vec<f32>> {
            let value = outputs
                .get(name)
                .with_context(|| format!("YuNet output `{name}` is missing"))?;
            let (_shape, data) = value
                .try_extract_tensor::<f32>()
                .with_context(|| format!("extracting YuNet output `{name}`"))?;
            Ok(data.to_vec())
        };

        let mut candidates = Vec::new();
        for stride in YUNET_STRIDES {
            candidates.extend(decode_stride(
                input.padded_width,
                input.padded_height,
                stride,
                &output(&format!("cls_{stride}"))?,
                &output(&format!("obj_{stride}"))?,
                &output(&format!("bbox_{stride}"))?,
                &output(&format!("kps_{stride}"))?,
                self.score_threshold,
            )?);
        }

        let width = input.width as f32;
        let height = input.height as f32;
        let landmark_max_x = input.width.saturating_sub(1).max(1) as f32;
        let landmark_max_y = input.height.saturating_sub(1).max(1) as f32;
        Ok(nms(candidates, self.nms_threshold, self.top_k)
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

fn repeat_input_for_batch(input: &YuNetInput, batch_size: usize) -> Result<Vec<f32>> {
    if batch_size == 0 {
        bail!("YuNet batch size must be greater than zero");
    }
    let capacity = input
        .values
        .len()
        .checked_mul(batch_size)
        .context("YuNet batched input size overflow")?;
    let mut values = Vec::with_capacity(capacity);
    for _ in 0..batch_size {
        values.extend_from_slice(&input.values);
    }
    Ok(values)
}

fn validate_thresholds(score: f32, nms: f32, top_k: usize) -> Result<()> {
    if !score.is_finite() || !(0.0..=1.0).contains(&score) {
        bail!("YuNet score threshold must be between 0 and 1");
    }
    if !nms.is_finite() || !(0.0..=1.0).contains(&nms) {
        bail!("YuNet NMS threshold must be between 0 and 1");
    }
    if top_k == 0 {
        bail!("YuNet Top-K must be greater than zero");
    }
    Ok(())
}

fn fnv1a64_file(path: &Path) -> Result<u64> {
    let bytes =
        std::fs::read(path).with_context(|| format!("reading YuNet model {}", path.display()))?;
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

fn preprocess(image: &DynamicImage) -> Result<YuNetInput> {
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
    let cells = rows * cols;
    if cls.len() != cells
        || obj.len() != cells
        || bbox.len() != cells * 4
        || kps.len() != cells * 10
    {
        bail!(
            "YuNet stride {stride} output dimensions are incompatible: cells={cells}, cls={}, obj={}, bbox={}, kps={}",
            cls.len(), obj.len(), bbox.len(), kps.len()
        );
    }

    let mut candidates = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let index = row * cols + col;
            let score = (cls[index].clamp(0.0, 1.0) * obj[index].clamp(0.0, 1.0)).sqrt();
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
            if valid {
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
    }
    Ok(candidates)
}

fn nms(mut candidates: Vec<Candidate>, threshold: f32, top_k: usize) -> Vec<Candidate> {
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    candidates.truncate(top_k.min(candidates.len()));
    let mut kept = Vec::new();
    'candidate: for candidate in candidates {
        for previous in &kept {
            if iou(&candidate, previous) > threshold {
                continue 'candidate;
            }
        }
        kept.push(candidate);
    }
    kept
}

fn iou(a: &Candidate, b: &Candidate) -> f32 {
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
    fn preprocessing_matches_opencv_bgr_padding() {
        let mut image = ImageBuffer::from_pixel(33, 17, Rgb([10u8, 20, 30]));
        image.put_pixel(1, 0, Rgb([40, 50, 60]));
        let input = preprocess(&DynamicImage::ImageRgb8(image)).unwrap();
        assert_eq!((input.padded_width, input.padded_height), (64, 32));
        let plane = 64usize * 32usize;
        assert_eq!(input.values[0], 30.0);
        assert_eq!(input.values[1], 60.0);
        assert_eq!(input.values[plane], 20.0);
        assert_eq!(input.values[2 * plane], 10.0);
        assert_eq!(input.values[63], 0.0);
    }

    #[test]
    fn batch_preprocessing_repeats_preprocessed_image_in_batch_order() {
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(33, 17, Rgb([10u8, 20, 30])));
        let input = preprocess(&image).unwrap();
        let batched = repeat_input_for_batch(&input, 2).unwrap();
        assert_eq!(batched.len(), input.values.len() * 2);
        assert_eq!(&batched[..input.values.len()], input.values.as_slice());
        assert_eq!(&batched[input.values.len()..], input.values.as_slice());
        assert!(repeat_input_for_batch(&input, 0).is_err());
    }

    #[test]
    fn stride_decode_matches_opencv_geometry() {
        let decoded = decode_stride(
            8,
            8,
            8,
            &[0.81],
            &[0.64],
            &[0.5, 0.5, 0.0, 0.0],
            &[0.25, 0.25, 0.75, 0.25, 0.5, 0.5, 0.3, 0.75, 0.7, 0.75],
            0.6,
        )
        .unwrap();
        assert_eq!(decoded.len(), 1);
        assert!((decoded[0].score - 0.72).abs() < 1e-6);
        assert!((decoded[0].width - 8.0).abs() < 1e-6);
        assert_eq!(decoded[0].landmarks[0], (2.0, 2.0));
    }

    #[test]
    fn nms_keeps_best_overlapping_candidate() {
        let make = |score, x| Candidate {
            score,
            x,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            landmarks: [(0.0, 0.0); 5],
        };
        let kept = nms(
            vec![make(0.8, 1.0), make(0.9, 0.0), make(0.7, 30.0)],
            0.3,
            10,
        );
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].score, 0.9);
        assert_eq!(kept[1].score, 0.7);
    }
}
