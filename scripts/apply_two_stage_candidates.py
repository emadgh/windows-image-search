from pathlib import Path

path = Path("src/indexer.rs")
text = path.read_text(encoding="utf-8")


def replace_once(old: str, new: str) -> None:
    global text
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"expected one match, found {count}: {old[:120]!r}")
    text = text.replace(old, new, 1)


replace_once(
    "const COLOR_HISTOGRAM_BINS: usize = 64;\nconst MAX_SIMILARITY_RESULTS: usize = 2_000;",
    "const COLOR_HISTOGRAM_BINS: usize = 64;\nconst MAX_SIMILARITY_RESULTS: usize = 2_000;\nconst CANDIDATE_PIPELINE_MIN_RECORDS: usize = 4_000;\nconst MAX_COMPONENT_CANDIDATES: usize = 3_000;",
)

helpers = r'''
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

fn component_candidate_limit(record_count: usize) -> usize {
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
        available_component |= add_top_metric_candidates(
            metrics,
            limit,
            &mut candidates,
            |metric| metric.histogram_similarity,
        ) > 0;
    }
    if settings.texture_weight > 0.0 {
        available_component |= add_top_metric_candidates(
            metrics,
            limit,
            &mut candidates,
            |metric| metric.hash_similarity,
        ) > 0;
    }
    if settings.clip_weight > 0.0 && clip_available {
        available_component |= add_top_metric_candidates(
            metrics,
            limit,
            &mut candidates,
            |metric| metric.clip_similarity,
        ) > 0;
    }
    if settings.dominant_color_weight > 0.0 {
        available_component |= add_top_metric_candidates(
            metrics,
            limit,
            &mut candidates,
            |metric| Some(metric.dominant_similarity),
        ) > 0;
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

'''

replace_once(
    "fn similarity_search(\n    db_path: &Path,",
    helpers + "fn similarity_search(\n    db_path: &Path,",
)

old_scoring = r'''    let query_key = normalized_path_key(query_path);
    let records = db::load_images(db_path)?;
    let mut scored = Vec::<(bool, ImageRecord)>::with_capacity(records.len());

    for mut record in records {
        // Normalize the path exactly once. It is carried as a bool into the
        // ranking stage instead of reallocating strings inside sort comparisons.
        let is_exact = normalized_path_key(&record.path) == query_key;
        if is_exact {
            record.score = Some(1.0);
            scored.push((true, record));
            continue;
        }

        let hash_similarity = record
            .visual_hash
            .map(|hash| perceptual_hash_similarity(query_hash, hash));
        let histogram_similarity = record
            .color_histogram
            .as_deref()
            .map(|histogram| histogram_intersection(&query_histogram, histogram));
        let dominant_similarity = rgb_similarity(query_dominant, record.dominant);

        // Cheap color rejection happens before touching the CLIP vector. This
        // avoids hundreds of floating-point operations for obvious mismatches.
        if !passes_color_gate(histogram_similarity, dominant_similarity, settings) {
            continue;
        }

        let clip_similarity = query_embedding.as_ref().and_then(|query| {
            record.embedding.as_deref().map(|embedding| {
                clip_similarity_with_normalized_query(query, embedding, record.embedding_normalized)
                    .clamp(0.0, 1.0)
            })
        });

        record.score = Some(hybrid_similarity(
            hash_similarity,
            histogram_similarity,
            clip_similarity,
            dominant_similarity,
            settings,
        ));
        scored.push((false, record));
    }
'''

new_scoring = r'''    let query_key = normalized_path_key(query_path);
    let records = db::load_images(db_path)?;
    let compute_hash = settings.texture_weight > 0.0;
    let compute_histogram = settings.color_distribution_weight > 0.0 || settings.strict_color_rejection;
    let compute_clip = settings.clip_weight > 0.0 && query_embedding.is_some();
    let mut metrics = Vec::<SimilarityMetrics>::with_capacity(records.len());

    for (index, record) in records.iter().enumerate() {
        let is_exact = normalized_path_key(&record.path) == query_key;
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
            record
                .visual_hash
                .map(|hash| perceptual_hash_similarity(query_hash, hash))
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

        // CLIP is the expensive brute-force component. Never touch its vector
        // when its slider is zero, and keep the strict color gate in front of it.
        let clip_similarity = if passes_gate && compute_clip {
            query_embedding.as_ref().and_then(|query| {
                record.embedding.as_deref().map(|embedding| {
                    clip_similarity_with_normalized_query(
                        query,
                        embedding,
                        record.embedding_normalized,
                    )
                    .clamp(0.0, 1.0)
                })
            })
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
    if records.len() > CANDIDATE_PIPELINE_MIN_RECORDS {
        let limit = component_candidate_limit(records.len());
        let _ = tx.send(WorkerMessage::Status(format!(
            "Two-stage similarity: {} indexed records → {} hybrid candidates (up to {limit} per enabled component)",
            records.len(),
            candidate_indices.len()
        )));
    }

    let mut scored = Vec::<(bool, ImageRecord)>::with_capacity(candidate_indices.len());
    for (index, mut record) in records.into_iter().enumerate() {
        let metric = metrics[index];
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
'''

replace_once(old_scoring, new_scoring)

tests = r'''
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

'''

replace_once(
    "    #[test]\n    fn committed_batch_survives_later_rollback() {",
    tests + "    #[test]\n    fn committed_batch_survives_later_rollback() {",
)

path.write_text(text, encoding="utf-8")
