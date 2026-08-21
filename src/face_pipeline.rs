use crate::{db, face_detection, face_scope, face_store, portable};
use anyhow::{Context, Result};
use image::GenericImageView;
use rusqlite::{params, Connection, OptionalExtension};
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
        eligible: usize,
    },
    Progress {
        root: PathBuf,
        visited: usize,
        eligible: usize,
        processed: usize,
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
        visited: usize,
        processed: usize,
        faces: usize,
        failures: usize,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FacePipelineSummary {
    pub roots_processed: usize,
    pub roots_unavailable: usize,
    pub images_eligible: usize,
    pub images_visited: usize,
    pub images_processed: usize,
    pub faces_detected: usize,
    pub failures: usize,
}

pub fn run_available_roots<D, F>(
    session_db_path: &Path,
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
    let detector_cache_revision = detector.cache_revision();
    let session_conn = db::open(session_db_path).with_context(|| {
        format!(
            "opening collection scope database {}",
            session_db_path.display()
        )
    })?;
    face_scope::ensure_schema_on(&session_conn)?;

    let mut summary = FacePipelineSummary::default();

    for root in roots {
        if !root.is_dir() || !portable::is_indexed_root(root) {
            summary.roots_unavailable += 1;
            emit(FacePipelineEvent::RootUnavailable { root: root.clone() });
            continue;
        }

        let eligible = face_scope::count_eligible_paths_on(&session_conn, root)?;
        summary.images_eligible += eligible;
        emit(FacePipelineEvent::RootStarted {
            root: root.clone(),
            eligible,
        });

        let db_path = portable::index_db_path(root);
        let mut conn = db::open(&db_path)
            .with_context(|| format!("opening portable face index {}", db_path.display()))?;
        face_store::ensure_schema(&conn)?;

        let mut root_visited = 0usize;
        let mut root_processed = 0usize;
        let mut root_faces = 0usize;
        let mut root_failures = 0usize;
        let mut cursor: Option<PathBuf> = None;

        loop {
            let batch = face_scope::eligible_batch_on(
                &session_conn,
                root,
                cursor.as_deref(),
                options.batch_size,
            )?;
            if batch.is_empty() {
                break;
            }

            for absolute in batch {
                cursor = Some(absolute.clone());
                root_visited += 1;

                let relative = match portable::relative_source_path(root, &absolute) {
                    Ok(path) => path,
                    Err(err) => {
                        root_failures += 1;
                        emit_failure(&mut emit, root, &absolute, &err);
                        emit_progress(
                            &mut emit,
                            root,
                            root_visited,
                            eligible,
                            root_processed,
                            root_faces,
                            root_failures,
                        );
                        continue;
                    }
                };

                let Some((size, modified)) = indexed_image_state(&conn, &relative)? else {
                    root_failures += 1;
                    emit(FacePipelineEvent::ImageFailed {
                        root: root.clone(),
                        image: absolute.clone(),
                        error: "collection member is missing from the portable root index"
                            .to_owned(),
                    });
                    emit_progress(
                        &mut emit,
                        root,
                        root_visited,
                        eligible,
                        root_processed,
                        root_faces,
                        root_failures,
                    );
                    continue;
                };

                if face_store::detection_is_current_with_revision(
                    &conn,
                    &relative,
                    size,
                    modified,
                    detector_id,
                    detector_version,
                    &detector_cache_revision,
                )? {
                    emit_progress(
                        &mut emit,
                        root,
                        root_visited,
                        eligible,
                        root_processed,
                        root_faces,
                        root_failures,
                    );
                    continue;
                }

                // Face state belongs to the indexed source revision. If the file
                // changed before the base watcher/indexer caught up, leave it
                // pending rather than committing geometry against stale metadata.
                if !filesystem_state_matches(&absolute, size, modified) {
                    root_failures += 1;
                    emit(FacePipelineEvent::ImageFailed {
                        root: root.clone(),
                        image: absolute.clone(),
                        error: "source state changed; base image index must update before face detection"
                            .to_owned(),
                    });
                    emit_progress(
                        &mut emit,
                        root,
                        root_visited,
                        eligible,
                        root_processed,
                        root_faces,
                        root_failures,
                    );
                    continue;
                }

                let result = (|| -> Result<usize> {
                    let (oriented, orientation) =
                        face_detection::decode_oriented_with_orientation(&absolute)?;
                    let (width, height) = oriented.dimensions();
                    let detections = detector.detect(&oriented)?;
                    let stored = face_store::replace_detections_with_revision(
                        &mut conn,
                        &relative,
                        size,
                        modified,
                        orientation,
                        width,
                        height,
                        detector_id,
                        detector_version,
                        &detector_cache_revision,
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

                emit_progress(
                    &mut emit,
                    root,
                    root_visited,
                    eligible,
                    root_processed,
                    root_faces,
                    root_failures,
                );
            }
        }

        summary.roots_processed += 1;
        summary.images_visited += root_visited;
        summary.images_processed += root_processed;
        summary.faces_detected += root_faces;
        summary.failures += root_failures;
        emit(FacePipelineEvent::RootFinished {
            root: root.clone(),
            visited: root_visited,
            processed: root_processed,
            faces: root_faces,
            failures: root_failures,
        });
    }

    Ok(summary)
}

fn emit_progress<F>(
    emit: &mut F,
    root: &Path,
    visited: usize,
    eligible: usize,
    processed: usize,
    faces: usize,
    failures: usize,
) where
    F: FnMut(FacePipelineEvent),
{
    emit(FacePipelineEvent::Progress {
        root: root.to_path_buf(),
        visited,
        eligible,
        processed,
        faces,
        failures,
    });
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

fn indexed_image_state(conn: &Connection, relative: &Path) -> Result<Option<(u64, i64)>> {
    conn.query_row(
        "SELECT size, modified FROM images WHERE path = ?1",
        params![relative.to_string_lossy().to_string()],
        |row| Ok((row.get::<_, i64>(0)?.max(0) as u64, row.get::<_, i64>(1)?)),
    )
    .optional()
    .context("loading portable image state for face detection")
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

    fn prepared_library(label: &str) -> (PathBuf, PathBuf) {
        let root = temp_root(label);
        std::fs::create_dir_all(&root).unwrap();
        let session = root.with_extension("session.sqlite3");
        portable::attach_root(&session, &root).unwrap();
        face_scope::ensure_schema(&session).unwrap();
        (root, session)
    }

    fn add_image(root: &Path, session: &Path, name: &str, width: u32) -> PathBuf {
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

        let relative = PathBuf::from(name);
        let portable_conn = db::open(&portable::index_db_path(root)).unwrap();
        db::upsert_image(
            &portable_conn,
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

        let session_conn = db::open(session).unwrap();
        db::upsert_image(
            &session_conn,
            &source,
            root,
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
        source
    }

    fn add_collection_folder(session: &Path, name: &str, folder: &Path, enabled: bool) -> i64 {
        let collection = db::create_collection(session, name).unwrap();
        db::add_collection_folders(session, collection.id, &[folder.to_path_buf()]).unwrap();
        face_scope::set_collection_enabled(session, collection.id, enabled).unwrap();
        collection.id
    }

    fn cleanup(root: &Path, session: &Path) {
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(session);
        let _ = std::fs::remove_file(session.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(session.with_extension("sqlite3-shm"));
    }

    #[test]
    fn bounded_batches_persist_zero_and_one_face_then_skip_current() {
        let (root, session) = prepared_library("bounded");
        add_image(&root, &session, "a.png", 10);
        add_image(&root, &session, "b.png", 11);
        add_image(&root, &session, "c.png", 12);
        add_collection_folder(&session, "People", &root, true);

        let mut detector = FakeDetector {
            version: "1",
            calls: 0,
        };
        let first = run_available_roots(
            &session,
            std::slice::from_ref(&root),
            &mut detector,
            FacePipelineOptions { batch_size: 1 },
            |_| {},
        )
        .unwrap();
        assert_eq!(first.images_eligible, 3);
        assert_eq!(first.images_processed, 3);
        assert_eq!(first.faces_detected, 2);
        assert_eq!(detector.calls, 3);

        let second = run_available_roots(
            &session,
            std::slice::from_ref(&root),
            &mut detector,
            FacePipelineOptions { batch_size: 1 },
            |_| {},
        )
        .unwrap();
        assert_eq!(second.images_eligible, 3);
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
        cleanup(&root, &session);
    }

    #[test]
    fn disabled_texture_collection_never_calls_detector() {
        let (root, session) = prepared_library("disabled");
        add_image(&root, &session, "tile-a.png", 10);
        add_image(&root, &session, "tile-b.png", 12);
        add_collection_folder(&session, "Textures", &root, false);

        let mut detector = FakeDetector {
            version: "1",
            calls: 0,
        };
        let result = run_available_roots(
            &session,
            std::slice::from_ref(&root),
            &mut detector,
            FacePipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.images_eligible, 0);
        assert_eq!(result.images_processed, 0);
        assert_eq!(detector.calls, 0);
        cleanup(&root, &session);
    }

    #[test]
    fn overlapping_collections_use_any_enabled_membership() {
        let (root, session) = prepared_library("overlap");
        let person = add_image(&root, &session, "person.png", 10);
        add_image(&root, &session, "texture.png", 12);
        add_collection_folder(&session, "Textures", &root, false);

        let people = db::create_collection(&session, "People").unwrap();
        db::add_collection_files(&session, people.id, std::slice::from_ref(&person)).unwrap();
        face_scope::set_collection_enabled(&session, people.id, true).unwrap();

        let mut detector = FakeDetector {
            version: "1",
            calls: 0,
        };
        let result = run_available_roots(
            &session,
            std::slice::from_ref(&root),
            &mut detector,
            FacePipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.images_eligible, 1);
        assert_eq!(result.images_processed, 1);
        assert_eq!(detector.calls, 1);
        let conn = db::open(&portable::index_db_path(&root)).unwrap();
        assert!(
            face_store::load_detection_state(&conn, Path::new("person.png"))
                .unwrap()
                .is_some()
        );
        assert!(
            face_store::load_detection_state(&conn, Path::new("texture.png"))
                .unwrap()
                .is_none()
        );
        cleanup(&root, &session);
    }

    #[test]
    fn disabling_then_reenabling_reuses_current_face_state() {
        let (root, session) = prepared_library("toggle");
        add_image(&root, &session, "person.png", 10);
        let collection_id = add_collection_folder(&session, "People", &root, true);

        let mut detector = FakeDetector {
            version: "1",
            calls: 0,
        };
        run_available_roots(
            &session,
            std::slice::from_ref(&root),
            &mut detector,
            FacePipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(detector.calls, 1);

        face_scope::set_collection_enabled(&session, collection_id, false).unwrap();
        let disabled = run_available_roots(
            &session,
            std::slice::from_ref(&root),
            &mut detector,
            FacePipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(disabled.images_eligible, 0);
        assert_eq!(detector.calls, 1);

        face_scope::set_collection_enabled(&session, collection_id, true).unwrap();
        let enabled_again = run_available_roots(
            &session,
            std::slice::from_ref(&root),
            &mut detector,
            FacePipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(enabled_again.images_eligible, 1);
        assert_eq!(enabled_again.images_processed, 0);
        assert_eq!(detector.calls, 1);
        cleanup(&root, &session);
    }

    #[test]
    fn detector_version_change_backfills_only_enabled_images() {
        let (root, session) = prepared_library("version");
        add_image(&root, &session, "person.png", 10);
        add_image(&root, &session, "texture.png", 12);
        let people = db::create_collection(&session, "People").unwrap();
        db::add_collection_files(&session, people.id, &[root.join("person.png")]).unwrap();
        face_scope::set_collection_enabled(&session, people.id, true).unwrap();
        add_collection_folder(
            &session,
            "Textures",
            &root.join("texture-folder-never-matches"),
            false,
        );

        let mut v1 = FakeDetector {
            version: "1",
            calls: 0,
        };
        run_available_roots(
            &session,
            std::slice::from_ref(&root),
            &mut v1,
            FacePipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(v1.calls, 1);

        let mut v2 = FakeDetector {
            version: "2",
            calls: 0,
        };
        let result = run_available_roots(
            &session,
            std::slice::from_ref(&root),
            &mut v2,
            FacePipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.images_eligible, 1);
        assert_eq!(result.images_processed, 1);
        assert_eq!(v2.calls, 1);
        cleanup(&root, &session);
    }

    #[test]
    fn one_failed_image_does_not_block_later_enabled_images() {
        let (root, session) = prepared_library("failure");
        add_image(&root, &session, "a.png", 13);
        add_image(&root, &session, "b.png", 14);
        add_collection_folder(&session, "People", &root, true);
        let mut detector = FakeDetector {
            version: "1",
            calls: 0,
        };
        let result = run_available_roots(
            &session,
            std::slice::from_ref(&root),
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
        cleanup(&root, &session);
    }

    #[test]
    fn unavailable_roots_are_reported_without_touching_scope() {
        let root = temp_root("missing");
        let session = root.with_extension("session.sqlite3");
        face_scope::ensure_schema(&session).unwrap();
        let mut detector = FakeDetector {
            version: "1",
            calls: 0,
        };
        let result = run_available_roots(
            &session,
            &[root],
            &mut detector,
            FacePipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.roots_unavailable, 1);
        assert_eq!(detector.calls, 0);
        let _ = std::fs::remove_file(session);
    }
}
