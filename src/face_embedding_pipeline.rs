use crate::{db, face_embedding, face_embedding_store, face_store, portable};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const DEFAULT_BATCH_SIZE: usize = 32;
const MAX_BATCH_SIZE: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaceEmbeddingPipelineOptions {
    pub batch_size: usize,
}

impl Default for FaceEmbeddingPipelineOptions {
    fn default() -> Self {
        Self {
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }
}

impl FaceEmbeddingPipelineOptions {
    pub fn sanitized(self) -> Self {
        Self {
            batch_size: self.batch_size.clamp(1, MAX_BATCH_SIZE),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaceEmbeddingPipelineEvent {
    RootStarted {
        root: PathBuf,
        pending: usize,
    },
    Progress {
        root: PathBuf,
        visited: usize,
        pending: usize,
        embedded: usize,
        failures: usize,
    },
    FaceFailed {
        root: PathBuf,
        face_id: String,
        image: PathBuf,
        error: String,
    },
    RootUnavailable {
        root: PathBuf,
    },
    RootFinished {
        root: PathBuf,
        visited: usize,
        embedded: usize,
        failures: usize,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FaceEmbeddingPipelineSummary {
    pub roots_processed: usize,
    pub roots_unavailable: usize,
    pub faces_pending: usize,
    pub faces_visited: usize,
    pub faces_embedded: usize,
    pub failures: usize,
}

pub fn run_available_roots<E, F>(
    roots: &[PathBuf],
    embedder: &mut E,
    options: FaceEmbeddingPipelineOptions,
    mut emit: F,
) -> Result<FaceEmbeddingPipelineSummary>
where
    E: face_embedding::FaceEmbedder,
    F: FnMut(FaceEmbeddingPipelineEvent),
{
    let options = options.sanitized();
    let model_id = embedder.model_id();
    let model_version = embedder.model_version();
    let dimension = embedder.embedding_dimension();
    let input_size = embedder.input_size();
    if dimension == 0 {
        anyhow::bail!("face embedder dimension must be non-zero");
    }
    if input_size == 0 {
        anyhow::bail!("face embedder input size must be non-zero");
    }

    let mut summary = FaceEmbeddingPipelineSummary::default();
    for root in roots {
        if !root.is_dir() || !portable::is_indexed_root(root) {
            summary.roots_unavailable += 1;
            emit(FaceEmbeddingPipelineEvent::RootUnavailable { root: root.clone() });
            continue;
        }

        let db_path = portable::index_db_path(root);
        let mut conn = db::open(&db_path)
            .with_context(|| format!("opening portable face embedding index {}", db_path.display()))?;
        face_store::ensure_schema(&conn)?;
        face_embedding_store::ensure_schema(&conn)?;
        let pending = face_embedding_store::count_pending(
            &conn,
            model_id,
            model_version,
            dimension,
            face_embedding::ALIGNMENT_REVISION,
        )?;
        summary.faces_pending += pending;
        emit(FaceEmbeddingPipelineEvent::RootStarted {
            root: root.clone(),
            pending,
        });

        let mut cursor: Option<String> = None;
        let mut root_visited = 0usize;
        let mut root_embedded = 0usize;
        let mut root_failures = 0usize;

        loop {
            let batch = face_embedding_store::candidate_batch(
                &conn,
                cursor.as_deref(),
                model_id,
                model_version,
                dimension,
                face_embedding::ALIGNMENT_REVISION,
                options.batch_size,
            )?;
            if batch.is_empty() {
                break;
            }

            for candidate in batch {
                cursor = Some(candidate.face_id.clone());
                root_visited += 1;
                let absolute = root.join(&candidate.image_path);

                if !filesystem_state_matches(
                    &absolute,
                    candidate.source_size,
                    candidate.source_modified,
                ) {
                    root_failures += 1;
                    emit(FaceEmbeddingPipelineEvent::FaceFailed {
                        root: root.clone(),
                        face_id: candidate.face_id.clone(),
                        image: absolute.clone(),
                        error: "source state changed; base/face detection must update before face embedding"
                            .to_owned(),
                    });
                    emit_progress(
                        &mut emit,
                        root,
                        root_visited,
                        pending,
                        root_embedded,
                        root_failures,
                    );
                    continue;
                }

                let result = (|| -> Result<()> {
                    let oriented = crate::face_detection::decode_oriented(&absolute)?;
                    let aligned = face_embedding::aligned_face_crop(
                        &oriented,
                        candidate.bbox,
                        &candidate.landmarks,
                        input_size,
                    )?;
                    let raw = embedder.embed(&aligned)?;
                    let normalized = face_embedding::normalize_embedding(raw, dimension)?;
                    face_embedding_store::replace_embedding(
                        &mut conn,
                        &candidate,
                        model_id,
                        model_version,
                        face_embedding::ALIGNMENT_REVISION,
                        &normalized,
                    )?;
                    Ok(())
                })();

                match result {
                    Ok(()) => root_embedded += 1,
                    Err(err) => {
                        root_failures += 1;
                        emit(FaceEmbeddingPipelineEvent::FaceFailed {
                            root: root.clone(),
                            face_id: candidate.face_id.clone(),
                            image: absolute,
                            error: format!("{err:#}"),
                        });
                    }
                }
                emit_progress(
                    &mut emit,
                    root,
                    root_visited,
                    pending,
                    root_embedded,
                    root_failures,
                );
            }
        }

        summary.roots_processed += 1;
        summary.faces_visited += root_visited;
        summary.faces_embedded += root_embedded;
        summary.failures += root_failures;
        emit(FaceEmbeddingPipelineEvent::RootFinished {
            root: root.clone(),
            visited: root_visited,
            embedded: root_embedded,
            failures: root_failures,
        });
    }
    Ok(summary)
}

fn emit_progress<F>(
    emit: &mut F,
    root: &Path,
    visited: usize,
    pending: usize,
    embedded: usize,
    failures: usize,
) where
    F: FnMut(FaceEmbeddingPipelineEvent),
{
    emit(FaceEmbeddingPipelineEvent::Progress {
        root: root.to_path_buf(),
        visited,
        pending,
        embedded,
        failures,
    });
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
    use crate::face_detection::{DetectedFace, FaceBox, FaceLandmark};
    use image::{DynamicImage, ImageBuffer, Rgb};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct FakeEmbedder {
        version: &'static str,
        calls: usize,
        fail_on_call: Option<usize>,
    }

    impl face_embedding::FaceEmbedder for FakeEmbedder {
        fn model_id(&self) -> &'static str {
            "fake-face-embedder"
        }

        fn model_version(&self) -> &'static str {
            self.version
        }

        fn input_size(&self) -> u32 {
            64
        }

        fn embedding_dimension(&self) -> usize {
            4
        }

        fn embed(&mut self, aligned_face: &DynamicImage) -> Result<Vec<f32>> {
            self.calls += 1;
            if self.fail_on_call == Some(self.calls) {
                anyhow::bail!("synthetic embedding failure");
            }
            assert_eq!(aligned_face.width(), 64);
            assert_eq!(aligned_face.height(), 64);
            Ok(vec![1.0, self.calls as f32, 0.5, 0.25])
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "wis-face-embedding-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn prepared_library(label: &str) -> (PathBuf, PathBuf) {
        let root = temp_root(label);
        std::fs::create_dir_all(&root).unwrap();
        let session = root.with_extension("session.sqlite3");
        portable::attach_root(&session, &root).unwrap();
        (root, session)
    }

    fn add_detected_faces(root: &Path, name: &str, face_count: usize) -> Vec<String> {
        let source = root.join(name);
        let image = ImageBuffer::from_pixel(120, 100, Rgb([20u8, 30, 40]));
        image.save(&source).unwrap();
        let meta = std::fs::metadata(&source).unwrap();
        let modified = meta
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let relative = PathBuf::from(name);
        let mut conn = db::open(&portable::index_db_path(root)).unwrap();
        db::upsert_image(
            &conn,
            &relative,
            Path::new(""),
            name,
            "png",
            meta.len(),
            modified,
            120,
            100,
            "",
            "",
            [20, 30, 40],
            1,
            &[1.0],
        )
        .unwrap();
        face_store::ensure_schema(&conn).unwrap();
        let detections: Vec<DetectedFace> = (0..face_count)
            .map(|index| DetectedFace {
                confidence: 0.95,
                bbox: FaceBox {
                    x: 0.08 + index as f32 * 0.35,
                    y: 0.15,
                    width: 0.25,
                    height: 0.35,
                },
                landmarks: vec![FaceLandmark {
                    x: 0.15 + index as f32 * 0.35,
                    y: 0.25,
                }],
            })
            .collect();
        face_store::replace_detections(
            &mut conn,
            &relative,
            meta.len(),
            modified,
            1,
            120,
            100,
            "fake-detector",
            "1",
            &detections,
        )
        .unwrap()
        .into_iter()
        .map(|face| face.face_id)
        .collect()
    }

    fn cleanup(root: &Path, session: &Path) {
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(session);
        let _ = std::fs::remove_file(session.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(session.with_extension("sqlite3-shm"));
    }

    #[test]
    fn bounded_backfill_persists_then_skips_current_faces() {
        let (root, session) = prepared_library("bounded");
        let face_ids = add_detected_faces(&root, "people.png", 2);
        let mut embedder = FakeEmbedder {
            version: "1",
            calls: 0,
            fail_on_call: None,
        };
        let first = run_available_roots(
            std::slice::from_ref(&root),
            &mut embedder,
            FaceEmbeddingPipelineOptions { batch_size: 1 },
            |_| {},
        )
        .unwrap();
        assert_eq!(first.faces_pending, 2);
        assert_eq!(first.faces_embedded, 2);
        assert_eq!(embedder.calls, 2);

        let second = run_available_roots(
            std::slice::from_ref(&root),
            &mut embedder,
            FaceEmbeddingPipelineOptions { batch_size: 1 },
            |_| {},
        )
        .unwrap();
        assert_eq!(second.faces_pending, 0);
        assert_eq!(second.faces_embedded, 0);
        assert_eq!(embedder.calls, 2);

        let conn = db::open(&portable::index_db_path(&root)).unwrap();
        for face_id in face_ids {
            let stored = face_embedding_store::load_embedding(&conn, &face_id)
                .unwrap()
                .unwrap();
            assert_eq!(stored.dimension, 4);
            assert!(stored.normalized);
        }
        cleanup(&root, &session);
    }

    #[test]
    fn model_revision_backfills_existing_faces() {
        let (root, session) = prepared_library("revision");
        add_detected_faces(&root, "person.png", 1);
        let mut v1 = FakeEmbedder {
            version: "1",
            calls: 0,
            fail_on_call: None,
        };
        run_available_roots(
            std::slice::from_ref(&root),
            &mut v1,
            FaceEmbeddingPipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        let mut v2 = FakeEmbedder {
            version: "2",
            calls: 0,
            fail_on_call: None,
        };
        let result = run_available_roots(
            std::slice::from_ref(&root),
            &mut v2,
            FaceEmbeddingPipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.faces_pending, 1);
        assert_eq!(result.faces_embedded, 1);
        assert_eq!(v2.calls, 1);
        cleanup(&root, &session);
    }

    #[test]
    fn one_failure_does_not_block_later_faces_and_retries_next_run() {
        let (root, session) = prepared_library("retry");
        add_detected_faces(&root, "people.png", 2);
        let mut flaky = FakeEmbedder {
            version: "1",
            calls: 0,
            fail_on_call: Some(1),
        };
        let first = run_available_roots(
            std::slice::from_ref(&root),
            &mut flaky,
            FaceEmbeddingPipelineOptions { batch_size: 1 },
            |_| {},
        )
        .unwrap();
        assert_eq!(first.faces_embedded, 1);
        assert_eq!(first.failures, 1);

        let mut retry = FakeEmbedder {
            version: "1",
            calls: 0,
            fail_on_call: None,
        };
        let second = run_available_roots(
            std::slice::from_ref(&root),
            &mut retry,
            FaceEmbeddingPipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(second.faces_pending, 1);
        assert_eq!(second.faces_embedded, 1);
        assert_eq!(retry.calls, 1);
        cleanup(&root, &session);
    }

    #[test]
    fn unavailable_root_is_skipped_without_error() {
        let root = temp_root("unavailable");
        let mut embedder = FakeEmbedder {
            version: "1",
            calls: 0,
            fail_on_call: None,
        };
        let result = run_available_roots(
            std::slice::from_ref(&root),
            &mut embedder,
            FaceEmbeddingPipelineOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(result.roots_unavailable, 1);
        assert_eq!(embedder.calls, 0);
    }
}
