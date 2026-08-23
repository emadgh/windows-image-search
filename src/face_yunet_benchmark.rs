use crate::{face_benchmark, face_detection};
use anyhow::{bail, Context, Result};
use face_detection::yunet_adapter::{
    YuNetExecutionProvider, YuNetOnnxAdapter, DEFAULT_NMS_THRESHOLD, DEFAULT_SCORE_THRESHOLD,
    DEFAULT_TOP_K,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
struct ModelConfig {
    model_path: PathBuf,
    provider: YuNetExecutionProvider,
    license: String,
    redistributable: bool,
    commercial_use: bool,
    source: String,
}

#[derive(Clone, Debug)]
struct ImageCase {
    image_id: String,
    image_path: PathBuf,
}

#[derive(Clone, Debug)]
struct RunnerManifest {
    model: ModelConfig,
    score_threshold: f32,
    nms_threshold: f32,
    top_k: usize,
    images: Vec<ImageCase>,
    ground_truth: BTreeMap<String, Vec<face_benchmark::NormalizedBox>>,
}

pub fn benchmark(path: &Path) -> Result<String> {
    let manifest = load_runner_manifest(path)?;

    let init_started = Instant::now();
    let mut adapter = YuNetOnnxAdapter::load(
        &manifest.model.model_path,
        manifest.model.provider,
        manifest.score_threshold,
        manifest.nms_threshold,
        manifest.top_k,
    )?;
    let init_duration = init_started.elapsed();
    let model_fingerprint = adapter.model_fingerprint();

    let mut predictions: BTreeMap<String, Vec<face_detection::DetectedFace>> = BTreeMap::new();
    let mut inference_times = Vec::with_capacity(manifest.images.len());

    for image in &manifest.images {
        let oriented = face_detection::decode_oriented(&image.image_path).with_context(|| {
            format!(
                "decoding YuNet benchmark image {}",
                image.image_path.display()
            )
        })?;
        let started = Instant::now();
        let detected = adapter
            .detect(&oriented)
            .with_context(|| format!("running YuNet on image {}", image.image_id))?;
        inference_times.push(started.elapsed());
        predictions.insert(image.image_id.clone(), detected);
    }

    let evaluator_manifest = build_evaluator_manifest(&manifest, &predictions, model_fingerprint)?;
    let temp_path = temporary_evaluator_path(path);
    std::fs::write(&temp_path, &evaluator_manifest).with_context(|| {
        format!(
            "writing temporary YuNet evaluator manifest {}",
            temp_path.display()
        )
    })?;
    let evaluated = face_benchmark::benchmark(&temp_path);
    let _ = std::fs::remove_file(&temp_path);
    let evaluated = evaluated?;

    let model_bytes = std::fs::metadata(&manifest.model.model_path)
        .with_context(|| {
            format!(
                "reading YuNet model metadata {}",
                manifest.model.model_path.display()
            )
        })?
        .len();

    let mut report = String::new();
    writeln!(report, "Windows Image Search YuNet ONNX Benchmark")?;
    writeln!(report, "application_version=v{}", env!("CARGO_PKG_VERSION"))?;
    writeln!(report, "runner_manifest={}", path.display())?;
    writeln!(report, "model_path={}", manifest.model.model_path.display())?;
    writeln!(report, "model_source={}", manifest.model.source)?;
    writeln!(report, "model_license={}", manifest.model.license)?;
    writeln!(
        report,
        "model_redistributable={}",
        manifest.model.redistributable
    )?;
    writeln!(
        report,
        "model_commercial_use={}",
        manifest.model.commercial_use
    )?;
    writeln!(report, "model_source_mode=external")?;
    writeln!(report, "provider={}", manifest.model.provider.as_str())?;
    writeln!(report, "model_bytes={model_bytes}")?;
    writeln!(report, "model_fingerprint_fnv1a64={model_fingerprint:016x}")?;
    writeln!(report, "images={}", manifest.images.len())?;
    writeln!(
        report,
        "ground_truth_faces={}",
        manifest.ground_truth.values().map(Vec::len).sum::<usize>()
    )?;
    writeln!(report, "score_threshold={:.4}", manifest.score_threshold)?;
    writeln!(report, "nms_threshold={:.4}", manifest.nms_threshold)?;
    writeln!(report, "top_k={}", manifest.top_k)?;
    writeln!(
        report,
        "init_ms={:.3}",
        init_duration.as_secs_f64() * 1000.0
    )?;
    append_latency(&mut report, &inference_times)?;
    writeln!(report)?;
    writeln!(report, "shared_evaluator_begin")?;
    write!(report, "{evaluated}")?;
    writeln!(report, "shared_evaluator_end")?;
    Ok(report)
}

fn load_runner_manifest(path: &Path) -> Result<RunnerManifest> {
    if path.as_os_str().is_empty() {
        bail!("YuNet runner manifest path is required");
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading YuNet runner manifest {}", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));

    let mut model: Option<ModelConfig> = None;
    let mut settings_seen = false;
    let mut score_threshold = DEFAULT_SCORE_THRESHOLD;
    let mut nms_threshold = DEFAULT_NMS_THRESHOLD;
    let mut top_k = DEFAULT_TOP_K;
    let mut images = Vec::new();
    let mut image_ids = BTreeSet::new();
    let mut ground_truth: BTreeMap<String, Vec<face_benchmark::NormalizedBox>> = BTreeMap::new();

    for (index, raw) in content.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let columns: Vec<&str> = raw.split('\t').map(str::trim).collect();
        match columns
            .first()
            .copied()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "model" => {
                expect_columns(&columns, 7, line, "model")?;
                if model.is_some() {
                    bail!("line {line}: only one YuNet model row is allowed");
                }
                let model_path = resolve_path(base, required(columns[1], line, "model path")?);
                let provider = parse_provider(columns[2], line)?;
                let license = required(columns[3], line, "model license")?.to_owned();
                let redistributable = parse_bool(columns[4], line, "redistributable")?;
                let commercial_use = parse_bool(columns[5], line, "commercial_use")?;
                let source = required(columns[6], line, "model source")?.to_owned();
                model = Some(ModelConfig {
                    model_path,
                    provider,
                    license,
                    redistributable,
                    commercial_use,
                    source,
                });
            }
            "settings" => {
                expect_columns(&columns, 4, line, "settings")?;
                if settings_seen {
                    bail!("line {line}: only one YuNet settings row is allowed");
                }
                settings_seen = true;
                score_threshold = parse_unit_f32(columns[1], line, "score threshold")?;
                nms_threshold = parse_unit_f32(columns[2], line, "NMS threshold")?;
                top_k = columns[3]
                    .parse::<usize>()
                    .with_context(|| format!("line {line}: invalid Top-K"))?;
                if top_k == 0 {
                    bail!("line {line}: Top-K must be greater than zero");
                }
            }
            "image" => {
                expect_columns(&columns, 3, line, "image")?;
                let image_id = required(columns[1], line, "image id")?.to_owned();
                if !image_ids.insert(image_id.clone()) {
                    bail!("line {line}: duplicate YuNet image id {image_id:?}");
                }
                let image_path = resolve_path(base, required(columns[2], line, "image path")?);
                images.push(ImageCase {
                    image_id,
                    image_path,
                });
            }
            "gt" => {
                expect_columns(&columns, 6, line, "gt")?;
                let image_id = required(columns[1], line, "image id")?.to_owned();
                let bbox = parse_box(&columns[2..6], line)?;
                ground_truth.entry(image_id).or_default().push(bbox);
            }
            other => bail!("line {line}: unsupported YuNet runner record type {other:?}"),
        }
    }

    let model = model.context("YuNet runner manifest is missing model row")?;
    if !model.model_path.is_file() {
        bail!(
            "YuNet ONNX file does not exist: {}",
            model.model_path.display()
        );
    }
    if images.is_empty() {
        bail!("YuNet runner manifest contains no image rows");
    }
    for image in &images {
        if !image.image_path.is_file() {
            bail!("YuNet image does not exist: {}", image.image_path.display());
        }
    }
    for image_id in ground_truth.keys() {
        if !image_ids.contains(image_id) {
            bail!("YuNet ground truth references undeclared image {image_id:?}");
        }
    }

    Ok(RunnerManifest {
        model,
        score_threshold,
        nms_threshold,
        top_k,
        images,
        ground_truth,
    })
}

fn build_evaluator_manifest(
    manifest: &RunnerManifest,
    predictions: &BTreeMap<String, Vec<face_detection::DetectedFace>>,
    model_fingerprint: u64,
) -> Result<String> {
    let mut output = String::new();
    writeln!(
        output,
        "model\tyunet-external\tfnv-{model_fingerprint:016x}\t{}\t{}\t{}\t{}\texternal\t{}",
        manifest.model.provider.as_str(),
        tsv_field(&manifest.model.license),
        manifest.model.redistributable,
        manifest.model.commercial_use,
        tsv_field(&manifest.model.source),
    )?;

    for image in &manifest.images {
        writeln!(output, "image\t{}", tsv_field(&image.image_id))?;
        if let Some(targets) = manifest.ground_truth.get(&image.image_id) {
            for bbox in targets {
                writeln!(
                    output,
                    "gt\t{}\t{:.8}\t{:.8}\t{:.8}\t{:.8}",
                    tsv_field(&image.image_id),
                    bbox.x,
                    bbox.y,
                    bbox.width,
                    bbox.height
                )?;
            }
        }
        if let Some(detected) = predictions.get(&image.image_id) {
            for face in detected {
                let bbox = face.bbox.clamped();
                if bbox.width <= 0.0 || bbox.height <= 0.0 {
                    continue;
                }
                writeln!(
                    output,
                    "pred\t{}\t{:.8}\t{:.8}\t{:.8}\t{:.8}\t{:.8}",
                    tsv_field(&image.image_id),
                    face.confidence.clamp(0.0, 1.0),
                    bbox.x,
                    bbox.y,
                    bbox.width,
                    bbox.height
                )?;
            }
        }
    }
    Ok(output)
}

fn append_latency(report: &mut String, times: &[Duration]) -> Result<()> {
    if times.is_empty() {
        return Ok(());
    }
    let mut millis: Vec<f64> = times
        .iter()
        .map(|value| value.as_secs_f64() * 1000.0)
        .collect();
    millis.sort_by(f64::total_cmp);
    let total_ms = millis.iter().sum::<f64>();
    let mean = total_ms / millis.len() as f64;
    let p50 = percentile(&millis, 0.50);
    let p95 = percentile(&millis, 0.95);
    let throughput = if total_ms <= f64::EPSILON {
        0.0
    } else {
        millis.len() as f64 / (total_ms / 1000.0)
    };
    writeln!(report, "inference_mean_ms={mean:.3}")?;
    writeln!(report, "inference_p50_ms={p50:.3}")?;
    writeln!(report, "inference_p95_ms={p95:.3}")?;
    writeln!(report, "inference_images_per_second={throughput:.3}")?;
    Ok(())
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * p.clamp(0.0, 1.0)).round() as usize;
    sorted[index]
}

fn parse_provider(value: &str, line: usize) -> Result<YuNetExecutionProvider> {
    match value.trim().to_ascii_lowercase().as_str() {
        "cpu" => Ok(YuNetExecutionProvider::Cpu),
        "directml" | "dml" => Ok(YuNetExecutionProvider::DirectMl),
        other => bail!("line {line}: YuNet provider must be cpu or directml, got {other:?}"),
    }
}

fn parse_bool(value: &str, line: usize, label: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => bail!("line {line}: {label} must be true/false"),
    }
}

fn parse_unit_f32(value: &str, line: usize, label: &str) -> Result<f32> {
    let parsed = value
        .parse::<f32>()
        .with_context(|| format!("line {line}: invalid {label}"))?;
    if !parsed.is_finite() || !(0.0..=1.0).contains(&parsed) {
        bail!("line {line}: {label} must be a finite value in [0,1]");
    }
    Ok(parsed)
}

fn parse_box(values: &[&str], line: usize) -> Result<face_benchmark::NormalizedBox> {
    if values.len() != 4 {
        bail!("line {line}: face box requires x, y, width, height");
    }
    let x = parse_unit_f32(values[0], line, "box x")?;
    let y = parse_unit_f32(values[1], line, "box y")?;
    let width = parse_unit_f32(values[2], line, "box width")?;
    let height = parse_unit_f32(values[3], line, "box height")?;
    if width <= 0.0 || height <= 0.0 || x + width > 1.000_001 || y + height > 1.000_001 {
        bail!("line {line}: face box must be positive and remain inside normalized [0,1]");
    }
    Ok(face_benchmark::NormalizedBox {
        x,
        y,
        width,
        height,
    })
}

fn expect_columns(columns: &[&str], expected: usize, line: usize, kind: &str) -> Result<()> {
    if columns.len() != expected {
        bail!(
            "line {line}: {kind} row requires {expected} tab-separated columns, got {}",
            columns.len()
        );
    }
    Ok(())
}

fn required<'a>(value: &'a str, line: usize, label: &str) -> Result<&'a str> {
    if value.trim().is_empty() {
        bail!("line {line}: {label} cannot be empty");
    }
    Ok(value.trim())
}

fn resolve_path(base: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn temporary_evaluator_path(manifest: &Path) -> PathBuf {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in manifest.to_string_lossy().as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    std::env::temp_dir().join(format!(
        "wis-yunet-eval-{}-{hash:016x}.tsv",
        std::process::id()
    ))
}

fn tsv_field(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_parser_is_explicit() {
        assert_eq!(
            parse_provider("dml", 1).unwrap(),
            YuNetExecutionProvider::DirectMl
        );
        assert!(parse_provider("cuda", 1).is_err());
    }

    #[test]
    fn evaluator_manifest_contains_detector_rows() {
        let manifest = RunnerManifest {
            model: ModelConfig {
                model_path: PathBuf::from("model.onnx"),
                provider: YuNetExecutionProvider::Cpu,
                license: "MIT".into(),
                redistributable: true,
                commercial_use: true,
                source: "opencv-zoo".into(),
            },
            score_threshold: 0.6,
            nms_threshold: 0.3,
            top_k: 5000,
            images: vec![ImageCase {
                image_id: "img-a".into(),
                image_path: PathBuf::from("a.jpg"),
            }],
            ground_truth: BTreeMap::from([(
                "img-a".into(),
                vec![face_benchmark::NormalizedBox {
                    x: 0.1,
                    y: 0.2,
                    width: 0.3,
                    height: 0.4,
                }],
            )]),
        };
        let predictions = BTreeMap::from([(
            "img-a".into(),
            vec![face_detection::DetectedFace {
                confidence: 0.9,
                bbox: face_detection::FaceBox {
                    x: 0.11,
                    y: 0.21,
                    width: 0.29,
                    height: 0.39,
                },
                landmarks: Vec::new(),
            }],
        )]);
        let text = build_evaluator_manifest(&manifest, &predictions, 0x1234).unwrap();
        assert!(text.contains("model\tyunet-external\tfnv-0000000000001234\tcpu\tMIT"));
        assert!(text.contains("image\timg-a"));
        assert!(text.contains("gt\timg-a\t0.10000000\t0.20000000\t0.30000001\t0.40000001"));
        assert!(text.contains("pred\timg-a\t0.89999998"));
    }

    #[test]
    fn box_parser_rejects_out_of_bounds_geometry() {
        assert!(parse_box(&["0.9", "0.1", "0.2", "0.2"], 1).is_err());
        assert!(parse_box(&["0.1", "0.1", "0", "0.2"], 1).is_err());
    }
}
