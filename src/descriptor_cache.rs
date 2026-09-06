use crate::db::{self, ImageRecord};
use anyhow::Result;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const DESCRIPTOR_CACHE_BUDGET: usize = 64 * 1024 * 1024;

struct Entry {
    path: PathBuf,
    revision: String,
    bytes: usize,
    records: Arc<Vec<ImageRecord>>,
}

#[derive(Default)]
pub struct DescriptorCache {
    entry: Option<Entry>,
}

impl DescriptorCache {
    pub const fn new() -> Self {
        Self { entry: None }
    }
    /// The revision and descriptors must be read on the caller's same snapshot.
    /// Legacy catalogs without revision tracking deliberately bypass caching.
    pub fn load(
        &mut self,
        snapshot: &Connection,
        path: &Path,
        budget: usize,
    ) -> Result<(Arc<Vec<ImageRecord>>, bool, usize)> {
        let revision = db::search_catalog_revision(snapshot)?;
        if let (Some(entry), Some(revision)) = (&self.entry, &revision) {
            if entry.path == path && &entry.revision == revision {
                let bytes = entry.bytes;
                if bytes <= budget {
                    return Ok((Arc::clone(&entry.records), true, bytes));
                }
            }
        }
        // Evict before loading a new version; don't retain two catalog versions.
        self.entry = None;
        let records = Arc::new(db::load_retrieval_images(snapshot)?);
        let bytes = retained_bytes(&records);
        let retained = if let Some(revision) = revision.filter(|_| bytes <= budget) {
            self.entry = Some(Entry {
                path: path.to_path_buf(),
                revision,
                bytes,
                records: Arc::clone(&records),
            });
            bytes
        } else {
            0
        };
        Ok((records, false, retained))
    }
}

fn retained_bytes(records: &Vec<ImageRecord>) -> usize {
    records.iter().fold(
        records
            .capacity()
            .saturating_mul(std::mem::size_of::<ImageRecord>()),
        |total, record| {
            total
                .saturating_add(record.path.capacity())
                .saturating_add(record.root.capacity())
                .saturating_add(record.file_name.capacity())
                .saturating_add(record.extension.capacity())
                .saturating_add(record.description.capacity())
                .saturating_add(record.keywords.capacity())
                .saturating_add(
                    record
                        .color_histogram
                        .as_ref()
                        .map_or(0, |v| v.capacity().saturating_mul(4)),
                )
                .saturating_add(
                    record
                        .material_texture
                        .as_ref()
                        .map_or(0, |v| v.capacity().saturating_mul(4)),
                )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_reuses_committed_revision_invalidates_on_update_and_obeys_budget() {
        let path = std::env::temp_dir().join(format!(
            "wis-descriptor-cache-{}.sqlite3",
            std::process::id()
        ));
        let writer = db::open(&path).unwrap();
        let source = Path::new("test.jpg");
        db::upsert_image(
            &writer,
            source,
            Path::new("."),
            "test.jpg",
            "jpg",
            1,
            1,
            10,
            10,
            "metadata excluded",
            "keywords",
            [1, 2, 3],
            1,
            &[1.0, 0.0],
        )
        .unwrap();
        let mut cache = DescriptorCache::default();
        let snapshot = db::open_search_snapshot(&path).unwrap();
        let (old, hit, bytes) = cache
            .load(&snapshot, &path, DESCRIPTOR_CACHE_BUDGET)
            .unwrap();
        assert!(!hit && bytes > 0);
        assert!(old[0].description.is_empty());
        writer.execute("UPDATE images SET width=20", []).unwrap();
        let (same, hit, _) = cache
            .load(&snapshot, &path, DESCRIPTOR_CACHE_BUDGET)
            .unwrap();
        assert!(hit && Arc::ptr_eq(&old, &same));
        assert_eq!(same[0].width, 10);
        drop(snapshot);
        let fresh = db::open_search_snapshot(&path).unwrap();
        let (new, hit, _) = cache.load(&fresh, &path, DESCRIPTOR_CACHE_BUDGET).unwrap();
        assert!(!hit);
        assert_eq!(new[0].width, 20);
        assert_eq!(old[0].width, 10);
        let (_, hit, retained) = cache.load(&fresh, &path, 0).unwrap();
        assert!(!hit);
        assert_eq!(retained, 0);
        assert!(cache.entry.is_none());
        drop(fresh);
        let before = db::search_catalog_revision(&writer).unwrap();
        writer
            .execute_batch("BEGIN; UPDATE images SET width=30; ROLLBACK;")
            .unwrap();
        assert_eq!(before, db::search_catalog_revision(&writer).unwrap());
        writer.execute("DELETE FROM images", []).unwrap();
        assert_ne!(before, db::search_catalog_revision(&writer).unwrap());
        let fresh = db::open_search_snapshot(&path).unwrap();
        assert!(cache
            .load(&fresh, &path, DESCRIPTOR_CACHE_BUDGET)
            .unwrap()
            .0
            .is_empty());
        drop(fresh);
        drop(writer);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }
}
