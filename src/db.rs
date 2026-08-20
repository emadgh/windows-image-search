use crate::material_texture;
use anyhow::{bail, Context, Result};
use rusqlite::{params, params_from_iter, Connection};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ImageRecord {
    pub rowid: usize,
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
    pub material_texture: Option<Vec<f32>>,
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
    conn.pragma_update(None, "foreign_keys", "ON")?;
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
            material_texture BLOB,
            material_texture_dim INTEGER,
            material_texture_version INTEGER NOT NULL DEFAULT 0,
            embedding BLOB,
            embedding_dim INTEGER,
            embedding_normalized INTEGER NOT NULL DEFAULT 0,
            last_seen_scan INTEGER NOT NULL DEFAULT 0,
            content_fingerprint INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_images_root ON images(root);
        CREATE INDEX IF NOT EXISTS idx_images_file_name ON images(file_name);

        CREATE TABLE IF NOT EXISTS discovered_images (
            path TEXT PRIMARY KEY NOT NULL,
            root TEXT NOT NULL,
            last_seen_scan INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_discovered_images_root
            ON discovered_images(root);
        CREATE INDEX IF NOT EXISTS idx_discovered_images_root_scan
            ON discovered_images(root, last_seen_scan);

        CREATE TABLE IF NOT EXISTS collections (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL COLLATE NOCASE UNIQUE
        );

        CREATE TABLE IF NOT EXISTS collection_folders (
            collection_id INTEGER NOT NULL,
            folder_path TEXT NOT NULL COLLATE NOCASE,
            PRIMARY KEY(collection_id, folder_path),
            FOREIGN KEY(collection_id) REFERENCES collections(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS collection_files (
            collection_id INTEGER NOT NULL,
            file_path TEXT NOT NULL COLLATE NOCASE,
            PRIMARY KEY(collection_id, file_path),
            FOREIGN KEY(collection_id) REFERENCES collections(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_collection_folders_collection
            ON collection_folders(collection_id);
        CREATE INDEX IF NOT EXISTS idx_collection_files_collection
            ON collection_files(collection_id);
        "#,
    )?;

    // v0.1.0 databases predate the hybrid visual descriptors. SQLite's
    // CREATE TABLE IF NOT EXISTS does not add new columns, so migrate them
    // explicitly and leave existing rows NULL until the next rescan/search.
    ensure_column(&conn, "images", "visual_hash", "INTEGER")?;
    ensure_column(&conn, "images", "color_histogram", "BLOB")?;
    ensure_column(&conn, "images", "color_histogram_dim", "INTEGER")?;
    ensure_column(&conn, "images", "material_texture", "BLOB")?;
    ensure_column(&conn, "images", "material_texture_dim", "INTEGER")?;
    ensure_column(
        &conn,
        "images",
        "material_texture_version",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &conn,
        "images",
        "embedding_normalized",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &conn,
        "images",
        "last_seen_scan",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(&conn, "images", "content_fingerprint", "INTEGER")?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_images_root_scan ON images(root, last_seen_scan)",
        [],
    )?;
    ensure_text_search_index(&conn)?;

    Ok(conn)
}

fn ensure_text_search_index(conn: &Connection) -> Result<()> {
    let existed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'images_fts')",
        [],
        |row| row.get(0),
    )?;

    conn.execute_batch(
        r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS images_fts USING fts5(
            file_name,
            path,
            description,
            keywords,
            content='images',
            content_rowid='rowid',
            tokenize='trigram'
        );

        CREATE TRIGGER IF NOT EXISTS images_fts_ai AFTER INSERT ON images BEGIN
            INSERT INTO images_fts(rowid, file_name, path, description, keywords)
            VALUES (new.rowid, new.file_name, new.path, new.description, new.keywords);
        END;

        CREATE TRIGGER IF NOT EXISTS images_fts_ad AFTER DELETE ON images BEGIN
            INSERT INTO images_fts(images_fts, rowid, file_name, path, description, keywords)
            VALUES ('delete', old.rowid, old.file_name, old.path, old.description, old.keywords);
        END;

        CREATE TRIGGER IF NOT EXISTS images_fts_au
        AFTER UPDATE OF file_name, path, description, keywords ON images BEGIN
            INSERT INTO images_fts(images_fts, rowid, file_name, path, description, keywords)
            VALUES ('delete', old.rowid, old.file_name, old.path, old.description, old.keywords);
            INSERT INTO images_fts(rowid, file_name, path, description, keywords)
            VALUES (new.rowid, new.file_name, new.path, new.description, new.keywords);
        END;
        "#,
    )?;

    if !existed {
        conn.execute("INSERT INTO images_fts(images_fts) VALUES('rebuild')", [])?;
    }
    Ok(())
}

fn fts_phrase(token: &str) -> String {
    format!("\"{}\"", token.replace('"', "\"\""))
}

fn like_pattern(token: &str) -> String {
    let escaped = token
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

pub fn search_text(conn: &Connection, query: &str) -> Result<Vec<PathBuf>> {
    let tokens: Vec<&str> = query
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        return Ok(Vec::new());
    }

    if tokens.iter().all(|token| token.chars().count() >= 3) {
        let expression = tokens
            .iter()
            .map(|token| fts_phrase(token))
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut stmt = conn.prepare(
            "SELECT images.path FROM images_fts JOIN images ON images.rowid = images_fts.rowid WHERE images_fts MATCH ?1 ORDER BY bm25(images_fts)",
        )?;
        let rows = stmt.query_map(params![expression], |row| row.get::<_, String>(0))?;
        return Ok(rows.filter_map(|row| row.ok()).map(PathBuf::from).collect());
    }

    // FTS5's trigram tokenizer cannot satisfy one- and two-character substring
    // queries. Preserve the old contains semantics with a parameterized LIKE
    // fallback on this background search connection.
    let clause = "(file_name LIKE ? ESCAPE '\\' COLLATE NOCASE OR path LIKE ? ESCAPE '\\' COLLATE NOCASE OR description LIKE ? ESCAPE '\\' COLLATE NOCASE OR keywords LIKE ? ESCAPE '\\' COLLATE NOCASE)";
    let sql = format!(
        "SELECT path FROM images WHERE {} ORDER BY file_name COLLATE NOCASE",
        std::iter::repeat_n(clause, tokens.len())
            .collect::<Vec<_>>()
            .join(" AND ")
    );
    let mut values = Vec::<String>::with_capacity(tokens.len() * 4);
    for token in tokens {
        let pattern = like_pattern(token);
        for _ in 0..4 {
            values.push(pattern.clone());
        }
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values.iter()), |row| {
        row.get::<_, String>(0)
    })?;
    Ok(rows.filter_map(|row| row.ok()).map(PathBuf::from).collect())
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
    tx.execute(
        "DELETE FROM discovered_images WHERE root = ?1",
        params![root_text],
    )?;
    tx.commit()?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collection {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CollectionMembership {
    pub folders: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
}

pub fn load_collections(db_path: &Path) -> Result<Vec<Collection>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare("SELECT id, name FROM collections ORDER BY name COLLATE NOCASE")?;
    let rows = stmt.query_map([], |row| {
        Ok(Collection {
            id: row.get(0)?,
            name: row.get(1)?,
        })
    })?;
    Ok(rows.filter_map(|row| row.ok()).collect())
}

pub fn create_collection(db_path: &Path, name: &str) -> Result<Collection> {
    let name = name.trim();
    if name.is_empty() {
        bail!("collection name cannot be empty");
    }
    let conn = open(db_path)?;
    conn.execute("INSERT INTO collections(name) VALUES(?1)", params![name])?;
    Ok(Collection {
        id: conn.last_insert_rowid(),
        name: name.to_owned(),
    })
}

pub fn rename_collection(db_path: &Path, collection_id: i64, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("collection name cannot be empty");
    }
    let conn = open(db_path)?;
    let changed = conn.execute(
        "UPDATE collections SET name = ?2 WHERE id = ?1",
        params![collection_id, name],
    )?;
    if changed == 0 {
        bail!("collection no longer exists");
    }
    Ok(())
}

pub fn delete_collection(db_path: &Path, collection_id: i64) -> Result<()> {
    let conn = open(db_path)?;
    conn.execute(
        "DELETE FROM collections WHERE id = ?1",
        params![collection_id],
    )?;
    Ok(())
}

pub fn add_collection_folders(
    db_path: &Path,
    collection_id: i64,
    folders: &[PathBuf],
) -> Result<usize> {
    let mut conn = open(db_path)?;
    let tx = conn.transaction()?;
    let mut inserted = 0usize;
    {
        let mut stmt = tx.prepare(
            "INSERT OR IGNORE INTO collection_folders(collection_id, folder_path) VALUES(?1, ?2)",
        )?;
        for folder in folders {
            inserted +=
                stmt.execute(params![collection_id, folder.to_string_lossy().to_string()])?;
        }
    }
    tx.commit()?;
    Ok(inserted)
}

pub fn add_collection_files(
    db_path: &Path,
    collection_id: i64,
    files: &[PathBuf],
) -> Result<usize> {
    let mut conn = open(db_path)?;
    let tx = conn.transaction()?;
    let mut inserted = 0usize;
    {
        let mut stmt = tx.prepare(
            "INSERT OR IGNORE INTO collection_files(collection_id, file_path) VALUES(?1, ?2)",
        )?;
        for file in files {
            inserted += stmt.execute(params![collection_id, file.to_string_lossy().to_string()])?;
        }
    }
    tx.commit()?;
    Ok(inserted)
}

pub fn remove_collection_folder(db_path: &Path, collection_id: i64, folder: &Path) -> Result<()> {
    let conn = open(db_path)?;
    conn.execute(
        "DELETE FROM collection_folders WHERE collection_id = ?1 AND folder_path = ?2",
        params![collection_id, folder.to_string_lossy().to_string()],
    )?;
    Ok(())
}

pub fn remove_collection_file(db_path: &Path, collection_id: i64, file: &Path) -> Result<()> {
    let conn = open(db_path)?;
    conn.execute(
        "DELETE FROM collection_files WHERE collection_id = ?1 AND file_path = ?2",
        params![collection_id, file.to_string_lossy().to_string()],
    )?;
    Ok(())
}

pub fn load_collection_membership(
    db_path: &Path,
    collection_id: i64,
) -> Result<CollectionMembership> {
    let conn = open(db_path)?;
    let folders = {
        let mut stmt = conn.prepare(
            "SELECT folder_path FROM collection_folders WHERE collection_id = ?1 ORDER BY folder_path COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![collection_id], |row| row.get::<_, String>(0))?;
        rows.filter_map(|row| row.ok()).map(PathBuf::from).collect()
    };
    let files = {
        let mut stmt = conn.prepare(
            "SELECT file_path FROM collection_files WHERE collection_id = ?1 ORDER BY file_path COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![collection_id], |row| row.get::<_, String>(0))?;
        rows.filter_map(|row| row.ok()).map(PathBuf::from).collect()
    };
    Ok(CollectionMembership { folders, files })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileState {
    pub size: u64,
    pub modified: i64,
    pub width: u32,
    pub height: u32,
    pub content_fingerprint: Option<u64>,
    pub has_embedding: bool,
}

pub fn load_file_states(conn: &Connection) -> Result<HashMap<PathBuf, FileState>> {
    let mut stmt = conn.prepare(
        "SELECT path, size, modified, width, height, content_fingerprint, embedding IS NOT NULL FROM images",
    )?;
    let rows = stmt.query_map([], |row| {
        let path = PathBuf::from(row.get::<_, String>(0)?);
        let size = row.get::<_, i64>(1)?.max(0) as u64;
        let modified = row.get::<_, i64>(2)?;
        let width = row.get::<_, i64>(3)?.max(0) as u32;
        let height = row.get::<_, i64>(4)?.max(0) as u32;
        let content_fingerprint = row.get::<_, Option<i64>>(5)?.map(|value| value as u64);
        let has_embedding = row.get::<_, bool>(6)?;
        Ok((
            path,
            FileState {
                size,
                modified,
                width,
                height,
                content_fingerprint,
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
            material_texture = NULL,
            material_texture_dim = NULL,
            material_texture_version = 0,
            embedding = NULL,
            embedding_dim = NULL,
            embedding_normalized = 0,
            content_fingerprint = NULL
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

pub fn set_material_texture(conn: &Connection, path: &Path, descriptor: &[f32]) -> Result<()> {
    conn.execute(
        r#"
        UPDATE images
        SET material_texture = ?2, material_texture_dim = ?3, material_texture_version = ?4
        WHERE path = ?1
        "#,
        params![
            path.to_string_lossy().to_string(),
            encode_f32_vec(descriptor),
            descriptor.len() as i64,
            material_texture::VERSION,
        ],
    )?;
    Ok(())
}

pub fn set_content_fingerprint(conn: &Connection, path: &Path, fingerprint: u64) -> Result<()> {
    conn.execute(
        "UPDATE images SET content_fingerprint = ?2 WHERE path = ?1",
        params![path.to_string_lossy().to_string(), fingerprint as i64],
    )?;
    Ok(())
}

pub fn paths_missing_visual_descriptor(conn: &Connection) -> Result<Vec<PathBuf>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM images WHERE visual_hash IS NULL OR color_histogram IS NULL OR material_texture IS NULL OR material_texture_version <> ?1 ORDER BY path",
    )?;
    let rows = stmt.query_map(params![material_texture::VERSION], |row| {
        row.get::<_, String>(0)
    })?;
    Ok(rows.filter_map(|r| r.ok()).map(PathBuf::from).collect())
}

pub fn set_embedding(conn: &Connection, path: &Path, embedding: &[f32]) -> Result<()> {
    let normalized = normalized_f32_vec(embedding);
    conn.execute(
        "UPDATE images SET embedding = ?2, embedding_dim = ?3, embedding_normalized = 1 WHERE path = ?1",
        params![
            path.to_string_lossy().to_string(),
            encode_f32_vec(&normalized),
            normalized.len() as i64
        ],
    )?;
    Ok(())
}

pub fn paths_missing_embedding(conn: &Connection) -> Result<Vec<PathBuf>> {
    let mut stmt = conn.prepare("SELECT path FROM images WHERE embedding IS NULL ORDER BY path")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).map(PathBuf::from).collect())
}

fn like_prefix_pattern(path: &Path) -> String {
    let mut text = path.to_string_lossy().to_string();
    if !text.ends_with(std::path::MAIN_SEPARATOR) {
        text.push(std::path::MAIN_SEPARATOR);
    }
    let escaped = text
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("{escaped}%")
}

pub fn delete_path_tree(conn: &Connection, target: &Path) -> Result<Vec<PathBuf>> {
    let target_text = target.to_string_lossy().to_string();
    let prefix = like_prefix_pattern(target);
    let mut stmt = conn.prepare(
        "SELECT path FROM images WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\' COLLATE NOCASE",
    )?;
    let paths: Vec<PathBuf> = stmt
        .query_map(params![target_text, prefix], |row| row.get::<_, String>(0))?
        .filter_map(|row| row.ok())
        .map(PathBuf::from)
        .collect();
    drop(stmt);

    if !paths.is_empty() {
        conn.execute(
            "DELETE FROM images WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\' COLLATE NOCASE",
            params![
                target.to_string_lossy().to_string(),
                like_prefix_pattern(target)
            ],
        )?;
    }
    Ok(paths)
}

pub fn next_scan_generation(conn: &Connection) -> Result<i64> {
    let current: i64 = conn.query_row(
        "SELECT MAX(value) FROM (SELECT COALESCE(MAX(last_seen_scan), 0) AS value FROM images UNION ALL SELECT COALESCE(MAX(last_seen_scan), 0) AS value FROM discovered_images)",
        [],
        |row| row.get::<_, Option<i64>>(0).map(|value| value.unwrap_or(0)),
    )?;
    if current == i64::MAX {
        conn.execute("UPDATE images SET last_seen_scan = 0", [])?;
        conn.execute("UPDATE discovered_images SET last_seen_scan = 0", [])?;
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

pub fn mark_discovered_paths_seen(
    conn: &mut Connection,
    generation: i64,
    candidates: &[(PathBuf, PathBuf)],
) -> Result<usize> {
    let tx = conn.transaction()?;
    let mut updated = 0usize;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO discovered_images(path, root, last_seen_scan) VALUES(?1, ?2, ?3) ON CONFLICT(path) DO UPDATE SET root = excluded.root, last_seen_scan = excluded.last_seen_scan",
        )?;
        for (root, path) in candidates {
            updated += stmt.execute(params![
                path.to_string_lossy().to_string(),
                root.to_string_lossy().to_string(),
                generation,
            ])?;
        }
    }
    tx.commit()?;
    Ok(updated)
}

pub fn delete_stale_discovered_for_root(
    conn: &Connection,
    root: &Path,
    generation: i64,
) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM discovered_images WHERE root = ?1 AND last_seen_scan <> ?2",
        params![root.to_string_lossy().to_string(), generation],
    )?)
}

pub fn load_discovered_paths(db_path: &Path) -> Result<Vec<PathBuf>> {
    let conn = open(db_path)?;
    let mut stmt =
        conn.prepare("SELECT path FROM discovered_images ORDER BY path COLLATE NOCASE")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.filter_map(|row| row.ok()).map(PathBuf::from).collect())
}

pub fn load_root_counts(db_path: &Path) -> Result<HashMap<PathBuf, (usize, usize)>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT r.path,
               (SELECT COUNT(*) FROM discovered_images d WHERE d.root = r.path),
               (SELECT COUNT(*) FROM images i WHERE i.root = r.path)
        FROM roots r
        ORDER BY r.path COLLATE NOCASE
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            PathBuf::from(row.get::<_, String>(0)?),
            row.get::<_, i64>(1)?.max(0) as usize,
            row.get::<_, i64>(2)?.max(0) as usize,
        ))
    })?;
    let mut counts = HashMap::new();
    for row in rows {
        let (root, discovered, indexed) = row?;
        counts.insert(root, (discovered.max(indexed), indexed));
    }
    Ok(counts)
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

#[derive(Clone, Debug)]
pub struct AnnEmbedding {
    pub rowid: usize,
    pub embedding: Vec<f32>,
}

pub fn load_search_images(db_path: &Path) -> Result<Vec<ImageRecord>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT path, root, file_name, extension, size, modified, width, height,
               description, keywords, dominant_r, dominant_g, dominant_b,
               visual_hash, color_histogram, color_histogram_dim,
               embedding_normalized, rowid,
               material_texture, material_texture_dim, material_texture_version
        FROM images
        ORDER BY file_name COLLATE NOCASE
        "#,
    )?;

    let rows = stmt.query_map([], |row| {
        let histogram_blob: Option<Vec<u8>> = row.get(14)?;
        let histogram_dim: Option<i64> = row.get(15)?;
        let color_histogram = histogram_blob
            .and_then(|bytes| decode_f32_vec(&bytes, histogram_dim.unwrap_or(0) as usize));
        let visual_hash_signed: Option<i64> = row.get(13)?;
        let material_texture_blob: Option<Vec<u8>> = row.get(18)?;
        let material_texture_dim: Option<i64> = row.get(19)?;
        let material_texture_version = row.get::<_, i64>(20)?;
        let material_texture = if material_texture_version == material_texture::VERSION {
            material_texture_blob.and_then(|bytes| {
                decode_f32_vec(&bytes, material_texture_dim.unwrap_or(0).max(0) as usize)
            })
        } else {
            None
        };
        Ok(ImageRecord {
            rowid: row.get::<_, i64>(17)?.max(0) as usize,
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
            material_texture,
            score: None,
        })
    })?;
    Ok(rows.filter_map(|row| row.ok()).collect())
}

pub fn load_embeddings_for_rowids(
    db_path: &Path,
    rowids: &HashSet<usize>,
) -> Result<HashMap<usize, (Vec<f32>, bool)>> {
    if rowids.is_empty() {
        return Ok(HashMap::new());
    }
    let conn = open(db_path)?;
    let mut output = HashMap::with_capacity(rowids.len());
    let mut ids: Vec<usize> = rowids.iter().copied().collect();
    ids.sort_unstable();

    for chunk in ids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT rowid, embedding, embedding_dim, embedding_normalized FROM images WHERE rowid IN ({placeholders}) AND embedding IS NOT NULL"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params = chunk.iter().map(|id| *id as i64);
        let rows = stmt.query_map(params_from_iter(params), |row| {
            let rowid = row.get::<_, i64>(0)?.max(0) as usize;
            let bytes: Vec<u8> = row.get(1)?;
            let dim = row.get::<_, i64>(2)?.max(0) as usize;
            let normalized = row.get::<_, bool>(3)?;
            Ok((rowid, bytes, dim, normalized))
        })?;
        for row in rows {
            let (rowid, bytes, dim, normalized) = row?;
            if let Some(values) = decode_f32_vec(&bytes, dim) {
                output.insert(rowid, (values, normalized));
            }
        }
    }
    Ok(output)
}

pub fn load_ann_embeddings(db_path: &Path) -> Result<Vec<AnnEmbedding>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT rowid, embedding, embedding_dim, embedding_normalized FROM images WHERE embedding IS NOT NULL ORDER BY rowid",
    )?;
    let rows = stmt.query_map([], |row| {
        let rowid = row.get::<_, i64>(0)?.max(0) as usize;
        let bytes: Vec<u8> = row.get(1)?;
        let dim = row.get::<_, i64>(2)?.max(0) as usize;
        let normalized = row.get::<_, bool>(3)?;
        Ok((rowid, bytes, dim, normalized))
    })?;

    let mut output = Vec::new();
    for row in rows {
        let (rowid, bytes, dim, normalized) = row?;
        let Some(values) = decode_f32_vec(&bytes, dim) else {
            continue;
        };
        output.push(AnnEmbedding {
            rowid,
            embedding: if normalized {
                values
            } else {
                normalized_f32_vec(&values)
            },
        });
    }
    Ok(output)
}

pub fn ann_index_signature(db_path: &Path) -> Result<u64> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT rowid, path, size, modified, COALESCE(embedding_dim, 0), embedding_normalized FROM images WHERE embedding IS NOT NULL ORDER BY rowid",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, bool>(5)?,
        ))
    })?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    1_u32.hash(&mut hasher); // bump when embedding/index semantics change.
    for row in rows {
        let (rowid, path, size, modified, dim, normalized) = row?;
        rowid.hash(&mut hasher);
        path.hash(&mut hasher);
        size.hash(&mut hasher);
        modified.hash(&mut hasher);
        dim.hash(&mut hasher);
        normalized.hash(&mut hasher);
    }
    Ok(hasher.finish())
}

fn normalized_f32_vec(values: &[f32]) -> Vec<f32> {
    let norm_sq = values.iter().map(|value| value * value).sum::<f32>();
    if norm_sq <= f32::EPSILON {
        return values.to_vec();
    }
    let inverse = norm_sq.sqrt().recip();
    values.iter().map(|value| value * inverse).collect()
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
    fn collections_persist_deduplicate_recursive_membership_and_delete_safely() {
        let db_path = temp_db_path("collections");
        let root = std::env::temp_dir().join("windows-image-search-collection-root");
        let assigned = root.join("assigned");
        let first = assigned.join("first.jpg");
        let nested = assigned.join("nested").join("second.jpg");
        let manual = root.join("manual.jpg");

        {
            let conn = open(&db_path).unwrap();
            for path in [&first, &nested, &manual] {
                let name = path.file_name().unwrap().to_string_lossy();
                upsert_image(
                    &conn,
                    path,
                    &root,
                    &name,
                    "jpg",
                    1,
                    1,
                    8,
                    8,
                    "",
                    "",
                    [1, 2, 3],
                    1,
                    &[1.0],
                )
                .unwrap();
            }
        }

        let collection = create_collection(&db_path, "Materials").unwrap();
        add_collection_folders(&db_path, collection.id, std::slice::from_ref(&assigned)).unwrap();
        add_collection_files(&db_path, collection.id, &[first.clone(), manual.clone()]).unwrap();

        let persisted = load_collections(&db_path).unwrap();
        assert_eq!(persisted, vec![collection.clone()]);
        let membership = load_collection_membership(&db_path, collection.id).unwrap();
        assert_eq!(membership.folders, vec![assigned.clone()]);
        assert_eq!(membership.files.len(), 2);

        // Reconstruct the same effective membership used by the UI from the
        // persisted folder/file rules plus lightweight indexed summaries. A
        // file assigned both ways must still appear only once.
        let effective: std::collections::HashSet<PathBuf> = load_image_summaries(&db_path)
            .unwrap()
            .into_iter()
            .filter(|summary| {
                membership.files.iter().any(|file| file == &summary.path)
                    || membership
                        .folders
                        .iter()
                        .any(|folder| summary.path.starts_with(folder))
            })
            .map(|summary| summary.path)
            .collect();
        assert_eq!(effective.len(), 3);
        assert!(effective.contains(&first));
        assert!(effective.contains(&nested));
        assert!(effective.contains(&manual));

        rename_collection(&db_path, collection.id, "Stone Library").unwrap();
        assert_eq!(load_collections(&db_path).unwrap()[0].name, "Stone Library");

        delete_collection(&db_path, collection.id).unwrap();
        assert!(load_collections(&db_path).unwrap().is_empty());
        assert!(load_collection_membership(&db_path, collection.id)
            .unwrap()
            .folders
            .is_empty());
        // Deleting a collection must never delete indexed/source image records.
        assert_eq!(load_file_states(&open(&db_path).unwrap()).unwrap().len(), 3);

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }

    #[test]
    fn delete_path_tree_removes_only_the_requested_subtree() {
        let db_path = temp_db_path("delete-path-tree");
        let root = std::env::temp_dir().join("windows-image-search-delete-root");
        let folder = root.join("folder");
        let first = folder.join("first.jpg");
        let second = folder.join("nested").join("second.jpg");
        let keep = root.join("keep.jpg");

        {
            let conn = open(&db_path).unwrap();
            for path in [&first, &second, &keep] {
                let name = path.file_name().unwrap().to_string_lossy();
                upsert_image(
                    &conn,
                    path,
                    &root,
                    &name,
                    "jpg",
                    1,
                    1,
                    8,
                    8,
                    "",
                    "",
                    [1, 2, 3],
                    1,
                    &[1.0],
                )
                .unwrap();
            }
            let removed = delete_path_tree(&conn, &folder).unwrap();
            assert_eq!(removed.len(), 2);
            let states = load_file_states(&conn).unwrap();
            assert_eq!(states.len(), 1);
            assert!(states.contains_key(&keep));
        }

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }

    #[test]
    fn fts_text_search_supports_substrings_and_and_semantics() {
        let db_path = temp_db_path("fts-text-search");
        let root = std::env::temp_dir().join("windows-image-search-fts-root");
        let brown = root.join("BrownMarble_A01.jpg");
        let gray = root.join("SilverCement_B02.jpg");

        {
            let conn = open(&db_path).unwrap();
            upsert_image(
                &conn,
                &brown,
                &root,
                "BrownMarble_A01.jpg",
                "jpg",
                1,
                1,
                32,
                32,
                "warm stone with gold veins",
                "brown marble polished",
                [120, 70, 40],
                1,
                &[1.0],
            )
            .unwrap();
            upsert_image(
                &conn,
                &gray,
                &root,
                "SilverCement_B02.jpg",
                "jpg",
                1,
                1,
                32,
                32,
                "cool concrete texture",
                "gray cement",
                [130, 130, 130],
                2,
                &[1.0],
            )
            .unwrap();

            let substring = search_text(&conn, "marb").unwrap();
            assert_eq!(substring, vec![brown.clone()]);

            let and_query = search_text(&conn, "brown vein").unwrap();
            assert_eq!(and_query, vec![brown.clone()]);

            let short_fallback = search_text(&conn, "A0").unwrap();
            assert_eq!(short_fallback, vec![brown.clone()]);

            // Updating searchable metadata must refresh FTS, while embedding-only
            // updates do not need to touch the FTS index.
            upsert_image(
                &conn,
                &brown,
                &root,
                "BrownMarble_A01.jpg",
                "jpg",
                2,
                2,
                32,
                32,
                "warm stone without the previous metallic term",
                "brown marble polished",
                [120, 70, 40],
                1,
                &[1.0],
            )
            .unwrap();
            assert!(search_text(&conn, "gold").unwrap().is_empty());
        }

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
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

        let search_records = load_search_images(&db_path).unwrap();
        let summaries = load_image_summaries(&db_path).unwrap();
        assert_eq!(search_records.len(), 1);
        assert_eq!(summaries.len(), 1);
        assert!(search_records[0].color_histogram.is_some());
        assert_eq!(summaries[0].path, search_records[0].path);
        assert_eq!(summaries[0].file_name, search_records[0].file_name);
        assert_eq!(summaries[0].description, search_records[0].description);
        assert_eq!(summaries[0].dominant, search_records[0].dominant);

        // Heavy CLIP vectors stay out of UI/search records and are loaded only
        // for the rowids selected by the candidate stage.
        let rowids = std::collections::HashSet::from([search_records[0].rowid]);
        let embeddings = load_embeddings_for_rowids(&db_path, &rowids).unwrap();
        let (embedding, normalized) = embeddings.get(&search_records[0].rowid).unwrap();
        assert_eq!(embedding.len(), 4);
        assert!(*normalized);

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
                    width: 32,
                    height: 32,
                    content_fingerprint: None,
                    has_embedding: false,
                })
            );
            assert_eq!(
                states.get(&second),
                Some(&FileState {
                    size: 222,
                    modified: 2002,
                    width: 64,
                    height: 64,
                    content_fingerprint: None,
                    has_embedding: true,
                })
            );
        }

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }
}
