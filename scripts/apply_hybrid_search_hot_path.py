from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


# -----------------------------------------------------------------------------
# embedding.rs: normalize model output once at the service boundary. Query
# vectors and newly persisted image vectors therefore share unit L2 norm.
# -----------------------------------------------------------------------------
path = Path("src/embedding.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''    let embeddings = model
        .embed(paths, Some(batch_size.max(1)))
        .context("embedding images with persistent CLIP model")?;

    Ok(EmbeddingResponse {
        embeddings,
''',
    '''    let mut embeddings = model
        .embed(paths, Some(batch_size.max(1)))
        .context("embedding images with persistent CLIP model")?;
    for embedding in &mut embeddings {
        normalize_embedding(embedding);
    }

    Ok(EmbeddingResponse {
        embeddings,
''',
    "normalize service embeddings",
)
text = replace_once(
    text,
    '''#[cfg(test)]
mod tests {
''',
    '''fn normalize_embedding(values: &mut [f32]) {
    let norm_sq = values.iter().map(|value| value * value).sum::<f32>();
    if norm_sq <= f32::EPSILON {
        return;
    }
    let inverse = norm_sq.sqrt().recip();
    for value in values {
        *value *= inverse;
    }
}

#[cfg(test)]
mod tests {
''',
    "embedding normalization helper",
)
text = replace_once(
    text,
    '''    #[test]
    fn model_is_reused_until_thread_setting_changes() {
''',
    '''    #[test]
    fn embedding_normalization_produces_unit_vectors() {
        let mut values = vec![3.0, 4.0];
        normalize_embedding(&mut values);
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
    }

    #[test]
    fn model_is_reused_until_thread_setting_changes() {
''',
    "embedding normalization test",
)
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# db.rs: persist whether a stored embedding is normalized. Existing databases
# migrate with false, so old vectors keep correct cosine fallback semantics.
# -----------------------------------------------------------------------------
path = Path("src/db.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''    pub embedding: Option<Vec<f32>>,
    pub score: Option<f32>,
''',
    '''    pub embedding: Option<Vec<f32>>,
    pub embedding_normalized: bool,
    pub score: Option<f32>,
''',
    "ImageRecord normalization flag",
)
text = replace_once(
    text,
    '''            embedding BLOB,
            embedding_dim INTEGER,
            last_seen_scan INTEGER NOT NULL DEFAULT 0
''',
    '''            embedding BLOB,
            embedding_dim INTEGER,
            embedding_normalized INTEGER NOT NULL DEFAULT 0,
            last_seen_scan INTEGER NOT NULL DEFAULT 0
''',
    "fresh normalized flag schema",
)
text = replace_once(
    text,
    '''    ensure_column(
        &conn,
        "images",
        "last_seen_scan",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
''',
    '''    ensure_column(
        &conn,
        "images",
        "embedding_normalized",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &conn,
        "images",
        "last_seen_scan",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
''',
    "normalized flag migration",
)
text = replace_once(
    text,
    '''            embedding = NULL,
            embedding_dim = NULL
''',
    '''            embedding = NULL,
            embedding_dim = NULL,
            embedding_normalized = 0
''',
    "reset normalization flag on image change",
)
text = replace_once(
    text,
    '''pub fn set_embedding(conn: &Connection, path: &Path, embedding: &[f32]) -> Result<()> {
    conn.execute(
        "UPDATE images SET embedding = ?2, embedding_dim = ?3 WHERE path = ?1",
        params![
            path.to_string_lossy().to_string(),
            encode_f32_vec(embedding),
            embedding.len() as i64
        ],
    )?;
    Ok(())
}
''',
    '''pub fn set_embedding(conn: &Connection, path: &Path, embedding: &[f32]) -> Result<()> {
    let normalized = normalized_f32_vec(embedding);
    conn.execute(
        "UPDATE images SET embedding = ?2, embedding_dim = ?3, embedding_normalized = 1 WHERE path = ?1",
        params![
            path.to_string_lossy().to_string(),
            encode_f32_vec(&normalized),
            normalized.len() as i64
        ],
    )?;
    Ok(())
}
''',
    "normalize stored embeddings",
)
text = replace_once(
    text,
    '''               visual_hash, color_histogram, color_histogram_dim,
               embedding, embedding_dim
''',
    '''               visual_hash, color_histogram, color_histogram_dim,
               embedding, embedding_dim, embedding_normalized
''',
    "load normalization flag",
)
text = replace_once(
    text,
    '''        let visual_hash_signed: Option<i64> = row.get(13)?;
        Ok(ImageRecord {
''',
    '''        let visual_hash_signed: Option<i64> = row.get(13)?;
        let embedding_normalized = row.get::<_, bool>(18)?;
        Ok(ImageRecord {
''',
    "decode normalization flag",
)
text = replace_once(
    text,
    '''            color_histogram,
            embedding,
            score: None,
''',
    '''            color_histogram,
            embedding,
            embedding_normalized,
            score: None,
''',
    "store normalization flag in record",
)
text = replace_once(
    text,
    '''fn encode_f32_vec(values: &[f32]) -> Vec<u8> {
''',
    '''fn normalized_f32_vec(values: &[f32]) -> Vec<f32> {
    let norm_sq = values.iter().map(|value| value * value).sum::<f32>();
    if norm_sq <= f32::EPSILON {
        return values.to_vec();
    }
    let inverse = norm_sq.sqrt().recip();
    values.iter().map(|value| value * inverse).collect()
}

fn encode_f32_vec(values: &[f32]) -> Vec<u8> {
''',
    "db normalization helper",
)
# The existing summary regression already calls set_embedding; strengthen it.
text = replace_once(
    text,
    '''        assert!(full[0].embedding.is_some());
        assert!(full[0].color_histogram.is_some());
''',
    '''        assert!(full[0].embedding.is_some());
        assert!(full[0].embedding_normalized);
        assert!(full[0].color_histogram.is_some());
''',
    "normalized persistence regression assertion",
)
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# indexer.rs: gate cheaply before CLIP work, normalize paths only once per
# record, and partially select a bounded top result set before sorting.
# -----------------------------------------------------------------------------
path = Path("src/indexer.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "const COLOR_HISTOGRAM_BINS: usize = 64;\n",
    "const COLOR_HISTOGRAM_BINS: usize = 64;\nconst MAX_SIMILARITY_RESULTS: usize = 2_000;\n",
    "top-k constant",
)

start = text.index("fn similarity_search(")
end = text.index("\nfn query_clip_embedding", start)
new_similarity = '''fn similarity_search(
    db_path: &Path,
    query_path: &Path,
    settings: SimilaritySettings,
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    tx: &Sender<WorkerMessage>,
) -> Result<Vec<ImageSummary>> {
    let indexing_settings = indexing_settings.sanitized();
    let conn = db::open(db_path)?;

    let missing_visual = db::paths_missing_visual_descriptor(&conn)?;
    if !missing_visual.is_empty() {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Upgrading texture/color index: {} image{}…",
            missing_visual.len(),
            if missing_visual.len() == 1 { "" } else { "s" }
        )));
        build_visual_descriptors(&conn, &missing_visual, indexing_settings.decode_workers, tx)?;
    }

    let query_image = decode_image(query_path)?;
    let (query_dominant, query_hash, query_histogram) = visual_descriptor(&query_image);

    let query_embedding = match query_clip_embedding(
        embedding_service,
        query_path,
        indexing_settings.clip_threads,
    ) {
        Ok((embedding, model_reloaded)) => {
            let _ = tx.send(WorkerMessage::Status(if model_reloaded {
                "CLIP model initialized for this query; future searches will reuse it".to_owned()
            } else {
                "Reusing loaded CLIP model for query".to_owned()
            }));
            Some(embedding)
        }
        Err(err) => {
            let _ = tx.send(WorkerMessage::Status(format!(
                "CLIP unavailable; using texture/color similarity only ({err})"
            )));
            None
        }
    };

    let query_key = normalized_path_key(query_path);
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
                clip_similarity_with_normalized_query(
                    query,
                    embedding,
                    record.embedding_normalized,
                )
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

    if scored.len() > MAX_SIMILARITY_RESULTS {
        scored.select_nth_unstable_by(MAX_SIMILARITY_RESULTS, compare_ranked_records);
        scored.truncate(MAX_SIMILARITY_RESULTS);
    }
    scored.sort_by(compare_ranked_records);

    Ok(scored
        .into_iter()
        .map(|(_, record)| ImageSummary::from(record))
        .collect())
}

fn compare_ranked_records(
    a: &(bool, ImageRecord),
    b: &(bool, ImageRecord),
) -> std::cmp::Ordering {
    b.0.cmp(&a.0).then_with(|| {
        b.1.score
            .unwrap_or(f32::NEG_INFINITY)
            .total_cmp(&a.1.score.unwrap_or(f32::NEG_INFINITY))
    })
}
'''
text = text[:start] + new_similarity + text[end:]

# Replace generic cosine with a hot path that assumes the query was normalized
# once by EmbeddingService. Legacy candidate vectors still use a correct fallback.
old_cos_start = text.index("fn cosine_similarity(")
old_cos_end = text.index("\nfn rgb_similarity", old_cos_start)
new_cos = '''fn clip_similarity_with_normalized_query(
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
'''
text = text[:old_cos_start] + new_cos + text[old_cos_end:]

# Add a regression test for normalized/new versus legacy vector scoring.
text = replace_once(
    text,
    '''    #[test]
    fn perceptual_hash_prefers_identical_pattern() {
''',
    '''    #[test]
    fn normalized_query_similarity_matches_legacy_candidate_fallback() {
        let query = [0.6, 0.8];
        let normalized_candidate = [0.6, 0.8];
        let legacy_candidate = [3.0, 4.0];
        let normalized =
            clip_similarity_with_normalized_query(&query, &normalized_candidate, true);
        let legacy = clip_similarity_with_normalized_query(&query, &legacy_candidate, false);
        assert!((normalized - 1.0).abs() < 1e-6);
        assert!((legacy - 1.0).abs() < 1e-6);
    }

    #[test]
    fn perceptual_hash_prefers_identical_pattern() {
''',
    "normalized cosine regression test",
)
path.write_text(text, encoding="utf-8")

print("Hybrid search hot-path patch applied")
