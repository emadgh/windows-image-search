use crate::db;
use anyhow::{bail, Context, Result};
use fastembed::{ImageEmbedding, ImageEmbeddingModel, ImageInitOptions};
use ort::ep::DirectML;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DEFAULT_SAMPLE_COUNT: usize = 32;
const MAX_SAMPLE_COUNT: usize = 256;
const BATCH_SIZES: [usize; 5] = [1, 4, 8, 16, 32];
const TIMING_REPEATS: usize = 3;

pub fn default_sample_count() -> usize {
    DEFAULT_SAMPLE_COUNT
}

#[derive(Clone, Copy, Debug)]
enum Backend {
    Cpu,
    DirectMl,
}

impl Backend {
    fn label(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::DirectMl => "directml",
        }
    }
}

#[derive(Debug)]
struct BatchMeasurement {
    batch_size: usize,
    median: Duration,
    min: Duration,
    max: Duration,
}

#[derive(Debug)]
struct BackendReport {
    backend: Backend,
    init: Duration,
    warmup: Duration,
    measurements: Vec<BatchMeasurement>,
}

pub fn benchmark(db_path: &Path, model_cache: &Path, requested_samples: usize) -> Result<String> {
    let available: Vec<PathBuf> = db::load_image_summaries(db_path)?
        .into_iter()
        .map(|record| record.path)
        .filter(|path| path.is_file())
        .collect();
    if available.is_empty() {
        bail!("CLIP runtime benchmark needs at least one indexed image file that still exists");
    }

    let sample_count = requested_samples
        .max(1)
        .min(MAX_SAMPLE_COUNT)
        .min(available.len());
    let samples = sample_evenly(&available, sample_count);
    std::fs::create_dir_all(model_cache)
        .with_context(|| format!("creating model cache {}", model_cache.display()))?;

    let cpu_threads = benchmark_cpu_threads();
    let cpu = benchmark_backend(Backend::Cpu, model_cache, &samples, cpu_threads)?;
    let directml = benchmark_backend(Backend::DirectMl, model_cache, &samples, cpu_threads);

    let mut report = String::new();
    writeln!(report, "Windows Image Search CLIP Runtime Benchmark")?;
    writeln!(report, "application_version=v{}", env!("CARGO_PKG_VERSION"))?;
    writeln!(report, "model=ClipVitB32")?;
    writeln!(report, "samples_requested={requested_samples}")?;
    writeln!(report, "samples_used={}", samples.len())?;
    writeln!(report, "cpu_threads={cpu_threads}")?;
    writeln!(report, "timing_repeats={TIMING_REPEATS}")?;
    writeln!(
        report,
        "batch_sizes={}",
        BATCH_SIZES
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )?;
    writeln!(report, "production_backend=cpu")?;
    writeln!(report, "production_behavior_changed=false")?;
    append_backend_report(&mut report, &cpu, samples.len())?;

    match directml {
        Ok(directml) => {
            writeln!(report, "directml_available=true")?;
            append_backend_report(&mut report, &directml, samples.len())?;
            append_speedups(&mut report, &cpu, &directml)?;
        }
        Err(err) => {
            writeln!(report, "directml_available=false")?;
            writeln!(
                report,
                "directml_error={}",
                one_line_error(&format!("{err:#}"))
            )?;
        }
    }

    writeln!(
        report,
        "notes=Each backend is initialized once, warmed up once, then the same sampled image set is embedded three times per batch size; median wall time is reported. DirectML failure is diagnostic only and never changes the production CPU path."
    )?;
    Ok(report)
}

fn benchmark_backend(
    backend: Backend,
    model_cache: &Path,
    samples: &[PathBuf],
    cpu_threads: usize,
) -> Result<BackendReport> {
    let init_started = Instant::now();
    let options = match backend {
        Backend::Cpu => ImageInitOptions::new(ImageEmbeddingModel::ClipVitB32)
            .with_cache_dir(model_cache.to_path_buf())
            .with_show_download_progress(true)
            .with_intra_threads(cpu_threads),
        Backend::DirectMl => ImageInitOptions::new(ImageEmbeddingModel::ClipVitB32)
            .with_cache_dir(model_cache.to_path_buf())
            .with_show_download_progress(true)
            .with_execution_providers(vec![DirectML::default().into()]),
    };
    let mut model = ImageEmbedding::try_new(options)
        .with_context(|| format!("initializing {} CLIP runtime", backend.label()))?;
    let init = init_started.elapsed();

    let warmup_started = Instant::now();
    let warmup = model
        .embed(vec![samples[0].clone()], Some(1))
        .with_context(|| format!("warming up {} CLIP runtime", backend.label()))?;
    if warmup.len() != 1 {
        bail!(
            "{} warmup returned {} embeddings for one image",
            backend.label(),
            warmup.len()
        );
    }
    let warmup = warmup_started.elapsed();

    let mut measurements = Vec::with_capacity(BATCH_SIZES.len());
    for batch_size in BATCH_SIZES {
        let mut timings = Vec::with_capacity(TIMING_REPEATS);
        for _ in 0..TIMING_REPEATS {
            let started = Instant::now();
            let embeddings = model
                .embed(samples.to_vec(), Some(batch_size))
                .with_context(|| {
                    format!(
                        "embedding {} samples with {} batch {}",
                        samples.len(),
                        backend.label(),
                        batch_size
                    )
                })?;
            let elapsed = started.elapsed();
            if embeddings.len() != samples.len() {
                bail!(
                    "{} batch {} returned {} embeddings for {} images",
                    backend.label(),
                    batch_size,
                    embeddings.len(),
                    samples.len()
                );
            }
            timings.push(elapsed);
        }
        timings.sort_unstable();
        measurements.push(BatchMeasurement {
            batch_size,
            median: timings[timings.len() / 2],
            min: timings[0],
            max: timings[timings.len() - 1],
        });
    }

    Ok(BackendReport {
        backend,
        init,
        warmup,
        measurements,
    })
}

fn append_backend_report(
    report: &mut String,
    backend: &BackendReport,
    sample_count: usize,
) -> Result<()> {
    let prefix = backend.backend.label();
    writeln!(report, "{prefix}_init_ms={:.3}", ms(backend.init))?;
    writeln!(report, "{prefix}_warmup_ms={:.3}", ms(backend.warmup))?;
    for measurement in &backend.measurements {
        let batch = measurement.batch_size;
        let median_ms = ms(measurement.median);
        let images_per_second = if median_ms > f64::EPSILON {
            sample_count as f64 * 1_000.0 / median_ms
        } else {
            0.0
        };
        writeln!(report, "{prefix}_batch_{batch}_median_ms={median_ms:.3}")?;
        writeln!(
            report,
            "{prefix}_batch_{batch}_min_ms={:.3}",
            ms(measurement.min)
        )?;
        writeln!(
            report,
            "{prefix}_batch_{batch}_max_ms={:.3}",
            ms(measurement.max)
        )?;
        writeln!(
            report,
            "{prefix}_batch_{batch}_images_per_second={images_per_second:.3}"
        )?;
    }
    if let Some(best) = backend
        .measurements
        .iter()
        .min_by(|left, right| left.median.cmp(&right.median))
    {
        writeln!(report, "{prefix}_best_batch_size={}", best.batch_size)?;
        writeln!(report, "{prefix}_best_median_ms={:.3}", ms(best.median))?;
    }
    Ok(())
}

fn append_speedups(
    report: &mut String,
    cpu: &BackendReport,
    directml: &BackendReport,
) -> Result<()> {
    for cpu_measurement in &cpu.measurements {
        let Some(dml_measurement) = directml
            .measurements
            .iter()
            .find(|item| item.batch_size == cpu_measurement.batch_size)
        else {
            continue;
        };
        let dml_ms = ms(dml_measurement.median);
        let speedup = if dml_ms > f64::EPSILON {
            ms(cpu_measurement.median) / dml_ms
        } else {
            0.0
        };
        writeln!(
            report,
            "directml_vs_cpu_batch_{}_speedup_x={speedup:.3}",
            cpu_measurement.batch_size
        )?;
    }
    Ok(())
}

fn benchmark_cpu_threads() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4)
        .saturating_sub(1)
        .max(1)
        .min(4)
}

fn sample_evenly(paths: &[PathBuf], count: usize) -> Vec<PathBuf> {
    if paths.len() <= count {
        return paths.to_vec();
    }
    if count <= 1 {
        return vec![paths[0].clone()];
    }

    let last = paths.len() - 1;
    (0..count)
        .map(|index| paths[index * last / (count - 1)].clone())
        .collect()
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn one_line_error(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('=', ":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_is_even_and_keeps_endpoints() {
        let paths: Vec<PathBuf> = (0..9)
            .map(|index| PathBuf::from(format!("{index}.jpg")))
            .collect();
        let sampled = sample_evenly(&paths, 4);
        assert_eq!(sampled.len(), 4);
        assert_eq!(sampled.first(), paths.first());
        assert_eq!(sampled.last(), paths.last());
    }

    #[test]
    fn one_line_errors_are_report_safe() {
        assert_eq!(one_line_error("first\nsecond=value"), "first second:value");
    }

    #[test]
    fn benchmark_thread_count_is_bounded() {
        assert!((1..=4).contains(&benchmark_cpu_threads()));
    }
}
