use crate::{ann, db, face_store, thumbnail_cache};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const INDEX_DIR_NAME: &str = ".imagesearch";
pub const INDEX_DB_NAME: &str = "index.sqlite3";
pub const THUMBNAIL_DIR_NAME: &str = "thumbnails";
pub const ANN_DIR_NAME: &str = "ann-index";
pub(crate) const PORTABLE_SCHEMA_VERSION: i64 = 2;
const ATTACHED_DB: &str = "portable_root";

#[derive(Clone, Debug)]
pub struct AttachOutcome {
    pub library_id: String,
    pub images: usize,
    pub migrated_legacy_rows: bool,
    pub reused_existing_index: bool,
}

pub fn index_dir(root: &Path) -> PathBuf {
    root.join(INDEX_DIR_NAME)
}

pub fn index_db_path(root: &Path) -> PathBuf {
    index_dir(root).join(INDEX_DB_NAME)
}

pub fn thumbnail_dir(root: &Path) -> PathBuf {
    index_dir(root).join(THUMBNAIL_DIR_NAME)
}

pub fn ann_dir(root: &Path) -> PathBuf {
    index_dir(root).join(ANN_DIR_NAME)
}

pub fn is_indexed_root(root: &Path) -> bool {
    index_db_path(root).is_file()
}

pub fn is_internal_path(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    matches!(
        relative.components().next(),
        Some(Component::Normal(name)) if name == INDEX_DIR_NAME
    )
}

pub fn relative_source_path(root: &Path, source: &Path) -> Result<PathBuf> {
    let relative = source.strip_prefix(root).with_context(|| {
        format!(
            "{} is outside indexed root {}",
            source.display(),
            root.display()
        )
    })?;
    if relative.as_os_str().is_empty() {
        bail!("indexed source path cannot be the root directory itself");
    }
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!("indexed source path is not a safe root-relative path");
    }
    Ok(relative.to_path_buf())
}

pub fn absolute_source_path(root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!(
            "portable index contains an unsafe source path: {}",
            relative.display()
        );
    }
    Ok(root.join(relative))
}

pub fn indexed_root_for_path<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|root| path.starts_with(root) && !is_internal_path(root, path))
        .max_by_key(|root| root.components().count())
}

pub fn prepare_registered_roots(session_db_path: &Path, roots: &[PathBuf]) -> Vec<String> {
    let mut warnings = Vec::new();
    for root in roots {
        if !root.exists() {
            warnings.push(format!("Portable index unavailable: {}", root.display()));
            continue;
        }
        if let Err(err) = attach_root(session_db_path, root) {
            warnings.push(format!(
                "Cannot attach portable index {}: {err:#}",
                root.display()
            ));
        }
    }
    warnings
}

pub fn attach_root(session_db_path: &Path, root: &Path) -> Result<AttachOutcome> {
    if !root.is_dir() {
        bail!("indexed root is unavailable: {}", root.display());
    }

    let portable_path = index_db_path(root);
    let existed_before = portable_path.is_file();
    ensure_portable_layout(root)?;
    let (library_id, ready, portable_count) = portable_identity(root)?;

    let session = db::open(session_db_path)?;
    ensure_session_registry(&session)?;
    let legacy_count = root_image_count(&session, root)?;
    drop(session);

    let mut migrated = false;
    if !ready {
        if legacy_count > 0 {
            replace_root_from_session(session_db_path, root)?;
            migrate_legacy_thumbnails(session_db_path, root);
            migrated = true;
        } else if portable_count > 0 {
            // Protect a populated portable DB if metadata was lost/upgraded.
            mark_portable_ready(root)?;
        } else {
            mark_portable_ready(root)?;
        }
    }

    let images = import_root_into_session(session_db_path, root, &library_id)?;
    // ANN is a derived cache; a missing/stale dump must never make attachment
    // fail because exact search can still use stored embeddings.
    let _ = refresh_ann(root);
    Ok(AttachOutcome {
        library_id,
        images,
        migrated_legacy_rows: migrated,
        reused_existing_index: existed_before && !migrated,
    })
}

pub fn sync_paths_from_session(conn: &mut Connection, paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }

    let mut grouped: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT root FROM images WHERE path = ?1")?;
        for path in paths {
            let root_text = stmt.query_row(params![path.to_string_lossy().to_string()], |row| {
                row.get::<_, String>(0)
            });
            let Ok(root_text) = root_text else {
                continue;
            };
            let root = PathBuf::from(root_text);
            if root.is_absolute() && root.exists() {
                grouped.entry(root).or_default().push(path.clone());
            }
        }
    }

    for (root, root_paths) in grouped {
        mirror_paths_for_root(conn, &root, &root_paths)?;
    }
    Ok(())
}

pub fn refresh_ann(root: &Path) -> Result<bool> {
    ann::prepare_index(&index_db_path(root))
}

fn migrate_legacy_thumbnails(session_db_path: &Path, root: &Path) {
    let fallback = thumbnail_cache::cache_dir_for_db(session_db_path);
    let Ok(conn) = db::open(session_db_path) else {
        return;
    };
    let Ok(mut stmt) = conn.prepare("SELECT path FROM images WHERE root = ?1") else {
        return;
    };
    let Ok(rows) = stmt.query_map(params![root.to_string_lossy().to_string()], |row| {
        row.get::<_, String>(0)
    }) else {
        return;
    };
    let sources: Vec<PathBuf> = rows.filter_map(|row| row.ok()).map(PathBuf::from).collect();
    drop(stmt);

    for source in sources {
        let old = thumbnail_cache::cache_path(&fallback, &source);
        if !old.is_file() {
            continue;
        }
        let Ok(new) = thumbnail_cache::cache_path_for_root(root, &source) else {
            continue;
        };
        if new.exists() {
            continue;
        }
        if let Some(parent) = new.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::copy(old, new);
    }
}

pub fn remove_absolute_paths(roots: &[PathBuf], paths: &[PathBuf]) -> Result<()> {
    let mut grouped: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for path in paths {
        if let Some(root) = indexed_root_for_path(path, roots) {
            grouped.entry(root.clone()).or_default().push(path.clone());
        }
    }

    for (root, values) in grouped {
        if !is_indexed_root(&root) {
            continue;
        }
        let conn = db::open(&index_db_path(&root))?;
        for absolute in values {
            let relative = relative_source_path(&root, &absolute)?;
            let _ = db::delete_path_tree(&conn, &relative)?;
        }
    }
    Ok(())
}

pub fn replace_root_from_session(session_db_path: &Path, root: &Path) -> Result<usize> {
    ensure_portable_layout(root)?;
    let portable_path = index_db_path(root);
    let mut conn = db::open(session_db_path)?;
    attach_database(&conn, &portable_path)?;

    let root_text = root.to_string_lossy().to_string();
    let prefix = root_prefix(root);
    let result = (|| -> Result<usize> {
        let tx = conn.transaction()?;
        tx.execute(
            &format!("INSERT OR IGNORE INTO {ATTACHED_DB}.roots(path) VALUES('.')"),
            [],
        )?;
        let copied = tx.execute(
            &format!(
                "INSERT INTO {ATTACHED_DB}.images({}) \
                 SELECT CASE WHEN path = ?1 THEN '' ELSE substr(path, length(?2) + 1) END, '', {} \
                 FROM main.images WHERE root = ?1 \
                 ON CONFLICT(path) DO UPDATE SET {}",
                image_columns_with_path(),
                image_columns_without_path_root(),
                image_update_assignments()
            ),
            params![root_text, prefix],
        )?;
        tx.execute(
            &format!(
                "DELETE FROM {ATTACHED_DB}.images \
                 WHERE path NOT IN (\
                    SELECT CASE WHEN path = ?1 THEN '' ELSE substr(path, length(?2) + 1) END \
                    FROM main.images WHERE root = ?1\
                 )"
            ),
            params![root_text, prefix],
        )?;
        set_attached_meta(&tx, "schema_version", &PORTABLE_SCHEMA_VERSION.to_string())?;
        set_attached_meta(&tx, "migration_complete", "1")?;
        tx.commit()?;
        Ok(copied)
    })();
    let _ = detach_database(&conn);
    result
}

fn mirror_paths_for_root(conn: &mut Connection, root: &Path, paths: &[PathBuf]) -> Result<()> {
    ensure_portable_layout(root)?;
    let portable_path = index_db_path(root);
    attach_database(conn, &portable_path)?;
    let root_text = root.to_string_lossy().to_string();

    let result = (|| -> Result<()> {
        let tx = conn.transaction()?;
        for absolute in paths {
            let relative = relative_source_path(root, absolute)?;
            let relative_text = relative.to_string_lossy().to_string();
            let absolute_text = absolute.to_string_lossy().to_string();
            tx.execute(
                &format!(
                    "INSERT INTO {ATTACHED_DB}.images({}) \
                     SELECT ?1, '', {} FROM main.images WHERE path = ?2 AND root = ?3 \
                     ON CONFLICT(path) DO UPDATE SET {}",
                    image_columns_with_path(),
                    image_columns_without_path_root(),
                    image_update_assignments()
                ),
                params![relative_text, absolute_text, root_text],
            )?;
        }
        set_attached_meta(&tx, "schema_version", &PORTABLE_SCHEMA_VERSION.to_string())?;
        set_attached_meta(&tx, "migration_complete", "1")?;
        tx.commit()?;
        Ok(())
    })();
    let _ = detach_database(conn);
    result
}

fn import_root_into_session(
    session_db_path: &Path,
    root: &Path,
    library_id: &str,
) -> Result<usize> {
    let portable_path = index_db_path(root);
    let mut conn = db::open(session_db_path)?;
    ensure_session_registry(&conn)?;
    relocate_registered_library_if_needed(&mut conn, library_id, root)?;
    attach_database(&conn, &portable_path)?;

    let root_text = root.to_string_lossy().to_string();
    let prefix = root_prefix(root);
    let result = (|| -> Result<usize> {
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO roots(path) VALUES(?1)",
            params![root_text],
        )?;
        tx.execute("DELETE FROM images WHERE root = ?1", params![root_text])?;
        let copied = tx.execute(
            &format!(
                "INSERT INTO main.images({}) \
                 SELECT CASE WHEN path = '' THEN ?1 ELSE ?2 || path END, ?1, {} \
                 FROM {ATTACHED_DB}.images",
                image_columns_with_path(),
                image_columns_without_path_root()
            ),
            params![root_text, prefix],
        )?;
        // A Windows drive letter/root path can later belong to another portable
        // library. Keep a one-to-one registry so stale library ids cannot claim the
        // new drive while the durable library id remains stored on the drive itself.
        tx.execute(
            "DELETE FROM portable_root_registry WHERE root_path = ?1 AND library_id <> ?2",
            params![root_text, library_id],
        )?;
        tx.execute(
            "INSERT INTO portable_root_registry(library_id, root_path) VALUES(?1, ?2) \
             ON CONFLICT(library_id) DO UPDATE SET root_path = excluded.root_path",
            params![library_id, root_text],
        )?;
        tx.commit()?;
        Ok(copied)
    })();
    let _ = detach_database(&conn);
    result
}

fn relocate_registered_library_if_needed(
    conn: &mut Connection,
    library_id: &str,
    new_root: &Path,
) -> Result<()> {
    let old_root: Option<String> = conn
        .query_row(
            "SELECT root_path FROM portable_root_registry WHERE library_id = ?1",
            params![library_id],
            |row| row.get(0),
        )
        .ok();
    let Some(old_root) = old_root else {
        return Ok(());
    };
    let old = PathBuf::from(&old_root);
    if old == new_root {
        return Ok(());
    }

    let tx = conn.transaction()?;
    relocate_collection_table(&tx, "collection_folders", "folder_path", &old, new_root)?;
    relocate_collection_table(&tx, "collection_files", "file_path", &old, new_root)?;
    tx.execute("DELETE FROM images WHERE root = ?1", params![old_root])?;
    tx.execute("DELETE FROM roots WHERE path = ?1", params![old_root])?;
    tx.commit()?;
    Ok(())
}

fn relocate_collection_table(
    conn: &Connection,
    table: &str,
    path_column: &str,
    old_root: &Path,
    new_root: &Path,
) -> Result<()> {
    let sql = format!("SELECT collection_id, {path_column} FROM {table}");
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|row| row.ok())
        .collect();
    drop(stmt);

    for (collection_id, stored) in rows {
        let old_path = PathBuf::from(&stored);
        let Ok(relative) = old_path.strip_prefix(old_root) else {
            continue;
        };
        let relocated = new_root.join(relative).to_string_lossy().to_string();
        conn.execute(
            &format!("INSERT OR IGNORE INTO {table}(collection_id, {path_column}) VALUES(?1, ?2)"),
            params![collection_id, relocated],
        )?;
        conn.execute(
            &format!("DELETE FROM {table} WHERE collection_id = ?1 AND {path_column} = ?2"),
            params![collection_id, stored],
        )?;
    }
    Ok(())
}

fn ensure_portable_layout(root: &Path) -> Result<()> {
    // Existing portable databases are preflighted read-only before db::open can
    // run SQLite migrations or rewrite metadata. Unknown/newer formats are
    // therefore never mutated by an older application build.
    if index_db_path(root).is_file() {
        crate::portable_verify::preflight_existing_index(root)?;
    }
    let dir = index_dir(root);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating portable index directory {}", dir.display()))?;
    std::fs::create_dir_all(thumbnail_dir(root))?;
    std::fs::create_dir_all(ann_dir(root))?;
    let conn = db::open(&index_db_path(root))?;
    face_store::ensure_schema(&conn)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS portable_meta(\
             key TEXT PRIMARY KEY NOT NULL,\
             value TEXT NOT NULL\
         );",
    )?;
    if meta_value(&conn, "library_id")?.is_none() {
        set_meta(&conn, "library_id", &new_library_id(root))?;
    }
    set_meta(
        &conn,
        "schema_version",
        &PORTABLE_SCHEMA_VERSION.to_string(),
    )?;
    set_meta(
        &conn,
        "format",
        crate::portable_verify::PORTABLE_FORMAT_MARKER,
    )?;
    Ok(())
}

fn portable_identity(root: &Path) -> Result<(String, bool, usize)> {
    let conn = db::open(&index_db_path(root))?;
    let library_id =
        meta_value(&conn, "library_id")?.context("portable index has no library id")?;
    let ready = meta_value(&conn, "migration_complete")?.as_deref() == Some("1");
    let count = conn.query_row("SELECT COUNT(*) FROM images", [], |row| {
        row.get::<_, i64>(0)
    })?;
    Ok((library_id, ready, count.max(0) as usize))
}

fn mark_portable_ready(root: &Path) -> Result<()> {
    let conn = db::open(&index_db_path(root))?;
    set_meta(&conn, "migration_complete", "1")
}

fn ensure_session_registry(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS portable_root_registry(\
             library_id TEXT PRIMARY KEY NOT NULL,\
             root_path TEXT NOT NULL\
         );",
    )?;
    Ok(())
}

fn root_image_count(conn: &Connection, root: &Path) -> Result<usize> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM images WHERE root = ?1",
        params![root.to_string_lossy().to_string()],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count.max(0) as usize)
}

fn meta_value(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM portable_meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .ok())
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO portable_meta(key, value) VALUES(?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn set_attached_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO {ATTACHED_DB}.portable_meta(key, value) VALUES(?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value"
        ),
        params![key, value],
    )?;
    Ok(())
}

fn attach_database(conn: &Connection, path: &Path) -> Result<()> {
    conn.execute(
        &format!("ATTACH DATABASE ?1 AS {ATTACHED_DB}"),
        params![path.to_string_lossy().to_string()],
    )?;
    Ok(())
}

fn detach_database(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!("DETACH DATABASE {ATTACHED_DB}"))?;
    Ok(())
}

fn root_prefix(root: &Path) -> String {
    let mut value = root.to_string_lossy().to_string();
    if !value.ends_with(std::path::MAIN_SEPARATOR) {
        value.push(std::path::MAIN_SEPARATOR);
    }
    value
}

fn new_library_id(root: &Path) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.to_string_lossy().hash(&mut hasher);
    nanos.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    format!("{:016x}{:016x}", hasher.finish(), nanos as u64)
}

fn image_columns_with_path() -> &'static str {
    "path, root, file_name, extension, size, modified, width, height, description, keywords, \
     dominant_r, dominant_g, dominant_b, visual_hash, color_histogram, color_histogram_dim, \
     material_texture, material_texture_dim, material_texture_version, embedding, embedding_dim, \
     embedding_normalized, last_seen_scan, content_fingerprint, descriptor_source, preview_revision, preview_edge"
}

fn image_columns_without_path_root() -> &'static str {
    "file_name, extension, size, modified, width, height, description, keywords, \
     dominant_r, dominant_g, dominant_b, visual_hash, color_histogram, color_histogram_dim, \
     material_texture, material_texture_dim, material_texture_version, embedding, embedding_dim, \
     embedding_normalized, last_seen_scan, content_fingerprint, descriptor_source, preview_revision, preview_edge"
}

fn image_update_assignments() -> &'static str {
    "root = excluded.root, \
     file_name = excluded.file_name, extension = excluded.extension, \
     size = excluded.size, modified = excluded.modified, width = excluded.width, height = excluded.height, \
     description = excluded.description, keywords = excluded.keywords, \
     dominant_r = excluded.dominant_r, dominant_g = excluded.dominant_g, dominant_b = excluded.dominant_b, \
     visual_hash = excluded.visual_hash, color_histogram = excluded.color_histogram, \
     color_histogram_dim = excluded.color_histogram_dim, material_texture = excluded.material_texture, \
     material_texture_dim = excluded.material_texture_dim, \
     material_texture_version = excluded.material_texture_version, embedding = excluded.embedding, \
     embedding_dim = excluded.embedding_dim, embedding_normalized = excluded.embedding_normalized, \
     last_seen_scan = excluded.last_seen_scan, content_fingerprint = excluded.content_fingerprint, \
     descriptor_source = excluded.descriptor_source, preview_revision = excluded.preview_revision, \
     preview_edge = excluded.preview_edge"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "wis-portable-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn relative_paths_round_trip_without_mount_prefix() {
        let root = temp_dir("relative");
        let source = root.join("tiles").join("stone").join("face-01.jpg");
        let relative = relative_source_path(&root, &source).unwrap();
        assert_eq!(
            relative,
            PathBuf::from("tiles").join("stone").join("face-01.jpg")
        );
        assert_eq!(absolute_source_path(&root, &relative).unwrap(), source);
    }

    #[test]
    fn internal_imagesearch_paths_are_never_source_images() {
        let root = temp_dir("internal");
        assert!(is_internal_path(
            &root,
            &root.join(".imagesearch").join("thumbnails").join("a.jpg")
        ));
        assert!(!is_internal_path(&root, &root.join("photos").join("a.jpg")));
    }

    #[test]
    fn existing_portable_index_rehydrates_under_a_new_root_prefix() {
        let first_root = temp_dir("move-a");
        let second_root = temp_dir("move-b");
        std::fs::create_dir_all(&first_root).unwrap();
        std::fs::create_dir_all(&second_root).unwrap();
        let first_session = first_root.parent().unwrap().join(format!(
            "wis-portable-session-a-{}-{}.sqlite3",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let second_session = first_session.with_file_name(format!(
            "wis-portable-session-b-{}-{}.sqlite3",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = first_root.join("material").join("face.jpg");

        {
            let conn = db::open(&first_session).unwrap();
            db::add_root(&first_session, &first_root).unwrap();
            db::upsert_image(
                &conn,
                &source,
                &first_root,
                "face.jpg",
                "jpg",
                123,
                456,
                64,
                64,
                "",
                "stone",
                [10, 20, 30],
                7,
                &[1.0, 0.0],
            )
            .unwrap();
            db::set_embedding(&conn, &source, &[0.6, 0.8]).unwrap();
        }
        let first = attach_root(&first_session, &first_root).unwrap();
        assert!(first.migrated_legacy_rows);
        assert_eq!(first.images, 1);

        std::fs::rename(index_dir(&first_root), index_dir(&second_root)).unwrap();
        let second = attach_root(&second_session, &second_root).unwrap();
        assert!(second.reused_existing_index);
        assert_eq!(second.images, 1);
        let summaries = db::load_image_summaries(&second_session).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0].path,
            second_root.join("material").join("face.jpg")
        );
        assert_eq!(summaries[0].root, second_root);
        assert_eq!(db::load_ann_embeddings(&second_session).unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(first_root);
        let _ = std::fs::remove_dir_all(second_root);
        for path in [&first_session, &second_session] {
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(format!("{}-wal", path.display()));
            let _ = std::fs::remove_file(format!("{}-shm", path.display()));
        }
    }
}
