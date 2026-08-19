from pathlib import Path


def patch(path: str, replacements):
    file = Path(path)
    text = file.read_text(encoding="utf-8")
    for old, new in replacements:
        if new in text:
            continue
        count = text.count(old)
        if count != 1:
            raise SystemExit(f"{path}: expected one match, found {count}: {old[:100]!r}")
        text = text.replace(old, new, 1)
    file.write_text(text, encoding="utf-8")


patch("src/main.rs", [
    ("mod db;\n", "mod ann;\nmod db;\n"),
])

# DB: expose light search records plus selective embedding reads for ANN/hybrid rerank.
db = Path("src/db.rs")
text = db.read_text(encoding="utf-8")
text = text.replace(
    "use std::collections::HashMap;\nuse std::path::{Path, PathBuf};",
    "use std::collections::{HashMap, HashSet};\nuse std::hash::{Hash, Hasher};\nuse std::path::{Path, PathBuf};",
    1,
)
text = text.replace(
    "pub struct ImageRecord {\n    pub path: PathBuf,",
    "pub struct ImageRecord {\n    pub rowid: usize,\n    pub path: PathBuf,",
    1,
)
text = text.replace(
    "               embedding, embedding_dim, embedding_normalized\n        FROM images",
    "               embedding, embedding_dim, embedding_normalized, rowid\n        FROM images",
    1,
)
text = text.replace(
    "        Ok(ImageRecord {\n            path: PathBuf::from(row.get::<_, String>(0)?),",
    "        Ok(ImageRecord {\n            rowid: row.get::<_, i64>(19)?.max(0) as usize,\n            path: PathBuf::from(row.get::<_, String>(0)?),",
    1,
)

insert_marker = "\nfn normalized_f32_vec(values: &[f32]) -> Vec<f32> {"
if "pub fn load_search_images" not in text:
    helpers = r'''

#[derive(Clone, Debug)]
pub struct AnnEmbedding {
    pub rowid: usize,
    pub embedding: Vec<f32>,
}

pub fn load_search_images(db_path: &Path) -> Result<Vec<ImageRecord>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT path, root, file_name, extension, size, modified, width, height,
               description, keywords, dominant_r, dominant_g, dominant_b,
               visual_hash, color_histogram, color_histogram_dim,
               embedding_normalized, rowid
        FROM images
        ORDER BY file_name COLLATE NOCASE
        "#,
    )?;

    let rows = stmt.query_map([], |row| {
        let histogram_blob: Option<Vec<u8>> = row.get(14)?;
        let histogram_dim: Option<i64> = row.get(15)?;
        let color_histogram = histogram_blob
            .and_then(|bytes| decode_f32_vec(&bytes, histogram_dim.unwrap_or(0) as usize));
        let visual_hash_signed: Option<i64> = row.get(13)?;
        Ok(ImageRecord {
            rowid: row.get::<_, i64>(17)?.max(0) as usize,
            path: PathBuf::from(row.get::<_, String>(0)?),
            root: PathBuf::from(row.get::<_, String>(1)?),
            file_name: row.get(2)?,
            extension: row.get(3)?,
            size: row.get::<_, i64>(4)?.max(0) as u64,
            modified: row.get(5)?,
            width: row.get::<_, i64>(6)?.max(0) as u32,
            height: row.get::<_, i64>(7)?.max(0) as u32,
            description: row.get(8)?,
            keywords: row.get(9)?,
            dominant: [
                row.get::<_, i64>(10)?.clamp(0, 255) as u8,
                row.get::<_, i64>(11)?.clamp(0, 255) as u8,
                row.get::<_, i64>(12)?.clamp(0, 255) as u8,
            ],
            visual_hash: visual_hash_signed.map(|value| value as u64),
            color_histogram,
            embedding: None,
            embedding_normalized: row.get::<_, bool>(16)?,
            score: None,
        })
    })?;
    Ok(rows.filter_map(|row| row.ok()).collect())
}

pub fn load_embeddings_for_rowids(
    db_path: &Path,
    rowids: &HashSet<usize>,
) -> Result<HashMap<usize, (Vec<f32>, bool)>> {
    if rowids.is_empty() {
        return Ok(HashMap::new());
    }
    let conn = open(db_path)?;
    let mut output = HashMap::with_capacity(rowids.len());
    let mut ids: Vec<usize> = rowids.iter().copied().collect();
    ids.sort_unstable();

    for chunk in ids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT rowid, embedding, embedding_dim, embedding_normalized FROM images WHERE rowid IN ({placeholders}) AND embedding IS NOT NULL"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params = chunk.iter().map(|id| *id as i64);
        let rows = stmt.query_map(params_from_iter(params), |row| {
            let rowid = row.get::<_, i64>(0)?.max(0) as usize;
            let bytes: Vec<u8> = row.get(1)?;
            let dim = row.get::<_, i64>(2)?.max(0) as usize;
            let normalized = row.get::<_, bool>(3)?;
            Ok((rowid, bytes, dim, normalized))
        })?;
        for row in rows {
            let (rowid, bytes, dim, normalized) = row?;
            if let Some(values) = decode_f32_vec(&bytes, dim) {
                output.insert(rowid, (values, normalized));
            }
        }
    }
    Ok(output)
}

pub fn load_ann_embeddings(db_path: &Path) -> Result<Vec<AnnEmbedding>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT rowid, embedding, embedding_dim, embedding_normalized FROM images WHERE embedding IS NOT NULL ORDER BY rowid",
    )?;
    let rows = stmt.query_map([], |row| {
        let rowid = row.get::<_, i64>(0)?.max(0) as usize;
        let bytes: Vec<u8> = row.get(1)?;
        let dim = row.get::<_, i64>(2)?.max(0) as usize;
        let normalized = row.get::<_, bool>(3)?;
        Ok((rowid, bytes, dim, normalized))
    })?;

    let mut output = Vec::new();
    for row in rows {
        let (rowid, bytes, dim, normalized) = row?;
        let Some(values) = decode_f32_vec(&bytes, dim) else {
            continue;
        };
        output.push(AnnEmbedding {
            rowid,
            embedding: if normalized { values } else { normalized_f32_vec(&values) },
        });
    }
    Ok(output)
}

pub fn ann_index_signature(db_path: &Path) -> Result<u64> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT rowid, path, size, modified, COALESCE(embedding_dim, 0), embedding_normalized FROM images WHERE embedding IS NOT NULL ORDER BY rowid",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, bool>(5)?,
        ))
    })?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    1_u32.hash(&mut hasher); // bump when embedding/index semantics change.
    for row in rows {
        let (rowid, path, size, modified, dim, normalized) = row?;
        rowid.hash(&mut hasher);
        path.hash(&mut hasher);
        size.hash(&mut hasher);
        modified.hash(&mut hasher);
        dim.hash(&mut hasher);
        normalized.hash(&mut hasher);
    }
    Ok(hasher.finish())
}
'''
    text = text.replace(insert_marker, helpers + insert_marker, 1)

db.write_text(text, encoding="utf-8")

# Indexer: large semantic pool comes from persisted HNSW; exact CLIP is loaded only for final candidates.
idx = Path("src/indexer.rs")
text = idx.read_text(encoding="utf-8")
text = text.replace(
    "use crate::db::{self, ImageRecord, ImageSummary};",
    "use crate::ann;\nuse crate::db::{self, ImageRecord, ImageSummary};",
    1,
)

old_query = r'''    let query_embedding = match query_clip_embedding(
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
'''
new_query = r'''    let query_embedding = if settings.clip_weight > 0.0 {
        match query_clip_embedding(
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
        }
    } else {
        None
    };

    let query_key = normalized_path_key(query_path);
    let mut records = db::load_search_images(db_path)?;
'''
if old_query not in text:
    raise SystemExit("indexer query block not found")
text = text.replace(old_query, new_query, 1)

old_metrics_start = r'''    let compute_hash = settings.texture_weight > 0.0;
    let compute_histogram =
        settings.color_distribution_weight > 0.0 || settings.strict_color_rejection;
    let compute_clip = settings.clip_weight > 0.0 && query_embedding.is_some();
    let mut metrics = Vec::<SimilarityMetrics>::with_capacity(records.len());

    for (index, record) in records.iter().enumerate() {'''
new_metrics_start = r'''    let compute_hash = settings.texture_weight > 0.0;
    let compute_histogram =
        settings.color_distribution_weight > 0.0 || settings.strict_color_rejection;
    let compute_clip = settings.clip_weight > 0.0 && query_embedding.is_some();
    let large_ann_search = compute_clip && records.len() > CANDIDATE_PIPELINE_MIN_RECORDS;
    let ann_scores = if large_ann_search {
        let limit = component_candidate_limit(records.len());
        match ann::search_candidates(db_path, query_embedding.as_deref().unwrap_or(&[]), limit) {
            Ok(scores) if !scores.is_empty() => {
                let _ = tx.send(WorkerMessage::Status(format!(
                    "HNSW semantic retrieval: {} approximate CLIP candidates from {} indexed records",
                    scores.len(), records.len()
                )));
                Some(scores)
            }
            Ok(_) => None,
            Err(err) => {
                let _ = tx.send(WorkerMessage::Status(format!(
                    "HNSW unavailable; falling back to brute-force CLIP candidates ({err:#})"
                )));
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
        db::load_embeddings_for_rowids(db_path, &all_rowids)?
    };
    let mut metrics = Vec::<SimilarityMetrics>::with_capacity(records.len());

    for (index, record) in records.iter().enumerate() {'''
if old_metrics_start not in text:
    raise SystemExit("metrics start not found")
text = text.replace(old_metrics_start, new_metrics_start, 1)

old_clip = r'''        // CLIP is the expensive brute-force component. Never touch its vector
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
'''
new_clip = r'''        // For large indexes an HNSW lookup supplies the initial semantic pool.
        // Small indexes and ANN failures preserve the exact brute-force path.
        let clip_similarity = if passes_gate && compute_clip {
            if let Some(scores) = &ann_scores {
                scores.get(&record.rowid).copied()
            } else {
                query_embedding.as_ref().and_then(|query| {
                    fallback_embeddings.get(&record.rowid).map(|(embedding, normalized)| {
                        clip_similarity_with_normalized_query(query, embedding, *normalized)
                            .clamp(0.0, 1.0)
                    })
                })
            }
        } else {
            None
        };
'''
if old_clip not in text:
    raise SystemExit("clip block not found")
text = text.replace(old_clip, new_clip, 1)

old_candidates = r'''    let candidate_indices = choose_candidate_indices(&metrics, settings, query_embedding.is_some());
    if records.len() > CANDIDATE_PIPELINE_MIN_RECORDS {
        let limit = component_candidate_limit(records.len());
        let _ = tx.send(WorkerMessage::Status(format!(
            "Two-stage similarity: {} indexed records → {} hybrid candidates (up to {limit} per enabled component)",
            records.len(),
            candidate_indices.len()
        )));
    }

    let mut scored = Vec::<(bool, ImageRecord)>::with_capacity(candidate_indices.len());
'''
new_candidates = r'''    let candidate_indices = choose_candidate_indices(&metrics, settings, query_embedding.is_some());

    // ANN provides approximate semantic candidate generation only. The final
    // hybrid rerank always uses exact CLIP cosine values for every union member,
    // including candidates introduced by color/texture components.
    if compute_clip && ann_scores.is_some() {
        let candidate_rowids: HashSet<usize> = candidate_indices
            .iter()
            .map(|index| records[*index].rowid)
            .collect();
        let exact_embeddings = db::load_embeddings_for_rowids(db_path, &candidate_rowids)?;
        if let Some(query) = &query_embedding {
            for index in &candidate_indices {
                if metrics[*index].is_exact {
                    metrics[*index].clip_similarity = Some(1.0);
                    continue;
                }
                metrics[*index].clip_similarity = exact_embeddings
                    .get(&records[*index].rowid)
                    .map(|(embedding, normalized)| {
                        clip_similarity_with_normalized_query(query, embedding, *normalized)
                            .clamp(0.0, 1.0)
                    });
            }
        }
    }

    if records.len() > CANDIDATE_PIPELINE_MIN_RECORDS {
        let limit = component_candidate_limit(records.len());
        let _ = tx.send(WorkerMessage::Status(format!(
            "Two-stage similarity: {} indexed records → {} exact hybrid rerank candidates (up to {limit} per enabled component)",
            records.len(),
            candidate_indices.len()
        )));
    }

    let mut scored = Vec::<(bool, ImageRecord)>::with_capacity(candidate_indices.len());
'''
if old_candidates not in text:
    raise SystemExit("candidate block not found")
text = text.replace(old_candidates, new_candidates, 1)

idx.write_text(text, encoding="utf-8")
