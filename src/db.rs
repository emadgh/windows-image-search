use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ImageRecord {
    pub path: PathBuf,
    pub root: PathBuf,
    pub file_name: String,
    pub extension: String,
    pub size: u64,
    pub modified: i64,
    pub width: u32,
    pub height: u32,
    pub description: String,
    pub keywords: String,
    pub dominant: [u8; 3],
    pub visual_hash: Option<u64>,
    pub color_histogram: Option<Vec<f32>>,
    pub embedding: Option<Vec<f32>>,
    pub score: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct ImageSummary {
    pub path: PathBuf,
    pub root: PathBuf,
    pub file_name: String,
    pub extension: String,
    pub size: u64,
    pub modified: i64,
    pub width: u32,
    pub height: u32,
    pub description: String,
    pub keywords: String,
    pub dominant: [u8; 3],
    pub score: Option<f32>,
}

impl From<ImageRecord> for ImageSummary {
    fn from(record: ImageRecord) -> Self {
        Self {
            path: record.path,
            root: record.root,
            file_name: record.file_name,
            extension: record.extension,
            size: record.size,
            modified: record.modified,
            width: record.width,
            height: record.height,
            description: record.description,
            keywords: record.keywords,
            dominant: record.dominant,
            score: record.score,
        }
    }
}

pub fn open(db_path: &Path) -> Result<Connection> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)
        .with_context(|| format!("opening database {}", db_path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS roots (
            path TEXT PRIMARY KEY NOT NULL
        );

        CREATE TABLE IF NOT EXISTS images (
            path TEXT PRIMARY KEY NOT NULL,
            root TEXT NOT NULL,
            file_name TEXT NOT NULL,
            extension TEXT NOT NULL,
            size INTEGER NOT NULL,
            modified INTEGER NOT NULL,
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            keywords TEXT NOT NULL DEFAULT '',
            dominant_r INTEGER NOT NULL DEFAULT 0,
            dominant_g INTEGER NOT NULL DEFAULT 0,
            dominant_b INTEGER NOT NULL DEFAULT 0,
            visual_hash INTEGER,
            color_histogram BLOB,
            color_histogram_dim INTEGER,
            embedding BLOB,
            embedding_dim INTEGER,
            last_seen_scan INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_images_root ON images(root);
        CREATE INDEX IF NOT EXISTS idx_images_file_name ON images(file_name);
        "#,
    )?;

    // v0.1.0 databases predate the hybrid visual descriptors. SQLite's
    // CREATE TABLE IF NOT EXISTS does not add new columns, so migrate them
    // explicitly and leave existing rows NULL until the next rescan/search.
    ensure_column(&conn, "images", "visual_hash", "INTEGER")?;
    ensure_column(&conn, "images", "color_histogram", "BLOB")?;
    ensure_column(&conn, "images", "color_histogram_dim", "INTEGER")?;
    ensure_column(
        &conn,
        "images",
        "last_seen_scan",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_images_root_scan ON images(root, last_seen_scan)",
        [],
    )?;

    Ok(conn)
}

fn ensure_column(conn: &Connection, table: &str, column: &str, declaration: &str) -> Result<()> {
    let exists = {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let found = rows.any(|row| row.is_ok_and(|name| name == column));
        found
    };

    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {declaration}"),
            [],
        )?;
    }
    Ok(())
}

pub fn load_roots(db_path: &Path) -> Result<Vec<PathBuf>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare("SELECT path FROM roots ORDER BY path COLLATE NOCASE")?;
    let roots = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .map(PathBuf::from)
        .collect();
    Ok(roots)
}

pub fn add_root(db_path: &Path, root: &Path) -> Result<()> {
    let conn = open(db_path)?;
    conn.execute(
        "INSERT OR IGNORE INTO roots(path) VALUES(?1)",
        params![root.to_string_lossy().to_string()],
    )?;
    Ok(())
}

pub fn remove_root(db_path: &Path, root: &Path) -> Result<()> {
    let mut conn = open(db_path)?;
    let tx = conn.transaction()?;
    let root_text = root.to_string_lossy().to_string();
    tx.execute("DELETE FROM roots WHERE path = ?1", params![root_text])?;
    tx.execute("DELETE FROM images WHERE root = ?1", params![root_text])?;
    tx.commit()?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileState {
    pub size: u64,
    pub modified: i64,
    pub has_embedding: bool,
}

pub fn load_file_states(conn: &Connection) -> Result<HashMap<PathBuf, FileState>> {
    let mut stmt =
        conn.prepare("SELECT path, size, modified, embedding IS NOT NULL FROM images")?;
    let rows = stmt.query_map([], |row| {
        let path = PathBuf::from(row.get::<_, String>(0)?);
        let size = row.get::<_, i64>(1)?.max(0) as u64;
        let modified = row.get::<_, i64>(2)?;
        let has_embedding = row.get::<_, bool>(3)?;
        Ok((
            path,
            FileState {
                size,
                modified,
                has_embedding,
            },
        ))
    })?;

    let mut states = HashMap::new();
    for row in rows {
        let (path, state) = row?;
        states.insert(path, state);
    }
    Ok(states)
}

#[allow(clippy::too_many_arguments)]
pub fn upsert_image(
    conn: &Connection,
    path: &Path,
    root: &Path,
    file_name: &str,
    extension: &str,
    size: u64,
    modified: i64,
    width: u32,
    height: u32,
    description: &str,
    keywords: &str,
    dominant: [u8; 3],
    visual_hash: u64,
    color_histogram: &[f32],
) -> Result<()> {
    let histogram_blob = encode_f32_vec(color_histogram);
    conn.execute(
        r#"
        INSERT INTO images(
            path, root, file_name, extension, size, modified, width, height,
            description, keywords, dominant_r, dominant_g, dominant_b,
            visual_hash, color_histogram, color_histogram_dim,
            embedding, embedding_dim
        ) VALUES(
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, NULL, NULL
        )
        ON CONFLICT(path) DO UPDATE SET
            root = excluded.root,
            file_name = excluded.file_name,
            extension = excluded.extension,
            size = excluded.size,
            modified = excluded.modified,
            width = excluded.width,
            height = excluded.height,
            description = excluded.description,
            keywords = excluded.keywords,
            dominant_r = excluded.dominant_r,
            dominant_g = excluded.dominant_g,
            dominant_b = excluded.dominant_b,
            visual_hash = excluded.visual_hash,
            color_histogram = excluded.color_histogram,
            color_histogram_dim = excluded.color_histogram_dim,
            embedding = NULL,
            embedding_dim = NULL
        "#,
        params![
            path.to_string_lossy().to_string(),
            root.to_string_lossy().to_string(),
            file_name,
            extension,
            size as i64,
            modified,
            width as i64,
            height as i64,
            description,
            keywords,
            dominant[0] as i64,
            dominant[1] as i64,
            dominant[2] as i64,
            visual_hash as i64,
            histogram_blob,
            color_histogram.len() as i64,
        ],
    )?;
    Ok(())
}

pub fn set_visual_descriptor(
    conn: &Connection,
    path: &Path,
    visual_hash: u64,
    color_histogram: &[f32],
) -> Result<()> {
    conn.execute(
        r#"
        UPDATE images
        SET visual_hash = ?2, color_histogram = ?3, color_histogram_dim = ?4
        WHERE path = ?1
        "#,
        params![
            path.to_string_lossy().to_string(),
            visual_hash as i64,
            encode_f32_vec(color_histogram),
            color_histogram.len() as i64,
        ],
    )?;
    Ok(())
}

pub fn paths_missing_visual_descriptor(conn: &Connection) -> Result<Vec<PathBuf>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM images WHERE visual_hash IS NULL OR color_histogram IS NULL ORDER BY path",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).map(PathBuf::from).collect())
}

pub fn set_embedding(conn: &Connection, path: &Path, embedding: &[f32]) -> Result<()> {
    conn.execute(
        "UPDATE images SET embedding = ?2, embedding_dim = ?3 WHERE path = ?1",
        params![
            path.to_string_lossy().to_string(),
            encode_f32_vec(embedding),
            embedding.len() as i64
        ],
    )?;
    Ok(())
}

pub fn paths_missing_embedding(conn: &Connection) -> Result<Vec<PathBuf>> {
    let mut stmt = conn.prepare("SELECT path FROM images WHERE embedding IS NULL ORDER BY path")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).map(PathBuf::from).collect())
}

pub fn next_scan_generation(conn: &Connection) -> Result<i64> {
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(last_seen_scan), 0) FROM images",
        [],
        |row| row.get(0),
    )?;
    if current == i64::MAX {
        conn.execute("UPDATE images SET last_seen_scan = 0", [])?;
        Ok(1)
    } else {
        Ok((current + 1).max(1))
    }
}

pub fn mark_paths_seen<'a, I>(conn: &mut Connection, generation: i64, paths: I) -> Result<usize>
where
    I: IntoIterator<Item = &'a PathBuf>,
{
    let tx = conn.transaction()?;
    let mut updated = 0usize;
    {
        let mut stmt = tx.prepare("UPDATE images SET last_seen_scan = ?1 WHERE path = ?2")?;
        for path in paths {
            updated += stmt.execute(params![generation, path.to_string_lossy().to_string()])?;
        }
    }
    tx.commit()?;
    Ok(updated)
}

pub fn delete_stale_for_root(conn: &Connection, root: &Path, generation: i64) -> Result<usize> {
    let root_text = root.to_string_lossy().to_string();
    Ok(conn.execute(
        "DELETE FROM images WHERE root = ?1 AND last_seen_scan <> ?2",
        params![root_text, generation],
    )?)
}

pub fn load_image_summaries(db_path: &Path) -> Result<Vec<ImageSummary>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT path, root, file_name, extension, size, modified, width, height,
               description, keywords, dominant_r, dominant_g, dominant_b
        FROM images
        ORDER BY file_name COLLATE NOCASE
        "#,
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(ImageSummary {
            path: PathBuf::from(row.get::<_, String>(0)?),
            root: PathBuf::from(row.get::<_, String>(1)?),
            file_name: row.get(2)?,
            extension: row.get(3)?,
            size: row.get::<_, i64>(4)?.max(0) as u64,
            modified: row.get(5)?,
            width: row.get::<_, i64>(6)?.max(0) as u32,
            height: row.get::<_, i64>(7)?.max(0) as u32,
            description: row.get(8)?,
            keywords: row.get(9)?,
            dominant: [
                row.get::<_, i64>(10)?.clamp(0, 255) as u8,
                row.get::<_, i64>(11)?.clamp(0, 255) as u8,
                row.get::<_, i64>(12)?.clamp(0, 255) as u8,
            ],
            score: None,
        })
    })?;

    Ok(rows.filter_map(|row| row.ok()).collect())
}

pub fn load_images(db_path: &Path) -> Result<Vec<ImageRecord>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT path, root, file_name, extension, size, modified, width, height,
               description, keywords, dominant_r, dominant_g, dominant_b,
               visual_hash, color_histogram, color_histogram_dim,
               embedding, embedding_dim
        FROM images
        ORDER BY file_name COLLATE NOCASE
        "#,
    )?;

    let rows = stmt.query_map([], |row| {
        let histogram_blob: Option<Vec<u8>> = row.get(14)?;
        let histogram_dim: Option<i64> = row.get(15)?;
        let color_histogram = histogram_blob
            .and_then(|bytes| decode_f32_vec(&bytes, histogram_dim.unwrap_or(0) as usize));

        let embedding_blob: Option<Vec<u8>> = row.get(16)?;
        let embedding_dim: Option<i64> = row.get(17)?;
        let embedding = embedding_blob
            .and_then(|bytes| decode_f32_vec(&bytes, embedding_dim.unwrap_or(0) as usize));

        let visual_hash_signed: Option<i64> = row.get(13)?;
        Ok(ImageRecord {
            path: PathBuf::from(row.get::<_, String>(0)?),
            root: PathBuf::from(row.get::<_, String>(1)?),
            file_name: row.get(2)?,
            extension: row.get(3)?,
            size: row.get::<_, i64>(4)?.max(0) as u64,
            modified: row.get(5)?,
            width: row.get::<_, i64>(6)?.max(0) as u32,
            height: row.get::<_, i64>(7)?.max(0) as u32,
            description: row.get(8)?,
            keywords: row.get(9)?,
            dominant: [
                row.get::<_, i64>(10)?.clamp(0, 255) as u8,
                row.get::<_, i64>(11)?.clamp(0, 255) as u8,
                row.get::<_, i64>(12)?.clamp(0, 255) as u8,
            ],
            visual_hash: visual_hash_signed.map(|value| value as u64),
            color_histogram,
            embedding,
            score: None,
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn encode_f32_vec(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decode_f32_vec(bytes: &[u8], dim: usize) -> Option<Vec<f32>> {
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut values = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        values.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    if dim != 0 && dim != values.len() {
        return None;
    }
    Some(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "windows-image-search-{label}-{}-{nonce}.sqlite3",
            std::process::id()
        ))
    }

    #[test]
    fn scan_generation_prunes_stale_rows_only_after_explicit_cleanup() {
        let db_path = temp_db_path("scan-generation");
        let root = std::env::temp_dir().join("windows-image-search-scan-root");
        let present = root.join("present.jpg");
        let stale = root.join("stale.jpg");

        {
            let mut conn = open(&db_path).unwrap();
            for (path, name) in [(&present, "present.jpg"), (&stale, "stale.jpg")] {
                upsert_image(
                    &conn,
                    path,
                    &root,
                    name,
                    "jpg",
                    100,
                    200,
                    16,
                    16,
                    "",
                    "",
                    [1, 2, 3],
                    42,
                    &[1.0],
                )
                .unwrap();
            }

            let generation = next_scan_generation(&conn).unwrap();
            mark_paths_seen(&mut conn, generation, std::iter::once(&present)).unwrap();

            // Simulate an interruption before cleanup: stale data must still exist.
            assert_eq!(load_file_states(&conn).unwrap().len(), 2);

            assert_eq!(delete_stale_for_root(&conn, &root, generation).unwrap(), 1);
            let states = load_file_states(&conn).unwrap();
            assert!(states.contains_key(&present));
            assert!(!states.contains_key(&stale));
        }

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }

    #[test]
    fn lightweight_summaries_match_metadata_without_feature_blobs() {
        let db_path = temp_db_path("lightweight-summary");
        let root = std::env::temp_dir().join("windows-image-search-summary-root");
        let image = root.join("sample.jpg");

        {
            let conn = open(&db_path).unwrap();
            upsert_image(
                &conn,
                &image,
                &root,
                "sample.jpg",
                "jpg",
                1234,
                5678,
                320,
                240,
                "description",
                "keyword",
                [12, 34, 56],
                0x1234,
                &[0.2, 0.8],
            )
            .unwrap();
            set_embedding(&conn, &image, &[0.1, 0.2, 0.3, 0.4]).unwrap();
        }

        let full = load_images(&db_path).unwrap();
        let summaries = load_image_summaries(&db_path).unwrap();
        assert_eq!(full.len(), 1);
        assert_eq!(summaries.len(), 1);
        assert!(full[0].embedding.is_some());
        assert!(full[0].color_histogram.is_some());
        assert_eq!(summaries[0].path, full[0].path);
        assert_eq!(summaries[0].file_name, full[0].file_name);
        assert_eq!(summaries[0].description, full[0].description);
        assert_eq!(summaries[0].dominant, full[0].dominant);

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }

    #[test]
    fn load_file_states_returns_all_persisted_rows() {
        let db_path = temp_db_path("file-state-cache");
        let root = std::env::temp_dir().join("windows-image-search-indexed-root");
        let first = root.join("first.jpg");
        let second = root.join("second.jpg");

        {
            let conn = open(&db_path).unwrap();
            upsert_image(
                &conn,
                &first,
                &root,
                "first.jpg",
                "jpg",
                111,
                1001,
                32,
                32,
                "",
                "",
                [10, 20, 30],
                0xAA55,
                &[1.0, 0.0],
            )
            .unwrap();
            upsert_image(
                &conn,
                &second,
                &root,
                "second.jpg",
                "jpg",
                222,
                2002,
                64,
                64,
                "",
                "",
                [40, 50, 60],
                0x55AA,
                &[0.0, 1.0],
            )
            .unwrap();
            set_embedding(&conn, &second, &[0.25, 0.75]).unwrap();

            let states = load_file_states(&conn).unwrap();
            assert_eq!(states.len(), 2);
            assert_eq!(
                states.get(&first),
                Some(&FileState {
                    size: 111,
                    modified: 1001,
                    has_embedding: false,
                })
            );
            assert_eq!(
                states.get(&second),
                Some(&FileState {
                    size: 222,
                    modified: 2002,
                    has_embedding: true,
                })
            );
        }

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }
}
