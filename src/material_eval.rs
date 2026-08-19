use crate::{db, material_texture};
use anyhow::{bail, Context, Result};
use fastembed::{ImageEmbedding, ImageEmbeddingModel, ImageInitOptions};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MODEL_BATCH_SIZE: usize = 16;

#[derive(Clone, Copy, Debug)]
enum EvalModel {
    ClipVitB32,
    UnicomVitB16,
    UnicomVitB32,
    NomicEmbedVisionV15,
    Resnet50,
}

const EVAL_MODELS: [EvalModel; 5] = [
    EvalModel::ClipVitB32,
    EvalModel::UnicomVitB16,
    EvalModel::UnicomVitB32,
    EvalModel::NomicEmbedVisionV15,
    EvalModel::Resnet50,
];

impl EvalModel {
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct ManifestItem {
    group: String,
    path: PathBuf,
}

#[derive(Clone, Debug)]
struct EvalItem {
    group: String,
    path: PathBuf,
    rowid: usize,
    visual_hash: u64,
    material_texture: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
struct RankMetrics {
    ranks: Vec<usize>,
}

impl RankMetrics {
    fn record(&mut self, rank: usize) {
        self.ranks.push(rank.max(1));
    }

    fn queries(&self) -> usize {
        self.ranks.len()
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

pub fn benchmark(db_path: &Path, model_cache: &Path, manifest_path: &Path) -> Result<String> {
    let manifest_text = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("reading material evaluation manifest {}", manifest_path.display()))?;
    let base_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let manifest = parse_manifest_text(&manifest_text, base_dir)?;
    validate_group_sizes(&manifest)?;

    let records = db::load_search_images(db_path)?;
    let mut by_path = HashMap::with_capacity(records.len());
    for record in records {
        by_path.insert(normalized_path_key(&record.path), record);
    }

    let mut items = Vec::with_capacity(manifest.len());
    for manifest_item in &manifest {
        if !manifest_item.path.is_file() {
            bail!(
                "material evaluation source does not exist: {}",
                manifest_item.path.display()
            );
        }
        let key = normalized_path_key(&manifest_item.path);
        let Some(record) = by_path.get(&key) else {
            bail!(
                "material evaluation path is not indexed: {}",
                manifest_item.path.display()
            );
        };
        let Some(visual_hash) = record.visual_hash else {
            bail!(
                "material evaluation path is missing dHash; rescan first: {}",
                manifest_item.path.display()
            );
        };
        let Some(material) = record.material_texture.as_ref() else {
            bail!(
                "material evaluation path is missing the current material descriptor; rescan first: {}",
                manifest_item.path.display()
            );
        };
        if material.len() != material_texture::DIM {
            bail!(
                "material evaluation path has an unexpected material descriptor dimension: {}",
                manifest_item.path.display()
            );
        }
        items.push(EvalItem {
            group: manifest_item.group.clone(),
            path: record.path.clone(),
            rowid: record.rowid,
            visual_hash,
            material_texture: material.clone(),
        });
    }

    let group_sizes = group_size_map(&manifest);
    let group_size_values: Vec<usize> = group_sizes.values().copied().collect();
    let all_indices: Vec<usize> = (0..items.len()).collect();

    let dhash = evaluate_scores(&items, &all_indices, |query, candidate| {
        Some(perceptual_hash_similarity(query.visual_hash, candidate.visual_hash))
    });
    let gradient = evaluate_scores(&items, &all_indices, |query, candidate| {
        material_texture::gradient_similarity(&query.material_texture, &candidate.material_texture)
    });
    let lbp = evaluate_scores(&items, &all_indices, |query, candidate| {
        material_texture::lbp_similarity(&query.material_texture, &candidate.material_texture)
    });
    let material = evaluate_scores(&items, &all_indices, |query, candidate| {
        material_texture::similarity(&query.material_texture, &candidate.material_texture)
    });
    let texture_hybrid = evaluate_scores(&items, &all_indices, |query, candidate| {
        let hash = perceptual_hash_similarity(query.visual_hash, candidate.visual_hash);
        let texture =
            material_texture::similarity(&query.material_texture, &candidate.material_texture)?;
        material_texture::combine_with_dhash(Some(hash), Some(texture))
    });

    let rowids: HashSet<usize> = items.iter().map(|item| item.rowid).collect();
    let stored_embeddings = db::load_embeddings_for_rowids(db_path, &rowids)?;
    let mut production_embeddings = HashMap::<usize, Vec<f32>>::new();
    for item in &items {
        let Some((embedding, already_normalized)) = stored_embeddings.get(&item.rowid) else {
            continue;
        };
        let mut embedding = embedding.clone();
        if !*already_normalized {
            normalize(&mut embedding);
        }
        production_embeddings.insert(item.rowid, embedding);
    }
    let clip_indices = indices_with_group_positives(
        &items,
        items
            .iter()
            .enumerate()
            .filter(|(_, item)| production_embeddings.contains_key(&item.rowid))
            .map(|(index, _)| index),
    );
    let production_clip = evaluate_scores(&items, &clip_indices, |query, candidate| {
        let left = production_embeddings.get(&query.rowid)?;
        let right = production_embeddings.get(&candidate.rowid)?;
        Some(dot(left, right))
    });

    std::fs::create_dir_all(model_cache)
        .with_context(|| format!("creating model cache {}", model_cache.display()))?;
    let cpu_threads = benchmark_cpu_threads();

    let mut report = String::new();
    writeln!(report, "Windows Image Search Labeled Material Evaluation")?;
    writeln!(report, "application_version=v{}", env!("CARGO_PKG_VERSION"))?;
    writeln!(report, "manifest={}", manifest_path.display())?;
    writeln!(report, "production_behavior_changed=false")?;
    writeln!(report, "groups={}", group_sizes.len())?;
    writeln!(report, "images={}", items.len())?;
    writeln!(report, "group_size_min={}", group_size_values.iter().min().copied().unwrap_or(0))?;
    writeln!(
        report,
        "group_size_p50={}",
        percentile_usize(&group_size_values, 0.50)
    )?;
    writeln!(report, "group_size_max={}", group_size_values.iter().max().copied().unwrap_or(0))?;
    for (group, size) in &group_sizes {
        writeln!(report, "group.{}.images={size}", report_key(group))?;
    }
    writeln!(report, "descriptor_coverage={}/{}", items.len(), items.len())?;
    writeln!(
        report,
        "stored_production_clip_coverage={}/{}",
        production_embeddings.len(),
        items.len()
    )?;
    writeln!(report, "model_backend=cpu")?;
    writeln!(report, "model_batch_size={MODEL_BATCH_SIZE}")?;
    writeln!(report, "model_cpu_threads={cpu_threads}")?;

    append_metrics(&mut report, "indexed_dhash", &dhash)?;
    append_metrics(&mut report, "indexed_gradient", &gradient)?;
    append_metrics(&mut report, "indexed_lbp", &lbp)?;
    append_metrics(&mut report, "indexed_material", &material)?;
    append_metrics(&mut report, "indexed_texture_hybrid", &texture_hybrid)?;
    if production_clip.queries() > 0 {
        append_metrics(&mut report, "stored_production_clip", &production_clip)?;
    } else {
        writeln!(report, "stored_production_clip.status=insufficient_coverage")?;
    }

    let paths: Vec<PathBuf> = items.iter().map(|item| item.path.clone()).collect();
    let mut successful_models = 0usize;
    for eval_model in EVAL_MODELS {
        match benchmark_model(eval_model, model_cache, &items, &paths, cpu_threads) {
            Ok(model_report) => {
                successful_models += 1;
                append_model_report(&mut report, &model_report, items.len())?;
            }
            Err(err) => {
                let prefix = format!("model.{}", eval_model.label());
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
        "notes=Every image is used as a query, the query image itself is excluded, and the first different image from the same labeled group is the relevant retrieval target. Recall@K therefore means at least one other face of the same material/design appeared within K results. This diagnostic does not alter stored production embeddings or search defaults."
    )?;
    Ok(report)
}

#[derive(Debug)]
struct ModelReport {
    model: EvalModel,
    init: Duration,
    embedding_time: Duration,
    embedding_dim: usize,
    metrics: RankMetrics,
}

fn benchmark_model(
    eval_model: EvalModel,
    model_cache: &Path,
    items: &[EvalItem],
    paths: &[PathBuf],
    cpu_threads: usize,
) -> Result<ModelReport> {
    let init_started = Instant::now();
    let options = ImageInitOptions::new(eval_model.fastembed_model())
        .with_cache_dir(model_cache.to_path_buf())
        .with_show_download_progress(true)
        .with_intra_threads(cpu_threads);
    let mut model = ImageEmbedding::try_new(options)
        .with_context(|| format!("initializing {} on CPU", eval_model.label()))?;
    let init = init_started.elapsed();

    let embedding_started = Instant::now();
    let mut embeddings = model
        .embed(paths, Some(MODEL_BATCH_SIZE))
        .with_context(|| format!("embedding labeled material set with {}", eval_model.label()))?;
    let embedding_time = embedding_started.elapsed();
    if embeddings.len() != items.len() {
        bail!(
            "{} returned {} embeddings for {} labeled images",
            eval_model.label(),
            embeddings.len(),
            items.len()
        );
    }
    for embedding in &mut embeddings {
        normalize(embedding);
    }
    let embedding_dim = embeddings.first().map(Vec::len).unwrap_or(0);
    if embedding_dim == 0 || embeddings.iter().any(|embedding| embedding.len() != embedding_dim) {
        bail!("{} returned inconsistent embedding dimensions", eval_model.label());
    }

    let all_indices: Vec<usize> = (0..items.len()).collect();
    let metrics = evaluate_scores(items, &all_indices, |query, candidate| {
        let query_index = items.iter().position(|item| item.rowid == query.rowid)?;
        let candidate_index = items.iter().position(|item| item.rowid == candidate.rowid)?;
        Some(dot(&embeddings[query_index], &embeddings[candidate_index]))
    });

    Ok(ModelReport {
        model: eval_model,
        init,
        embedding_time,
        embedding_dim,
        metrics,
    })
}

fn append_model_report(report: &mut String, model: &ModelReport, image_count: usize) -> Result<()> {
    let prefix = format!("model.{}", model.model.label());
    let embed_ms = ms(model.embedding_time);
    let images_per_second = if embed_ms > f64::EPSILON {
        image_count as f64 * 1_000.0 / embed_ms
    } else {
        0.0
    };
    writeln!(report, "{prefix}.status=ok")?;
    writeln!(report, "{prefix}.embedding_dim={}", model.embedding_dim)?;
    writeln!(report, "{prefix}.init_ms={:.3}", ms(model.init))?;
    writeln!(report, "{prefix}.embed_ms={embed_ms:.3}")?;
    writeln!(
        report,
        "{prefix}.images_per_second={images_per_second:.3}"
    )?;
    append_metrics(report, &prefix, &model.metrics)?;
    Ok(())
}

fn append_metrics(report: &mut String, prefix: &str, metrics: &RankMetrics) -> Result<()> {
    writeln!(report, "{prefix}.queries={}", metrics.queries())?;
    writeln!(
        report,
        "{prefix}.recall_at_1={:.4}",
        metrics.recall_at(1)
    )?;
    writeln!(
        report,
        "{prefix}.recall_at_5={:.4}",
        metrics.recall_at(5)
    )?;
    writeln!(
        report,
        "{prefix}.recall_at_10={:.4}",
        metrics.recall_at(10)
    )?;
    writeln!(
        report,
        "{prefix}.recall_at_25={:.4}",
        metrics.recall_at(25)
    )?;
    writeln!(report, "{prefix}.mrr={:.4}", metrics.mrr())?;
    writeln!(
        report,
        "{prefix}.mean_first_relevant_rank={:.3}",
        metrics.mean_rank()
    )?;
    Ok(())
}

fn evaluate_scores<F>(items: &[EvalItem], eligible_indices: &[usize], mut score: F) -> RankMetrics
where
    F: FnMut(&EvalItem, &EvalItem) -> Option<f32>,
{
    let eligible: HashSet<usize> = eligible_indices.iter().copied().collect();
    let mut metrics = RankMetrics::default();

    for &query_index in eligible_indices {
        let query = &items[query_index];
        let has_relevant = eligible_indices
            .iter()
            .any(|&index| index != query_index && items[index].group == query.group);
        if !has_relevant {
            continue;
        }

        let mut ranked = Vec::<(usize, f32)>::new();
        for (candidate_index, candidate) in items.iter().enumerate() {
            if candidate_index == query_index || !eligible.contains(&candidate_index) {
                continue;
            }
            if let Some(value) = score(query, candidate) {
                ranked.push((candidate_index, value));
            }
        }
        ranked.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });

        if let Some(rank) = ranked
            .iter()
            .position(|(candidate_index, _)| items[*candidate_index].group == query.group)
        {
            metrics.record(rank + 1);
        }
    }
    metrics
}

fn indices_with_group_positives<I>(items: &[EvalItem], indices: I) -> Vec<usize>
where
    I: IntoIterator<Item = usize>,
{
    let candidates: Vec<usize> = indices.into_iter().collect();
    let mut counts = HashMap::<&str, usize>::new();
    for &index in &candidates {
        *counts.entry(items[index].group.as_str()).or_default() += 1;
    }
    candidates
        .into_iter()
        .filter(|&index| counts.get(items[index].group.as_str()).copied().unwrap_or(0) >= 2)
        .collect()
}

fn parse_manifest_text(text: &str, base_dir: &Path) -> Result<Vec<ManifestItem>> {
    let mut output = Vec::<ManifestItem>::new();
    let mut path_groups = HashMap::<String, String>::new();

    for (line_index, raw_line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((group_raw, path_raw)) = raw_line.split_once('\t') else {
            bail!("manifest line {line_number} must contain group<TAB>path");
        };
        let group = group_raw.trim();
        let path_text = path_raw.trim();
        if line_number == 1
            && group.eq_ignore_ascii_case("group")
            && path_text.eq_ignore_ascii_case("path")
        {
            continue;
        }
        if group.is_empty() || path_text.is_empty() {
            bail!("manifest line {line_number} has an empty group or path");
        }

        let raw_path = PathBuf::from(path_text);
        let path = if raw_path.is_absolute() {
            raw_path
        } else {
            base_dir.join(raw_path)
        };
        let key = normalized_path_key(&path);
        if let Some(existing_group) = path_groups.get(&key) {
            if existing_group != group {
                bail!(
                    "manifest path is assigned to multiple groups: {} ({existing_group} vs {group})",
                    path.display()
                );
            }
            continue;
        }
        path_groups.insert(key, group.to_owned());
        output.push(ManifestItem {
            group: group.to_owned(),
            path,
        });
    }

    if output.is_empty() {
        bail!("material evaluation manifest contains no image rows");
    }
    Ok(output)
}

fn validate_group_sizes(items: &[ManifestItem]) -> Result<()> {
    let groups = group_size_map(items);
    let too_small: Vec<String> = groups
        .iter()
        .filter(|(_, count)| **count < 2)
        .map(|(group, count)| format!("{group} ({count})"))
        .collect();
    if !too_small.is_empty() {
        bail!(
            "every material evaluation group needs at least two distinct images; too small: {}",
            too_small.join(", ")
        );
    }
    Ok(())
}

fn group_size_map(items: &[ManifestItem]) -> BTreeMap<String, usize> {
    let mut groups = BTreeMap::<String, usize>::new();
    for item in items {
        *groups.entry(item.group.clone()).or_default() += 1;
    }
    groups
}

fn normalized_path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn perceptual_hash_similarity(a: u64, b: u64) -> f32 {
    1.0 - ((a ^ b).count_ones() as f32 / 64.0)
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

fn benchmark_cpu_threads() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4)
        .saturating_sub(1)
        .max(1)
        .min(4)
}

fn percentile_usize(values: &[usize], percentile: f64) -> usize {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let index = (((sorted.len() - 1) as f64 * percentile.clamp(0.0, 1.0)).round() as usize)
        .min(sorted.len() - 1);
    sorted[index]
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn report_key(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
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

    fn synthetic_items() -> Vec<EvalItem> {
        vec![
            EvalItem {
                group: "a".to_owned(),
                path: PathBuf::from("a1.jpg"),
                rowid: 1,
                visual_hash: 0,
                material_texture: vec![0.0; material_texture::DIM],
            },
            EvalItem {
                group: "a".to_owned(),
                path: PathBuf::from("a2.jpg"),
                rowid: 2,
                visual_hash: 0,
                material_texture: vec![0.0; material_texture::DIM],
            },
            EvalItem {
                group: "a".to_owned(),
                path: PathBuf::from("a3.jpg"),
                rowid: 3,
                visual_hash: 0,
                material_texture: vec![0.0; material_texture::DIM],
            },
            EvalItem {
                group: "b".to_owned(),
                path: PathBuf::from("b1.jpg"),
                rowid: 4,
                visual_hash: 0,
                material_texture: vec![0.0; material_texture::DIM],
            },
            EvalItem {
                group: "b".to_owned(),
                path: PathBuf::from("b2.jpg"),
                rowid: 5,
                visual_hash: 0,
                material_texture: vec![0.0; material_texture::DIM],
            },
        ]
    }

    #[test]
    fn manifest_supports_header_comments_relative_paths_and_same_group_dedup() {
        let parsed = parse_manifest_text(
            "group\tpath\n# comment\na\tone.jpg\na\tone.jpg\na\ttwo.jpg\nb\tthree.jpg\nb\tfour.jpg\n",
            Path::new("C:/eval"),
        )
        .unwrap();
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[0].group, "a");
        assert_eq!(parsed[0].path, Path::new("C:/eval").join("one.jpg"));
        validate_group_sizes(&parsed).unwrap();
    }

    #[test]
    fn manifest_rejects_path_assigned_to_different_groups() {
        let error = parse_manifest_text(
            "a\tone.jpg\nb\tone.jpg\nb\ttwo.jpg\n",
            Path::new("C:/eval"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("multiple groups"));
    }

    #[test]
    fn group_validation_rejects_singletons() {
        let parsed = parse_manifest_text(
            "a\tone.jpg\na\ttwo.jpg\nb\tthree.jpg\n",
            Path::new("C:/eval"),
        )
        .unwrap();
        let error = validate_group_sizes(&parsed).unwrap_err();
        assert!(error.to_string().contains("b (1)"));
    }

    #[test]
    fn ranking_excludes_self_and_uses_first_of_multiple_positives() {
        let items = synthetic_items();
        let scores = [
            [1.0, 0.7, 0.6, 0.9, 0.8],
            [0.7, 1.0, 0.95, 0.4, 0.3],
            [0.6, 0.95, 1.0, 0.2, 0.1],
            [0.9, 0.4, 0.2, 1.0, 0.85],
            [0.8, 0.3, 0.1, 0.85, 1.0],
        ];
        let indices: Vec<usize> = (0..items.len()).collect();
        let metrics = evaluate_scores(&items, &indices, |query, candidate| {
            let query_index = query.rowid - 1;
            let candidate_index = candidate.rowid - 1;
            Some(scores[query_index][candidate_index])
        });
        assert_eq!(metrics.queries(), 5);
        // Query a1 ranks b1, b2 before the first same-group image => rank 3.
        assert_eq!(metrics.ranks[0], 3);
        // Query a2 has a3 as the best non-self result => rank 1.
        assert_eq!(metrics.ranks[1], 1);
    }

    #[test]
    fn metrics_report_expected_recall_and_mrr() {
        let mut metrics = RankMetrics::default();
        for rank in [1, 3, 12, 30] {
            metrics.record(rank);
        }
        assert!((metrics.recall_at(1) - 0.25).abs() < 1e-9);
        assert!((metrics.recall_at(5) - 0.50).abs() < 1e-9);
        assert!((metrics.recall_at(10) - 0.50).abs() < 1e-9);
        assert!((metrics.recall_at(25) - 0.75).abs() < 1e-9);
        let expected = (1.0 + 1.0 / 3.0 + 1.0 / 12.0 + 1.0 / 30.0) / 4.0;
        assert!((metrics.mrr() - expected).abs() < 1e-9);
    }

    #[test]
    fn group_positive_filter_drops_unpaired_coverage() {
        let items = synthetic_items();
        let filtered = indices_with_group_positives(&items, [0, 1, 3]);
        assert_eq!(filtered, vec![0, 1]);
    }
}
