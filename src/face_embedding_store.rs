use crate::face_detection::{FaceBox, FaceLandmark};
use crate::face_embedding;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingCandidate {
    pub face_id: String,
    pub image_path: PathBuf,
    pub bbox: FaceBox,
    pub landmarks: Vec<FaceLandmark>,
    pub detector_id: String,
    pub detector_version: String,
    pub detector_cache_revision: String,
    pub detection_schema_version: i64,
    pub source_size: u64,
    pub source_modified: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredFaceEmbedding {
    pub face_id: String,
    pub model_id: String,
    pub model_version: String,
    pub model_cache_revision: String,
    pub schema_version: i64,
    pub alignment_revision: i64,
    pub dimension: usize,
    pub normalized: bool,
    pub detector_id: String,
    pub detector_version: String,
    pub detector_cache_revision: String,
    pub detection_schema_version: i64,
    pub source_size: u64,
    pub source_modified: i64,
    pub values: Vec<f32>,
}

pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS face_embeddings (
            face_id TEXT PRIMARY KEY NOT NULL,
            model_id TEXT NOT NULL,
            model_version TEXT NOT NULL,
            model_cache_revision TEXT NOT NULL DEFAULT '',
            schema_version INTEGER NOT NULL,
            alignment_revision INTEGER NOT NULL,
            dimension INTEGER NOT NULL,
            normalized INTEGER NOT NULL,
            embedding BLOB NOT NULL,
            detector_id TEXT NOT NULL,
            detector_version TEXT NOT NULL,
            detector_cache_revision TEXT NOT NULL DEFAULT '',
            detection_schema_version INTEGER NOT NULL,
            source_size INTEGER NOT NULL,
            source_modified INTEGER NOT NULL,
            completed_at INTEGER NOT NULL DEFAULT (unixepoch()),
            FOREIGN KEY(face_id) REFERENCES faces(face_id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_face_embeddings_model_revision
            ON face_embeddings(model_id, model_version, schema_version, alignment_revision);
        CREATE INDEX IF NOT EXISTS idx_face_embeddings_detection_revision
            ON face_embeddings(detector_id, detector_version, detection_schema_version);
        "#,
    )?;
    ensure_column(
        conn,
        "face_embeddings",
        "model_cache_revision",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        conn,
        "face_embeddings",
        "detector_cache_revision",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    conn.execute(
        "UPDATE face_embeddings SET model_cache_revision = model_version WHERE model_cache_revision = ''",
        [],
    )?;
    conn.execute(
        "UPDATE face_embeddings SET detector_cache_revision = detector_version WHERE detector_cache_revision = ''",
        [],
    )?;
    conn.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_face_embeddings_model_cache_revision
            ON face_embeddings(model_id, model_version, model_cache_revision, schema_version, alignment_revision);
        CREATE INDEX IF NOT EXISTS idx_face_embeddings_detection_cache_revision
            ON face_embeddings(detector_id, detector_version, detector_cache_revision, detection_schema_version);
        "#,
    )?;
    Ok(())
}

pub fn count_pending(
    conn: &Connection,
    model_id: &str,
    model_version: &str,
    dimension: usize,
    alignment_revision: i64,
) -> Result<usize> {
    count_pending_with_revision(
        conn,
        model_id,
        model_version,
        model_version,
        dimension,
        alignment_revision,
    )
}

pub fn count_pending_with_revision(
    conn: &Connection,
    model_id: &str,
    model_version: &str,
    model_cache_revision: &str,
    dimension: usize,
    alignment_revision: i64,
) -> Result<usize> {
    validate_model_revision(model_id, model_version, dimension, alignment_revision)?;
    validate_cache_revision(model_cache_revision)?;
    let count = conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM faces f
        LEFT JOIN face_embeddings e ON e.face_id = f.face_id
        WHERE e.face_id IS NULL
           OR e.model_id <> ?1
           OR e.model_version <> ?2
           OR e.model_cache_revision <> ?3
           OR e.schema_version <> ?4
           OR e.alignment_revision <> ?5
           OR e.dimension <> ?6
           OR e.normalized <> 1
           OR e.detector_id <> f.detector_id
           OR e.detector_version <> f.detector_version
           OR e.detector_cache_revision <> f.detector_cache_revision
           OR e.detection_schema_version <> f.schema_version
           OR e.source_size <> f.source_size
           OR e.source_modified <> f.source_modified
        "#,
        params![
            model_id,
            model_version,
            model_cache_revision,
            face_embedding::SCHEMA_VERSION,
            alignment_revision,
            dimension as i64,
        ],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count.max(0) as usize)
}

pub fn candidate_batch(
    conn: &Connection,
    after_face_id: Option<&str>,
    model_id: &str,
    model_version: &str,
    dimension: usize,
    alignment_revision: i64,
    limit: usize,
) -> Result<Vec<EmbeddingCandidate>> {
    candidate_batch_with_revision(
        conn,
        after_face_id,
        model_id,
        model_version,
        model_version,
        dimension,
        alignment_revision,
        limit,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn candidate_batch_with_revision(
    conn: &Connection,
    after_face_id: Option<&str>,
    model_id: &str,
    model_version: &str,
    model_cache_revision: &str,
    dimension: usize,
    alignment_revision: i64,
    limit: usize,
) -> Result<Vec<EmbeddingCandidate>> {
    validate_model_revision(model_id, model_version, dimension, alignment_revision)?;
    validate_cache_revision(model_cache_revision)?;
    let limit = limit.max(1);
    let mut stmt = conn.prepare(
        r#"
        SELECT f.face_id, f.image_path,
               f.bbox_x, f.bbox_y, f.bbox_width, f.bbox_height,
               f.landmarks, f.landmark_count,
               f.detector_id, f.detector_version, f.detector_cache_revision, f.schema_version,
               f.source_size, f.source_modified
        FROM faces f
        LEFT JOIN face_embeddings e ON e.face_id = f.face_id
        WHERE (?1 IS NULL OR f.face_id > ?1)
          AND (
               e.face_id IS NULL
            OR e.model_id <> ?2
            OR e.model_version <> ?3
            OR e.model_cache_revision <> ?4
            OR e.schema_version <> ?5
            OR e.alignment_revision <> ?6
            OR e.dimension <> ?7
            OR e.normalized <> 1
            OR e.detector_id <> f.detector_id
            OR e.detector_version <> f.detector_version
            OR e.detector_cache_revision <> f.detector_cache_revision
            OR e.detection_schema_version <> f.schema_version
            OR e.source_size <> f.source_size
            OR e.source_modified <> f.source_modified
          )
        ORDER BY f.face_id
        LIMIT ?8
        "#,
    )?;
    let rows = stmt.query_map(
        params![
            after_face_id,
            model_id,
            model_version,
            model_cache_revision,
            face_embedding::SCHEMA_VERSION,
            alignment_revision,
            dimension as i64,
            limit as i64,
        ],
        |row| {
            let landmark_blob: Option<Vec<u8>> = row.get(6)?;
            let landmark_count = row.get::<_, i64>(7)?.max(0) as usize;
            Ok(EmbeddingCandidate {
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
                detector_id: row.get(8)?,
                detector_version: row.get(9)?,
                detector_cache_revision: row.get(10)?,
                detection_schema_version: row.get(11)?,
                source_size: row.get::<_, i64>(12)?.max(0) as u64,
                source_modified: row.get(13)?,
            })
        },
    )?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("loading pending face embedding candidates")
}

pub fn embedding_is_current(
    conn: &Connection,
    face_id: &str,
    model_id: &str,
    model_version: &str,
    dimension: usize,
    alignment_revision: i64,
) -> Result<bool> {
    embedding_is_current_with_revision(
        conn,
        face_id,
        model_id,
        model_version,
        model_version,
        dimension,
        alignment_revision,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn embedding_is_current_with_revision(
    conn: &Connection,
    face_id: &str,
    model_id: &str,
    model_version: &str,
    model_cache_revision: &str,
    dimension: usize,
    alignment_revision: i64,
) -> Result<bool> {
    validate_model_revision(model_id, model_version, dimension, alignment_revision)?;
    validate_cache_revision(model_cache_revision)?;
    let current = conn
        .query_row(
            r#"
            SELECT 1
            FROM face_embeddings e
            JOIN faces f ON f.face_id = e.face_id
            WHERE e.face_id = ?1
              AND e.model_id = ?2
              AND e.model_version = ?3
              AND e.model_cache_revision = ?4
              AND e.schema_version = ?5
              AND e.alignment_revision = ?6
              AND e.dimension = ?7
              AND e.normalized = 1
              AND e.detector_id = f.detector_id
              AND e.detector_version = f.detector_version
              AND e.detector_cache_revision = f.detector_cache_revision
              AND e.detection_schema_version = f.schema_version
              AND e.source_size = f.source_size
              AND e.source_modified = f.source_modified
            LIMIT 1
            "#,
            params![
                face_id,
                model_id,
                model_version,
                model_cache_revision,
                face_embedding::SCHEMA_VERSION,
                alignment_revision,
                dimension as i64,
            ],
            |_| Ok(()),
        )
        .optional()?;
    Ok(current.is_some())
}

pub fn replace_embedding(
    conn: &mut Connection,
    candidate: &EmbeddingCandidate,
    model_id: &str,
    model_version: &str,
    alignment_revision: i64,
    values: &[f32],
) -> Result<()> {
    replace_embedding_with_revision(
        conn,
        candidate,
        model_id,
        model_version,
        model_version,
        alignment_revision,
        values,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn replace_embedding_with_revision(
    conn: &mut Connection,
    candidate: &EmbeddingCandidate,
    model_id: &str,
    model_version: &str,
    model_cache_revision: &str,
    alignment_revision: i64,
    values: &[f32],
) -> Result<()> {
    validate_model_revision(model_id, model_version, values.len(), alignment_revision)?;
    validate_cache_revision(model_cache_revision)?;
    if values.iter().any(|value| !value.is_finite()) {
        bail!("cannot persist a face embedding with non-finite values");
    }
    let norm_sq = values
        .iter()
        .map(|value| (*value as f64) * (*value as f64))
        .sum::<f64>();
    if (norm_sq.sqrt() - 1.0).abs() > 1e-3 {
        bail!("face embedding must be L2-normalized before persistence");
    }

    let tx = conn.transaction()?;
    let face_is_current = tx.query_row(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM faces
            WHERE face_id = ?1
              AND image_path = ?2
              AND detector_id = ?3
              AND detector_version = ?4
              AND detector_cache_revision = ?5
              AND schema_version = ?6
              AND source_size = ?7
              AND source_modified = ?8
        )
        "#,
        params![
            candidate.face_id,
            candidate.image_path.to_string_lossy().to_string(),
            candidate.detector_id,
            candidate.detector_version,
            candidate.detector_cache_revision,
            candidate.detection_schema_version,
            candidate.source_size as i64,
            candidate.source_modified,
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if !face_is_current {
        bail!("face detection changed before embedding commit; leaving face pending");
    }

    let blob = encode_embedding(values);
    tx.execute(
        r#"
        INSERT INTO face_embeddings(
            face_id, model_id, model_version, model_cache_revision, schema_version, alignment_revision,
            dimension, normalized, embedding,
            detector_id, detector_version, detector_cache_revision, detection_schema_version,
            source_size, source_modified, completed_at
        ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?10, ?11, ?12, ?13, ?14, unixepoch())
        ON CONFLICT(face_id) DO UPDATE SET
            model_id = excluded.model_id,
            model_version = excluded.model_version,
            model_cache_revision = excluded.model_cache_revision,
            schema_version = excluded.schema_version,
            alignment_revision = excluded.alignment_revision,
            dimension = excluded.dimension,
            normalized = excluded.normalized,
            embedding = excluded.embedding,
            detector_id = excluded.detector_id,
            detector_version = excluded.detector_version,
            detector_cache_revision = excluded.detector_cache_revision,
            detection_schema_version = excluded.detection_schema_version,
            source_size = excluded.source_size,
            source_modified = excluded.source_modified,
            completed_at = excluded.completed_at
        "#,
        params![
            candidate.face_id,
            model_id,
            model_version,
            model_cache_revision,
            face_embedding::SCHEMA_VERSION,
            alignment_revision,
            values.len() as i64,
            blob,
            candidate.detector_id,
            candidate.detector_version,
            candidate.detector_cache_revision,
            candidate.detection_schema_version,
            candidate.source_size as i64,
            candidate.source_modified,
        ],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn load_embedding(conn: &Connection, face_id: &str) -> Result<Option<StoredFaceEmbedding>> {
    let row = conn
        .query_row(
            r#"
            SELECT face_id, model_id, model_version, model_cache_revision, schema_version, alignment_revision,
                   dimension, normalized, embedding,
                   detector_id, detector_version, detector_cache_revision, detection_schema_version,
                   source_size, source_modified
            FROM face_embeddings
            WHERE face_id = ?1
            "#,
            params![face_id],
            |row| {
                let dimension = row.get::<_, i64>(6)?.max(0) as usize;
                let blob: Vec<u8> = row.get(8)?;
                let values = decode_embedding(&blob, dimension).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        blob.len(),
                        rusqlite::types::Type::Blob,
                        "invalid face embedding blob".into(),
                    )
                })?;
                Ok(StoredFaceEmbedding {
                    face_id: row.get(0)?,
                    model_id: row.get(1)?,
                    model_version: row.get(2)?,
                    model_cache_revision: row.get(3)?,
                    schema_version: row.get(4)?,
                    alignment_revision: row.get(5)?,
                    dimension,
                    normalized: row.get::<_, i64>(7)? != 0,
                    values,
                    detector_id: row.get(9)?,
                    detector_version: row.get(10)?,
                    detector_cache_revision: row.get(11)?,
                    detection_schema_version: row.get(12)?,
                    source_size: row.get::<_, i64>(13)?.max(0) as u64,
                    source_modified: row.get(14)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

fn validate_model_revision(
    model_id: &str,
    model_version: &str,
    dimension: usize,
    alignment_revision: i64,
) -> Result<()> {
    if model_id.trim().is_empty() || model_version.trim().is_empty() {
        bail!("face embedding model id/version cannot be empty");
    }
    if dimension == 0 {
        bail!("face embedding dimension must be non-zero");
    }
    if alignment_revision <= 0 {
        bail!("face alignment revision must be positive");
    }
    Ok(())
}

fn validate_cache_revision(cache_revision: &str) -> Result<()> {
    if cache_revision.trim().is_empty() {
        bail!("face embedding cache revision cannot be empty");
    }
    Ok(())
}

fn ensure_column(conn: &Connection, table: &str, column: &str, declaration: &str) -> Result<()> {
    let exists = {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        rows.any(|row| row.is_ok_and(|name| name == column))
    };
    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {declaration}"),
            [],
        )?;
    }
    Ok(())
}

fn encode_embedding(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decode_embedding(bytes: &[u8], dimension: usize) -> Option<Vec<f32>> {
    if bytes.len() != dimension.checked_mul(4)? {
        return None;
    }
    let mut values = Vec::with_capacity(dimension);
    for chunk in bytes.chunks_exact(4) {
        values.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Some(values)
}

fn decode_landmarks(bytes: &[u8], count: usize) -> Option<Vec<FaceLandmark>> {
    if bytes.len() != count.checked_mul(8)? {
        return None;
    }
    let mut output = Vec::with_capacity(count);
    for chunk in bytes.chunks_exact(8) {
        output.push(FaceLandmark {
            x: f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
            y: f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
        });
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face_detection::{DetectedFace, FaceBox};
    use crate::face_store;
    use rusqlite::params;
    use std::path::Path;

    fn connection_with_face() -> (Connection, EmbeddingCandidate) {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE images (
                path TEXT PRIMARY KEY NOT NULL,
                size INTEGER NOT NULL,
                modified INTEGER NOT NULL,
                content_fingerprint INTEGER
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO images(path, size, modified, content_fingerprint) VALUES('a.jpg', 100, 200, 7)",
            [],
        )
        .unwrap();
        face_store::ensure_schema(&conn).unwrap();
        ensure_schema(&conn).unwrap();
        let faces = face_store::replace_detections(
            &mut conn,
            Path::new("a.jpg"),
            100,
            200,
            1,
            100,
            100,
            "fake-detector",
            "1",
            &[DetectedFace {
                confidence: 0.9,
                bbox: FaceBox {
                    x: 0.1,
                    y: 0.2,
                    width: 0.3,
                    height: 0.4,
                },
                landmarks: vec![FaceLandmark { x: 0.2, y: 0.3 }],
            }],
        )
        .unwrap();
        let candidate = candidate_batch(&conn, None, "fake-embedder", "1", 2, 1, 8)
            .unwrap()
            .remove(0);
        assert_eq!(candidate.face_id, faces[0].face_id);
        (conn, candidate)
    }

    #[test]
    fn embedding_round_trip_and_revision_checks() {
        let (mut conn, candidate) = connection_with_face();
        let values = vec![0.6, 0.8];
        replace_embedding(&mut conn, &candidate, "fake-embedder", "1", 1, &values).unwrap();
        assert!(
            embedding_is_current(&conn, &candidate.face_id, "fake-embedder", "1", 2, 1).unwrap()
        );
        assert!(
            !embedding_is_current(&conn, &candidate.face_id, "fake-embedder", "2", 2, 1).unwrap()
        );
        assert!(
            !embedding_is_current(&conn, &candidate.face_id, "fake-embedder", "1", 2, 2).unwrap()
        );
        let stored = load_embedding(&conn, &candidate.face_id).unwrap().unwrap();
        assert_eq!(stored.values, values);
        assert!(stored.normalized);
    }

    #[test]
    fn cache_revision_backfills_without_changing_model_version() {
        let (mut conn, candidate) = connection_with_face();
        let values = vec![0.6, 0.8];
        replace_embedding_with_revision(
            &mut conn,
            &candidate,
            "fake-embedder",
            "1",
            "weights-a",
            1,
            &values,
        )
        .unwrap();
        assert!(embedding_is_current_with_revision(
            &conn,
            &candidate.face_id,
            "fake-embedder",
            "1",
            "weights-a",
            2,
            1,
        )
        .unwrap());
        assert!(!embedding_is_current_with_revision(
            &conn,
            &candidate.face_id,
            "fake-embedder",
            "1",
            "weights-b",
            2,
            1,
        )
        .unwrap());
        assert_eq!(
            count_pending_with_revision(&conn, "fake-embedder", "1", "weights-b", 2, 1).unwrap(),
            1
        );
    }

    #[test]
    fn replacing_detection_cascades_embedding() {
        let (mut conn, candidate) = connection_with_face();
        replace_embedding(&mut conn, &candidate, "fake-embedder", "1", 1, &[0.6, 0.8]).unwrap();
        conn.execute(
            "UPDATE images SET modified = 201 WHERE path = ?1",
            params!["a.jpg"],
        )
        .unwrap();
        assert!(load_embedding(&conn, &candidate.face_id).unwrap().is_none());
    }
}
