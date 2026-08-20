use crate::face_detection::{FaceBox, FaceLandmark};
use crate::{face_embedding, portable};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 5_000;
const READ_BATCH_SIZE: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceEmbeddingRevision {
    pub model_id: String,
    pub model_version: String,
    pub schema_version: i64,
    pub alignment_revision: i64,
    pub dimension: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FaceSimilarityQuery {
    pub root: PathBuf,
    pub library_id: String,
    pub face_id: String,
    pub relative_image_path: PathBuf,
    pub revision: FaceEmbeddingRevision,
    pub values: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaceSimilarityOptions {
    pub limit: usize,
    pub collapse_same_image: bool,
}

impl Default for FaceSimilarityOptions {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            collapse_same_image: true,
        }
    }
}

impl FaceSimilarityOptions {
    fn sanitized(self) -> Self {
        Self {
            limit: self.limit.clamp(1, MAX_LIMIT),
            collapse_same_image: self.collapse_same_image,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FaceSimilarityMatch {
    pub root: PathBuf,
    pub library_id: String,
    pub face_id: String,
    pub image_path: PathBuf,
    pub relative_image_path: PathBuf,
    pub bbox: FaceBox,
    pub landmarks: Vec<FaceLandmark>,
    pub similarity: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceSimilaritySearchReport {
    pub roots_searched: usize,
    pub roots_unavailable: usize,
    pub rows_considered: usize,
    pub invalid_embeddings_skipped: usize,
    pub matches: Vec<FaceSimilarityMatch>,
}

#[derive(Clone, Debug)]
struct SearchRow {
    face_id: String,
    image_path: PathBuf,
    bbox: FaceBox,
    landmarks: Vec<FaceLandmark>,
    values: Vec<f32>,
}

pub fn load_query(root: &Path, face_id: &str) -> Result<FaceSimilarityQuery> {
    if face_id.trim().is_empty() {
        bail!("query face id cannot be empty");
    }
    let conn = open_read_only_root(root)?;
    let library_id = library_id(&conn)?;
    let row = conn
        .query_row(
            r#"
            SELECT f.image_path,
                   e.model_id, e.model_version, e.schema_version,
                   e.alignment_revision, e.dimension, e.embedding
            FROM face_embeddings e
            JOIN faces f ON f.face_id = e.face_id
            JOIN face_detection_state s ON s.image_path = f.image_path
            JOIN images i ON i.path = f.image_path
            WHERE e.face_id = ?1
              AND e.schema_version = ?2
              AND e.normalized = 1
              AND e.detector_id = f.detector_id
              AND e.detector_version = f.detector_version
              AND e.detection_schema_version = f.schema_version
              AND e.source_size = f.source_size
              AND e.source_modified = f.source_modified
              AND s.detector_id = f.detector_id
              AND s.detector_version = f.detector_version
              AND s.schema_version = f.schema_version
              AND s.source_size = f.source_size
              AND s.source_modified = f.source_modified
              AND i.size = f.source_size
              AND i.modified = f.source_modified
            LIMIT 1
            "#,
            params![face_id, face_embedding::SCHEMA_VERSION],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Vec<u8>>(6)?,
                ))
            },
        )
        .optional()?;
    let (image_path, model_id, model_version, schema_version, alignment_revision, dimension, blob) =
        row.context("query face has no current compatible embedding")?;
    let dimension =
        usize::try_from(dimension).context("query face embedding dimension is invalid")?;
    let values =
        decode_embedding(&blob, dimension).context("query face embedding blob is corrupt")?;
    validate_normalized_embedding(&values)?;

    Ok(FaceSimilarityQuery {
        root: root.to_path_buf(),
        library_id,
        face_id: face_id.to_owned(),
        relative_image_path: PathBuf::from(image_path),
        revision: FaceEmbeddingRevision {
            model_id,
            model_version,
            schema_version,
            alignment_revision,
            dimension,
        },
        values,
    })
}

pub fn search_available_roots(
    roots: &[PathBuf],
    query: &FaceSimilarityQuery,
    options: FaceSimilarityOptions,
) -> Result<FaceSimilaritySearchReport> {
    validate_query(query)?;
    let options = options.sanitized();
    let mut report = FaceSimilaritySearchReport::default();

    for root in roots {
        let conn = match open_read_only_root(root) {
            Ok(conn) => conn,
            Err(_) => {
                report.roots_unavailable += 1;
                continue;
            }
        };
        let current_library_id = match library_id(&conn) {
            Ok(value) => value,
            Err(_) => {
                report.roots_unavailable += 1;
                continue;
            }
        };
        report.roots_searched += 1;
        let mut cursor: Option<String> = None;

        loop {
            let batch =
                load_compatible_batch(&conn, cursor.as_deref(), &query.revision, READ_BATCH_SIZE)?;
            if batch.is_empty() {
                break;
            }

            for row in batch {
                cursor = Some(row.face_id.clone());
                report.rows_considered += 1;
                if current_library_id == query.library_id && row.face_id == query.face_id {
                    continue;
                }
                if validate_normalized_embedding(&row.values).is_err() {
                    report.invalid_embeddings_skipped += 1;
                    continue;
                }
                let similarity =
                    face_embedding::cosine_similarity_normalized(&query.values, &row.values)?
                        .clamp(-1.0, 1.0);
                let absolute = match portable::absolute_source_path(root, &row.image_path) {
                    Ok(path) => path,
                    Err(_) => continue,
                };
                let candidate = FaceSimilarityMatch {
                    root: root.clone(),
                    library_id: current_library_id.clone(),
                    face_id: row.face_id,
                    image_path: absolute,
                    relative_image_path: row.image_path,
                    bbox: row.bbox,
                    landmarks: row.landmarks,
                    similarity,
                };
                insert_bounded_match(&mut report.matches, candidate, options);
            }
        }
    }

    report.matches.sort_by(compare_match);
    Ok(report)
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
            "opening portable face index read-only {}",
            db_path.display()
        )
    })
}

fn library_id(conn: &Connection) -> Result<String> {
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

fn load_compatible_batch(
    conn: &Connection,
    after_face_id: Option<&str>,
    revision: &FaceEmbeddingRevision,
    limit: usize,
) -> Result<Vec<SearchRow>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT f.face_id, f.image_path,
               f.bbox_x, f.bbox_y, f.bbox_width, f.bbox_height,
               f.landmarks, f.landmark_count, e.embedding
        FROM face_embeddings e
        JOIN faces f ON f.face_id = e.face_id
        JOIN face_detection_state s ON s.image_path = f.image_path
        JOIN images i ON i.path = f.image_path
        WHERE (?1 IS NULL OR f.face_id > ?1)
          AND e.model_id = ?2
          AND e.model_version = ?3
          AND e.schema_version = ?4
          AND e.alignment_revision = ?5
          AND e.dimension = ?6
          AND e.normalized = 1
          AND e.detector_id = f.detector_id
          AND e.detector_version = f.detector_version
          AND e.detection_schema_version = f.schema_version
          AND e.source_size = f.source_size
          AND e.source_modified = f.source_modified
          AND s.detector_id = f.detector_id
          AND s.detector_version = f.detector_version
          AND s.schema_version = f.schema_version
          AND s.source_size = f.source_size
          AND s.source_modified = f.source_modified
          AND i.size = f.source_size
          AND i.modified = f.source_modified
        ORDER BY f.face_id
        LIMIT ?7
        "#,
    )?;
    let rows = stmt.query_map(
        params![
            after_face_id,
            revision.model_id,
            revision.model_version,
            revision.schema_version,
            revision.alignment_revision,
            revision.dimension as i64,
            limit.max(1) as i64,
        ],
        |row| {
            let landmark_blob: Option<Vec<u8>> = row.get(6)?;
            let landmark_count = row.get::<_, i64>(7)?.max(0) as usize;
            let blob: Vec<u8> = row.get(8)?;
            Ok(SearchRow {
                face_id: row.get(0)?,
                image_path: PathBuf::from(row.get::<_, String>(1)?),
                bbox: FaceBox {
                    x: row.get(2)?,
                    y: row.get(3)?,
                    width: row.get(4)?,
                    height: row.get(5)?,
                },
                landmarks: landmark_blob
                    .as_deref()
                    .and_then(|bytes| decode_landmarks(bytes, landmark_count))
                    .unwrap_or_default(),
                values: decode_embedding(&blob, revision.dimension).unwrap_or_default(),
            })
        },
    )?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("loading compatible face embeddings")
}

fn validate_query(query: &FaceSimilarityQuery) -> Result<()> {
    if query.face_id.trim().is_empty() || query.library_id.trim().is_empty() {
        bail!("face similarity query must have a face id and library id");
    }
    if query.revision.model_id.trim().is_empty()
        || query.revision.model_version.trim().is_empty()
        || query.revision.schema_version != face_embedding::SCHEMA_VERSION
        || query.revision.alignment_revision <= 0
        || query.revision.dimension == 0
        || query.values.len() != query.revision.dimension
    {
        bail!("face similarity query has an invalid embedding revision");
    }
    validate_normalized_embedding(&query.values)
}

fn validate_normalized_embedding(values: &[f32]) -> Result<()> {
    if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
        bail!("face embedding must be finite and non-empty");
    }
    let norm_sq = values
        .iter()
        .map(|value| (*value as f64) * (*value as f64))
        .sum::<f64>();
    if (norm_sq.sqrt() - 1.0).abs() > 1e-3 {
        bail!("face embedding is not L2-normalized");
    }
    Ok(())
}

fn insert_bounded_match(
    matches: &mut Vec<FaceSimilarityMatch>,
    candidate: FaceSimilarityMatch,
    options: FaceSimilarityOptions,
) {
    if options.collapse_same_image {
        if let Some(index) = matches.iter().position(|existing| {
            existing.library_id == candidate.library_id
                && existing.relative_image_path == candidate.relative_image_path
        }) {
            if compare_match(&candidate, &matches[index]) == Ordering::Less {
                matches[index] = candidate;
            }
            return;
        }
    }

    if matches.len() < options.limit {
        matches.push(candidate);
        return;
    }
    let Some((worst_index, worst)) = matches
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| compare_match(left, right))
    else {
        return;
    };
    if compare_match(&candidate, worst) == Ordering::Less {
        matches[worst_index] = candidate;
    }
}

fn compare_match(left: &FaceSimilarityMatch, right: &FaceSimilarityMatch) -> Ordering {
    right
        .similarity
        .total_cmp(&left.similarity)
        .then_with(|| left.library_id.cmp(&right.library_id))
        .then_with(|| {
            left.relative_image_path
                .to_string_lossy()
                .cmp(&right.relative_image_path.to_string_lossy())
        })
        .then_with(|| left.face_id.cmp(&right.face_id))
}

fn decode_embedding(bytes: &[u8], dimension: usize) -> Option<Vec<f32>> {
    if bytes.len() != dimension.checked_mul(4)? {
        return None;
    }
    Some(
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect(),
    )
}

fn decode_landmarks(bytes: &[u8], count: usize) -> Option<Vec<FaceLandmark>> {
    if bytes.len() != count.checked_mul(8)? {
        return None;
    }
    Some(
        bytes
            .chunks_exact(8)
            .map(|chunk| FaceLandmark {
                x: f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
                y: f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face_detection::{DetectedFace, FaceBox};
    use crate::{db, face_embedding_store, face_store};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "wis-face-search-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn setup_root(label: &str, library: &str) -> PathBuf {
        let root = temp_root(label);
        std::fs::create_dir_all(portable::index_dir(&root)).unwrap();
        let conn = db::open(&portable::index_db_path(&root)).unwrap();
        face_store::ensure_schema(&conn).unwrap();
        face_embedding_store::ensure_schema(&conn).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS portable_meta(key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO portable_meta(key, value) VALUES('library_id', ?1)",
            params![library],
        )
        .unwrap();
        root
    }

    fn add_image_faces(
        root: &Path,
        image_path: &str,
        vectors: &[Vec<f32>],
        model_id: &str,
    ) -> Vec<String> {
        let mut conn = db::open(&portable::index_db_path(root)).unwrap();
        conn.execute(
            r#"INSERT INTO images(
                path, root, file_name, extension, size, modified, width, height,
                description, keywords, dominant_r, dominant_g, dominant_b
            ) VALUES(?1, '', ?2, 'jpg', 100, 200, 100, 100, '', '', 0, 0, 0)"#,
            params![image_path, image_path],
        )
        .unwrap();
        let detections: Vec<DetectedFace> = vectors
            .iter()
            .enumerate()
            .map(|(index, _)| DetectedFace {
                confidence: 0.99 - index as f32 * 0.01,
                bbox: FaceBox {
                    x: 0.05 + index as f32 * 0.2,
                    y: 0.1,
                    width: 0.15,
                    height: 0.2,
                },
                landmarks: vec![FaceLandmark {
                    x: 0.1 + index as f32 * 0.2,
                    y: 0.15,
                }],
            })
            .collect();
        let stored = face_store::replace_detections(
            &mut conn,
            Path::new(image_path),
            100,
            200,
            1,
            100,
            100,
            "detector",
            "1",
            &detections,
        )
        .unwrap();
        let candidates =
            face_embedding_store::candidate_batch(&conn, None, model_id, "1", 2, 1, 32).unwrap();
        for (face, vector) in stored.iter().zip(vectors.iter()) {
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.face_id == face.face_id)
                .unwrap();
            let normalized = face_embedding::normalize_embedding(vector.clone(), 2).unwrap();
            face_embedding_store::replace_embedding(
                &mut conn,
                candidate,
                model_id,
                "1",
                1,
                &normalized,
            )
            .unwrap();
        }
        stored.into_iter().map(|face| face.face_id).collect()
    }

    #[test]
    fn cross_root_search_excludes_self_collapses_images_and_skips_incompatible_roots() {
        let query_root = setup_root("query", "lib-query");
        let people_root = setup_root("people", "lib-people");
        let wrong_root = setup_root("wrong", "lib-wrong");
        let missing_root = temp_root("missing");

        let query_face =
            add_image_faces(&query_root, "query.jpg", &[vec![1.0, 0.0]], "embedder").remove(0);
        add_image_faces(
            &people_root,
            "person.jpg",
            &[vec![0.99, 0.01], vec![0.8, 0.2]],
            "embedder",
        );
        add_image_faces(&people_root, "far.jpg", &[vec![0.0, 1.0]], "embedder");
        add_image_faces(&wrong_root, "wrong.jpg", &[vec![1.0, 0.0]], "other-model");

        let query = load_query(&query_root, &query_face).unwrap();
        let report = search_available_roots(
            &[
                query_root.clone(),
                people_root.clone(),
                wrong_root.clone(),
                missing_root,
            ],
            &query,
            FaceSimilarityOptions {
                limit: 10,
                collapse_same_image: true,
            },
        )
        .unwrap();

        assert_eq!(report.roots_searched, 3);
        assert_eq!(report.roots_unavailable, 1);
        assert_eq!(report.matches.len(), 2);
        assert_eq!(
            report.matches[0].relative_image_path,
            PathBuf::from("person.jpg")
        );
        assert!(report.matches[0].similarity > report.matches[1].similarity);
        assert!(report.matches.iter().all(|item| item.face_id != query_face));

        let _ = std::fs::remove_dir_all(query_root);
        let _ = std::fs::remove_dir_all(people_root);
        let _ = std::fs::remove_dir_all(wrong_root);
    }

    #[test]
    fn stale_detection_revision_is_excluded() {
        let query_root = setup_root("stale-query", "lib-a");
        let candidate_root = setup_root("stale-candidate", "lib-b");
        let query_face =
            add_image_faces(&query_root, "query.jpg", &[vec![1.0, 0.0]], "embedder").remove(0);
        let candidate_face = add_image_faces(
            &candidate_root,
            "candidate.jpg",
            &[vec![1.0, 0.0]],
            "embedder",
        )
        .remove(0);
        let conn = db::open(&portable::index_db_path(&candidate_root)).unwrap();
        conn.execute(
            "UPDATE faces SET detector_version = '2' WHERE face_id = ?1",
            params![candidate_face],
        )
        .unwrap();

        let query = load_query(&query_root, &query_face).unwrap();
        let report = search_available_roots(
            &[candidate_root.clone()],
            &query,
            FaceSimilarityOptions::default(),
        )
        .unwrap();
        assert!(report.matches.is_empty());

        let _ = std::fs::remove_dir_all(query_root);
        let _ = std::fs::remove_dir_all(candidate_root);
    }

    #[test]
    fn bounded_top_k_is_deterministic_for_equal_scores() {
        let query_root = setup_root("tie-query", "lib-query");
        let root_b = setup_root("tie-b", "lib-b");
        let root_a = setup_root("tie-a", "lib-a");
        let query_face =
            add_image_faces(&query_root, "query.jpg", &[vec![1.0, 0.0]], "embedder").remove(0);
        add_image_faces(&root_b, "b.jpg", &[vec![1.0, 0.0]], "embedder");
        add_image_faces(&root_a, "a.jpg", &[vec![1.0, 0.0]], "embedder");

        let query = load_query(&query_root, &query_face).unwrap();
        let report = search_available_roots(
            &[root_b.clone(), root_a.clone()],
            &query,
            FaceSimilarityOptions {
                limit: 1,
                collapse_same_image: false,
            },
        )
        .unwrap();
        assert_eq!(report.matches.len(), 1);
        assert_eq!(report.matches[0].library_id, "lib-a");

        let _ = std::fs::remove_dir_all(query_root);
        let _ = std::fs::remove_dir_all(root_a);
        let _ = std::fs::remove_dir_all(root_b);
    }
}
