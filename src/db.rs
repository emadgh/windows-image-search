use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
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
            embedding_dim INTEGER
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

    Ok(conn)
}

fn ensure_column(conn: &Connection, table: &str, column: &str, declaration: &str) -> Result<()> {
    let exists = {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        stmt.query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|row| row.ok())
            .any(|name| name == column)
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

pub fn existing_file_state(conn: &Connection, path: &Path) -> Result<Option<(u64, i64, bool)>> {
    let path_text = path.to_string_lossy().to_string();
    let state = conn
        .query_row(
            "SELECT size, modified, embedding IS NOT NULL FROM images WHERE path = ?1",
            params![path_text],
            |row| {
                let size: i64 = row.get(0)?;
                let modified: i64 = row.get(1)?;
                let has_embedding: bool = row.get(2)?;
                Ok((size.max(0) as u64, modified, has_embedding))
            },
        )
        .optional()?;
    Ok(state)
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

pub fn delete_missing_for_root(conn: &Connection, root: &Path, seen: &[PathBuf]) -> Result<usize> {
    let root_text = root.to_string_lossy().to_string();
    let mut stmt = conn.prepare("SELECT path FROM images WHERE root = ?1")?;
    let existing: Vec<String> = stmt
        .query_map(params![root_text], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    let seen_set: std::collections::HashSet<String> = seen
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let mut removed = 0;
    for path in existing {
        if !seen_set.contains(&path) {
            removed += conn.execute("DELETE FROM images WHERE path = ?1", params![path])?;
        }
    }
    Ok(removed)
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
