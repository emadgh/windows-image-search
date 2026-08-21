use crate::face_detection::{FaceBox, FaceLandmark};
use crate::face_similarity::{self, FaceSimilarityOptions, FaceSimilarityQuery};
use crate::portable;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct IndexedFaceChoice {
    pub face_id: String,
    pub ordinal: usize,
    pub confidence: f32,
    pub bbox: FaceBox,
    pub landmarks: Vec<FaceLandmark>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexedFaceSuggestion {
    pub root: PathBuf,
    pub face_id: String,
    pub image_path: PathBuf,
    pub ordinal: usize,
    pub confidence: f32,
    pub bbox: FaceBox,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IndexedFaceSearchOptions {
    pub min_similarity: f32,
    pub limit: usize,
}

impl Default for IndexedFaceSearchOptions {
    fn default() -> Self {
        Self {
            min_similarity: 0.45,
            limit: 100,
        }
    }
}

impl IndexedFaceSearchOptions {
    pub fn sanitized(self) -> Self {
        Self {
            min_similarity: self.min_similarity.clamp(-1.0, 1.0),
            limit: self.limit.clamp(1, 5_000),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexedFaceSearchHit {
    pub root: PathBuf,
    pub library_id: String,
    pub face_id: String,
    pub image_path: PathBuf,
    pub bbox: FaceBox,
    pub landmarks: Vec<FaceLandmark>,
    pub similarity: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndexedFaceSearchReport {
    pub roots_searched: usize,
    pub roots_unavailable: usize,
    pub rows_considered: usize,
    pub matches: Vec<IndexedFaceSearchHit>,
}

pub fn list_searchable_faces(
    roots: &[PathBuf],
    limit: usize,
) -> Result<Vec<IndexedFaceSuggestion>> {
    let limit = limit.clamp(1, 2_000);
    if roots.is_empty() {
        return Ok(Vec::new());
    }
    let per_root_limit = limit.div_ceil(roots.len()).max(1);
    let mut suggestions = Vec::new();

    for root in roots {
        let Ok(conn) = open_read_only(root) else {
            continue;
        };
        let mut stmt = conn.prepare(
            r#"
            SELECT f.face_id, f.image_path, f.face_ordinal, f.confidence,
                   f.bbox_x, f.bbox_y, f.bbox_width, f.bbox_height
            FROM faces f
            JOIN face_detection_state s ON s.image_path = f.image_path
            JOIN images i ON i.path = f.image_path
            JOIN face_embeddings e ON e.face_id = f.face_id
            WHERE s.detector_id = f.detector_id
              AND s.detector_version = f.detector_version
              AND s.detector_cache_revision = f.detector_cache_revision
              AND s.schema_version = f.schema_version
              AND s.source_size = f.source_size
              AND s.source_modified = f.source_modified
              AND i.size = f.source_size
              AND i.modified = f.source_modified
              AND e.normalized = 1
              AND e.detector_id = f.detector_id
              AND e.detector_version = f.detector_version
              AND e.detector_cache_revision = f.detector_cache_revision
              AND e.detection_schema_version = f.schema_version
              AND e.source_size = f.source_size
              AND e.source_modified = f.source_modified
            ORDER BY f.confidence DESC, f.image_path COLLATE NOCASE, f.face_ordinal
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![per_root_limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?.max(0) as usize,
                row.get::<_, f32>(3)?,
                FaceBox {
                    x: row.get(4)?,
                    y: row.get(5)?,
                    width: row.get(6)?,
                    height: row.get(7)?,
                },
            ))
        })?;
        for row in rows {
            let (face_id, relative, ordinal, confidence, bbox) = row?;
            let relative = PathBuf::from(relative);
            let Ok(image_path) = portable::absolute_source_path(root, &relative) else {
                continue;
            };
            suggestions.push(IndexedFaceSuggestion {
                root: root.clone(),
                face_id,
                image_path,
                ordinal,
                confidence,
                bbox,
            });
        }
    }

    suggestions.sort_by(|left, right| {
        right
            .confidence
            .total_cmp(&left.confidence)
            .then_with(|| left.image_path.cmp(&right.image_path))
            .then_with(|| left.ordinal.cmp(&right.ordinal))
    });
    suggestions.truncate(limit);
    Ok(suggestions)
}

pub fn list_persisted_faces(root: &Path, image_path: &Path) -> Result<Vec<IndexedFaceChoice>> {
    let relative = portable::relative_source_path(root, image_path)?;
    let conn = open_read_only(root)?;
    let relative_text = relative.to_string_lossy().to_string();
    let mut stmt = conn.prepare(
        r#"
        SELECT f.face_id, f.face_ordinal, f.confidence,
               f.bbox_x, f.bbox_y, f.bbox_width, f.bbox_height,
               f.landmarks, f.landmark_count
        FROM faces f
        JOIN face_detection_state s ON s.image_path = f.image_path
        JOIN images i ON i.path = f.image_path
        WHERE f.image_path = ?1
          AND s.detector_id = f.detector_id
          AND s.detector_version = f.detector_version
          AND s.detector_cache_revision = f.detector_cache_revision
          AND s.schema_version = f.schema_version
          AND s.source_size = f.source_size
          AND s.source_modified = f.source_modified
          AND i.size = f.source_size
          AND i.modified = f.source_modified
        ORDER BY f.face_ordinal ASC
        "#,
    )?;
    let rows = stmt.query_map(params![relative_text], |row| {
        let landmark_blob: Option<Vec<u8>> = row.get(7)?;
        let landmark_count = row.get::<_, i64>(8)?.max(0) as usize;
        Ok(IndexedFaceChoice {
            face_id: row.get(0)?,
            ordinal: row.get::<_, i64>(1)?.max(0) as usize,
            confidence: row.get(2)?,
            bbox: FaceBox {
                x: row.get(3)?,
                y: row.get(4)?,
                width: row.get(5)?,
                height: row.get(6)?,
            },
            landmarks: landmark_blob
                .as_deref()
                .and_then(|bytes| decode_landmarks(bytes, landmark_count))
                .unwrap_or_default(),
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("loading persisted faces for indexed image")
}

pub fn search_indexed_face(
    roots: &[PathBuf],
    query_root: &Path,
    face_id: &str,
    options: IndexedFaceSearchOptions,
) -> Result<IndexedFaceSearchReport> {
    if face_id.trim().is_empty() {
        bail!("face id cannot be empty");
    }
    let query = face_similarity::load_query(query_root, face_id)?;
    search_embedding_query(roots, &query, options)
}

pub fn search_embedding_query(
    roots: &[PathBuf],
    query: &FaceSimilarityQuery,
    options: IndexedFaceSearchOptions,
) -> Result<IndexedFaceSearchReport> {
    let options = options.sanitized();
    let report = face_similarity::search_available_roots(
        roots,
        query,
        FaceSimilarityOptions {
            limit: options.limit,
            collapse_same_image: true,
        },
    )?;
    let matches = report
        .matches
        .into_iter()
        .filter(|item| item.similarity >= options.min_similarity)
        .map(|item| IndexedFaceSearchHit {
            root: item.root,
            library_id: item.library_id,
            face_id: item.face_id,
            image_path: item.image_path,
            bbox: item.bbox,
            landmarks: item.landmarks,
            similarity: item.similarity,
        })
        .collect();
    Ok(IndexedFaceSearchReport {
        roots_searched: report.roots_searched,
        roots_unavailable: report.roots_unavailable,
        rows_considered: report.rows_considered,
        matches,
    })
}

fn open_read_only(root: &Path) -> Result<Connection> {
    if !root.is_dir() {
        bail!("portable root is unavailable: {}", root.display());
    }
    let db_path = portable::index_db_path(root);
    if !db_path.is_file() {
        bail!("portable index does not exist: {}", db_path.display());
    }
    Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening portable face index read-only {}", db_path.display()))
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

    #[test]
    fn options_clamp_threshold_and_top_k() {
        assert_eq!(
            IndexedFaceSearchOptions {
                min_similarity: 9.0,
                limit: 0,
            }
            .sanitized(),
            IndexedFaceSearchOptions {
                min_similarity: 1.0,
                limit: 1,
            }
        );
    }

    #[test]
    fn landmark_decoder_rejects_wrong_length() {
        assert!(decode_landmarks(&[0; 7], 1).is_none());
        let decoded = decode_landmarks(&[0; 8], 1).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0], FaceLandmark { x: 0.0, y: 0.0 });
    }
}
