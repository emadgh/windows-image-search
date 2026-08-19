use crate::db;
use anyhow::{bail, Context, Result};
use hnsw_rs::prelude::{AnnT, DistCosine, Hnsw, HnswIo};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

const INDEX_DIR_NAME: &str = "ann-index";
const INDEX_BASENAME: &str = "clip-cosine-v1";
const MANIFEST_NAME: &str = "clip-cosine-v1.manifest";
const MANIFEST_VERSION: u32 = 1;
const MAX_CONNECTIONS: usize = 24;
const MAX_LAYERS: usize = 16;
const EF_CONSTRUCTION: usize = 200;
const DEFAULT_BENCHMARK_QUERIES: usize = 32;
const BENCHMARK_K_VALUES: [usize; 3] = [10, 50, 100];

#[derive(Clone, Debug)]
struct Manifest {
    signature: u64,
    basename: String,
    count: usize,
}

pub fn search_candidates(
    db_path: &Path,
    query: &[f32],
    limit: usize,
) -> Result<HashMap<usize, f32>> {
    if query.is_empty() || limit == 0 {
        return Ok(HashMap::new());
    }

    let index_dir = index_dir_for_db(db_path);
    let (manifest, _) = ensure_manifest(db_path, &index_dir)?;
    if manifest.count == 0 {
        return Ok(HashMap::new());
    }

    // HnswIo owns reload state that the loaded Hnsw may borrow, so the loader
    // intentionally stays in the same scope and outlives every HNSW access.
    let mut loader = HnswIo::new(&index_dir, &manifest.basename);
    let hnsw: Hnsw<f32, DistCosine> = loader
        .load_hnsw::<f32, DistCosine>()
        .context("loading persisted CLIP HNSW index")?;

    let k = limit.min(manifest.count);
    if k == 0 {
        return Ok(HashMap::new());
    }
    let ef = search_ef(k, manifest.count);
    let neighbours = hnsw.search(query, k, ef);

    Ok(neighbours
        .into_iter()
        .map(|neighbour| {
            // DistCosine returns cosine distance. The application historically
            // clamps CLIP cosine similarity to [0, 1], so preserve that scale.
            let similarity = (1.0_f32 - neighbour.distance).clamp(0.0_f32, 1.0_f32);
            (neighbour.d_id, similarity)
        })
        .collect())
}

pub fn benchmark(db_path: &Path, requested_queries: usize) -> Result<String> {
    let entries = db::load_ann_embeddings(db_path)?;
    if entries.len() < 2 {
        bail!("ANN benchmark requires at least 2 indexed CLIP embeddings");
    }

    let index_dir = index_dir_for_db(db_path);
    let prepare_started = Instant::now();
    let (manifest, rebuilt) = ensure_manifest(db_path, &index_dir)?;
    let prepare_ms = prepare_started.elapsed().as_secs_f64() * 1_000.0;
    if manifest.count == 0 {
        bail!("ANN benchmark found no persisted CLIP vectors");
    }

    let load_started = Instant::now();
    // Keep the reloader alive for the full benchmark. hnsw_rs can back a
    // reloaded graph with data owned by HnswIo, so returning Hnsw from a helper
    // with a local loader would violate that lifetime contract.
    let mut loader = HnswIo::new(&index_dir, &manifest.basename);
    let hnsw: Hnsw<f32, DistCosine> = loader
        .load_hnsw::<f32, DistCosine>()
        .context("loading persisted CLIP HNSW index")?;
    let load_ms = load_started.elapsed().as_secs_f64() * 1_000.0;

    let query_indices = sample_indices(entries.len(), requested_queries.max(1));
    let k_values: Vec<usize> = BENCHMARK_K_VALUES
        .into_iter()
        .map(|k| k.min(entries.len()))
        .filter(|k| *k > 0)
        .fold(Vec::new(), |mut values, k| {
            if values.last().copied() != Some(k) {
                values.push(k);
            }
            values
        });
    let max_k = *k_values.last().context("benchmark has no K values")?;
    let ef = search_ef(max_k, manifest.count);

    let mut ann_us = Vec::with_capacity(query_indices.len());
    let mut exact_us = Vec::with_capacity(query_indices.len());
    let mut overlap_totals = vec![0usize; k_values.len()];

    for index in &query_indices {
        let query = &entries[*index].embedding;

        let exact_started = Instant::now();
        let exact = exact_top_rowids(&entries, query, max_k);
        exact_us.push(exact_started.elapsed().as_micros());

        let ann_started = Instant::now();
        let ann: Vec<usize> = hnsw
            .search(query, max_k.min(manifest.count), ef)
            .into_iter()
            .map(|neighbour| neighbour.d_id)
            .collect();
        ann_us.push(ann_started.elapsed().as_micros());

        for (slot, k) in k_values.iter().copied().enumerate() {
            let exact_set: HashSet<usize> = exact.iter().take(k).copied().collect();
            overlap_totals[slot] += ann
                .iter()
                .take(k)
                .filter(|rowid| exact_set.contains(rowid))
                .count();
        }
    }

    let ann_avg = average_ms(&ann_us);
    let exact_avg = average_ms(&exact_us);
    let speedup = if ann_avg > f64::EPSILON {
        exact_avg / ann_avg
    } else {
        0.0
    };

    let mut report = String::new();
    writeln!(report, "Windows Image Search ANN Benchmark")?;
    writeln!(report, "application_version=v{}", env!("CARGO_PKG_VERSION"))?;
    writeln!(report, "vectors={}", entries.len())?;
    writeln!(report, "queries={}", query_indices.len())?;
    writeln!(report, "hnsw_rebuilt={rebuilt}")?;
    writeln!(report, "index_prepare_ms={prepare_ms:.3}")?;
    writeln!(report, "index_load_ms={load_ms:.3}")?;
    writeln!(report, "hnsw_m={MAX_CONNECTIONS}")?;
    writeln!(report, "hnsw_ef_construction={EF_CONSTRUCTION}")?;
    writeln!(report, "hnsw_ef_search={ef}")?;
    writeln!(report, "ann_avg_ms={ann_avg:.3}")?;
    writeln!(report, "ann_p50_ms={:.3}", percentile_ms(&ann_us, 0.50))?;
    writeln!(report, "ann_p95_ms={:.3}", percentile_ms(&ann_us, 0.95))?;
    writeln!(report, "bruteforce_avg_ms={exact_avg:.3}")?;
    writeln!(
        report,
        "bruteforce_p50_ms={:.3}",
        percentile_ms(&exact_us, 0.50)
    )?;
    writeln!(
        report,
        "bruteforce_p95_ms={:.3}",
        percentile_ms(&exact_us, 0.95)
    )?;
    writeln!(report, "warm_query_speedup={speedup:.2}x")?;

    for (slot, k) in k_values.iter().copied().enumerate() {
        let denominator = query_indices.len() * k;
        let recall = if denominator == 0 {
            0.0
        } else {
            overlap_totals[slot] as f64 / denominator as f64
        };
        writeln!(report, "recall@{k}={:.2}%", recall * 100.0)?;
    }

    Ok(report)
}

pub fn default_benchmark_queries() -> usize {
    DEFAULT_BENCHMARK_QUERIES
}

pub fn prepare_index(db_path: &Path) -> Result<bool> {
    let index_dir = index_dir_for_db(db_path);
    let (_, rebuilt) = ensure_manifest(db_path, &index_dir)?;
    Ok(rebuilt)
}

fn ensure_manifest(db_path: &Path, index_dir: &Path) -> Result<(Manifest, bool)> {
    let signature = db::ann_index_signature(db_path)?;
    match load_manifest(index_dir) {
        Ok(manifest)
            if manifest.signature == signature && dump_exists(index_dir, &manifest.basename) =>
        {
            Ok((manifest, false))
        }
        _ => Ok((rebuild(db_path, index_dir, signature)?, true)),
    }
}

fn search_ef(k: usize, count: usize) -> usize {
    (k + 512).min(count).max(k)
}

fn rebuild(db_path: &Path, index_dir: &Path, signature: u64) -> Result<Manifest> {
    let entries = db::load_ann_embeddings(db_path)?;
    if entries.is_empty() {
        let manifest = Manifest {
            signature,
            basename: INDEX_BASENAME.to_owned(),
            count: 0,
        };
        store_manifest(index_dir, &manifest)?;
        return Ok(manifest);
    }

    std::fs::create_dir_all(index_dir)
        .with_context(|| format!("creating ANN index directory {}", index_dir.display()))?;

    let hnsw = Hnsw::<f32, DistCosine>::new(
        MAX_CONNECTIONS,
        entries.len(),
        MAX_LAYERS,
        EF_CONSTRUCTION,
        DistCosine {},
    );
    let refs: Vec<(&Vec<f32>, usize)> = entries
        .iter()
        .map(|entry| (&entry.embedding, entry.rowid))
        .collect();
    hnsw.parallel_insert(&refs);

    let basename = hnsw
        .file_dump(index_dir, INDEX_BASENAME)
        .context("persisting CLIP HNSW index")?;
    let manifest = Manifest {
        signature,
        basename,
        count: entries.len(),
    };
    store_manifest(index_dir, &manifest)?;
    Ok(manifest)
}

fn exact_top_rowids(entries: &[db::AnnEmbedding], query: &[f32], limit: usize) -> Vec<usize> {
    let mut ranked: Vec<(usize, f32)> = entries
        .iter()
        .map(|entry| (entry.rowid, dot_product(query, &entry.embedding)))
        .collect();
    if ranked.len() > limit {
        ranked.select_nth_unstable_by(limit, |a, b| b.1.total_cmp(&a.1));
        ranked.truncate(limit);
    }
    ranked.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
    ranked.into_iter().map(|(rowid, _)| rowid).collect()
}

fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(left, right)| left * right)
        .sum()
}

fn sample_indices(total: usize, requested: usize) -> Vec<usize> {
    if total == 0 {
        return Vec::new();
    }
    let count = requested.clamp(1, total);
    (0..count).map(|index| index * total / count).collect()
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

fn index_dir_for_db(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(INDEX_DIR_NAME)
}

fn manifest_path(index_dir: &Path) -> PathBuf {
    index_dir.join(MANIFEST_NAME)
}

fn dump_exists(index_dir: &Path, basename: &str) -> bool {
    index_dir.join(format!("{basename}.hnsw.graph")).exists()
        && index_dir.join(format!("{basename}.hnsw.data")).exists()
}

fn load_manifest(index_dir: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(manifest_path(index_dir))?;
    let mut version = None;
    let mut signature = None;
    let mut basename = None;
    let mut count = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "version" => version = value.trim().parse::<u32>().ok(),
            "signature" => signature = u64::from_str_radix(value.trim(), 16).ok(),
            "basename" => basename = Some(value.trim().to_owned()),
            "count" => count = value.trim().parse::<usize>().ok(),
            _ => {}
        }
    }
    if version != Some(MANIFEST_VERSION) {
        bail!("unsupported ANN manifest version");
    }
    Ok(Manifest {
        signature: signature.context("ANN manifest has no signature")?,
        basename: basename
            .filter(|value| !value.is_empty())
            .context("ANN manifest has no basename")?,
        count: count.context("ANN manifest has no count")?,
    })
}

fn store_manifest(index_dir: &Path, manifest: &Manifest) -> Result<()> {
    std::fs::create_dir_all(index_dir)?;
    let destination = manifest_path(index_dir);
    let temporary = destination.with_extension("manifest.tmp");
    std::fs::write(
        &temporary,
        format!(
            "version={}\nsignature={:016x}\nbasename={}\ncount={}\n",
            MANIFEST_VERSION, manifest.signature, manifest.basename, manifest.count
        ),
    )?;
    if destination.exists() {
        let _ = std::fs::remove_file(&destination);
    }
    std::fs::rename(&temporary, &destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn manifest_round_trip_preserves_signature_and_basename() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("wis-ann-manifest-{nonce}"));
        let manifest = Manifest {
            signature: 0xAABB_CCDD_1122_3344,
            basename: "clip-test".to_owned(),
            count: 123,
        };
        store_manifest(&dir, &manifest).unwrap();
        let loaded = load_manifest(&dir).unwrap();
        assert_eq!(loaded.signature, manifest.signature);
        assert_eq!(loaded.basename, manifest.basename);
        assert_eq!(loaded.count, manifest.count);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn benchmark_sampling_is_even_and_unique() {
        assert_eq!(sample_indices(10, 4), vec![0, 2, 5, 7]);
        assert_eq!(sample_indices(3, 10), vec![0, 1, 2]);
        assert!(sample_indices(0, 10).is_empty());
    }

    #[test]
    fn exact_top_rows_prefer_identical_normalized_vector() {
        let entries = vec![
            db::AnnEmbedding {
                rowid: 11,
                embedding: vec![1.0, 0.0],
            },
            db::AnnEmbedding {
                rowid: 22,
                embedding: vec![0.8, 0.6],
            },
            db::AnnEmbedding {
                rowid: 33,
                embedding: vec![0.0, 1.0],
            },
        ];
        let ranked = exact_top_rowids(&entries, &[1.0, 0.0], 2);
        assert_eq!(ranked, vec![11, 22]);
    }

    #[test]
    fn percentile_uses_sorted_duration_distribution() {
        let values = [1_000, 2_000, 10_000, 4_000, 3_000];
        assert_eq!(percentile_ms(&values, 0.50), 3.0);
        assert_eq!(percentile_ms(&values, 0.95), 10.0);
    }
}
