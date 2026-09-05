use anyhow::{bail, Context, Result};
use image::{DynamicImage, GenericImageView, ImageBuffer, Rgb};
use ort::{ep::DirectML, session::Session, value::TensorRef};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const SFACE_INPUT_SIZE: u32 = 112;
pub const SFACE_EMBEDDING_DIMENSION: usize = 128;

// OpenCV SFace canonical 5-point geometry for a 112x112 aligned crop.
const SFACE_REFERENCE: [(f32, f32); 5] = [
    (38.2946, 51.6963),
    (73.5318, 51.5014),
    (56.0252, 71.7366),
    (41.5493, 92.3655),
    (70.7299, 92.2041),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SFaceExecutionProvider {
    Cpu,
    DirectMl,
}

impl SFaceExecutionProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::DirectMl => "directml",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LandmarkPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SimilarityTransform {
    a: f32,
    b: f32,
    tx: f32,
    ty: f32,
}

impl SimilarityTransform {
    fn source_to_target(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x - self.b * y + self.tx,
            self.b * x + self.a * y + self.ty,
        )
    }

    fn target_to_source(self, x: f32, y: f32) -> Option<(f32, f32)> {
        let dx = x - self.tx;
        let dy = y - self.ty;
        let determinant = self.a * self.a + self.b * self.b;
        if determinant <= f32::EPSILON {
            return None;
        }
        Some((
            (self.a * dx + self.b * dy) / determinant,
            (-self.b * dx + self.a * dy) / determinant,
        ))
    }
}

#[derive(Clone, Debug)]
pub struct SFaceModelMetadata {
    pub model_path: PathBuf,
    pub source: String,
    pub license: String,
    pub redistributable: bool,
    pub commercial_use: bool,
}

impl SFaceModelMetadata {
    pub fn validate_external(&self) -> Result<()> {
        if self.model_path.as_os_str().is_empty() {
            bail!("SFace ONNX path is required");
        }
        if !self.model_path.is_file() {
            bail!(
                "SFace ONNX file does not exist: {}",
                self.model_path.display()
            );
        }
        if self.source.trim().is_empty() || self.license.trim().is_empty() {
            bail!("SFace source/license metadata cannot be empty");
        }
        Ok(())
    }

    pub fn file_size(&self) -> Result<u64> {
        Ok(std::fs::metadata(&self.model_path)
            .with_context(|| format!("reading SFace model metadata {}", self.model_path.display()))?
            .len())
    }

    pub fn file_fingerprint_fnv1a64(&self) -> Result<u64> {
        let bytes = std::fs::read(&self.model_path)
            .with_context(|| format!("reading SFace model {}", self.model_path.display()))?;
        let mut hash = 0xcbf29ce484222325u64;
        for byte in bytes {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        Ok(hash)
    }
}

#[derive(Clone, Debug)]
pub struct SFaceInferenceStats {
    pub provider: SFaceExecutionProvider,
    pub init: Duration,
    pub inference: Duration,
    pub dimension: usize,
}

#[derive(Clone, Debug)]
pub struct SFaceBatchInferenceStats {
    pub provider: SFaceExecutionProvider,
    pub init: Duration,
    pub inference: Duration,
    pub batch_size: usize,
    pub dimension: usize,
}

pub struct SFaceOnnxAdapter {
    session: Session,
    provider: SFaceExecutionProvider,
    init: Duration,
}

impl SFaceOnnxAdapter {
    pub fn load(model_path: &Path, provider: SFaceExecutionProvider) -> Result<Self> {
        if !model_path.is_file() {
            bail!("SFace ONNX file does not exist: {}", model_path.display());
        }
        let started = Instant::now();
        let builder = Session::builder().context("creating SFace ONNX session builder")?;
        let mut builder = match provider {
            SFaceExecutionProvider::Cpu => builder,
            SFaceExecutionProvider::DirectMl => builder
                .with_execution_providers([DirectML::default().build()])
                .map_err(|err| anyhow::anyhow!("configuring DirectML for SFace: {err}"))?,
        };
        let session = builder
            .commit_from_file(model_path)
            .with_context(|| format!("loading external SFace ONNX {}", model_path.display()))?;
        Ok(Self {
            session,
            provider,
            init: started.elapsed(),
        })
    }

    pub fn provider(&self) -> SFaceExecutionProvider {
        self.provider
    }

    pub fn init_duration(&self) -> Duration {
        self.init
    }

    pub fn embed_aligned(
        &mut self,
        aligned: &DynamicImage,
    ) -> Result<(Vec<f32>, SFaceInferenceStats)> {
        let input = sface_nchw_rgb_f32(aligned)?;
        let tensor = TensorRef::from_array_view(([1usize, 3, 112, 112], input.as_slice()))
            .context("creating SFace ONNX input tensor")?;
        let started = Instant::now();
        let outputs = self
            .session
            .run(ort::inputs![tensor])
            .context("running SFace ONNX inference")?;
        let inference = started.elapsed();
        let (_shape, values) = outputs[0]
            .try_extract_tensor::<f32>()
            .context("extracting SFace ONNX output tensor")?;
        if values.len() != SFACE_EMBEDDING_DIMENSION {
            bail!(
                "SFace ONNX returned {} values; expected {}",
                values.len(),
                SFACE_EMBEDDING_DIMENSION
            );
        }
        let embedding = normalize(values.to_vec())?;
        Ok((
            embedding,
            SFaceInferenceStats {
                provider: self.provider,
                init: self.init,
                inference,
                dimension: values.len(),
            },
        ))
    }

    pub fn embed_aligned_batch(
        &mut self,
        aligned: &[DynamicImage],
    ) -> Result<(Vec<Vec<f32>>, SFaceBatchInferenceStats)> {
        if aligned.is_empty() {
            bail!("SFace batch must contain at least one aligned face");
        }
        let batch_size = aligned.len();
        let input = sface_batch_nchw_rgb_f32(aligned)?;
        let tensor = TensorRef::from_array_view((
            [
                batch_size,
                3usize,
                SFACE_INPUT_SIZE as usize,
                SFACE_INPUT_SIZE as usize,
            ],
            input.as_slice(),
        ))
        .context("creating batched SFace ONNX input tensor")?;
        let started = Instant::now();
        let outputs = self
            .session
            .run(ort::inputs![tensor])
            .context("running batched SFace ONNX inference")?;
        let inference = started.elapsed();
        let (_shape, values) = outputs[0]
            .try_extract_tensor::<f32>()
            .context("extracting batched SFace ONNX output tensor")?;
        let expected = batch_size * SFACE_EMBEDDING_DIMENSION;
        if values.len() != expected {
            bail!(
                "batched SFace ONNX returned {} values; expected {} for batch size {}",
                values.len(),
                expected,
                batch_size
            );
        }
        let mut embeddings = Vec::with_capacity(batch_size);
        for chunk in values.chunks_exact(SFACE_EMBEDDING_DIMENSION) {
            embeddings.push(normalize(chunk.to_vec())?);
        }
        Ok((
            embeddings,
            SFaceBatchInferenceStats {
                provider: self.provider,
                init: self.init,
                inference,
                batch_size,
                dimension: SFACE_EMBEDDING_DIMENSION,
            },
        ))
    }

    pub fn align_and_embed(
        &mut self,
        image: &DynamicImage,
        landmarks: [LandmarkPoint; 5],
    ) -> Result<(Vec<f32>, SFaceInferenceStats)> {
        let aligned = align_sface_112(image, landmarks)?;
        self.embed_aligned(&aligned)
    }
}

pub fn align_sface_112(
    image: &DynamicImage,
    landmarks: [LandmarkPoint; 5],
) -> Result<DynamicImage> {
    if image.width() == 0 || image.height() == 0 {
        bail!("cannot align SFace input from an empty image");
    }
    if landmarks
        .iter()
        .any(|point| !point.x.is_finite() || !point.y.is_finite())
    {
        bail!("SFace landmarks must be finite pixel coordinates");
    }
    let source = landmarks.map(|point| (point.x, point.y));
    let transform = similarity_transform(source, SFACE_REFERENCE)?;
    let rgb = image.to_rgb8();
    let mut output = ImageBuffer::<Rgb<u8>, Vec<u8>>::new(SFACE_INPUT_SIZE, SFACE_INPUT_SIZE);
    for y in 0..SFACE_INPUT_SIZE {
        for x in 0..SFACE_INPUT_SIZE {
            let source_point = transform
                .target_to_source(x as f32, y as f32)
                .context("SFace alignment transform is singular")?;
            output.put_pixel(x, y, bilinear_rgb(&rgb, source_point.0, source_point.1));
        }
    }
    Ok(DynamicImage::ImageRgb8(output))
}

pub fn sface_nchw_rgb_f32(image: &DynamicImage) -> Result<Vec<f32>> {
    if image.dimensions() != (SFACE_INPUT_SIZE, SFACE_INPUT_SIZE) {
        bail!(
            "SFace input must be {}x{}, got {}x{}",
            SFACE_INPUT_SIZE,
            SFACE_INPUT_SIZE,
            image.width(),
            image.height()
        );
    }
    let rgb = image.to_rgb8();
    let plane = (SFACE_INPUT_SIZE * SFACE_INPUT_SIZE) as usize;
    let mut output = vec![0.0f32; plane * 3];
    for (index, pixel) in rgb.pixels().enumerate() {
        output[index] = pixel[0] as f32;
        output[plane + index] = pixel[1] as f32;
        output[2 * plane + index] = pixel[2] as f32;
    }
    Ok(output)
}

pub fn sface_batch_nchw_rgb_f32(images: &[DynamicImage]) -> Result<Vec<f32>> {
    if images.is_empty() {
        bail!("SFace batch must contain at least one aligned face");
    }
    let per_face = (SFACE_INPUT_SIZE * SFACE_INPUT_SIZE) as usize * 3;
    let mut output = Vec::with_capacity(per_face * images.len());
    for image in images {
        output.extend(sface_nchw_rgb_f32(image)?);
    }
    Ok(output)
}

fn similarity_transform(
    source: [(f32, f32); 5],
    target: [(f32, f32); 5],
) -> Result<SimilarityTransform> {
    let count = source.len() as f32;
    let source_center = (
        source.iter().map(|point| point.0).sum::<f32>() / count,
        source.iter().map(|point| point.1).sum::<f32>() / count,
    );
    let target_center = (
        target.iter().map(|point| point.0).sum::<f32>() / count,
        target.iter().map(|point| point.1).sum::<f32>() / count,
    );

    let mut denominator = 0.0f32;
    let mut a_numerator = 0.0f32;
    let mut b_numerator = 0.0f32;
    for (source_point, target_point) in source.iter().zip(target.iter()) {
        let x = source_point.0 - source_center.0;
        let y = source_point.1 - source_center.1;
        let u = target_point.0 - target_center.0;
        let v = target_point.1 - target_center.1;
        denominator += x * x + y * y;
        a_numerator += x * u + y * v;
        b_numerator += x * v - y * u;
    }
    if denominator <= f32::EPSILON {
        bail!("SFace landmarks do not define a usable similarity transform");
    }
    let a = a_numerator / denominator;
    let b = b_numerator / denominator;
    if a * a + b * b <= f32::EPSILON {
        bail!("SFace landmark similarity transform collapsed to zero scale");
    }
    let tx = target_center.0 - a * source_center.0 + b * source_center.1;
    let ty = target_center.1 - b * source_center.0 - a * source_center.1;
    Ok(SimilarityTransform { a, b, tx, ty })
}

fn bilinear_rgb(image: &ImageBuffer<Rgb<u8>, Vec<u8>>, x: f32, y: f32) -> Rgb<u8> {
    let width = image.width() as i32;
    let height = image.height() as i32;
    if x < 0.0 || y < 0.0 || x > (width - 1) as f32 || y > (height - 1) as f32 {
        return Rgb([0, 0, 0]);
    }
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let p00 = image.get_pixel(x0 as u32, y0 as u32);
    let p10 = image.get_pixel(x1 as u32, y0 as u32);
    let p01 = image.get_pixel(x0 as u32, y1 as u32);
    let p11 = image.get_pixel(x1 as u32, y1 as u32);
    let mut out = [0u8; 3];
    for channel in 0..3 {
        let top = p00[channel] as f32 * (1.0 - fx) + p10[channel] as f32 * fx;
        let bottom = p01[channel] as f32 * (1.0 - fx) + p11[channel] as f32 * fx;
        out[channel] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
    Rgb(out)
}

fn normalize(values: Vec<f32>) -> Result<Vec<f32>> {
    if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
        bail!("SFace embedding must be a finite non-empty vector");
    }
    let norm_sq = values
        .iter()
        .map(|value| (*value as f64) * (*value as f64))
        .sum::<f64>();
    if norm_sq <= f64::EPSILON {
        bail!("SFace embedding has zero length");
    }
    let norm = norm_sq.sqrt() as f32;
    Ok(values.into_iter().map(|value| value / norm).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_transform_maps_reference_geometry_to_itself() {
        let transform = similarity_transform(SFACE_REFERENCE, SFACE_REFERENCE).unwrap();
        for point in SFACE_REFERENCE {
            let mapped = transform.source_to_target(point.0, point.1);
            assert!((mapped.0 - point.0).abs() < 1e-4);
            assert!((mapped.1 - point.1).abs() < 1e-4);
        }
    }

    #[test]
    fn similarity_transform_recovers_translation_and_scale() {
        let source = SFACE_REFERENCE.map(|(x, y)| (x * 2.0 + 10.0, y * 2.0 + 20.0));
        let transform = similarity_transform(source, SFACE_REFERENCE).unwrap();
        for (source_point, target_point) in source.iter().zip(SFACE_REFERENCE.iter()) {
            let mapped = transform.source_to_target(source_point.0, source_point.1);
            assert!((mapped.0 - target_point.0).abs() < 1e-3);
            assert!((mapped.1 - target_point.1).abs() < 1e-3);
        }
    }

    #[test]
    fn nchw_preprocessing_preserves_rgb_planes_without_scaling() {
        let mut image = ImageBuffer::from_pixel(112, 112, Rgb([10u8, 20, 30]));
        image.put_pixel(1, 0, Rgb([40, 50, 60]));
        let values = sface_nchw_rgb_f32(&DynamicImage::ImageRgb8(image)).unwrap();
        let plane = 112usize * 112usize;
        assert_eq!(values[0], 10.0);
        assert_eq!(values[1], 40.0);
        assert_eq!(values[plane], 20.0);
        assert_eq!(values[plane + 1], 50.0);
        assert_eq!(values[2 * plane], 30.0);
        assert_eq!(values[2 * plane + 1], 60.0);
    }

    #[test]
    fn batch_preprocessing_concatenates_nchw_faces_in_batch_order() {
        let first = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(
            SFACE_INPUT_SIZE,
            SFACE_INPUT_SIZE,
            Rgb([10u8, 20, 30]),
        ));
        let second = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(
            SFACE_INPUT_SIZE,
            SFACE_INPUT_SIZE,
            Rgb([40u8, 50, 60]),
        ));
        let values = sface_batch_nchw_rgb_f32(&[first, second]).unwrap();
        let plane = (SFACE_INPUT_SIZE * SFACE_INPUT_SIZE) as usize;
        let per_face = plane * 3;
        assert_eq!(values.len(), per_face * 2);
        assert_eq!(values[0], 10.0);
        assert_eq!(values[plane], 20.0);
        assert_eq!(values[2 * plane], 30.0);
        assert_eq!(values[per_face], 40.0);
        assert_eq!(values[per_face + plane], 50.0);
        assert_eq!(values[per_face + 2 * plane], 60.0);
        assert!(sface_batch_nchw_rgb_f32(&[]).is_err());
    }

    #[test]
    fn normalize_produces_unit_vector_and_rejects_zero() {
        let values = normalize(vec![3.0, 4.0]).unwrap();
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert!(normalize(vec![0.0, 0.0]).is_err());
    }

    #[test]
    fn metadata_requires_external_file_and_license_fields() {
        let metadata = SFaceModelMetadata {
            model_path: PathBuf::from("definitely-missing-sface.onnx"),
            source: "user-supplied".to_owned(),
            license: "external".to_owned(),
            redistributable: false,
            commercial_use: false,
        };
        assert!(metadata.validate_external().is_err());
    }
}
