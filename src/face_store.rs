use crate::face_detection::{self, DetectedFace, FaceBox, FaceLandmark};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

const FACE_ID_FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FACE_ID_FNV_PRIME: u64 = 0x100000001b3;

#[derive(Clone, Debug, PartialEq)]
pub struct StoredFace {
    pub face_id: String,
    pub image_path: PathBuf,
    pub ordinal: usize,
    pub detector_id: String,
    pub detector_version: String,
    pub confidence: f32,
    pub bbox: FaceBox,
    pub landmarks: Vec<FaceLandmark>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectionState {
    pub image_path: PathBuf,
    pub detector_id: String,
    pub detector_version: String,
    pub schema_version: i64,
    pub source_size: u64,
    pub source_modified: i64,
    pub exif_orientation: u32,
    pub oriented_width: u32,
    pub oriented_height: u32,
    pub face_count: usize,
}

pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS face_detection_state (
            image_path TEXT PRIMARY KEY NOT NULL,
            detector_id TEXT NOT NULL,
            detector_version TEXT NOT NULL,
            schema_version INTEGER NOT NULL,
            source_size INTEGER NOT NULL,
            source_modified INTEGER NOT NULL,
            exif_orientation INTEGER NOT NULL DEFAULT 1,
            oriented_width INTEGER NOT NULL,
            oriented_height INTEGER NOT NULL,
            face_count INTEGER NOT NULL,
            completed_at INTEGER NOT NULL DEFAULT (unixepoch()),
            FOREIGN KEY(image_path) REFERENCES images(path) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS faces (
            face_id TEXT PRIMARY KEY NOT NULL,
            image_path TEXT NOT NULL,
            face_ordinal INTEGER NOT NULL,
            detector_id TEXT NOT NULL,
            detector_version TEXT NOT NULL,
            schema_version INTEGER NOT NULL,
            confidence REAL NOT NULL,
            bbox_x REAL NOT NULL,
            bbox_y REAL NOT NULL,
            bbox_width REAL NOT NULL,
            bbox_height REAL NOT NULL,
            landmarks BLOB,
            landmark_count INTEGER NOT NULL DEFAULT 0,
            source_size INTEGER NOT NULL,
            source_modified INTEGER NOT NULL,
            UNIQUE(image_path, face_ordinal),
            FOREIGN KEY(image_path) REFERENCES face_detection_state(image_path) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_faces_image_path ON faces(image_path);
        CREATE INDEX IF NOT EXISTS idx_faces_detector_revision
            ON faces(detector_id, detector_version, schema_version);
        CREATE INDEX IF NOT EXISTS idx_face_detection_revision
            ON face_detection_state(detector_id, detector_version, schema_version);

        CREATE TRIGGER IF NOT EXISTS images_face_detection_invalidate
        AFTER UPDATE OF size, modified, content_fingerprint ON images
        WHEN old.size <> new.size
          OR old.modified <> new.modified
          OR old.content_fingerprint IS NOT new.content_fingerprint
        BEGIN
            DELETE FROM face_detection_state WHERE image_path = old.path;
        END;
        "#,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn detection_is_current(
    conn: &Connection,
    image_path: &Path,
    source_size: u64,
    source_modified: i64,
    detector_id: &str,
    detector_version: &str,
) -> Result<bool> {
    let current = conn
        .query_row(
            r#"
            SELECT 1
            FROM face_detection_state
            WHERE image_path = ?1
              AND detector_id = ?2
              AND detector_version = ?3
              AND schema_version = ?4
              AND source_size = ?5
              AND source_modified = ?6
            LIMIT 1
            "#,
            params![
                image_path.to_string_lossy().to_string(),
                detector_id,
                detector_version,
                face_detection::SCHEMA_VERSION,
                source_size as i64,
                source_modified,
            ],
            |_| Ok(()),
        )
        .is_ok();
    Ok(current)
}

pub fn paths_needing_detection(
    conn: &Connection,
    detector_id: &str,
    detector_version: &str,
) -> Result<Vec<PathBuf>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT images.path
        FROM images
        LEFT JOIN face_detection_state state ON state.image_path = images.path
        WHERE state.image_path IS NULL
           OR state.detector_id <> ?1
           OR state.detector_version <> ?2
           OR state.schema_version <> ?3
           OR state.source_size <> images.size
           OR state.source_modified <> images.modified
        ORDER BY images.path COLLATE NOCASE
        "#,
    )?;
    let rows = stmt.query_map(
        params![detector_id, detector_version, face_detection::SCHEMA_VERSION],
        |row| row.get::<_, String>(0),
    )?;
    Ok(rows.filter_map(|row| row.ok()).map(PathBuf::from).collect())
}

#[allow(clippy::too_many_arguments)]
pub fn replace_detections(
    conn: &mut Connection,
    image_path: &Path,
    source_size: u64,
    source_modified: i64,
    exif_orientation: u32,
    oriented_width: u32,
    oriented_height: u32,
    detector_id: &str,
    detector_version: &str,
    detections: &[DetectedFace],
) -> Result<Vec<StoredFace>> {
    if detector_id.trim().is_empty() || detector_version.trim().is_empty() {
        bail!("face detector id/version cannot be empty");
    }
    if !(1..=8).contains(&exif_orientation) {
        bail!("invalid EXIF orientation: {exif_orientation}");
    }
    if oriented_width == 0 || oriented_height == 0 {
        bail!("oriented image dimensions must be non-zero");
    }

    let mut normalized: Vec<DetectedFace> = detections
        .iter()
        .cloned()
        .map(DetectedFace::normalized)
        .filter(|face| face.bbox.width > 0.0 && face.bbox.height > 0.0)
        .collect();
    normalized.sort_by(compare_faces);

    let image_text = image_path.to_string_lossy().to_string();
    let tx = conn.transaction()?;
    let image_exists = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM images WHERE path = ?1)",
        params![image_text],
        |row| row.get::<_, bool>(0),
    )?;
    if !image_exists {
        bail!("cannot store face detections for an image that is not indexed");
    }

    tx.execute(
        "DELETE FROM face_detection_state WHERE image_path = ?1",
        params![image_path.to_string_lossy().to_string()],
    )?;
    tx.execute(
        r#"
        INSERT INTO face_detection_state(
            image_path, detector_id, detector_version, schema_version,
            source_size, source_modified, exif_orientation,
            oriented_width, oriented_height, face_count
        ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        "#,
        params![
            image_path.to_string_lossy().to_string(),
            detector_id,
            detector_version,
            face_detection::SCHEMA_VERSION,
            source_size as i64,
            source_modified,
            exif_orientation as i64,
            oriented_width as i64,
            oriented_height as i64,
            normalized.len() as i64,
        ],
    )?;

    let mut stored = Vec::with_capacity(normalized.len());
    for (ordinal, face) in normalized.into_iter().enumerate() {
        let face_id = stable_face_id(
            image_path,
            detector_id,
            detector_version,
            face_detection::SCHEMA_VERSION,
            ordinal,
        );
        let landmark_blob = encode_landmarks(&face.landmarks);
        tx.execute(
            r#"
            INSERT INTO faces(
                face_id, image_path, face_ordinal, detector_id, detector_version,
                schema_version, confidence, bbox_x, bbox_y, bbox_width, bbox_height,
                landmarks, landmark_count, source_size, source_modified
            ) VALUES(
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
            )
            "#,
            params![
                face_id,
                image_path.to_string_lossy().to_string(),
                ordinal as i64,
                detector_id,
                detector_version,
                face_detection::SCHEMA_VERSION,
                face.confidence,
                face.bbox.x,
                face.bbox.y,
                face.bbox.width,
                face.bbox.height,
                landmark_blob,
                face.landmarks.len() as i64,
                source_size as i64,
                source_modified,
            ],
        )?;
        stored.push(StoredFace {
            face_id,
            image_path: image_path.to_path_buf(),
            ordinal,
            detector_id: detector_id.to_owned(),
            detector_version: detector_version.to_owned(),
            confidence: face.confidence,
            bbox: face.bbox,
            landmarks: face.landmarks,
        });
    }
    tx.commit()?;
    Ok(stored)
}

pub fn load_detection_state(conn: &Connection, image_path: &Path) -> Result<Option<DetectionState>> {
    let row = conn.query_row(
        r#"
        SELECT image_path, detector_id, detector_version, schema_version,
               source_size, source_modified, exif_orientation,
               oriented_width, oriented_height, face_count
        FROM face_detection_state
        WHERE image_path = ?1
        "#,
        params![image_path.to_string_lossy().to_string()],
        |row| {
            Ok(DetectionState {
                image_path: PathBuf::from(row.get::<_, String>(0)?),
                detector_id: row.get(1)?,
                detector_version: row.get(2)?,
                schema_version: row.get(3)?,
                source_size: row.get::<_, i64>(4)?.max(0) as u64,
                source_modified: row.get(5)?,
                exif_orientation: row.get::<_, i64>(6)?.clamp(1, 8) as u32,
                oriented_width: row.get::<_, i64>(7)?.max(0) as u32,
                oriented_height: row.get::<_, i64>(8)?.max(0) as u32,
                face_count: row.get::<_, i64>(9)?.max(0) as usize,
            })
        },
    );
    match row {
        Ok(state) => Ok(Some(state)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub fn load_faces(conn: &Connection, image_path: &Path) -> Result<Vec<StoredFace>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT face_id, image_path, face_ordinal, detector_id, detector_version,
               confidence, bbox_x, bbox_y, bbox_width, bbox_height,
               landmarks, landmark_count
        FROM faces
        WHERE image_path = ?1
        ORDER BY face_ordinal
        "#,
    )?;
    let rows = stmt.query_map(params![image_path.to_string_lossy().to_string()], |row| {
        let blob: Option<Vec<u8>> = row.get(10)?;
        let count = row.get::<_, i64>(11)?.max(0) as usize;
        Ok(StoredFace {
            face_id: row.get(0)?,
            image_path: PathBuf::from(row.get::<_, String>(1)?),
            ordinal: row.get::<_, i64>(2)?.max(0) as usize,
            detector_id: row.get(3)?,
            detector_version: row.get(4)?,
            confidence: row.get(5)?,
            bbox: FaceBox {
                x: row.get(6)?,
                y: row.get(7)?,
                width: row.get(8)?,
                height: row.get(9)?,
            },
            landmarks: blob
                .as_deref()
                .and_then(|bytes| decode_landmarks(bytes, count))
                .unwrap_or_default(),
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("loading stored face detections")
}

fn compare_faces(left: &DetectedFace, right: &DetectedFace) -> std::cmp::Ordering {
    left.bbox
        .x
        .total_cmp(&right.bbox.x)
        .then_with(|| left.bbox.y.total_cmp(&right.bbox.y))
        .then_with(|| left.bbox.width.total_cmp(&right.bbox.width))
        .then_with(|| left.bbox.height.total_cmp(&right.bbox.height))
        .then_with(|| right.confidence.total_cmp(&left.confidence))
}

fn stable_face_id(
    image_path: &Path,
    detector_id: &str,
    detector_version: &str,
    schema_version: i64,
    ordinal: usize,
) -> String {
    let path = image_path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let mut hash = FACE_ID_FNV_OFFSET;
    for bytes in [
        path.as_bytes(),
        detector_id.as_bytes(),
        detector_version.as_bytes(),
        &schema_version.to_le_bytes(),
        &(ordinal as u64).to_le_bytes(),
    ] {
        for &byte in bytes {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(FACE_ID_FNV_PRIME);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(FACE_ID_FNV_PRIME);
    }
    format!("face-{hash:016x}")
}

fn encode_landmarks(landmarks: &[FaceLandmark]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(landmarks.len() * 8);
    for landmark in landmarks {
        bytes.extend_from_slice(&landmark.x.to_le_bytes());
        bytes.extend_from_slice(&landmark.y.to_le_bytes());
    }
    bytes
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

    fn test_connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
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
        ensure_schema(&conn).unwrap();
        conn
    }

    fn insert_image(conn: &Connection, path: &str) {
        conn.execute(
            "INSERT INTO images(path, size, modified, content_fingerprint) VALUES(?1, 100, 200, 7)",
            params![path],
        )
        .unwrap();
    }

    fn face(x: f32, confidence: f32) -> DetectedFace {
        DetectedFace {
            confidence,
            bbox: FaceBox {
                x,
                y: 0.2,
                width: 0.25,
                height: 0.3,
            },
            landmarks: vec![FaceLandmark { x: x + 0.05, y: 0.3 }],
        }
    }

    #[test]
    fn zero_face_detection_is_persisted_as_current_state() {
        let mut conn = test_connection();
        insert_image(&conn, "people/empty.jpg");
        let stored = replace_detections(
            &mut conn,
            Path::new("people/empty.jpg"),
            100,
            200,
            1,
            1920,
            1080,
            "test-detector",
            "1",
            &[],
        )
        .unwrap();
        assert!(stored.is_empty());
        let state = load_detection_state(&conn, Path::new("people/empty.jpg"))
            .unwrap()
            .unwrap();
        assert_eq!(state.face_count, 0);
        assert!(detection_is_current(
            &conn,
            Path::new("people/empty.jpg"),
            100,
            200,
            "test-detector",
            "1"
        )
        .unwrap());
        assert!(!paths_needing_detection(&conn, "test-detector", "1")
            .unwrap()
            .contains(&PathBuf::from("people/empty.jpg")));
    }

    #[test]
    fn multiple_faces_are_sorted_and_keep_stable_ids_within_revision() {
        let mut conn = test_connection();
        insert_image(&conn, "people/group.jpg");
        let first = replace_detections(
            &mut conn,
            Path::new("people/group.jpg"),
            100,
            200,
            1,
            1000,
            600,
            "test-detector",
            "1",
            &[face(0.7, 0.8), face(0.1, 0.9)],
        )
        .unwrap();
        let second = replace_detections(
            &mut conn,
            Path::new("people/group.jpg"),
            100,
            200,
            1,
            1000,
            600,
            "test-detector",
            "1",
            &[face(0.1, 0.9), face(0.7, 0.8)],
        )
        .unwrap();
        assert_eq!(first.iter().map(|face| &face.face_id).collect::<Vec<_>>(), second.iter().map(|face| &face.face_id).collect::<Vec<_>>());
        assert!(second[0].bbox.x < second[1].bbox.x);
        assert_eq!(load_faces(&conn, Path::new("people/group.jpg")).unwrap().len(), 2);
    }

    #[test]
    fn source_fingerprint_update_invalidates_state_and_faces() {
        let mut conn = test_connection();
        insert_image(&conn, "people/person.jpg");
        replace_detections(
            &mut conn,
            Path::new("people/person.jpg"),
            100,
            200,
            1,
            800,
            600,
            "test-detector",
            "1",
            &[face(0.2, 0.95)],
        )
        .unwrap();
        conn.execute(
            "UPDATE images SET content_fingerprint = 8 WHERE path = 'people/person.jpg'",
            [],
        )
        .unwrap();
        assert!(load_detection_state(&conn, Path::new("people/person.jpg"))
            .unwrap()
            .is_none());
        assert!(load_faces(&conn, Path::new("people/person.jpg"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn deleting_image_cascades_detection_state_and_faces() {
        let mut conn = test_connection();
        insert_image(&conn, "people/delete.jpg");
        replace_detections(
            &mut conn,
            Path::new("people/delete.jpg"),
            100,
            200,
            1,
            800,
            600,
            "test-detector",
            "1",
            &[face(0.2, 0.95)],
        )
        .unwrap();
        conn.execute("DELETE FROM images WHERE path = 'people/delete.jpg'", [])
            .unwrap();
        assert!(load_detection_state(&conn, Path::new("people/delete.jpg"))
            .unwrap()
            .is_none());
        assert!(load_faces(&conn, Path::new("people/delete.jpg"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn interrupted_replacement_does_not_destroy_committed_detection() {
        let mut conn = test_connection();
        insert_image(&conn, "people/resume.jpg");
        replace_detections(
            &mut conn,
            Path::new("people/resume.jpg"),
            100,
            200,
            1,
            800,
            600,
            "test-detector",
            "1",
            &[face(0.2, 0.95)],
        )
        .unwrap();
        {
            let tx = conn.transaction().unwrap();
            tx.execute(
                "DELETE FROM face_detection_state WHERE image_path = 'people/resume.jpg'",
                [],
            )
            .unwrap();
            // Simulated interruption: dropping the transaction rolls it back.
        }
        assert_eq!(load_faces(&conn, Path::new("people/resume.jpg")).unwrap().len(), 1);
    }
}
