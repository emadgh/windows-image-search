use crate::db;
use crate::embedding::EmbeddingService;
use crate::thumbnail_cache;
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

const DEFAULT_SAMPLE_COUNT: usize = 64;
const EMBEDDING_BATCH_SIZE: usize = 16;

pub fn default_sample_count() -> usize {
    DEFAULT_SAMPLE_COUNT
}

fn benchmark_clip_threads() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4)
        .saturating_sub(1)
        .max(1)
        .min(4)
}

pub fn benchmark(db_path: &Path, model_cache: &Path, requested_samples: usize) -> Result<String> {
    let available: Vec<PathBuf> = db::load_image_summaries(db_path)?
        .into_iter()
        .map(|record| record.path)
        .filter(|path| path.is_file())
        .collect();
    if available.len() < 3 {
        bail!("CLIP preview benchmark needs at least 3 indexed image files that still exist");
    }

    let requested_samples = requested_samples.max(3).min(available.len());
    let selected = sample_evenly(&available, requested_samples);
    let cache_dir = thumbnail_cache::cache_dir_for_db(db_path);
    let mut source_paths = Vec::with_capacity(selected.len());
    let mut preview_paths = Vec::with_capacity(selected.len());

    for source in selected {
        let _ = thumbnail_cache::load_or_build(&cache_dir, &source);
        let preview = thumbnail_cache::cache_path(&cache_dir, &source);
        if preview.is_file() {
            source_paths.push(source);
            preview_paths.push(preview);
        }
    }

    if source_paths.len() < 3 {
        bail!(
            "only {} usable cached previews were available; at least 3 are required",
            source_paths.len()
        );
    }

    let clip_threads = benchmark_clip_threads();
    let service = EmbeddingService::new(model_cache.to_path_buf());

    let original_started = Instant::now();
    let original_response = service
        .embed(
            source_paths.clone(),
            EMBEDDING_BATCH_SIZE,
            clip_threads,
        )
        .context("embedding original images for CLIP preview benchmark")?;
    let original_elapsed = original_started.elapsed();

    let preview_started = Instant::now();
    let preview_response = service
        .embed(
            preview_paths.clone(),
            EMBEDDING_BATCH_SIZE,
            clip_threads,
        )
        .context("embedding cached previews for CLIP preview benchmark")?;
    let preview_elapsed = preview_started.elapsed();

    if original_response.embeddings.len() != source_paths.len()
        || preview_response.embeddings.len() != source_paths.len()
    {
        bail!(
            "CLIP returned inconsistent benchmark dimensions: originals={}, previews={}, samples={}",
            original_response.embeddings.len(),
            preview_response.embeddings.len(),
            source_paths.len()
        );
    }

    let original_embeddings = original_response.embeddings;
    let preview_embeddings = preview_response.embeddings;
    let mut pair_cosines: Vec<f32> = original_embeddings
        .iter()
        .zip(preview_embeddings.iter())
        .map(|(original, preview)| dot_similarity(original, preview))
        .collect();
    pair_cosines.sort_by(|a, b| a.total_cmp(b));

    let recall_10 = mean_recall_at(&original_embeddings, &preview_embeddings, 10);
    let recall_25 = mean_recall_at(&original_embeddings, &preview_embeddings, 25);
    let top1_agreement = mean_top1_agreement(&original_embeddings, &preview_embeddings);
    let pair_mean = mean(&pair_cosines);
    let pair_p05 = percentile(&pair_cosines, 0.05);
    let pair_min = pair_cosines.first().copied().unwrap_or(0.0);

    let original_ms = original_elapsed.as_secs_f64() * 1_000.0;
    let preview_ms = preview_elapsed.as_secs_f64() * 1_000.0;
    let speedup = if preview_ms > f64::EPSILON {
        original_ms / preview_ms
    } else {
        0.0
    };

    Ok(format!(
        "Windows Image Search CLIP preview benchmark\n\
version={}\n\
samples_requested={}\n\
samples_used={}\n\
preview_edge_px={}\n\
clip_threads={}\n\
embedding_batch_size={}\n\
original_embedding_ms={:.3}\n\
preview_embedding_ms={:.3}\n\
preview_speedup_x={:.3}\n\
original_model_reloaded={}\n\
preview_model_reloaded={}\n\
pair_cosine_mean={:.6}\n\
pair_cosine_p05={:.6}\n\
pair_cosine_min={:.6}\n\
retrieval_recall_at_10={:.6}\n\
retrieval_recall_at_25={:.6}\n\
top1_agreement={:.6}\n\
production_clip_input=original\n\
notes=Recall compares original-query/original-corpus baseline against original-query/preview-corpus retrieval. Production behavior is intentionally unchanged until representative material-image results justify switching.\n",
        env!("CARGO_PKG_VERSION"),
        requested_samples,
        source_paths.len(),
        thumbnail_cache::CACHE_EDGE,
        clip_threads,
        EMBEDDING_BATCH_SIZE,
        original_ms,
        preview_ms,
        speedup,
        original_response.model_reloaded,
        preview_response.model_reloaded,
        pair_mean,
        pair_p05,
        pair_min,
        recall_10,
        recall_25,
        top1_agreement,
    ))
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
        .map(|index| {
            let source_index = index * last / (count - 1);
            paths[source_index].clone()
        })
        .collect()
}

fn dot_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return -1.0;
    }
    a.iter().zip(b.iter()).map(|(&x, &y)| x * y).sum::<f32>()
}

fn top_indices(query: &[f32], corpus: &[Vec<f32>], exclude: usize, k: usize) -> Vec<usize> {
    let mut ranked: Vec<(usize, f32)> = corpus
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != exclude)
        .map(|(index, embedding)| (index, dot_similarity(query, embedding)))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked.truncate(k.min(ranked.len()));
    ranked.into_iter().map(|(index, _)| index).collect()
}

fn mean_recall_at(original: &[Vec<f32>], preview: &[Vec<f32>], k: usize) -> f32 {
    if original.len() != preview.len() || original.len() < 2 {
        return 0.0;
    }

    let effective_k = k.min(original.len() - 1).max(1);
    let total = original
        .iter()
        .enumerate()
        .map(|(index, query)| {
            let baseline = top_indices(query, original, index, effective_k);
            let candidate = top_indices(query, preview, index, effective_k);
            let candidate: HashSet<usize> = candidate.into_iter().collect();
            baseline
                .into_iter()
                .filter(|item| candidate.contains(item))
                .count() as f32
                / effective_k as f32
        })
        .sum::<f32>();
    total / original.len() as f32
}

fn mean_top1_agreement(original: &[Vec<f32>], preview: &[Vec<f32>]) -> f32 {
    if original.len() != preview.len() || original.len() < 2 {
        return 0.0;
    }

    let matches = original
        .iter()
        .enumerate()
        .filter(|(index, query)| {
            top_indices(query, original, *index, 1) == top_indices(query, preview, *index, 1)
        })
        .count();
    matches as f32 / original.len() as f32
}

fn mean(values: &[f32]) -> f32 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}

fn percentile(sorted: &[f32], fraction: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f32 * fraction.clamp(0.0, 1.0)).round() as usize;
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_sampling_includes_both_ends() {
        let paths: Vec<PathBuf> = (0..10)
            .map(|index| PathBuf::from(format!("{index}.jpg")))
            .collect();
        let sampled = sample_evenly(&paths, 4);
        assert_eq!(sampled.first(), paths.first());
        assert_eq!(sampled.last(), paths.last());
        assert_eq!(sampled.len(), 4);
    }

    #[test]
    fn identical_preview_corpus_has_perfect_recall() {
        let corpus = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.9, 0.1, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
        ];
        assert!((mean_recall_at(&corpus, &corpus, 2) - 1.0).abs() < 1e-6);
        assert!((mean_top1_agreement(&corpus, &corpus) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn percentile_handles_sorted_values() {
        let values = [0.1, 0.2, 0.3, 0.4, 0.5];
        assert_eq!(percentile(&values, 0.0), 0.1);
        assert_eq!(percentile(&values, 1.0), 0.5);
    }
}
