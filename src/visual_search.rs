use crate::db::{self, ImageRecord, ImageSummary};
use crate::embedding::EmbeddingService;
use crate::indexer::{safe_descriptor_image, visual_descriptor, WorkerMessage};
use crate::settings::{ClipExecutionProvider, IndexingSettings};
use crate::{ann, material_texture};
use anyhow::{Context, Result};
use image::DynamicImage;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
const MAX_SIMILARITY_RESULTS: usize = 2_000;
const CANDIDATE_PIPELINE_MIN_RECORDS: usize = 4_000;
const MAX_COMPONENT_CANDIDATES: usize = 3_000;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimilaritySettings {
    pub color_distribution_weight: f32,
    pub texture_weight: f32,
    pub clip_weight: f32,
    pub dominant_color_weight: f32,
    pub strict_color_rejection: bool,
    pub min_color_distribution_match: f32,
    pub max_dominant_color_difference: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimilarityPreset {
    General,
    Material,
    NearDuplicate,
}

impl SimilarityPreset {
    pub const ALL: [Self; 3] = [Self::General, Self::Material, Self::NearDuplicate];

    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General appearance (experimental)",
            Self::Material => "Material / texture",
            Self::NearDuplicate => "Possible duplicates (experimental)",
        }
    }

    pub fn settings(self) -> SimilaritySettings {
        let mut settings = SimilaritySettings::default();
        match self {
            Self::General => {
                settings.color_distribution_weight = 20.0;
                settings.texture_weight = 15.0;
                settings.clip_weight = 60.0;
                settings.strict_color_rejection = false;
            }
            Self::Material => {}
            Self::NearDuplicate => {
                settings.color_distribution_weight = 20.0;
                settings.texture_weight = 70.0;
                settings.clip_weight = 5.0;
                settings.strict_color_rejection = false;
            }
        }
        settings
    }

    pub fn matching(settings: SimilaritySettings) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.settings() == settings)
    }
}

#[derive(Debug)]
pub struct SimilaritySearchResults {
    pub images: Vec<ImageSummary>,
    pub semantic_unavailable: Option<String>,
    pub details: HashMap<PathBuf, crate::visual_query::ScoreBreakdown>,
    pub timings: crate::visual_query::SearchTimings,
}

#[derive(Debug)]
pub enum VisualSearchEvent {
    Status(String),
    Finished(std::result::Result<SimilaritySearchResults, String>),
}

struct VisualSearchSender<'a> {
    tx: &'a Sender<WorkerMessage>,
    request: &'a crate::search_request::SearchRequest,
}

// A cancelled worker may still be inside a decoder or persisted ANN rebuild.
// Serialize visual workers so its replacement cannot race those cache writes.
static VISUAL_SEARCH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl VisualSearchSender<'_> {
    fn status(&self, message: String) {
        if self.request.check().is_ok() {
            let _ = self.tx.send(WorkerMessage::VisualSearch {
                id: self.request.id,
                event: VisualSearchEvent::Status(message),
            });
        }
    }
}

impl Default for SimilaritySettings {
    fn default() -> Self {
        Self {
            color_distribution_weight: 44.0,
            texture_weight: 31.0,
            clip_weight: 20.0,
            dominant_color_weight: 5.0,
            strict_color_rejection: true,
            min_color_distribution_match: 30.0,
            max_dominant_color_difference: 30.0,
        }
    }
}

pub fn spawn_similarity_search(
    db_path: PathBuf,
    query_path: PathBuf,
    settings: SimilaritySettings,
    indexing_settings: IndexingSettings,
    embedding_service: EmbeddingService,
    request: crate::search_request::SearchRequest,
    indexing_active: bool,
    options: crate::visual_query::VisualQueryOptions,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let sender = VisualSearchSender {
            tx: &tx,
            request: &request,
        };
        sender.status("Preparing hybrid visual search…".to_owned());
        let result = similarity_search(
            &db_path,
            &query_path,
            settings,
            indexing_settings,
            &embedding_service,
            &sender,
            indexing_active,
            &options,
        )
        .map_err(|err| format!("Similarity search failed: {err:#}"));
        if request.check().is_ok() {
            let _ = tx.send(WorkerMessage::VisualSearch {
                id: request.id,
                event: VisualSearchEvent::Finished(result),
            });
        }
    });
}

#[derive(Clone, Copy, Debug)]
struct SimilarityMetrics {
    index: usize,
    is_exact: bool,
    hash_similarity: Option<f32>,
    histogram_similarity: Option<f32>,
    clip_similarity: Option<f32>,
    dominant_similarity: f32,
    passes_color_gate: bool,
}

pub(crate) fn component_candidate_limit(record_count: usize) -> usize {
    record_count
        .div_ceil(2)
        .clamp(MAX_SIMILARITY_RESULTS, MAX_COMPONENT_CANDIDATES)
}

fn all_eligible_candidate_indices(metrics: &[SimilarityMetrics]) -> HashSet<usize> {
    metrics
        .iter()
        .filter(|metric| metric.is_exact || metric.passes_color_gate)
        .map(|metric| metric.index)
        .collect()
}

fn add_top_metric_candidates<F>(
    metrics: &[SimilarityMetrics],
    limit: usize,
    candidates: &mut HashSet<usize>,
    score: F,
) -> usize
where
    F: Fn(&SimilarityMetrics) -> Option<f32>,
{
    let mut ranked: Vec<(usize, f32)> = metrics
        .iter()
        .filter(|metric| !metric.is_exact && metric.passes_color_gate)
        .filter_map(|metric| score(metric).map(|value| (metric.index, value)))
        .collect();
    if ranked.len() > limit {
        ranked.select_nth_unstable_by(limit, |a, b| b.1.total_cmp(&a.1));
        ranked.truncate(limit);
    }
    let selected = ranked.len();
    candidates.extend(ranked.into_iter().map(|(index, _)| index));
    selected
}

fn choose_candidate_indices(
    metrics: &[SimilarityMetrics],
    settings: SimilaritySettings,
    clip_available: bool,
) -> HashSet<usize> {
    if metrics.len() <= CANDIDATE_PIPELINE_MIN_RECORDS {
        return all_eligible_candidate_indices(metrics);
    }

    let limit = component_candidate_limit(metrics.len());
    let mut candidates: HashSet<usize> = metrics
        .iter()
        .filter(|metric| metric.is_exact)
        .map(|metric| metric.index)
        .collect();
    let mut available_component = false;

    if settings.color_distribution_weight > 0.0 {
        available_component |=
            add_top_metric_candidates(metrics, limit, &mut candidates, |metric| {
                metric.histogram_similarity
            }) > 0;
    }
    if settings.texture_weight > 0.0 {
        available_component |=
            add_top_metric_candidates(metrics, limit, &mut candidates, |metric| {
                metric.hash_similarity
            }) > 0;
    }
    if settings.clip_weight > 0.0 && clip_available {
        available_component |=
            add_top_metric_candidates(metrics, limit, &mut candidates, |metric| {
                metric.clip_similarity
            }) > 0;
    }
    if settings.dominant_color_weight > 0.0 {
        available_component |=
            add_top_metric_candidates(metrics, limit, &mut candidates, |metric| {
                Some(metric.dominant_similarity)
            }) > 0;
    }

    let eligible_count = metrics
        .iter()
        .filter(|metric| metric.is_exact || metric.passes_color_gate)
        .count();
    let minimum_useful = MAX_SIMILARITY_RESULTS.min(eligible_count);

    // Preserve the old full-scan semantics for zero-weight searches or sparse
    // descriptor sets that could not provide enough real candidates.
    if !available_component || candidates.len() < minimum_useful {
        return all_eligible_candidate_indices(metrics);
    }

    candidates
}

fn similarity_search(
    db_path: &Path,
    query_path: &Path,
    settings: SimilaritySettings,
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    sender: &VisualSearchSender<'_>,
    indexing_active: bool,
    options: &crate::visual_query::VisualQueryOptions,
) -> Result<SimilaritySearchResults> {
    let total_started = std::time::Instant::now();
    let mut timings = crate::visual_query::SearchTimings::default();
    let request = sender.request;
    request.check()?;
    let _search_guard = VISUAL_SEARCH_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("visual search worker lock was poisoned"))?;
    request.check()?;
    let indexing_settings = indexing_settings.sanitized();
    let conn = db::open_search_reader(db_path)?;
    let roots = db::load_roots_from_connection(&conn)?;

    let missing_visual = db::paths_missing_visual_descriptor(&conn)?;
    if !missing_visual.is_empty() {
        sender.status(format!(
            "Searching committed index; {} pending descriptor{} skipped. Rescan to complete the index.",
            missing_visual.len(),
            if missing_visual.len() == 1 { "" } else { "s" }
        ));
    }

    request.check()?;
    let decode_started = std::time::Instant::now();
    let mut temporary_crop = None;
    let (query_image, query_input_path, _) = if options.description.is_some() {
        (DynamicImage::new_rgb8(1, 1), PathBuf::new(), false)
    } else {
        safe_descriptor_image(query_path, &roots, indexing_settings)?
    };
    let (query_image, query_input_path) = if let Some(region) = options.region {
        let cropped = region.crop(&query_image)?;
        let path = std::env::temp_dir().join(format!(
            "wis-query-{}-{}.png",
            std::process::id(),
            request.id
        ));
        temporary_crop = Some(crate::visual_query::TemporaryQueryImage(path.clone()));
        cropped.save(&path)?;
        (cropped, path)
    } else {
        (query_image, query_input_path)
    };
    let (query_dominant, query_hash, query_histogram, query_material_texture) =
        visual_descriptor(&query_image);
    request.check()?;
    timings.decode_ms = decode_started.elapsed().as_secs_f64() * 1000.0;
    let inference_started = std::time::Instant::now();

    let mut semantic_unavailable = None;
    let query_embedding = if let Some(description) = &options.description {
        Some(embedding_service.embed_description(description.clone(), request.clone())?)
    } else if settings.clip_weight > 0.0 {
        match query_clip_embedding(
            embedding_service,
            &query_input_path,
            indexing_settings.clip_threads,
            indexing_settings.clip_execution_provider,
            request,
        ) {
            Ok((embedding, model_reloaded, active_provider, fallback_reason)) => {
                let mut status = if model_reloaded {
                    format!(
                        "CLIP model initialized on {} for this query; future searches will reuse it",
                        active_provider.label()
                    )
                } else {
                    format!(
                        "Reusing loaded CLIP model on {} for query",
                        active_provider.label()
                    )
                };
                if let Some(reason) = fallback_reason {
                    status.push_str(&format!(" — {reason}"));
                }
                sender.status(status);
                Some(embedding)
            }
            Err(err) => {
                request.check()?;
                semantic_unavailable = Some(format!("{err:#}"));
                sender.status(format!(
                    "CLIP unavailable; using texture/color similarity only ({err})"
                ));
                None
            }
        }
    } else {
        None
    };

    request.check()?;
    drop(temporary_crop);
    timings.inference_ms = inference_started.elapsed().as_secs_f64() * 1000.0;
    let retrieval_started = std::time::Instant::now();
    let query_key = normalized_path_key(query_path);
    // Start the read transaction after query inference, so a model download or
    // long inference wait cannot pin the WAL checkpoint for the whole wait.
    let snapshot = db::open_search_snapshot(db_path)?;
    static DESCRIPTORS: std::sync::Mutex<crate::descriptor_cache::DescriptorCache> =
        std::sync::Mutex::new(crate::descriptor_cache::DescriptorCache::new());
    let (catalog, cache_hit, retained_bytes) = DESCRIPTORS
        .lock()
        .map_err(|_| anyhow::anyhow!("descriptor cache lock poisoned"))?
        .load(
            &snapshot,
            db_path,
            crate::descriptor_cache::DESCRIPTOR_CACHE_BUDGET,
        )?;
    timings.descriptor_cache_hit = cache_hit;
    timings.descriptor_cache_bytes = retained_bytes;
    let records: Vec<&ImageRecord> = catalog
        .iter()
        .filter(|record| {
            options
                .eligible_paths
                .as_ref()
                .is_none_or(|paths| paths.contains(&record.path))
        })
        .collect();
    let compute_hash = settings.texture_weight > 0.0;
    let compute_histogram =
        settings.color_distribution_weight > 0.0 || settings.strict_color_rejection;
    let compute_clip = settings.clip_weight > 0.0 && query_embedding.is_some();
    let large_ann_search = !indexing_active
        && options.eligible_paths.is_none()
        && compute_clip
        && records.len() > CANDIDATE_PIPELINE_MIN_RECORDS
        && snapshot.query_row(
            "SELECT count(*) FROM images WHERE embedding IS NOT NULL",
            [],
            |row| row.get::<_, usize>(0),
        )? > CANDIDATE_PIPELINE_MIN_RECORDS;
    if indexing_active {
        sender.status("Searching a committed snapshot while indexing continues; exact semantic retrieval avoids a changing ANN cache".to_owned());
    }
    let ann_scores = if large_ann_search {
        let limit = component_candidate_limit(records.len());
        match ann::search_candidates(db_path, query_embedding.as_deref().unwrap_or(&[]), limit) {
            Ok(scores) if !scores.is_empty() => {
                sender.status(format!(
                    "HNSW semantic retrieval: {} approximate CLIP candidates from {} indexed records",
                    scores.len(), records.len()
                ));
                Some(scores)
            }
            Ok(_) => None,
            Err(err) => {
                sender.status(format!(
                    "HNSW unavailable; falling back to brute-force CLIP candidates ({err:#})"
                ));
                None
            }
        }
    } else {
        None
    };

    let all_rowids: HashSet<usize> = if compute_clip && ann_scores.is_none() {
        records.iter().map(|record| record.rowid).collect()
    } else {
        HashSet::new()
    };
    let fallback_embeddings = if all_rowids.is_empty() {
        HashMap::new()
    } else {
        db::load_embeddings_for_rowids_from_connection(&snapshot, &all_rowids)?
    };
    let mut metrics = Vec::<SimilarityMetrics>::with_capacity(records.len());

    for (index, record) in records.iter().enumerate() {
        request.check()?;
        let is_exact = options.region.is_none()
            && options.description.is_none()
            && normalized_path_key(&record.path) == query_key;
        if is_exact {
            metrics.push(SimilarityMetrics {
                index,
                is_exact: true,
                hash_similarity: Some(1.0),
                histogram_similarity: Some(1.0),
                clip_similarity: Some(1.0),
                dominant_similarity: 1.0,
                passes_color_gate: true,
            });
            continue;
        }

        let hash_similarity = if compute_hash {
            let dhash = record
                .visual_hash
                .map(|hash| perceptual_hash_similarity(query_hash, hash));
            let material = record.material_texture.as_deref().and_then(|descriptor| {
                material_texture::similarity(&query_material_texture, descriptor)
            });
            material_texture::combine_with_dhash(dhash, material)
        } else {
            None
        };
        let histogram_similarity = if compute_histogram {
            record
                .color_histogram
                .as_deref()
                .map(|histogram| histogram_intersection(&query_histogram, histogram))
        } else {
            None
        };
        let dominant_similarity = rgb_similarity(query_dominant, record.dominant);
        let passes_gate = passes_color_gate(histogram_similarity, dominant_similarity, settings);

        // For large indexes an HNSW lookup supplies the initial semantic pool.
        // Small indexes and ANN failures preserve the exact brute-force path.
        let clip_similarity = if passes_gate && compute_clip {
            if let Some(scores) = &ann_scores {
                scores.get(&record.rowid).copied()
            } else {
                query_embedding.as_ref().and_then(|query| {
                    fallback_embeddings
                        .get(&record.rowid)
                        .map(|(embedding, normalized)| {
                            clip_similarity_with_normalized_query(query, embedding, *normalized)
                                .clamp(0.0, 1.0)
                        })
                })
            }
        } else {
            None
        };

        metrics.push(SimilarityMetrics {
            index,
            is_exact: false,
            hash_similarity,
            histogram_similarity,
            clip_similarity,
            dominant_similarity,
            passes_color_gate: passes_gate,
        });
    }

    let candidate_indices = choose_candidate_indices(&metrics, settings, query_embedding.is_some());
    request.check()?;

    // ANN provides approximate semantic candidate generation only. The final
    // hybrid rerank always uses exact CLIP cosine values for every union member,
    // including candidates introduced by color/texture components.
    if compute_clip && ann_scores.is_some() {
        let candidate_rowids: HashSet<usize> = candidate_indices
            .iter()
            .map(|index| records[*index].rowid)
            .collect();
        let exact_embeddings =
            db::load_embeddings_for_rowids_from_connection(&snapshot, &candidate_rowids)?;
        if let Some(query) = &query_embedding {
            for index in &candidate_indices {
                request.check()?;
                if metrics[*index].is_exact {
                    metrics[*index].clip_similarity = Some(1.0);
                    continue;
                }
                metrics[*index].clip_similarity =
                    exact_embeddings
                        .get(&records[*index].rowid)
                        .map(|(embedding, normalized)| {
                            clip_similarity_with_normalized_query(query, embedding, *normalized)
                                .clamp(0.0, 1.0)
                        });
            }
        }
    }

    // Keep the committed snapshot until result-only metadata hydration finishes.
    timings.retrieval_ms = retrieval_started.elapsed().as_secs_f64() * 1000.0;
    let ranking_started = std::time::Instant::now();
    if records.len() > CANDIDATE_PIPELINE_MIN_RECORDS {
        let limit = component_candidate_limit(records.len());
        sender.status(format!(
            "Two-stage similarity: {} indexed records → {} exact hybrid rerank candidates (up to {limit} per enabled component)",
            records.len(),
            candidate_indices.len()
        ));
    }

    let mut scored = Vec::<(bool, ImageRecord)>::with_capacity(candidate_indices.len());
    let mut details = HashMap::new();
    for (index, record) in records.into_iter().enumerate() {
        request.check()?;
        let metric = metrics[index];
        if options.description.is_some() && metric.clip_similarity.is_none() {
            continue;
        }
        if !metric.is_exact && (!candidate_indices.contains(&index) || !metric.passes_color_gate) {
            continue;
        }
        let mut record = record.clone();
        if metric.is_exact || (candidate_indices.contains(&index) && metric.passes_color_gate) {
            details.insert(
                record.path.clone(),
                crate::visual_query::ScoreBreakdown {
                    values: [
                        metric.histogram_similarity,
                        metric.hash_similarity,
                        metric.clip_similarity,
                        Some(metric.dominant_similarity),
                    ],
                    weights: [
                        settings.color_distribution_weight,
                        settings.texture_weight,
                        settings.clip_weight,
                        settings.dominant_color_weight,
                    ],
                    exact_source: metric.is_exact,
                },
            );
        }
        if metric.is_exact {
            record.score = Some(1.0);
            scored.push((true, record));
            continue;
        }
        if !candidate_indices.contains(&index) || !metric.passes_color_gate {
            continue;
        }

        record.score = Some(hybrid_similarity(
            metric.hash_similarity,
            metric.histogram_similarity,
            metric.clip_similarity,
            metric.dominant_similarity,
            settings,
        ));
        scored.push((false, record));
    }

    if scored.len() > MAX_SIMILARITY_RESULTS {
        scored.select_nth_unstable_by(MAX_SIMILARITY_RESULTS, compare_ranked_records);
        scored.truncate(MAX_SIMILARITY_RESULTS);
    }
    scored.sort_by(compare_ranked_records);
    request.check()?;
    let kept: HashSet<_> = scored
        .iter()
        .map(|(_, record)| record.path.clone())
        .collect();
    details.retain(|path, _| kept.contains(path));
    timings.ranking_ms = ranking_started.elapsed().as_secs_f64() * 1000.0;
    let metadata_started = std::time::Instant::now();
    let rowids = scored.iter().map(|(_, record)| record.rowid).collect();
    let mut metadata = db::load_result_metadata(&snapshot, &rowids, request)?;
    for (_, record) in &mut scored {
        let (description, keywords) = metadata
            .remove(&record.rowid)
            .context("ranked image disappeared from its search snapshot")?;
        record.description = description;
        record.keywords = keywords;
    }
    drop(snapshot);
    timings.metadata_ms = metadata_started.elapsed().as_secs_f64() * 1000.0;
    timings.total_ms = total_started.elapsed().as_secs_f64() * 1000.0;

    Ok(SimilaritySearchResults {
        images: scored
            .into_iter()
            .map(|(_, record)| ImageSummary::from(record))
            .collect(),
        semantic_unavailable,
        details,
        timings,
    })
}

fn compare_ranked_records(a: &(bool, ImageRecord), b: &(bool, ImageRecord)) -> std::cmp::Ordering {
    b.0.cmp(&a.0).then_with(|| {
        b.1.score
            .unwrap_or(f32::NEG_INFINITY)
            .total_cmp(&a.1.score.unwrap_or(f32::NEG_INFINITY))
    })
}

fn query_clip_embedding(
    embedding_service: &EmbeddingService,
    query_path: &Path,
    clip_threads: usize,
    requested_provider: ClipExecutionProvider,
    request: &crate::search_request::SearchRequest,
) -> Result<(Vec<f32>, bool, ClipExecutionProvider, Option<String>)> {
    let response = embedding_service.embed_with_provider_cancellable(
        vec![query_path.to_path_buf()],
        1,
        clip_threads,
        requested_provider,
        request.clone(),
    )?;
    let model_reloaded = response.model_reloaded;
    let active_provider = response.active_provider;
    let fallback_reason = response.fallback_reason.clone();
    let embedding = response
        .embeddings
        .into_iter()
        .next()
        .context("CLIP returned no query embedding")?;
    Ok((embedding, model_reloaded, active_provider, fallback_reason))
}

fn passes_color_gate(
    histogram_similarity: Option<f32>,
    dominant_similarity: f32,
    settings: SimilaritySettings,
) -> bool {
    if !settings.strict_color_rejection {
        return true;
    }

    if histogram_similarity
        .is_some_and(|similarity| similarity * 100.0 < settings.min_color_distribution_match)
    {
        return false;
    }

    let dominant_difference = (1.0 - dominant_similarity).clamp(0.0, 1.0) * 100.0;
    dominant_difference <= settings.max_dominant_color_difference
}

fn hybrid_similarity(
    hash_similarity: Option<f32>,
    histogram_similarity: Option<f32>,
    clip_similarity: Option<f32>,
    dominant_similarity: f32,
    settings: SimilaritySettings,
) -> f32 {
    // User-controlled weights are normalized over whichever descriptors are
    // available for a candidate. They do not need to sum to exactly 100%.
    let mut weighted = 0.0f32;
    let mut weight = 0.0f32;

    let dominant_weight = settings.dominant_color_weight.max(0.0);
    if dominant_weight > 0.0 {
        weighted += dominant_weight * dominant_similarity;
        weight += dominant_weight;
    }

    let histogram_weight = settings.color_distribution_weight.max(0.0);
    if let Some(value) = histogram_similarity.filter(|_| histogram_weight > 0.0) {
        weighted += histogram_weight * value;
        weight += histogram_weight;
    }

    let texture_weight = settings.texture_weight.max(0.0);
    if let Some(value) = hash_similarity.filter(|_| texture_weight > 0.0) {
        weighted += texture_weight * value;
        weight += texture_weight;
    }

    let clip_weight = settings.clip_weight.max(0.0);
    if let Some(value) = clip_similarity.filter(|_| clip_weight > 0.0) {
        weighted += clip_weight * value;
        weight += clip_weight;
    }

    if weight <= f32::EPSILON {
        0.0
    } else {
        (weighted / weight).clamp(0.0, 1.0)
    }
}

fn perceptual_hash_similarity(a: u64, b: u64) -> f32 {
    1.0 - ((a ^ b).count_ones() as f32 / 64.0)
}

fn histogram_intersection(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| x.min(y))
        .sum::<f32>()
        .clamp(0.0, 1.0)
}

fn clip_similarity_with_normalized_query(
    query: &[f32],
    candidate: &[f32],
    candidate_normalized: bool,
) -> f32 {
    if query.len() != candidate.len() || query.is_empty() {
        return -1.0;
    }

    let dot = query
        .iter()
        .zip(candidate.iter())
        .map(|(&x, &y)| x * y)
        .sum::<f32>();
    if candidate_normalized {
        return dot;
    }

    let candidate_norm_sq = candidate.iter().map(|value| value * value).sum::<f32>();
    if candidate_norm_sq <= f32::EPSILON {
        -1.0
    } else {
        dot / candidate_norm_sq.sqrt()
    }
}

fn rgb_similarity(a: [u8; 3], b: [u8; 3]) -> f32 {
    let dr = a[0] as f32 - b[0] as f32;
    let dg = a[1] as f32 - b[1] as f32;
    let db = a[2] as f32 - b[2] as f32;
    let distance = (dr * dr + dg * dg + db * db).sqrt();
    (1.0 - distance / (255.0 * 3.0f32.sqrt())).clamp(0.0, 1.0)
}

fn normalized_path_key(path: &Path) -> String {
    let key = path.to_string_lossy().replace('/', "\\");
    if cfg!(windows) {
        key.to_ascii_lowercase()
    } else {
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::COLOR_HISTOGRAM_BINS;
    #[test]
    fn cancelled_visual_query_exits_before_opening_database_or_decoding() {
        let mut session = crate::search_request::SearchSession::default();
        let request = session.start();
        session.cancel();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = VisualSearchSender {
            tx: &tx,
            request: &request,
        };
        sender.status("obsolete progress".to_owned());
        assert!(rx.try_recv().is_err());
        let service = EmbeddingService::new(PathBuf::new());
        let result = similarity_search(
            Path::new("missing-search-test-db"),
            Path::new("missing-search-test-image"),
            SimilaritySettings::default(),
            IndexingSettings::default(),
            &service,
            &sender,
            false,
            &Default::default(),
        );
        assert!(result.unwrap_err().to_string().contains("cancelled"));
    }

    #[test]
    fn normalized_query_similarity_matches_legacy_candidate_fallback() {
        let query = [0.6, 0.8];
        let normalized_candidate = [0.6, 0.8];
        let legacy_candidate = [3.0, 4.0];
        let normalized = clip_similarity_with_normalized_query(&query, &normalized_candidate, true);
        let legacy = clip_similarity_with_normalized_query(&query, &legacy_candidate, false);
        assert!((normalized - 1.0).abs() < 1e-6);
        assert!((legacy - 1.0).abs() < 1e-6);
    }

    #[test]
    fn perceptual_hash_prefers_identical_pattern() {
        let hash = 0xA55A_A55A_0FF0_0FF0u64;
        assert_eq!(perceptual_hash_similarity(hash, hash), 1.0);
        assert_eq!(perceptual_hash_similarity(hash, !hash), 0.0);
    }

    #[test]
    fn histogram_intersection_prefers_same_color_distribution() {
        let mut brown = vec![0.0; COLOR_HISTOGRAM_BINS];
        let mut gray = vec![0.0; COLOR_HISTOGRAM_BINS];
        brown[37] = 0.8;
        brown[38] = 0.2;
        gray[21] = 1.0;

        assert!((histogram_intersection(&brown, &brown) - 1.0).abs() < 1e-6);
        assert_eq!(histogram_intersection(&brown, &gray), 0.0);
    }

    #[test]
    fn chromatic_query_penalizes_achromatic_candidate() {
        let brown = [150, 82, 38];
        let similar_brown = [145, 88, 46];
        let gray = [128, 128, 128];

        let settings = SimilaritySettings::default();
        let colored_dominant = rgb_similarity(brown, similar_brown);
        let gray_dominant = rgb_similarity(brown, gray);

        assert!(passes_color_gate(Some(0.72), colored_dominant, settings));
        assert!(!passes_color_gate(Some(0.12), gray_dominant, settings));

        let colored_score = hybrid_similarity(
            Some(0.75),
            Some(0.72),
            Some(0.70),
            colored_dominant,
            settings,
        );
        let gray_score =
            hybrid_similarity(Some(0.75), Some(0.72), Some(0.70), gray_dominant, settings);

        assert!(colored_score > gray_score);
    }

    #[test]
    fn custom_weights_change_ranking_influence() {
        let mut texture_only = SimilaritySettings::default();
        texture_only.color_distribution_weight = 0.0;
        texture_only.texture_weight = 100.0;
        texture_only.clip_weight = 0.0;
        texture_only.dominant_color_weight = 0.0;
        texture_only.strict_color_rejection = false;

        let texture_score =
            hybrid_similarity(Some(0.92), Some(0.05), Some(0.10), 0.10, texture_only);
        assert!((texture_score - 0.92).abs() < 1e-6);

        let mut clip_only = texture_only;
        clip_only.texture_weight = 0.0;
        clip_only.clip_weight = 100.0;
        let clip_score = hybrid_similarity(Some(0.92), Some(0.05), Some(0.77), 0.10, clip_only);
        assert!((clip_score - 0.77).abs() < 1e-6);
    }

    #[test]
    fn strict_color_gate_rejects_weak_histogram_match() {
        let mut settings = SimilaritySettings::default();
        settings.min_color_distribution_match = 40.0;
        settings.max_dominant_color_difference = 100.0;
        assert!(!passes_color_gate(Some(0.25), 0.95, settings));
        assert!(passes_color_gate(Some(0.60), 0.95, settings));
    }

    #[test]
    fn general_preset_keeps_semantic_matches_despite_different_colors() {
        let general = SimilarityPreset::General.settings();
        assert!(passes_color_gate(Some(0.1), 0.1, general));
        assert!(!passes_color_gate(
            Some(0.1),
            0.1,
            SimilarityPreset::Material.settings()
        ));
        let semantic_match = hybrid_similarity(Some(0.2), Some(0.1), Some(0.95), 0.1, general);
        let color_match = hybrid_similarity(Some(0.2), Some(0.95), Some(0.1), 0.95, general);
        assert!(semantic_match > color_match);
    }

    #[test]
    fn duplicate_preset_prefers_layout_over_semantic_only_matches() {
        let settings = SimilarityPreset::NearDuplicate.settings();
        let layout_match = hybrid_similarity(Some(0.95), Some(0.5), Some(0.2), 0.5, settings);
        let semantic_match = hybrid_similarity(Some(0.2), Some(0.5), Some(0.95), 0.5, settings);
        assert!(layout_match > semantic_match);
        let mut customized = settings;
        customized.strict_color_rejection = true;
        assert_eq!(SimilarityPreset::matching(customized), None);
    }

    #[test]
    fn all_zero_weights_are_safe() {
        let settings = SimilaritySettings {
            color_distribution_weight: 0.0,
            texture_weight: 0.0,
            clip_weight: 0.0,
            dominant_color_weight: 0.0,
            strict_color_rejection: false,
            min_color_distribution_match: 0.0,
            max_dominant_color_difference: 100.0,
        };
        assert_eq!(
            hybrid_similarity(Some(1.0), Some(1.0), Some(1.0), 1.0, settings),
            0.0
        );
    }

    #[test]
    fn clip_cannot_outvote_bad_color_and_texture_match() {
        let settings = SimilaritySettings::default();
        let good = hybrid_similarity(Some(0.90), Some(0.88), Some(0.62), 0.90, settings);
        let semantically_close_but_wrong =
            hybrid_similarity(Some(0.35), Some(0.12), Some(0.95), 0.55, settings);

        assert!(good > semantically_close_but_wrong);
    }

    fn synthetic_metric(
        index: usize,
        hash: f32,
        histogram: f32,
        clip: f32,
        dominant: f32,
        passes_color_gate: bool,
    ) -> SimilarityMetrics {
        SimilarityMetrics {
            index,
            is_exact: false,
            hash_similarity: Some(hash),
            histogram_similarity: Some(histogram),
            clip_similarity: Some(clip),
            dominant_similarity: dominant,
            passes_color_gate,
        }
    }

    #[test]
    fn small_library_candidate_stage_preserves_bruteforce_eligibility() {
        let metrics: Vec<_> = (0..128)
            .map(|index| synthetic_metric(index, 0.5, 0.5, 0.5, 0.5, index % 5 != 0))
            .collect();
        let selected = choose_candidate_indices(&metrics, SimilaritySettings::default(), true);
        let expected: HashSet<_> = metrics
            .iter()
            .filter(|metric| metric.passes_color_gate)
            .map(|metric| metric.index)
            .collect();
        assert_eq!(selected, expected);
    }

    #[test]
    fn large_texture_only_search_selects_best_texture_candidates() {
        let mut settings = SimilaritySettings::default();
        settings.color_distribution_weight = 0.0;
        settings.texture_weight = 100.0;
        settings.clip_weight = 0.0;
        settings.dominant_color_weight = 0.0;
        settings.strict_color_rejection = false;

        let metrics: Vec<_> = (0..5_000)
            .map(|index| {
                synthetic_metric(
                    index,
                    index as f32 / 5_000.0,
                    1.0 - index as f32 / 5_000.0,
                    0.1,
                    0.1,
                    true,
                )
            })
            .collect();
        let selected = choose_candidate_indices(&metrics, settings, true);

        assert!(selected.contains(&4_999));
        assert!(!selected.contains(&0));
        assert_eq!(selected.len(), component_candidate_limit(metrics.len()));
    }

    #[test]
    fn strict_gate_excludes_even_a_top_component_candidate() {
        let mut settings = SimilaritySettings::default();
        settings.color_distribution_weight = 0.0;
        settings.texture_weight = 100.0;
        settings.clip_weight = 0.0;
        settings.dominant_color_weight = 0.0;

        let mut metrics: Vec<_> = (0..5_000)
            .map(|index| synthetic_metric(index, 0.5, 0.5, 0.5, 0.5, true))
            .collect();
        metrics[4_999].hash_similarity = Some(1.0);
        metrics[4_999].passes_color_gate = false;

        let selected = choose_candidate_indices(&metrics, settings, true);
        assert!(!selected.contains(&4_999));
    }

    #[test]
    fn exact_query_is_always_in_large_candidate_union() {
        let settings = SimilaritySettings {
            color_distribution_weight: 0.0,
            texture_weight: 100.0,
            clip_weight: 0.0,
            dominant_color_weight: 0.0,
            strict_color_rejection: false,
            min_color_distribution_match: 0.0,
            max_dominant_color_difference: 100.0,
        };
        let mut metrics: Vec<_> = (0..5_000)
            .map(|index| synthetic_metric(index, index as f32 / 5_000.0, 0.0, 0.0, 0.0, true))
            .collect();
        metrics[0].is_exact = true;
        metrics[0].hash_similarity = Some(0.0);

        let selected = choose_candidate_indices(&metrics, settings, false);
        assert!(selected.contains(&0));
    }

    #[test]
    fn zero_weight_large_search_falls_back_to_full_eligible_scan() {
        let settings = SimilaritySettings {
            color_distribution_weight: 0.0,
            texture_weight: 0.0,
            clip_weight: 0.0,
            dominant_color_weight: 0.0,
            strict_color_rejection: false,
            min_color_distribution_match: 0.0,
            max_dominant_color_difference: 100.0,
        };
        let metrics: Vec<_> = (0..5_000)
            .map(|index| synthetic_metric(index, 0.0, 0.0, 0.0, 0.0, true))
            .collect();
        let selected = choose_candidate_indices(&metrics, settings, true);
        assert_eq!(selected.len(), metrics.len());
    }
}
