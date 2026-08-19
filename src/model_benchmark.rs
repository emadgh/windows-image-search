use crate::db;
use anyhow::{bail, Context, Result};
use fastembed::{ImageEmbedding, ImageEmbeddingModel, ImageInitOptions};
use image::{imageops::FilterType, DynamicImage, GenericImageView, ImageFormat};
use std::cmp::Ordering;
use std::fmt::Write as _;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DEFAULT_QUERY_COUNT: usize = 24;
const MAX_QUERY_COUNT: usize = 64;
const MAX_CORPUS_COUNT: usize = 512;
const BATCH_SIZE: usize = 16;

pub fn default_query_count() -> usize {
    DEFAULT_QUERY_COUNT
}

#[derive(Clone, Copy, Debug)]
enum BenchmarkModel {
    ClipVitB32,
    UnicomVitB16,
    UnicomVitB32,
    NomicEmbedVisionV15,
    Resnet50,
}

const MODELS: [BenchmarkModel; 5] = [
    BenchmarkModel::ClipVitB32,
    BenchmarkModel::UnicomVitB16,
    BenchmarkModel::UnicomVitB32,
    BenchmarkModel::NomicEmbedVisionV15,
    BenchmarkModel::Resnet50,
];

impl BenchmarkModel {
    fn label(self) -> &'static str {
        match self {
            Self::ClipVitB32 => "clip_vit_b32",
            Self::UnicomVitB16 => "unicom_vit_b16",
            Self::UnicomVitB32 => "unicom_vit_b32",
            Self::NomicEmbedVisionV15 => "nomic_embed_vision_v1_5",
            Self::Resnet50 => "resnet50",
        }
    }

    fn fastembed_model(self) -> ImageEmbeddingModel {
        match self {
            Self::ClipVitB32 => ImageEmbeddingModel::ClipVitB32,
            Self::UnicomVitB16 => ImageEmbeddingModel::UnicomVitB16,
            Self::UnicomVitB32 => ImageEmbeddingModel::UnicomVitB32,
            Self::NomicEmbedVisionV15 => ImageEmbeddingModel::NomicEmbedVisionV15,
            Self::Resnet50 => ImageEmbeddingModel::Resnet50,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum QueryVariant {
    CenterCrop80,
    OffsetCrop70,
    HalfResolution,
}

const QUERY_VARIANTS: [QueryVariant; 3] = [
    QueryVariant::CenterCrop80,
    QueryVariant::OffsetCrop70,
    QueryVariant::HalfResolution,
];

impl QueryVariant {
    fn label(self) -> &'static str {
        match self {
            Self::CenterCrop80 => "center_crop_80",
            Self::OffsetCrop70 => "offset_crop_70",
            Self::HalfResolution => "half_resolution",
        }
    }
}

struct QueryCase {
    target_index: usize,
    variant: QueryVariant,
    png_bytes: Vec<u8>,
}

#[derive(Debug, Default)]
struct RankMetrics {
    ranks: Vec<usize>,
}

impl RankMetrics {
    fn record(&mut self, rank: usize) {
        self.ranks.push(rank.max(1));
    }

    fn recall_at(&self, k: usize) -> f64 {
        if self.ranks.is_empty() {
            return 0.0;
        }
        self.ranks.iter().filter(|&&rank| rank <= k).count() as f64 / self.ranks.len() as f64
    }

    fn mrr(&self) -> f64 {
        if self.ranks.is_empty() {
            return 0.0;
        }
        self.ranks
            .iter()
            .map(|&rank| 1.0 / rank as f64)
            .sum::<f64>()
            / self.ranks.len() as f64
    }

    fn mean_rank(&self) -> f64 {
        if self.ranks.is_empty() {
            return 0.0;
        }
        self.ranks.iter().sum::<usize>() as f64 / self.ranks.len() as f64
    }
}

struct ModelReport {
    model: BenchmarkModel,
    init: Duration,
    corpus_embed: Duration,
    query_embed: Duration,
    embedding_dim: usize,
    overall: RankMetrics,
    per_variant: Vec<(QueryVariant, RankMetrics)>,
}

pub fn benchmark(db_path: &Path, model_cache: &Path, requested_queries: usize) -> Result<String> {
    let available: Vec<PathBuf> = db::load_image_summaries(db_path)?
        .into_iter()
        .map(|record| record.path)
        .filter(|path| path.is_file())
        .collect();
    if available.is_empty() {
        bail!("image-model benchmark needs at least one indexed image file that still exists");
    }

    let corpus_target = available.len().min(MAX_CORPUS_COUNT);
    let corpus: Vec<PathBuf> = sample_evenly(&available, corpus_target)
        .into_iter()
        .filter(|path| image::open(path).is_ok())
        .collect();
    if corpus.is_empty() {
        bail!("image-model benchmark could not decode any bounded corpus image");
    }

    let query_count = requested_queries
        .max(1)
        .min(MAX_QUERY_COUNT)
        .min(corpus.len());
    let queries = build_query_cases(&corpus, query_count)?;
    if queries.is_empty() {
        bail!("image-model benchmark could not build any transformed query");
    }

    std::fs::create_dir_all(model_cache)
        .with_context(|| format!("creating model cache {}", model_cache.display()))?;
    let cpu_threads = benchmark_cpu_threads();

    let mut report = String::new();
    writeln!(report, "Windows Image Search Image Model Benchmark")?;
    writeln!(report, "application_version=v{}", env!("CARGO_PKG_VERSION"))?;
    writeln!(report, "backend=cpu")?;
    writeln!(report, "production_model=ClipVitB32")?;
    writeln!(report, "production_behavior_changed=false")?;
    writeln!(report, "available_indexed_files={}", available.len())?;
    writeln!(report, "corpus_limit={MAX_CORPUS_COUNT}")?;
    writeln!(report, "corpus_used={}", corpus.len())?;
    writeln!(report, "queries_requested={requested_queries}")?;
    writeln!(report, "query_sources_used={query_count}")?;
    writeln!(report, "query_cases={}", queries.len())?;
    writeln!(report, "batch_size={BATCH_SIZE}")?;
    writeln!(report, "cpu_threads={cpu_threads}")?;
    writeln!(
        report,
        "query_variants={}",
        QUERY_VARIANTS
            .iter()
            .map(|variant| variant.label())
            .collect::<Vec<_>>()
            .join(",")
    )?;

    let mut successful_models = 0usize;
    for benchmark_model in MODELS {
        match benchmark_model_once(benchmark_model, model_cache, &corpus, &queries, cpu_threads) {
            Ok(model_report) => {
                successful_models += 1;
                append_model_report(&mut report, &model_report, corpus.len(), queries.len())?;
            }
            Err(err) => {
                let prefix = benchmark_model.label();
                writeln!(report, "{prefix}.status=error")?;
                writeln!(
                    report,
                    "{prefix}.error={}",
                    one_line_error(&format!("{err:#}"))
                )?;
            }
        }
    }
    writeln!(report, "successful_models={successful_models}")?;
    writeln!(
        report,
        "model_cache_bytes_after={}",
        directory_size(model_cache)
    )?;
    writeln!(
        report,
        "notes=Ground truth is recovery of each transformed query's original indexed source within a deterministic bounded corpus. This compares crop/layout/scale robustness and throughput across models without changing stored production embeddings. Representative same-material labels and manual tile/marble/stone evaluation are still required before changing the production model."
    )?;
    Ok(report)
}

fn benchmark_model_once(
    benchmark_model: BenchmarkModel,
    model_cache: &Path,
    corpus: &[PathBuf],
    queries: &[QueryCase],
    cpu_threads: usize,
) -> Result<ModelReport> {
    let init_started = Instant::now();
    let options = ImageInitOptions::new(benchmark_model.fastembed_model())
        .with_cache_dir(model_cache.to_path_buf())
        .with_show_download_progress(true)
        .with_intra_threads(cpu_threads);
    let mut model = ImageEmbedding::try_new(options)
        .with_context(|| format!("initializing {} on CPU", benchmark_model.label()))?;
    let init = init_started.elapsed();

    let corpus_started = Instant::now();
    let mut corpus_embeddings = model
        .embed(corpus, Some(BATCH_SIZE))
        .with_context(|| format!("embedding corpus with {}", benchmark_model.label()))?;
    let corpus_embed = corpus_started.elapsed();
    if corpus_embeddings.len() != corpus.len() {
        bail!(
            "{} returned {} corpus embeddings for {} images",
            benchmark_model.label(),
            corpus_embeddings.len(),
            corpus.len()
        );
    }
    for embedding in &mut corpus_embeddings {
        normalize(embedding);
    }
    let embedding_dim = corpus_embeddings.first().map(Vec::len).unwrap_or(0);
    if embedding_dim == 0
        || corpus_embeddings
            .iter()
            .any(|item| item.len() != embedding_dim)
    {
        bail!(
            "{} returned inconsistent embedding dimensions",
            benchmark_model.label()
        );
    }

    let query_refs: Vec<&[u8]> = queries
        .iter()
        .map(|query| query.png_bytes.as_slice())
        .collect();
    let query_started = Instant::now();
    let mut query_embeddings = model
        .embed_bytes(&query_refs, Some(BATCH_SIZE))
        .with_context(|| {
            format!(
                "embedding transformed queries with {}",
                benchmark_model.label()
            )
        })?;
    let query_embed = query_started.elapsed();
    if query_embeddings.len() != queries.len() {
        bail!(
            "{} returned {} query embeddings for {} query images",
            benchmark_model.label(),
            query_embeddings.len(),
            queries.len()
        );
    }
    for embedding in &mut query_embeddings {
        normalize(embedding);
    }
    if query_embeddings
        .iter()
        .any(|item| item.len() != embedding_dim)
    {
        bail!(
            "{} query/corpus embedding dimensions differ",
            benchmark_model.label()
        );
    }

    let mut overall = RankMetrics::default();
    let mut per_variant: Vec<(QueryVariant, RankMetrics)> = QUERY_VARIANTS
        .iter()
        .copied()
        .map(|variant| (variant, RankMetrics::default()))
        .collect();

    for (query_index, (query, embedding)) in queries.iter().zip(query_embeddings.iter()).enumerate()
    {
        let rank = rank_for_query(embedding, &corpus_embeddings, query.target_index);
        overall.record(rank);
        let variant_index = query_index % QUERY_VARIANTS.len();
        per_variant[variant_index].1.record(rank);
    }

    Ok(ModelReport {
        model: benchmark_model,
        init,
        corpus_embed,
        query_embed,
        embedding_dim,
        overall,
        per_variant,
    })
}

fn append_model_report(
    report: &mut String,
    model: &ModelReport,
    corpus_count: usize,
    query_count: usize,
) -> Result<()> {
    let prefix = model.model.label();
    let corpus_ms = ms(model.corpus_embed);
    let query_ms = ms(model.query_embed);
    let corpus_ips = if corpus_ms > f64::EPSILON {
        corpus_count as f64 * 1_000.0 / corpus_ms
    } else {
        0.0
    };
    let query_avg_ms = if query_count > 0 {
        query_ms / query_count as f64
    } else {
        0.0
    };

    writeln!(report, "{prefix}.status=ok")?;
    writeln!(report, "{prefix}.embedding_dim={}", model.embedding_dim)?;
    writeln!(report, "{prefix}.init_ms={:.3}", ms(model.init))?;
    writeln!(report, "{prefix}.corpus_embed_ms={corpus_ms:.3}")?;
    writeln!(report, "{prefix}.corpus_images_per_second={corpus_ips:.3}")?;
    writeln!(report, "{prefix}.query_embed_ms={query_ms:.3}")?;
    writeln!(report, "{prefix}.query_embed_ms_avg={query_avg_ms:.3}")?;
    append_rank_metrics(report, prefix, "overall", &model.overall)?;
    for (variant, metrics) in &model.per_variant {
        append_rank_metrics(report, prefix, variant.label(), metrics)?;
    }
    Ok(())
}

fn append_rank_metrics(
    report: &mut String,
    model_prefix: &str,
    scope: &str,
    metrics: &RankMetrics,
) -> Result<()> {
    writeln!(
        report,
        "{model_prefix}.{scope}.recall_at_10={:.4}",
        metrics.recall_at(10)
    )?;
    writeln!(
        report,
        "{model_prefix}.{scope}.recall_at_25={:.4}",
        metrics.recall_at(25)
    )?;
    writeln!(report, "{model_prefix}.{scope}.mrr={:.4}", metrics.mrr())?;
    writeln!(
        report,
        "{model_prefix}.{scope}.mean_rank={:.3}",
        metrics.mean_rank()
    )?;
    Ok(())
}

fn build_query_cases(corpus: &[PathBuf], query_count: usize) -> Result<Vec<QueryCase>> {
    let source_indices = sample_even_indices(corpus.len(), query_count.min(corpus.len()));
    let mut queries = Vec::with_capacity(source_indices.len() * QUERY_VARIANTS.len());
    for target_index in source_indices {
        let source = match image::open(&corpus[target_index]) {
            Ok(image) => image,
            Err(_) => continue,
        };
        for variant in QUERY_VARIANTS {
            let transformed = transform_query(&source, variant);
            queries.push(QueryCase {
                target_index,
                variant,
                png_bytes: encode_png(&transformed)?,
            });
        }
    }
    Ok(queries)
}

fn transform_query(image: &DynamicImage, variant: QueryVariant) -> DynamicImage {
    match variant {
        QueryVariant::CenterCrop80 => crop_fraction(image, 0.80, 0.50, 0.50),
        QueryVariant::OffsetCrop70 => crop_fraction(image, 0.70, 0.85, 0.20),
        QueryVariant::HalfResolution => {
            let (width, height) = image.dimensions();
            image.resize_exact(
                (width / 2).max(1),
                (height / 2).max(1),
                FilterType::Triangle,
            )
        }
    }
}

fn crop_fraction(
    image: &DynamicImage,
    fraction: f32,
    horizontal_position: f32,
    vertical_position: f32,
) -> DynamicImage {
    let (width, height) = image.dimensions();
    let crop_width = ((width as f32 * fraction).round() as u32).clamp(1, width.max(1));
    let crop_height = ((height as f32 * fraction).round() as u32).clamp(1, height.max(1));
    let free_x = width.saturating_sub(crop_width);
    let free_y = height.saturating_sub(crop_height);
    let x = ((free_x as f32 * horizontal_position.clamp(0.0, 1.0)).round() as u32).min(free_x);
    let y = ((free_y as f32 * vertical_position.clamp(0.0, 1.0)).round() as u32).min(free_y);
    image.crop_imm(x, y, crop_width, crop_height)
}

fn encode_png(image: &DynamicImage) -> Result<Vec<u8>> {
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png)?;
    Ok(bytes.into_inner())
}

fn rank_for_query(query: &[f32], corpus: &[Vec<f32>], target_index: usize) -> usize {
    let target_score = dot(query, &corpus[target_index]);
    let better = corpus
        .iter()
        .enumerate()
        .filter(
            |(index, candidate)| match dot(query, candidate).total_cmp(&target_score) {
                Ordering::Greater => true,
                Ordering::Equal => *index < target_index,
                Ordering::Less => false,
            },
        )
        .count();
    better + 1
}

fn normalize(values: &mut [f32]) {
    let norm_sq = values.iter().map(|value| value * value).sum::<f32>();
    if norm_sq <= f32::EPSILON {
        return;
    }
    let inverse = norm_sq.sqrt().recip();
    for value in values {
        *value *= inverse;
    }
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right.iter()).map(|(a, b)| a * b).sum()
}

fn sample_evenly(paths: &[PathBuf], count: usize) -> Vec<PathBuf> {
    sample_even_indices(paths.len(), count)
        .into_iter()
        .map(|index| paths[index].clone())
        .collect()
}

fn sample_even_indices(len: usize, count: usize) -> Vec<usize> {
    if len == 0 || count == 0 {
        return Vec::new();
    }
    let count = count.min(len);
    if count == 1 {
        return vec![0];
    }
    let last = len - 1;
    (0..count).map(|index| index * last / (count - 1)).collect()
}

fn benchmark_cpu_threads() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4)
        .saturating_sub(1)
        .max(1)
        .min(4)
}

fn directory_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| match entry.metadata() {
            Ok(metadata) if metadata.is_file() => metadata.len(),
            Ok(metadata) if metadata.is_dir() => directory_size(&entry.path()),
            _ => 0,
        })
        .sum()
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
    fn even_sampling_is_deterministic_and_keeps_endpoints() {
        assert_eq!(sample_even_indices(10, 4), vec![0, 3, 6, 9]);
        assert_eq!(sample_even_indices(3, 10), vec![0, 1, 2]);
        assert_eq!(sample_even_indices(9, 1), vec![0]);
        assert!(sample_even_indices(0, 5).is_empty());
    }

    #[test]
    fn rank_metrics_report_recall_and_mrr() {
        let mut metrics = RankMetrics::default();
        metrics.record(1);
        metrics.record(12);
        metrics.record(30);
        assert!((metrics.recall_at(10) - 1.0 / 3.0).abs() < 1e-9);
        assert!((metrics.recall_at(25) - 2.0 / 3.0).abs() < 1e-9);
        let expected_mrr = (1.0 + 1.0 / 12.0 + 1.0 / 30.0) / 3.0;
        assert!((metrics.mrr() - expected_mrr).abs() < 1e-9);
    }

    #[test]
    fn ranking_uses_stable_index_tie_breaking() {
        let corpus = vec![vec![1.0, 0.0], vec![1.0, 0.0], vec![0.0, 1.0]];
        assert_eq!(rank_for_query(&[1.0, 0.0], &corpus, 0), 1);
        assert_eq!(rank_for_query(&[1.0, 0.0], &corpus, 1), 2);
    }

    #[test]
    fn transforms_are_safe_for_tiny_images() {
        let image = DynamicImage::new_rgb8(1, 1);
        for variant in QUERY_VARIANTS {
            let transformed = transform_query(&image, variant);
            let (width, height) = transformed.dimensions();
            assert!(width >= 1);
            assert!(height >= 1);
            assert!(!encode_png(&transformed).unwrap().is_empty());
        }
    }

    #[test]
    fn one_line_errors_are_report_safe() {
        assert_eq!(one_line_error("first\nsecond=value"), "first second:value");
    }
}
