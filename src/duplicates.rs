use crate::{db, search_request::SearchRequest};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct DuplicateGroup {
    pub exact: bool,
    pub paths: Vec<PathBuf>,
}
#[derive(Default, Debug)]
pub struct DuplicateReport {
    pub groups: Vec<DuplicateGroup>,
    pub skipped: usize,
    pub hashed: usize,
}

pub fn hash_file(path: &Path, request: &SearchRequest) -> Result<[u8; 32]> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let before = file.metadata()?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        request.check()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let after = std::fs::metadata(path)?;
    if before.len() != after.len() || before.modified()? != after.modified()? {
        anyhow::bail!("File changed during duplicate verification");
    }
    Ok(hash.finalize().into())
}

pub fn scan(
    db_path: &Path,
    eligible: Option<&std::collections::HashSet<PathBuf>>,
    request: &SearchRequest,
) -> Result<DuplicateReport> {
    let snapshot = db::open_search_snapshot(db_path)?;
    let mut records = db::load_search_images_from_connection(&snapshot)?;
    drop(snapshot);
    records.retain(|image| eligible.is_none_or(|paths| paths.contains(&image.path)));
    records.sort_by(|a, b| a.path.cmp(&b.path));
    let mut report = DuplicateReport::default();
    let mut sizes: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, image) in records.iter().enumerate() {
        if let Ok(meta) = std::fs::metadata(&image.path) {
            sizes.entry(meta.len()).or_default().push(index);
        } else {
            report.skipped += 1;
        }
    }
    let mut exact: HashMap<[u8; 32], Vec<PathBuf>> = HashMap::new();
    for indices in sizes.values().filter(|values| values.len() > 1) {
        for index in indices {
            request.check()?;
            let image = &records[*index];
            match hash_file(&image.path, request) {
                Ok(hash) => {
                    report.hashed += 1;
                    exact.entry(hash).or_default().push(image.path.clone());
                }
                Err(_) => {
                    request.check()?;
                    report.skipped += 1;
                }
            }
        }
    }
    report.groups.extend(
        exact
            .into_values()
            .filter(|paths| paths.len() > 1)
            .map(|paths| DuplicateGroup { exact: true, paths }),
    );
    // Eight disjoint byte bands bound candidate generation. Compare each member
    // to a group anchor; never chain unrelated images through transitive links.
    let mut bands: HashMap<(usize, u8), Vec<usize>> = HashMap::new();
    let mut visual: Vec<(u64, f64, Vec<PathBuf>)> = Vec::new();
    for image in &records {
        request.check()?;
        let Some(hash) = image.visual_hash else {
            continue;
        };
        if image.height == 0 {
            continue;
        }
        let ratio = image.width as f64 / image.height as f64;
        let mut candidates = std::collections::BTreeSet::new();
        for band in 0..8 {
            if let Some(indices) = bands.get(&(band, (hash >> (band * 8)) as u8)) {
                candidates.extend(indices.iter().rev().take(64).copied());
            }
        }
        let matched = candidates.into_iter().find(|index| {
            let (anchor, aspect, _) = &visual[*index];
            (hash ^ anchor).count_ones() <= 6 && (ratio / aspect - 1.0).abs() <= 0.10
        });
        if let Some(index) = matched {
            visual[index].2.push(image.path.clone());
        } else {
            let index = visual.len();
            visual.push((hash, ratio, vec![image.path.clone()]));
            for band in 0..8 {
                bands
                    .entry((band, (hash >> (band * 8)) as u8))
                    .or_default()
                    .push(index);
            }
        }
    }
    report.groups.extend(
        visual
            .into_iter()
            .filter(|(_, _, paths)| paths.len() > 1)
            .map(|(_, _, paths)| DuplicateGroup {
                exact: false,
                paths,
            }),
    );
    for group in &mut report.groups {
        group.paths.sort();
    }
    report
        .groups
        .sort_by(|a, b| b.exact.cmp(&a.exact).then(a.paths[0].cmp(&b.paths[0])));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_file_hash_distinguishes_equal_size_content_and_honors_cancel() {
        let mut session = crate::search_request::SearchSession::default();
        let request = session.start();
        let path = std::env::temp_dir().join(format!("wis-hash-test-{}.bin", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        let a = hash_file(&path, &request).unwrap();
        assert_eq!(a, <[u8; 32]>::from(Sha256::digest(b"abc")));
        std::fs::write(&path, b"abd").unwrap();
        assert_ne!(a, hash_file(&path, &request).unwrap());
        session.cancel();
        assert!(hash_file(&path, &request).is_err());
        let _ = std::fs::remove_file(path);
    }
}
