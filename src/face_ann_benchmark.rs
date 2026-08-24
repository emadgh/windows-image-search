use crate::{face_embedding, portable};
use anyhow::{bail, Context, Result};
use hnsw_rs::prelude::{AnnT, DistCosine, Hnsw};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

const MAX_CONNECTIONS: usize = 24;
const MAX_LAYERS: usize = 16;
const EF_CONSTRUCTION: usize = 200;
const DEFAULT_QUERY_COUNT: usize = 32;
const TARGET_CORPUS_SIZES: [usize; 6] = [1_000, 5_000, 10_000, 25_000, 50_000, 100_000];
const K_VALUES: [usize; 3] = [10, 25, 100];
const CROSSOVER_MIN_SPEEDUP: f64 = 1.50;
const CROSSOVER_MIN_RECALL_25: f64 = 0.98;
const CROSSOVER_MIN_RECALL_100: f64 = 0.95;
const FIXED_LOGICAL_ROW_BYTES: usize = 13 * 8;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RevisionKey {
    model_id: String,
    model_version: String,
    model_cache_revision: String,
    schema_version: i64,
    alignment_revision: i64,
    dimension: usize,
}

#[derive(Clone, Debug)]
struct EmbeddingRecord {
    face_id: String,
    image_key: String,
    values: Vec<f32>,
    raw_embedding_bytes: usize,
    logical_payload_bytes: usize,
}

#[derive(Default)]
struct LoadedCorpus {
    roots_searched: usize,
    roots_unavailable: usize,
    invalid_embeddings_skipped: usize,
    groups: BTreeMap<RevisionKey, Vec<EmbeddingRecord>>,
}

#[derive(Clone, Debug)]
struct SizeResult {
    corpus_size: usize,
    queries: usize,
    hnsw_build_ms: f64,
    hnsw_dump_bytes: u64,
    exact_avg_ms: f64,
    exact_p50_ms: f64,
    exact_p95_ms: f64,
    ann_avg_ms: f64,
    ann_p50_ms: f64,
    ann_p95_ms: f64,
    speedup: f64,
    recall_10: f64,
    recall_25: f64,
    recall_100: f64,
}

pub fn benchmark(roots: &[PathBuf], requested_queries: usize) -> Result<String> {
    if roots.is_empty() {
        bail!("face ANN benchmark requires at least one registered portable root");
    }
    let loaded = load_current_embeddings(roots)?;
    if loaded.groups.is_empty() {
        bail!("face ANN benchmark found no current compatible normalized face embeddings");
    }

    let mut report = String::new();
    writeln!(report, "Windows Image Search Face ANN Crossover Benchmark")?;
    writeln!(report, "application_version=v{}", env!("CARGO_PKG_VERSION"))?;
    writeln!(report, "roots_requested={}", roots.len())?;
    writeln!(report, "roots_searched={}", loaded.roots_searched)?;
    writeln!(report, "roots_unavailable={}", loaded.roots_unavailable)?;
    writeln!(
        report,
        "invalid_embeddings_skipped={}",
        loaded.invalid_embeddings_skipped
    )?;
    let total_searchable_faces = loaded.groups.values().map(Vec::len).sum::<usize>();
    let total_unique_images = loaded
        .groups
        .values()
        .flat_map(|records| records.iter().map(|record| record.image_key.as_str()))
        .collect::<HashSet<_>>()
        .len();
    writeln!(report, "revision_groups={}", loaded.groups.len())?;
    writeln!(report, "total_searchable_faces={total_searchable_faces}")?;
    writeln!(report, "total_unique_images={total_unique_images}")?;
    writeln!(report, "requested_queries={}", requested_queries.max(1))?;
    writeln!(report, "hnsw_m={MAX_CONNECTIONS}")?;
    writeln!(report, "hnsw_ef_construction={EF_CONSTRUCTION}")?;
    writeln!(report, "crossover_min_speedup={CROSSOVER_MIN_SPEEDUP:.2}")?;
    writeln!(
        report,
        "crossover_min_recall_at_25={:.2}%",
        CROSSOVER_MIN_RECALL_25 * 100.0
    )?;
    writeln!(
        report,
        "crossover_min_recall_at_100={:.2}%",
        CROSSOVER_MIN_RECALL_100 * 100.0
    )?;

    for (group_index, (revision, records)) in loaded.groups.iter().enumerate() {
        let group = group_index + 1;
        let raw_embedding_bytes = records
            .iter()
            .map(|record| record.raw_embedding_bytes as u64)
            .sum::<u64>();
        let logical_payload_bytes = records
            .iter()
            .map(|record| record.logical_payload_bytes as u64)
            .sum::<u64>();
        let unique_images = records
            .iter()
            .map(|record| record.image_key.as_str())
            .collect::<HashSet<_>>()
            .len();

        writeln!(report)?;
        writeln!(report, "revision_{group}_model_id={}", revision.model_id)?;
        writeln!(
            report,
            "revision_{group}_model_version={}",
            revision.model_version
        )?;
        writeln!(
            report,
            "revision_{group}_model_cache_revision={}",
            revision.model_cache_revision
        )?;
        writeln!(
            report,
            "revision_{group}_schema_version={}",
            revision.schema_version
        )?;
        writeln!(
            report,
            "revision_{group}_alignment_revision={}",
            revision.alignment_revision
        )?;
        writeln!(report, "revision_{group}_dimension={}", revision.dimension)?;
        writeln!(
            report,
            "revision_{group}_searchable_faces={}",
            records.len()
        )?;
        writeln!(report, "revision_{group}_unique_images={unique_images}")?;
        writeln!(
            report,
            "revision_{group}_raw_embedding_bytes={raw_embedding_bytes}"
        )?;
        writeln!(
            report,
            "revision_{group}_raw_embedding_bytes_per_face={:.2}",
            ratio_bytes(raw_embedding_bytes, records.len())
        )?;
        writeln!(
            report,
            "revision_{group}_logical_face_embedding_payload_estimate_bytes={logical_payload_bytes}"
        )?;
        writeln!(
            report,
            "revision_{group}_logical_payload_estimate_bytes_per_face={:.2}",
            ratio_bytes(logical_payload_bytes, records.len())
        )?;

        if records.len() < 2 {
            writeln!(
                report,
                "revision_{group}_benchmark_status=insufficient_faces"
            )?;
            writeln!(report, "revision_{group}_crossover_size=not_reached")?;
            continue;
        }

        for target in TARGET_CORPUS_SIZES {
            if target > records.len() {
                writeln!(report, "revision_{group}_n{target}_status=unavailable")?;
            }
        }
        let sizes = corpus_sizes(records.len());
        let mut size_results = Vec::with_capacity(sizes.len());
        for size in sizes {
            size_results.push(benchmark_size(records, size, requested_queries.max(1))?);
        }
        let crossover = first_crossover(&size_results);
        writeln!(
            report,
            "revision_{group}_crossover_size={}",
            crossover
                .map(|value| value.to_string())
                .unwrap_or_else(|| "not_reached".to_owned())
        )?;

        for result in &size_results {
            let prefix = format!("revision_{group}_n{}", result.corpus_size);
            writeln!(report, "{prefix}_status=measured")?;
            writeln!(report, "{prefix}_queries={}", result.queries)?;
            writeln!(report, "{prefix}_hnsw_build_ms={:.3}", result.hnsw_build_ms)?;
            writeln!(
                report,
                "{prefix}_hnsw_dump_bytes={}",
                result.hnsw_dump_bytes
            )?;
            writeln!(report, "{prefix}_exact_avg_ms={:.3}", result.exact_avg_ms)?;
            writeln!(report, "{prefix}_exact_p50_ms={:.3}", result.exact_p50_ms)?;
            writeln!(report, "{prefix}_exact_p95_ms={:.3}", result.exact_p95_ms)?;
            writeln!(report, "{prefix}_ann_avg_ms={:.3}", result.ann_avg_ms)?;
            writeln!(report, "{prefix}_ann_p50_ms={:.3}", result.ann_p50_ms)?;
            writeln!(report, "{prefix}_ann_p95_ms={:.3}", result.ann_p95_ms)?;
            writeln!(report, "{prefix}_speedup={:.3}x", result.speedup)?;
            writeln!(
                report,
                "{prefix}_recall_at_10={:.2}%",
                result.recall_10 * 100.0
            )?;
            writeln!(
                report,
                "{prefix}_recall_at_25={:.2}%",
                result.recall_25 * 100.0
            )?;
            writeln!(
                report,
                "{prefix}_recall_at_100={:.2}%",
                result.recall_100 * 100.0
            )?;
        }
    }

    writeln!(report)?;
    writeln!(
        report,
        "payload_estimate_note=logical payload estimate is a deterministic lower-bound over face/embedding field payloads and excludes SQLite page/index overhead"
    )?;
    writeln!(
        report,
        "hnsw_dump_note=hnsw_dump_bytes is serialized graph+data size; process peak memory should be captured by the benchmark gate for runtime memory evidence"
    )?;
    writeln!(
        report,
        "crossover_note=diagnostic crossover requires >=1.50x exact-search speedup with Recall@25 >=98% and Recall@100 >=95%; it does not change production search defaults"
    )?;
    Ok(report)
}

pub fn default_query_count() -> usize {
    DEFAULT_QUERY_COUNT
}

fn load_current_embeddings(roots: &[PathBuf]) -> Result<LoadedCorpus> {
    let mut loaded = LoadedCorpus::default();
    for root in roots {
        let conn = match open_read_only_root(root) {
            Ok(conn) => conn,
            Err(_) => {
                loaded.roots_unavailable += 1;
                continue;
            }
        };
        let library_id = match portable_library_id(&conn) {
            Ok(value) => value,
            Err(_) => {
                loaded.roots_unavailable += 1;
                continue;
            }
        };
        loaded.roots_searched += 1;

        let mut stmt = conn.prepare(
            r#"
            SELECT f.face_id, f.image_path, COALESCE(LENGTH(f.landmarks), 0),
                   f.detector_id, f.detector_version, f.detector_cache_revision,
                   e.model_id, e.model_version, e.model_cache_revision,
                   e.schema_version, e.alignment_revision, e.dimension, e.embedding
            FROM face_embeddings e
            JOIN faces f ON f.face_id = e.face_id
            JOIN face_detection_state s ON s.image_path = f.image_path
            JOIN images i ON i.path = f.image_path
            WHERE e.schema_version = ?1
              AND e.normalized = 1
              AND e.detector_id = f.detector_id
              AND e.detector_version = f.detector_version
              AND e.detector_cache_revision = f.detector_cache_revision
              AND e.detection_schema_version = f.schema_version
              AND e.source_size = f.source_size
              AND e.source_modified = f.source_modified
              AND s.detector_id = f.detector_id
              AND s.detector_version = f.detector_version
              AND s.detector_cache_revision = f.detector_cache_revision
              AND s.schema_version = f.schema_version
              AND s.source_size = f.source_size
              AND s.source_modified = f.source_modified
              AND i.size = f.source_size
              AND i.modified = f.source_modified
            ORDER BY e.model_id, e.model_version, e.model_cache_revision,
                     e.alignment_revision, e.dimension, f.face_id
            "#,
        )?;
        let rows = stmt.query_map([face_embedding::SCHEMA_VERSION], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?.max(0) as usize,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, i64>(11)?,
                row.get::<_, Vec<u8>>(12)?,
            ))
        })?;

        for row in rows {
            let (
                face_id,
                image_path,
                landmark_bytes,
                detector_id,
                detector_version,
                detector_cache_revision,
                model_id,
                model_version,
                model_cache_revision,
                schema_version,
                alignment_revision,
                dimension,
                blob,
            ) = row?;
            let Ok(dimension) = usize::try_from(dimension) else {
                loaded.invalid_embeddings_skipped += 1;
                continue;
            };
            let Some(values) = decode_embedding(&blob, dimension) else {
                loaded.invalid_embeddings_skipped += 1;
                continue;
            };
            if !is_normalized_embedding(&values) {
                loaded.invalid_embeddings_skipped += 1;
                continue;
            }
            let revision = RevisionKey {
                model_id: model_id.clone(),
                model_version: model_version.clone(),
                model_cache_revision: model_cache_revision.clone(),
                schema_version,
                alignment_revision,
                dimension,
            };
            let logical_payload_bytes = blob
                .len()
                .saturating_add(landmark_bytes)
                .saturating_add(face_id.len())
                .saturating_add(image_path.len())
                .saturating_add(detector_id.len())
                .saturating_add(detector_version.len())
                .saturating_add(detector_cache_revision.len())
                .saturating_add(model_id.len())
                .saturating_add(model_version.len())
                .saturating_add(model_cache_revision.len())
                .saturating_add(FIXED_LOGICAL_ROW_BYTES);
            loaded
                .groups
                .entry(revision)
                .or_default()
                .push(EmbeddingRecord {
                    face_id,
                    image_key: format!("{library_id}\0{image_path}"),
                    raw_embedding_bytes: blob.len(),
                    logical_payload_bytes,
                    values,
                });
        }
    }
    Ok(loaded)
}

fn benchmark_size(
    records: &[EmbeddingRecord],
    corpus_size: usize,
    requested_queries: usize,
) -> Result<SizeResult> {
    let corpus = &records[..corpus_size.min(records.len())];
    if corpus.len() < 2 {
        bail!("face ANN corpus requires at least two embeddings");
    }

    let build_started = Instant::now();
    let hnsw = Hnsw::<f32, DistCosine>::new(
        MAX_CONNECTIONS,
        corpus.len(),
        MAX_LAYERS,
        EF_CONSTRUCTION,
        DistCosine {},
    );
    let refs = corpus
        .iter()
        .enumerate()
        .map(|(index, record)| (&record.values, index))
        .collect::<Vec<_>>();
    hnsw.parallel_insert(&refs);
    let hnsw_build_ms = build_started.elapsed().as_secs_f64() * 1_000.0;
    let hnsw_dump_bytes = serialized_hnsw_bytes(&hnsw, corpus.len())?;

    let query_indices = sample_indices(corpus.len(), requested_queries);
    let max_k = K_VALUES
        .into_iter()
        .max()
        .unwrap_or(1)
        .min(corpus.len().saturating_sub(1));
    let search_k = max_k.saturating_add(1).min(corpus.len());
    let ef = search_ef(search_k, corpus.len());

    let mut exact_us = Vec::with_capacity(query_indices.len());
    let mut ann_us = Vec::with_capacity(query_indices.len());
    let mut overlap = [0usize; K_VALUES.len()];
    let mut denominators = [0usize; K_VALUES.len()];

    for query_index in query_indices.iter().copied() {
        let query = &corpus[query_index].values;

        let exact_started = Instant::now();
        let exact = exact_top_indices(corpus, query_index, max_k);
        exact_us.push(exact_started.elapsed().as_micros());

        let ann_started = Instant::now();
        let ann = hnsw
            .search(query, search_k, ef)
            .into_iter()
            .map(|neighbour| neighbour.d_id)
            .filter(|candidate| *candidate != query_index)
            .take(max_k)
            .collect::<Vec<_>>();
        ann_us.push(ann_started.elapsed().as_micros());

        for (slot, k) in K_VALUES.iter().copied().enumerate() {
            let effective_k = k.min(exact.len());
            if effective_k == 0 {
                continue;
            }
            let exact_set = exact
                .iter()
                .take(effective_k)
                .copied()
                .collect::<HashSet<_>>();
            overlap[slot] += ann
                .iter()
                .take(effective_k)
                .filter(|candidate| exact_set.contains(candidate))
                .count();
            denominators[slot] += effective_k;
        }
    }

    let exact_avg_ms = average_ms(&exact_us);
    let ann_avg_ms = average_ms(&ann_us);
    Ok(SizeResult {
        corpus_size: corpus.len(),
        queries: query_indices.len(),
        hnsw_build_ms,
        hnsw_dump_bytes,
        exact_avg_ms,
        exact_p50_ms: percentile_ms(&exact_us, 0.50),
        exact_p95_ms: percentile_ms(&exact_us, 0.95),
        ann_avg_ms,
        ann_p50_ms: percentile_ms(&ann_us, 0.50),
        ann_p95_ms: percentile_ms(&ann_us, 0.95),
        speedup: if ann_avg_ms > f64::EPSILON {
            exact_avg_ms / ann_avg_ms
        } else {
            0.0
        },
        recall_10: recall(overlap[0], denominators[0]),
        recall_25: recall(overlap[1], denominators[1]),
        recall_100: recall(overlap[2], denominators[2]),
    })
}

fn serialized_hnsw_bytes(hnsw: &Hnsw<f32, DistCosine>, corpus_size: usize) -> Result<u64> {
    let dir = std::env::temp_dir().join(format!(
        "wis-face-ann-benchmark-{}-{corpus_size}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating temporary HNSW dump directory {}", dir.display()))?;
    let dump_result = hnsw.file_dump(&dir, "face-cosine-benchmark");
    if let Err(err) = dump_result {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(err).context("serializing temporary face HNSW benchmark index");
    }
    let bytes = std::fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .sum::<u64>();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(bytes)
}

fn exact_top_indices(records: &[EmbeddingRecord], query_index: usize, limit: usize) -> Vec<usize> {
    let query = &records[query_index].values;
    let mut ranked = records
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != query_index)
        .map(|(index, record)| (index, dot_product(query, &record.values)))
        .collect::<Vec<_>>();
    if ranked.len() > limit {
        ranked.select_nth_unstable_by(limit, |left, right| right.1.total_cmp(&left.1));
        ranked.truncate(limit);
    }
    ranked.sort_unstable_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| records[left.0].face_id.cmp(&records[right.0].face_id))
    });
    ranked.into_iter().map(|(index, _)| index).collect()
}

fn corpus_sizes(total: usize) -> Vec<usize> {
    if total < 2 {
        return Vec::new();
    }
    let mut sizes = TARGET_CORPUS_SIZES
        .into_iter()
        .filter(|size| *size <= total)
        .collect::<Vec<_>>();
    if sizes.last().copied() != Some(total) {
        sizes.push(total);
    }
    sizes
}

fn sample_indices(total: usize, requested: usize) -> Vec<usize> {
    if total == 0 {
        return Vec::new();
    }
    let count = requested.clamp(1, total);
    (0..count).map(|index| index * total / count).collect()
}

fn first_crossover(results: &[SizeResult]) -> Option<usize> {
    results
        .iter()
        .find(|result| {
            result.speedup >= CROSSOVER_MIN_SPEEDUP
                && result.recall_25 >= CROSSOVER_MIN_RECALL_25
                && result.recall_100 >= CROSSOVER_MIN_RECALL_100
        })
        .map(|result| result.corpus_size)
}

fn search_ef(k: usize, count: usize) -> usize {
    (k + 512).min(count).max(k)
}

fn decode_embedding(blob: &[u8], dimension: usize) -> Option<Vec<f32>> {
    if dimension == 0 || blob.len() != dimension.checked_mul(4)? {
        return None;
    }
    Some(
        blob.chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect(),
    )
}

fn is_normalized_embedding(values: &[f32]) -> bool {
    if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
        return false;
    }
    let norm_sq = values
        .iter()
        .map(|value| (*value as f64) * (*value as f64))
        .sum::<f64>();
    (norm_sq - 1.0).abs() <= 0.02
}

fn dot_product(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

fn recall(overlap: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        overlap as f64 / denominator as f64
    }
}

fn average_ms(values_us: &[u128]) -> f64 {
    if values_us.is_empty() {
        return 0.0;
    }
    values_us.iter().copied().sum::<u128>() as f64 / values_us.len() as f64 / 1_000.0
}

fn percentile_ms(values_us: &[u128], percentile: f64) -> f64 {
    if values_us.is_empty() {
        return 0.0;
    }
    let mut sorted = values_us.to_vec();
    sorted.sort_unstable();
    let position = ((sorted.len() - 1) as f64 * percentile.clamp(0.0, 1.0)).round() as usize;
    sorted[position] as f64 / 1_000.0
}

fn ratio_bytes(bytes: u64, count: usize) -> f64 {
    if count == 0 {
        0.0
    } else {
        bytes as f64 / count as f64
    }
}

fn open_read_only_root(root: &Path) -> Result<Connection> {
    if !root.is_dir() {
        bail!("portable root is unavailable: {}", root.display());
    }
    let db_path = portable::index_db_path(root);
    if !db_path.is_file() {
        bail!("portable index does not exist: {}", db_path.display());
    }
    Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).with_context(|| {
        format!(
            "opening portable face benchmark index read-only {}",
            db_path.display()
        )
    })
}

fn portable_library_id(conn: &Connection) -> Result<String> {
    let value = conn
        .query_row(
            "SELECT value FROM portable_meta WHERE key = 'library_id'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .context("portable index has no library_id")?;
    if value.trim().is_empty() {
        bail!("portable index library_id is empty");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_record(id: usize, values: Vec<f32>) -> EmbeddingRecord {
        EmbeddingRecord {
            face_id: format!("face-{id:04}"),
            image_key: format!("lib\0image-{id:04}.jpg"),
            raw_embedding_bytes: values.len() * 4,
            logical_payload_bytes: values.len() * 4 + 100,
            values,
        }
    }

    #[test]
    fn corpus_sizes_include_measured_tail_without_duplicates() {
        assert_eq!(corpus_sizes(999), vec![999]);
        assert_eq!(corpus_sizes(1_000), vec![1_000]);
        assert_eq!(corpus_sizes(6_000), vec![1_000, 5_000, 6_000]);
        assert!(corpus_sizes(1).is_empty());
    }

    #[test]
    fn deterministic_query_sampling_is_even_and_bounded() {
        assert_eq!(sample_indices(10, 4), vec![0, 2, 5, 7]);
        assert_eq!(sample_indices(3, 20), vec![0, 1, 2]);
        assert!(sample_indices(0, 4).is_empty());
    }

    #[test]
    fn exact_ranking_excludes_query_and_prefers_high_cosine() {
        let records = vec![
            fake_record(0, vec![1.0, 0.0]),
            fake_record(1, vec![0.9, 0.1]),
            fake_record(2, vec![0.0, 1.0]),
        ];
        assert_eq!(exact_top_indices(&records, 0, 2), vec![1, 2]);
    }

    #[test]
    fn recall_handles_empty_and_partial_overlap() {
        assert_eq!(recall(0, 0), 0.0);
        assert!((recall(8, 10) - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn crossover_requires_speed_and_quality_thresholds() {
        let base = SizeResult {
            corpus_size: 1_000,
            queries: 8,
            hnsw_build_ms: 1.0,
            hnsw_dump_bytes: 1,
            exact_avg_ms: 2.0,
            exact_p50_ms: 2.0,
            exact_p95_ms: 2.0,
            ann_avg_ms: 1.0,
            ann_p50_ms: 1.0,
            ann_p95_ms: 1.0,
            speedup: 2.0,
            recall_10: 1.0,
            recall_25: 0.99,
            recall_100: 0.96,
        };
        assert_eq!(first_crossover(std::slice::from_ref(&base)), Some(1_000));
        let mut low_recall = base.clone();
        low_recall.recall_25 = 0.90;
        assert_eq!(first_crossover(&[low_recall]), None);
        let mut slow = base;
        slow.speedup = 1.2;
        assert_eq!(first_crossover(&[slow]), None);
    }

    #[test]
    fn embedding_decoder_rejects_malformed_or_non_normalized_values() {
        assert!(decode_embedding(&[0; 3], 1).is_none());
        assert!(!is_normalized_embedding(&[0.5, 0.5]));
        assert!(is_normalized_embedding(&[1.0, 0.0]));
    }
}
