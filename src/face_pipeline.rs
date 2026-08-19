use crate::{db, face_detection, face_store, portable};
use anyhow::{Context, Result};
use image::GenericImageView;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

const DEFAULT_BATCH_SIZE: usize = 16;
const MAX_BATCH_SIZE: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FacePipelineOptions {
    pub batch_size: usize,
}

impl Default for FacePipelineOptions {
    fn default() -> Self {
        Self {
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }
}

impl FacePipelineOptions {
    pub fn sanitized(self) -> Self {
        Self {
            batch_size: self.batch_size.clamp(1, MAX_BATCH_SIZE),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FacePipelineEvent {
    RootStarted {
        root: PathBuf,
        pending: usize,
    },
    Progress {
        root: PathBuf,
        processed: usize,
        pending: usize,
        faces: usize,
        failures: usize,
    },
    ImageFailed {
        root: PathBuf,
        image: PathBuf,
        error: String,
    },
    RootUnavailable {
        root: PathBuf,
    },
    RootFinished {
        root: PathBuf,
        processed: usize,
        faces: usize,
        failures: usize,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FacePipelineSummary {
    pub roots_processed: usize,
    pub roots_unavailable: usize,
    pub images_processed: usize,
    pub faces_detected: usize,
    pub failures: usize,
}

#[derive(Clone, Debug)]
struct PendingImage {
    relative_path: PathBuf,
    size: u64,
    modified: i64,
}

pub fn run_available_roots<D, F>(
    roots: &[PathBuf],
    detector: &mut D,
    options: FacePipelineOptions,
    mut emit: F,
) -> Result<FacePipelineSummary>
where
    D: face_detection::FaceDetector,
    F: FnMut(FacePipelineEvent),
{
    let options = options.sanitized();
    let detector_id = detector.detector_id();
    let detector_version = detector.detector_version();
    let mut summary = FacePipelineSummary::default();

    for root in roots {
        if !root.is_dir() || !portable::is_indexed_root(root) {
            summary.roots_unavailable += 1;
            emit(FacePipelineEvent::RootUnavailable { root: root.clone() });
            continue;
        }

        let db_path = portable::index_db_path(root);
        let mut conn = db::open(&db_path)
            .with_context(|| format!("opening portable face index {}", db_path.display()))?;
        face_store::ensure_schema(&conn)?;
        let pending = count_pending(&conn, detector_id, detector_version)?;
        emit(FacePipelineEvent::RootStarted {
            root: root.clone(),
            pending,
        });

        let mut root_processed = 0usize;
        let mut root_faces = 0usize;
        let mut root_failures = 0usize;
        let mut cursor: Option<String> = None;

        loop {
            let batch = pending_batch(
                &conn,
                detector_id,
                detector_version,
                cursor.as_deref(),
                options.batch_size,
            )?;
            if batch.is_empty() {
                break;
            }

            for item in batch {
                cursor = Some(item.relative_path.to_string_lossy().to_string());
                let absolute = match portable::absolute_source_path(root, &item.relative_path) {
                    Ok(path) => path,
                    Err(err) => {
                        root_failures += 1;
                        emit_failure(&mut emit, root, &item.relative_path, &err);
                        continue;
                    }
                };

                // Face state belongs to the indexed source revision. If the file
                // changed before the base watcher/indexer caught up, leave it
                // pending rather than committing geometry against stale metadata.
                if !filesystem_state_matches(&absolute, item.size, item.modified) {
                    root_failures += 1;
                    emit(FacePipelineEvent::ImageFailed {
                        root: root.clone(),
                        image: absolute,
                        error: "source state changed; base image index must update before face detection"
                            .to_owned(),
                    });
                    continue;
                }

                let result = (|| -> Result<usize> {
                    let (oriented, orientation) = face_detection::decode_oriented_with_orientation(&absolute)?;
                    let (width, height) = oriented.dimensions();
                    let detections = detector.detect(&oriented)?;
                    let stored = face_store::replace_detections(
                        &mut conn,
                        &item.relative_path,
                        item.size,
                        item.modified,
                        orientation,
                        width,
                        height,
                        detector_id,
                        detector_version,
                        &detections,
                    )?;
                    Ok(stored.len())
                })();

                match result {
                    Ok(face_count) => {
                        root_processed += 1;
                        root_faces += face_count;
                    }
                    Err(err) => {
                        root_failures += 1;
                        emit_failure(&mut emit, root, &absolute, &err);
                    }
                }

                emit(FacePipelineEvent::Progress {
                    root: root.clone(),
                    processed: root_processed,
                    pending,
                    faces: root_faces,
                    failures: root_failures,
                });
            }
        }

        summary.roots_processed += 1;
        summary.images_processed += root_processed;
        summary.faces_detected += root_faces;
        summary.failures += root_failures;
        emit(FacePipelineEvent::RootFinished {
            root: root.clone(),
            processed: root_processed,
            faces: root_faces,
            failures: root_failures,
        });
    }

    Ok(summary)
}

fn emit_failure<F>(emit: &mut F, root: &Path, image: &Path, err: &anyhow::Error)
where
    F: FnMut(FacePipelineEvent),
{
    emit(FacePipelineEvent::ImageFailed {
        root: root.to_path_buf(),
        image: image.to_path_buf(),
        error: format!("{err:#}"),
    });
}

fn count_pending(conn: &Connection, detector_id: &str, detector_version: &str) -> Result<usize> {
    let count = conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM images
        LEFT JOIN face_detection_state state ON state.image_path = images.path
        WHERE state.image_path IS NULL
           OR state.detector_id <> ?1
           OR state.detector_version <> ?2
           OR state.schema_version <> ?3
           OR state.source_size <> images.size
           OR state.source_modified <> images.modified
        "#,
        params![
            detector_id,
            detector_version,
            face_detection::SCHEMA_VERSION
        ],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count.max(0) as usize)
}

fn pending_batch(
    conn: &Connection,
    detector_id: &str,
    detector_version: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<PendingImage>> {
    let after = after.unwrap_or("");
    let mut stmt = conn.prepare(
        r#"
        SELECT images.path, images.size, images.modified
        FROM images
        LEFT JOIN face_detection_state state ON state.image_path = images.path
        WHERE images.path COLLATE NOCASE > ?4 COLLATE NOCASE
          AND (
               state.image_path IS NULL
            OR state.detector_id <> ?1
            OR state.detector_version <> ?2
            OR state.schema_version <> ?3
            OR state.source_size <> images.size
            OR state.source_modified <> images.modified
          )
        ORDER BY images.path COLLATE NOCASE
        LIMIT ?5
        "#,
    )?;
    let rows = stmt.query_map(
        params![
            detector_id,
            detector_version,
            face_detection::SCHEMA_VERSION,
            after,
            limit as i64,
        ],
        |row| {
            Ok(PendingImage {
                relative_path: PathBuf::from(row.get::<_, String>(0)?),
                size: row.get::<_, i64>(1)?.max(0) as u64,
                modified: row.get(2)?,
            })
        },
    )?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("loading bounded face-detection batch")
}

fn filesystem_state_matches(path: &Path, expected_size: u64, expected_modified: i64) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() != expected_size {
        return false;
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    modified == expected_modified
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face_detection::{DetectedFace, FaceBox};
    use image::{DynamicImage, ImageBuffer, Rgb};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct FakeDetector {
        version: &'static str,
        calls: usize,
    }

    impl face_detection::FaceDetector for FakeDetector {
        fn detector_id(&self) -> &'static str {
            "fake-detector"
        }

        fn detector_version(&self) -> &'static str {
            self.version
        }

        fn detect(&mut self, image: &DynamicImage) -> Result<Vec<DetectedFace>> {
            self.calls += 1;
            if image.width() == 13 {
                anyhow::bail!("synthetic detector failure");
            }
            if image.width() % 2 == 0 {
                Ok(vec![DetectedFace {
                    confidence: 0.9,
                    bbox: FaceBox {
                        x: 0.1,
                        y: 0.2,
                        width: 0.3,
                        height: 0.4,
                    },
                    landmarks: Vec::new(),
                }])
            } else {
                Ok(Vec::new())
            }
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "wis-face-pipeline-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn add_image(root: &Path, name: &str, width: u32) -> PathBuf {
        let source = root.join(name);
        let image = ImageBuffer::from_pixel(width, 9, Rgb([20u8, 30, 40]));
        image.save(&source).unwrap();
        let meta = std::fs::metadata(&source).unwrap();
        let modified = meta
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let portable_db = portable::index_db_path(root);
        let conn = db::open(&portable_db).unwrap();
        let relative = PathBuf::from(name);
        db::upsert_image(
            &conn,
            &relative,
            Path::new(""),
            name,
            "png",
            meta.len(),
            modified,
            width,
            9,
            "",
            "",
            [20, 30, 40],
            1,
            &[1.0],
        )
        .unwrap();
        relative
    }

    fn prepared_root(label: &str) -> PathBuf {
        let root = temp_root(label);
        std::fs::create_dir_all(&root).unwrap();
        let session = root.with_extension("session.sqlite3");
        portable::attach_root(&session, &root).unwrap();
        let _ = std::fs::remove_file(session);
        root
    }

    #[test]
    fn bounded_batches_persist_zero_and_one_face_then_skip_current() {
        let root = prepared_root("bounded");
        add_image(&root, "a.png", 10);
        add_image(&root, "b.png", 11);
        add_image(&root, "c.png", 12);
        let mut detector = FakeDetector {
            version: "1",
            calls: 0,
        };
        let first = run_available_roots(
            std::slice::from_ref(&root),
            &mut detector,
            FacePipelineOptions { batch_size: 1 },
            |_| {},
        )
        .unwrap();
        assert_eq!(first.images_processed, 3);
        assert_eq!(first.faces_detected, 2);
        assert_eq!(detector.calls, 3);

        let second = run_available_roots(
            std::slice::from_ref(&root),
            &mut detector,
            FacePipelineOptions { batch_size: 1 },
            |_| {},
        )
        .unwrap();
        assert_eq!(second.images_processed, 0);
        assert_eq!(detector.calls, 3);
        let conn = db::open(&portable::index_db_path(&root)).unwrap();
        assert_eq!(
            face_store::load_detection_state(&conn, Path::new("b.png"))
                .unwrap()
                .unwrap()
                .face_count,
            0
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn detector_version_change_backfills_current_images() {
        let root = prepared_root("version");
        add_image(&root, "a.png", 10);
        let mut v1 = FakeDetector {
            version: "1",
            calls: 0,
        };
        run_available_roots(&[root.clone()], &mut v1, FacePipelineOptions::default(), |_| {})
            .unwrap();
        let mut v2 = FakeDetector {
            version: "2",
            calls: 0,
        };
        let result = run_available_roots(
            &[root.clone()],
            &mut v2,
            FacePipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.images_processed, 1);
        assert_eq!(v2.calls, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn one_failed_image_does_not_block_later_images_or_mark_failure_current() {
        let root = prepared_root("failure");
        add_image(&root, "a.png", 13);
        add_image(&root, "b.png", 14);
        let mut detector = FakeDetector {
            version: "1",
            calls: 0,
        };
        let result = run_available_roots(
            &[root.clone()],
            &mut detector,
            FacePipelineOptions { batch_size: 1 },
            |_| {},
        )
        .unwrap();
        assert_eq!(result.images_processed, 1);
        assert_eq!(result.failures, 1);
        let conn = db::open(&portable::index_db_path(&root)).unwrap();
        assert!(face_store::load_detection_state(&conn, Path::new("a.png"))
            .unwrap()
            .is_none());
        assert!(face_store::load_detection_state(&conn, Path::new("b.png"))
            .unwrap()
            .is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unavailable_roots_are_reported_without_error() {
        let root = temp_root("missing");
        let mut detector = FakeDetector {
            version: "1",
            calls: 0,
        };
        let result = run_available_roots(
            &[root],
            &mut detector,
            FacePipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.roots_unavailable, 1);
        assert_eq!(detector.calls, 0);
    }
}
