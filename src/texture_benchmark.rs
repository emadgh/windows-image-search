use crate::{db, material_texture};
use anyhow::{bail, Result};
use image::{imageops::FilterType, DynamicImage, GenericImageView};
use std::cmp::Ordering;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DEFAULT_SAMPLE_COUNT: usize = 24;
const MAX_SAMPLE_COUNT: usize = 128;

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

#[derive(Debug)]
struct BenchmarkRecord {
    rowid: usize,
    path: PathBuf,
    visual_hash: u64,
    descriptor: Vec<f32>,
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

#[derive(Debug, Default)]
struct ScoreMetrics {
    dhash: RankMetrics,
    gradient: RankMetrics,
    lbp: RankMetrics,
    material: RankMetrics,
    hybrid: RankMetrics,
}

impl ScoreMetrics {
    fn record(&mut self, ranks: [usize; 5]) {
        self.dhash.record(ranks[0]);
        self.gradient.record(ranks[1]);
        self.lbp.record(ranks[2]);
        self.material.record(ranks[3]);
        self.hybrid.record(ranks[4]);
    }

    fn queries(&self) -> usize {
        self.dhash.ranks.len()
    }
}

pub fn default_sample_count() -> usize {
    DEFAULT_SAMPLE_COUNT
}

pub fn benchmark(db_path: &Path, requested_samples: usize) -> Result<String> {
    let corpus: Vec<BenchmarkRecord> = db::load_search_images(db_path)?
        .into_iter()
        .filter_map(|record| {
            let visual_hash = record.visual_hash?;
            let descriptor = record.material_texture?;
            if descriptor.len() != material_texture::DIM || !record.path.is_file() {
                return None;
            }
            Some(BenchmarkRecord {
                rowid: record.rowid,
                path: record.path,
                visual_hash,
                descriptor,
            })
        })
        .collect();

    if corpus.is_empty() {
        bail!(
            "material-texture benchmark needs at least one indexed image with a current texture descriptor"
        );
    }

    let sample_count = requested_samples
        .max(1)
        .min(MAX_SAMPLE_COUNT)
        .min(corpus.len());
    let sample_indices = sample_even_indices(corpus.len(), sample_count);

    let mut overall = ScoreMetrics::default();
    let mut per_variant: Vec<(QueryVariant, ScoreMetrics)> = QUERY_VARIANTS
        .iter()
        .copied()
        .map(|variant| (variant, ScoreMetrics::default()))
        .collect();
    let mut descriptor_times = Vec::<Duration>::new();
    let mut ranking_times = Vec::<Duration>::new();
    let mut decode_failures = 0usize;

    for sample_index in sample_indices {
        let target = &corpus[sample_index];
        let source = match image::open(&target.path) {
            Ok(image) => image,
            Err(_) => {
                decode_failures += 1;
                continue;
            }
        };

        for (variant_index, variant) in QUERY_VARIANTS.iter().copied().enumerate() {
            let transformed = transform_query(&source, variant);

            let descriptor_started = Instant::now();
            let query_hash = difference_hash(&transformed);
            let query_descriptor = material_texture::descriptor(&transformed);
            descriptor_times.push(descriptor_started.elapsed());

            let ranking_started = Instant::now();
            let ranks = ranks_for_query(query_hash, &query_descriptor, target, &corpus);
            ranking_times.push(ranking_started.elapsed());

            overall.record(ranks);
            per_variant[variant_index].1.record(ranks);
        }
    }

    if overall.queries() == 0 {
        bail!("material-texture benchmark could not decode any sampled indexed image");
    }

    let mut report = String::new();
    writeln!(report, "Windows Image Search Material Texture Benchmark")?;
    writeln!(report, "application_version=v{}", env!("CARGO_PKG_VERSION"))?;
    writeln!(report, "descriptor_version={}", material_texture::VERSION)?;
    writeln!(report, "descriptor_dim={}", material_texture::DIM)?;
    writeln!(report, "corpus_records={}", corpus.len())?;
    writeln!(report, "samples_requested={requested_samples}")?;
    writeln!(report, "samples_selected={sample_count}")?;
    writeln!(report, "source_decode_failures={decode_failures}")?;
    writeln!(report, "queries_completed={}", overall.queries())?;
    writeln!(
        report,
        "query_variants={}",
        QUERY_VARIANTS
            .iter()
            .map(|variant| variant.label())
            .collect::<Vec<_>>()
            .join(",")
    )?;
    writeln!(report, "production_behavior_changed=false")?;
    writeln!(
        report,
        "descriptor_compute_ms_avg={:.3}",
        average_ms(&descriptor_times)
    )?;
    writeln!(
        report,
        "descriptor_compute_ms_p95={:.3}",
        percentile_ms(&descriptor_times, 0.95)
    )?;
    writeln!(
        report,
        "corpus_rank_ms_avg={:.3}",
        average_ms(&ranking_times)
    )?;
    writeln!(
        report,
        "corpus_rank_ms_p95={:.3}",
        percentile_ms(&ranking_times, 0.95)
    )?;

    append_score_metrics(&mut report, "overall", &overall)?;
    for (variant, metrics) in &per_variant {
        append_score_metrics(&mut report, variant.label(), metrics)?;
    }

    writeln!(
        report,
        "notes=Ground truth is the original indexed image used to create each transformed query. This objectively measures crop/layout/scale robustness against the current corpus, but it does not replace representative same-material evaluation across different tile/marble/stone faces before closing issue #32."
    )?;
    Ok(report)
}

fn ranks_for_query(
    query_hash: u64,
    query_descriptor: &[f32],
    target: &BenchmarkRecord,
    corpus: &[BenchmarkRecord],
) -> [usize; 5] {
    let target_scores = scores_for_record(query_hash, query_descriptor, target);
    let mut better = [0usize; 5];

    for record in corpus {
        let scores = scores_for_record(query_hash, query_descriptor, record);
        for index in 0..scores.len() {
            if outranks(
                scores[index],
                record.rowid,
                target_scores[index],
                target.rowid,
            ) {
                better[index] += 1;
            }
        }
    }

    better.map(|count| count + 1)
}

fn scores_for_record(
    query_hash: u64,
    query_descriptor: &[f32],
    record: &BenchmarkRecord,
) -> [f32; 5] {
    let dhash = perceptual_hash_similarity(query_hash, record.visual_hash);
    let gradient =
        material_texture::gradient_similarity(query_descriptor, &record.descriptor).unwrap_or(0.0);
    let lbp = material_texture::lbp_similarity(query_descriptor, &record.descriptor).unwrap_or(0.0);
    let material =
        material_texture::similarity(query_descriptor, &record.descriptor).unwrap_or(0.0);
    let hybrid = material_texture::combine_with_dhash(Some(dhash), Some(material)).unwrap_or(0.0);
    [dhash, gradient, lbp, material, hybrid]
}

fn outranks(score: f32, rowid: usize, target_score: f32, target_rowid: usize) -> bool {
    match score.total_cmp(&target_score) {
        Ordering::Greater => true,
        Ordering::Equal => rowid < target_rowid,
        Ordering::Less => false,
    }
}

fn append_score_metrics(report: &mut String, prefix: &str, metrics: &ScoreMetrics) -> Result<()> {
    writeln!(report, "{prefix}.queries={}", metrics.queries())?;
    append_rank_metrics(report, prefix, "dhash", &metrics.dhash)?;
    append_rank_metrics(report, prefix, "gradient", &metrics.gradient)?;
    append_rank_metrics(report, prefix, "lbp", &metrics.lbp)?;
    append_rank_metrics(report, prefix, "material", &metrics.material)?;
    append_rank_metrics(report, prefix, "hybrid", &metrics.hybrid)?;
    Ok(())
}

fn append_rank_metrics(
    report: &mut String,
    prefix: &str,
    method: &str,
    metrics: &RankMetrics,
) -> Result<()> {
    writeln!(
        report,
        "{prefix}.{method}.recall_at_1={:.4}",
        metrics.recall_at(1)
    )?;
    writeln!(
        report,
        "{prefix}.{method}.recall_at_5={:.4}",
        metrics.recall_at(5)
    )?;
    writeln!(
        report,
        "{prefix}.{method}.recall_at_10={:.4}",
        metrics.recall_at(10)
    )?;
    writeln!(report, "{prefix}.{method}.mrr={:.4}", metrics.mrr())?;
    writeln!(
        report,
        "{prefix}.{method}.mean_rank={:.3}",
        metrics.mean_rank()
    )?;
    Ok(())
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

fn difference_hash(image: &DynamicImage) -> u64 {
    let gray = image.resize_exact(9, 8, FilterType::Triangle).to_luma8();
    let mut hash = 0u64;
    let mut bit = 0u32;
    for y in 0..8 {
        for x in 0..8 {
            if gray.get_pixel(x, y)[0] > gray.get_pixel(x + 1, y)[0] {
                hash |= 1u64 << bit;
            }
            bit += 1;
        }
    }
    hash
}

fn perceptual_hash_similarity(a: u64, b: u64) -> f32 {
    1.0 - ((a ^ b).count_ones() as f32 / 64.0)
}

fn sample_even_indices(len: usize, count: usize) -> Vec<usize> {
    if len == 0 || count == 0 {
        return Vec::new();
    }
    let count = count.min(len);
    (0..count).map(|index| index * len / count).collect()
}

fn average_ms(values: &[Duration]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().map(duration_ms).sum::<f64>() / values.len() as f64
}

fn percentile_ms(values: &[Duration], percentile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let index = (((sorted.len() - 1) as f64 * percentile.clamp(0.0, 1.0)).round() as usize)
        .min(sorted.len() - 1);
    duration_ms(&sorted[index])
}

fn duration_ms(duration: &Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_sampling_is_deterministic_and_unique() {
        assert_eq!(sample_even_indices(10, 4), vec![0, 2, 5, 7]);
        assert_eq!(sample_even_indices(3, 10), vec![0, 1, 2]);
        assert!(sample_even_indices(0, 5).is_empty());
    }

    #[test]
    fn rank_metrics_report_recall_and_mrr() {
        let mut metrics = RankMetrics::default();
        metrics.record(1);
        metrics.record(3);
        metrics.record(11);
        assert!((metrics.recall_at(1) - 1.0 / 3.0).abs() < 1e-9);
        assert!((metrics.recall_at(5) - 2.0 / 3.0).abs() < 1e-9);
        assert!((metrics.recall_at(10) - 2.0 / 3.0).abs() < 1e-9);
        let expected_mrr = (1.0 + 1.0 / 3.0 + 1.0 / 11.0) / 3.0;
        assert!((metrics.mrr() - expected_mrr).abs() < 1e-9);
    }

    #[test]
    fn query_transforms_are_safe_for_tiny_images() {
        let image = DynamicImage::new_rgb8(1, 1);
        for variant in QUERY_VARIANTS {
            let transformed = transform_query(&image, variant);
            let (width, height) = transformed.dimensions();
            assert!(width >= 1);
            assert!(height >= 1);
            assert_eq!(
                material_texture::descriptor(&transformed).len(),
                material_texture::DIM
            );
        }
    }

    #[test]
    fn tie_breaking_is_stable_by_rowid() {
        assert!(outranks(0.5, 2, 0.5, 3));
        assert!(!outranks(0.5, 4, 0.5, 3));
        assert!(outranks(0.6, 99, 0.5, 3));
    }
}
