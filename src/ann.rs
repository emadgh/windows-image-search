use crate::db;
use anyhow::{bail, Context, Result};
use hnsw_rs::prelude::{AnnT, DistCosine, Hnsw, HnswIo};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const INDEX_DIR_NAME: &str = "ann-index";
const INDEX_BASENAME: &str = "clip-cosine-v1";
const MANIFEST_NAME: &str = "clip-cosine-v1.manifest";
const MANIFEST_VERSION: u32 = 1;
const MAX_CONNECTIONS: usize = 24;
const MAX_LAYERS: usize = 16;
const EF_CONSTRUCTION: usize = 200;

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

    let signature = db::ann_index_signature(db_path)?;
    let index_dir = index_dir_for_db(db_path);
    let manifest = match load_manifest(&index_dir) {
        Ok(manifest)
            if manifest.signature == signature && dump_exists(&index_dir, &manifest.basename) =>
        {
            manifest
        }
        _ => rebuild(db_path, &index_dir, signature)?,
    };

    if manifest.count == 0 {
        return Ok(HashMap::new());
    }

    let mut loader = HnswIo::new(&index_dir, &manifest.basename);
    let hnsw: Hnsw<f32, DistCosine> = loader
        .load_hnsw::<f32, DistCosine>()
        .context("loading persisted CLIP HNSW index")?;

    let k = limit.min(manifest.count);
    if k == 0 {
        return Ok(HashMap::new());
    }
    let ef = (k + 512).min(manifest.count).max(k);
    let neighbours = hnsw.search(query, k, ef);

    Ok(neighbours
        .into_iter()
        .map(|neighbour| {
            // DistCosine returns cosine distance. The application historically
            // clamps CLIP cosine similarity to [0, 1], so preserve that scale.
            let similarity = (1.0 - neighbour.distance).clamp(0.0, 1.0);
            (neighbour.d_id, similarity)
        })
        .collect())
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
}
