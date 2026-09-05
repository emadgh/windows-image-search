use crate::{face_benchmark, face_detection, face_sface_adapter};
use anyhow::{bail, Context, Result};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug)]
struct FaceCase {
    face_id: String,
    person_id: String,
    image_path: PathBuf,
    landmarks: [face_sface_adapter::LandmarkPoint; 5],
}

#[derive(Clone, Debug)]
struct RunnerManifest {
    model: face_sface_adapter::SFaceModelMetadata,
    provider: face_sface_adapter::SFaceExecutionProvider,
    faces: Vec<FaceCase>,
}

pub fn benchmark(path: &Path) -> Result<String> {
    let manifest = load_runner_manifest(path)?;
    manifest.model.validate_external()?;
    if manifest.faces.len() < 2 {
        bail!("SFace benchmark needs at least two labeled face rows");
    }

    let mut adapter =
        face_sface_adapter::SFaceOnnxAdapter::load(&manifest.model.model_path, manifest.provider)?;
    let mut embeddings = Vec::with_capacity(manifest.faces.len());
    let mut aligned_faces = Vec::with_capacity(manifest.faces.len());
    let mut inference_times = Vec::with_capacity(manifest.faces.len());

    for face in &manifest.faces {
        let oriented = face_detection::decode_oriented(&face.image_path).with_context(|| {
            format!(
                "decoding SFace benchmark image {}",
                face.image_path.display()
            )
        })?;
        let landmarks = face
            .landmarks
            .map(|point| face_sface_adapter::LandmarkPoint {
                x: point.x * oriented.width() as f32,
                y: point.y * oriented.height() as f32,
            });
        let aligned = face_sface_adapter::align_sface_112(&oriented, landmarks)
            .with_context(|| format!("aligning SFace case {}", face.face_id))?;
        let (embedding, stats) = adapter
            .embed_aligned(&aligned)
            .with_context(|| format!("embedding SFace case {}", face.face_id))?;
        inference_times.push(stats.inference);
        embeddings.push(embedding);
        aligned_faces.push(aligned);
    }

    let evaluator_manifest = build_evaluator_manifest(&manifest, &embeddings)?;
    let temp_path = temporary_evaluator_path(path);
    std::fs::write(&temp_path, &evaluator_manifest).with_context(|| {
        format!(
            "writing temporary face evaluator manifest {}",
            temp_path.display()
        )
    })?;
    let evaluated = face_benchmark::benchmark(&temp_path);
    let _ = std::fs::remove_file(&temp_path);
    let evaluated = evaluated?;

    let mut report = String::new();
    writeln!(report, "Windows Image Search SFace ONNX Benchmark")?;
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
    writeln!(report, "provider={}", manifest.provider.as_str())?;
    writeln!(report, "model_bytes={}", manifest.model.file_size()?)?;
    writeln!(
        report,
        "model_fingerprint_fnv1a64={:016x}",
        manifest.model.file_fingerprint_fnv1a64()?
    )?;
    writeln!(report, "faces={}", manifest.faces.len())?;
    writeln!(
        report,
        "embedding_dimension={}",
        face_sface_adapter::SFACE_EMBEDDING_DIMENSION
    )?;
    writeln!(
        report,
        "init_ms={:.3}",
        adapter.init_duration().as_secs_f64() * 1000.0
    )?;
    append_latency(&mut report, &inference_times)?;
    append_batch_sweep(&mut report, &mut adapter, &aligned_faces)?;
    writeln!(report)?;
    writeln!(report, "shared_evaluator_begin")?;
    write!(report, "{evaluated}")?;
    writeln!(report, "shared_evaluator_end")?;
    Ok(report)
}

fn load_runner_manifest(path: &Path) -> Result<RunnerManifest> {
    if path.as_os_str().is_empty() {
        bail!("SFace runner manifest path is required");
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading SFace runner manifest {}", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let mut model: Option<face_sface_adapter::SFaceModelMetadata> = None;
    let mut provider = None;
    let mut faces = Vec::new();

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
                if columns.len() != 7 {
                    bail!("line {line}: model row requires 7 tab-separated columns");
                }
                if model.is_some() {
                    bail!("line {line}: only one model row is allowed");
                }
                let model_path = resolve_path(base, required(columns[1], line, "model path")?);
                provider = Some(parse_provider(columns[2], line)?);
                model = Some(face_sface_adapter::SFaceModelMetadata {
                    model_path,
                    license: required(columns[3], line, "model license")?.to_owned(),
                    redistributable: parse_bool(columns[4], line, "redistributable")?,
                    commercial_use: parse_bool(columns[5], line, "commercial_use")?,
                    source: required(columns[6], line, "model source")?.to_owned(),
                });
            }
            "face" => {
                if columns.len() != 14 {
                    bail!("line {line}: face row requires 14 tab-separated columns");
                }
                let face_id = required(columns[1], line, "face id")?.to_owned();
                let person_id = required(columns[2], line, "person id")?.to_owned();
                let image_path = resolve_path(base, required(columns[3], line, "image path")?);
                let mut values = [0.0f32; 10];
                for (offset, value) in values.iter_mut().enumerate() {
                    *value = columns[4 + offset]
                        .parse::<f32>()
                        .with_context(|| format!("line {line}: invalid landmark value"))?;
                    if !value.is_finite() || !(0.0..=1.0).contains(value) {
                        bail!(
                            "line {line}: landmarks must be normalized finite coordinates in [0,1]"
                        );
                    }
                }
                faces.push(FaceCase {
                    face_id,
                    person_id,
                    image_path,
                    landmarks: [
                        point(values[0], values[1]),
                        point(values[2], values[3]),
                        point(values[4], values[5]),
                        point(values[6], values[7]),
                        point(values[8], values[9]),
                    ],
                });
            }
            other => bail!("line {line}: unsupported SFace runner record type {other:?}"),
        }
    }

    let model = model.context("SFace runner manifest is missing model row")?;
    let provider = provider.context("SFace runner manifest is missing execution provider")?;
    if faces.is_empty() {
        bail!("SFace runner manifest contains no face rows");
    }
    let mut ids = std::collections::BTreeSet::new();
    for face in &faces {
        if !ids.insert(face.face_id.as_str()) {
            bail!("duplicate SFace face id {:?}", face.face_id);
        }
        if !face.image_path.is_file() {
            bail!("SFace image does not exist: {}", face.image_path.display());
        }
    }
    Ok(RunnerManifest {
        model,
        provider,
        faces,
    })
}

fn build_evaluator_manifest(manifest: &RunnerManifest, embeddings: &[Vec<f32>]) -> Result<String> {
    if embeddings.len() != manifest.faces.len() {
        bail!("SFace embedding count does not match labeled face count");
    }
    let fingerprint = manifest.model.file_fingerprint_fnv1a64()?;
    let mut output = String::new();
    writeln!(
        output,
        "model\tsface-external\tfnv-{fingerprint:016x}\t{}\t{}\t{}\t{}\texternal\t{}",
        manifest.provider.as_str(),
        tsv_field(&manifest.model.license),
        manifest.model.redistributable,
        manifest.model.commercial_use,
        tsv_field(&manifest.model.source),
    )?;
    for (query_index, query) in manifest.faces.iter().enumerate() {
        for (candidate_index, candidate) in manifest.faces.iter().enumerate() {
            if query_index == candidate_index {
                continue;
            }
            let similarity = cosine(&embeddings[query_index], &embeddings[candidate_index])?;
            writeln!(
                output,
                "identity\t{}\t{}\t{}\t{}\t{similarity:.8}",
                tsv_field(&query.face_id),
                tsv_field(&query.person_id),
                tsv_field(&candidate.face_id),
                tsv_field(&candidate.person_id),
            )?;
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
    writeln!(report, "inference_faces_per_second={throughput:.3}")?;
    Ok(())
}

fn append_batch_sweep(
    report: &mut String,
    adapter: &mut face_sface_adapter::SFaceOnnxAdapter,
    aligned_faces: &[image::DynamicImage],
) -> Result<()> {
    const BATCH_SIZES: [usize; 5] = [1, 2, 4, 8, 16];
    const MEASURED_ITERATIONS: usize = 5;

    writeln!(report, "batch_sweep_begin")?;
    writeln!(report, "batch_sweep_iterations={MEASURED_ITERATIONS}")?;
    for batch_size in BATCH_SIZES {
        let batch = (0..batch_size)
            .map(|index| aligned_faces[index % aligned_faces.len()].clone())
            .collect::<Vec<_>>();

        let supported = adapter.embed_aligned_batch(&batch);
        match supported {
            Ok((_warmup, warmup_stats)) => {
                let mut times = Vec::with_capacity(MEASURED_ITERATIONS);
                let mut dimensions_ok = true;
                for _ in 0..MEASURED_ITERATIONS {
                    let (embeddings, stats) = adapter.embed_aligned_batch(&batch)?;
                    dimensions_ok &= embeddings.len() == batch_size
                        && embeddings.iter().all(|embedding| {
                            embedding.len() == face_sface_adapter::SFACE_EMBEDDING_DIMENSION
                        });
                    times.push(stats.inference.as_secs_f64() * 1000.0);
                }
                times.sort_by(f64::total_cmp);
                let total_ms = times.iter().sum::<f64>();
                let mean_ms = total_ms / times.len() as f64;
                let p50_ms = percentile(&times, 0.50);
                let faces_per_second = if total_ms <= f64::EPSILON {
                    0.0
                } else {
                    (batch_size * times.len()) as f64 / (total_ms / 1000.0)
                };
                writeln!(report, "batch_{batch_size}_supported=true")?;
                writeln!(
                    report,
                    "batch_{batch_size}_warmup_ms={:.3}",
                    warmup_stats.inference.as_secs_f64() * 1000.0
                )?;
                writeln!(report, "batch_{batch_size}_mean_ms={mean_ms:.3}")?;
                writeln!(report, "batch_{batch_size}_p50_ms={p50_ms:.3}")?;
                writeln!(
                    report,
                    "batch_{batch_size}_faces_per_second={faces_per_second:.3}"
                )?;
                writeln!(report, "batch_{batch_size}_dimensions_ok={dimensions_ok}")?;
            }
            Err(err) => {
                writeln!(report, "batch_{batch_size}_supported=false")?;
                writeln!(
                    report,
                    "batch_{batch_size}_error={}",
                    tsv_field(&format!("{err:#}"))
                )?;
            }
        }
    }
    writeln!(report, "batch_sweep_end")?;
    Ok(())
}

fn cosine(left: &[f32], right: &[f32]) -> Result<f32> {
    if left.is_empty() || left.len() != right.len() {
        bail!("SFace embeddings must have equal non-zero dimensions");
    }
    Ok(left.iter().zip(right).map(|(a, b)| a * b).sum())
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * p.clamp(0.0, 1.0)).round() as usize;
    sorted[index]
}

fn parse_provider(value: &str, line: usize) -> Result<face_sface_adapter::SFaceExecutionProvider> {
    match value.trim().to_ascii_lowercase().as_str() {
        "cpu" => Ok(face_sface_adapter::SFaceExecutionProvider::Cpu),
        "directml" | "dml" => Ok(face_sface_adapter::SFaceExecutionProvider::DirectMl),
        other => bail!("line {line}: SFace provider must be cpu or directml, got {other:?}"),
    }
}

fn parse_bool(value: &str, line: usize, label: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => bail!("line {line}: {label} must be true/false"),
    }
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
        "wis-sface-eval-{}-{hash:016x}.tsv",
        std::process::id()
    ))
}

fn point(x: f32, y: f32) -> face_sface_adapter::LandmarkPoint {
    face_sface_adapter::LandmarkPoint { x, y }
}

fn tsv_field(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_manifest_contains_ordered_identity_pairs() {
        let root = std::env::temp_dir();
        let model_path = root.join("fake-sface-model.onnx");
        std::fs::write(&model_path, b"fake-model").unwrap();
        let manifest = RunnerManifest {
            model: face_sface_adapter::SFaceModelMetadata {
                model_path: model_path.clone(),
                source: "user-supplied".into(),
                license: "external-review".into(),
                redistributable: false,
                commercial_use: false,
            },
            provider: face_sface_adapter::SFaceExecutionProvider::Cpu,
            faces: vec![
                FaceCase {
                    face_id: "a".into(),
                    person_id: "p1".into(),
                    image_path: root.join("a.jpg"),
                    landmarks: [point(0.1, 0.1); 5],
                },
                FaceCase {
                    face_id: "b".into(),
                    person_id: "p1".into(),
                    image_path: root.join("b.jpg"),
                    landmarks: [point(0.2, 0.2); 5],
                },
                FaceCase {
                    face_id: "c".into(),
                    person_id: "p2".into(),
                    image_path: root.join("c.jpg"),
                    landmarks: [point(0.3, 0.3); 5],
                },
            ],
        };
        let embeddings = vec![vec![1.0, 0.0], vec![0.9, 0.1], vec![0.0, 1.0]];
        let text = build_evaluator_manifest(&manifest, &embeddings).unwrap();
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with("identity\t"))
                .count(),
            6
        );
        assert!(text.contains("identity\ta\tp1\tb\tp1"));
        assert!(text.contains("identity\ta\tp1\tc\tp2"));
        let _ = std::fs::remove_file(model_path);
    }

    #[test]
    fn provider_parser_is_explicit() {
        assert_eq!(
            parse_provider("dml", 1).unwrap(),
            face_sface_adapter::SFaceExecutionProvider::DirectMl
        );
        assert!(parse_provider("cuda", 1).is_err());
    }
}
