use crate::db;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const COLLECTION_FACE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS collection_face_settings (
    collection_id INTEGER PRIMARY KEY NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY(collection_id) REFERENCES collections(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_collection_face_settings_enabled
    ON collection_face_settings(enabled, collection_id);
"#;

const ELIGIBLE_PREDICATE: &str = r#"
(
    EXISTS (
        SELECT 1
        FROM collection_files files
        JOIN collection_face_settings settings
          ON settings.collection_id = files.collection_id
         AND settings.enabled = 1
        WHERE files.file_path = images.path COLLATE NOCASE
    )
    OR EXISTS (
        SELECT 1
        FROM collection_folders folders
        JOIN collection_face_settings settings
          ON settings.collection_id = folders.collection_id
         AND settings.enabled = 1
        WHERE length(images.path) >= length(folders.folder_path)
          AND substr(images.path, 1, length(folders.folder_path)) = folders.folder_path COLLATE NOCASE
          AND (
                length(images.path) = length(folders.folder_path)
             OR substr(images.path, length(folders.folder_path) + 1, 1) IN ('\\', '/')
          )
    )
)
"#;

pub fn ensure_schema(db_path: &Path) -> Result<()> {
    let conn = db::open(db_path)?;
    ensure_schema_on(&conn)
}

pub(crate) fn ensure_schema_on(conn: &Connection) -> Result<()> {
    conn.execute_batch(COLLECTION_FACE_SCHEMA)
        .context("creating collection face settings")?;
    Ok(())
}

pub fn load_collection_flags(db_path: &Path) -> Result<HashMap<i64, bool>> {
    let conn = db::open(db_path)?;
    ensure_schema_on(&conn)?;
    let mut stmt = conn.prepare(
        "SELECT collection_id, enabled FROM collection_face_settings WHERE enabled <> 0",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? != 0))
    })?;
    Ok(rows.filter_map(|row| row.ok()).collect())
}

pub fn set_collection_enabled(db_path: &Path, collection_id: i64, enabled: bool) -> Result<()> {
    let conn = db::open(db_path)?;
    ensure_schema_on(&conn)?;
    if enabled {
        conn.execute(
            r#"
            INSERT INTO collection_face_settings(collection_id, enabled)
            VALUES(?1, 1)
            ON CONFLICT(collection_id) DO UPDATE SET enabled = 1
            "#,
            params![collection_id],
        )?;
    } else {
        conn.execute(
            "DELETE FROM collection_face_settings WHERE collection_id = ?1",
            params![collection_id],
        )?;
    }
    Ok(())
}

pub fn count_eligible_paths(db_path: &Path, root: &Path) -> Result<usize> {
    let conn = db::open(db_path)?;
    ensure_schema_on(&conn)?;
    count_eligible_paths_on(&conn, root)
}

pub(crate) fn count_eligible_paths_on(conn: &Connection, root: &Path) -> Result<usize> {
    ensure_schema_on(conn)?;
    let sql = format!(
        "SELECT COUNT(*) FROM images WHERE root = ?1 COLLATE NOCASE AND {ELIGIBLE_PREDICATE}"
    );
    let count = conn.query_row(
        &sql,
        params![root.to_string_lossy().to_string()],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count.max(0) as usize)
}

pub fn eligible_batch(
    db_path: &Path,
    root: &Path,
    after: Option<&Path>,
    limit: usize,
) -> Result<Vec<PathBuf>> {
    let conn = db::open(db_path)?;
    ensure_schema_on(&conn)?;
    eligible_batch_on(&conn, root, after, limit)
}

pub(crate) fn eligible_batch_on(
    conn: &Connection,
    root: &Path,
    after: Option<&Path>,
    limit: usize,
) -> Result<Vec<PathBuf>> {
    ensure_schema_on(conn)?;
    let after = after
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let sql = format!(
        r#"
        SELECT images.path
        FROM images
        WHERE images.root = ?1 COLLATE NOCASE
          AND images.path COLLATE NOCASE > ?2 COLLATE NOCASE
          AND {ELIGIBLE_PREDICATE}
        ORDER BY images.path COLLATE NOCASE
        LIMIT ?3
        "#
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            root.to_string_lossy().to_string(),
            after,
            limit.max(1) as i64
        ],
        |row| row.get::<_, String>(0),
    )?;
    rows.map(|row| row.map(PathBuf::from))
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("loading collection-scoped face candidates")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "wis-face-scope-{label}-{}-{nonce}.sqlite3",
            std::process::id()
        ))
    }

    fn root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("wis-face-scope-root-{label}"))
    }

    fn add_image(db_path: &Path, root: &Path, relative: &str) -> PathBuf {
        let absolute = root.join(relative);
        let conn = db::open(db_path).unwrap();
        db::upsert_image(
            &conn,
            &absolute,
            root,
            absolute.file_name().unwrap().to_string_lossy().as_ref(),
            "jpg",
            10,
            20,
            100,
            100,
            "",
            "",
            [10, 20, 30],
            1,
            &[1.0],
        )
        .unwrap();
        absolute
    }

    #[test]
    fn collections_are_default_off_and_toggle_controls_scope() {
        let db_path = temp_db("default-off");
        let root = root("people");
        let image = add_image(&db_path, &root, "faces/a.jpg");
        let collection = db::create_collection(&db_path, "People").unwrap();
        db::add_collection_folders(&db_path, collection.id, &[root.join("faces")]).unwrap();

        assert_eq!(count_eligible_paths(&db_path, &root).unwrap(), 0);
        set_collection_enabled(&db_path, collection.id, true).unwrap();
        assert_eq!(count_eligible_paths(&db_path, &root).unwrap(), 1);
        assert_eq!(
            eligible_batch(&db_path, &root, None, 16).unwrap(),
            vec![image]
        );
        set_collection_enabled(&db_path, collection.id, false).unwrap();
        assert_eq!(count_eligible_paths(&db_path, &root).unwrap(), 0);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn overlapping_collections_use_or_semantics() {
        let db_path = temp_db("or");
        let root = root("mixed");
        let a = add_image(&db_path, &root, "a.jpg");
        let _b = add_image(&db_path, &root, "b.jpg");

        let textures = db::create_collection(&db_path, "Textures").unwrap();
        db::add_collection_folders(&db_path, textures.id, std::slice::from_ref(&root)).unwrap();

        let people = db::create_collection(&db_path, "People").unwrap();
        db::add_collection_files(&db_path, people.id, std::slice::from_ref(&a)).unwrap();
        set_collection_enabled(&db_path, people.id, true).unwrap();

        assert_eq!(count_eligible_paths(&db_path, &root).unwrap(), 1);
        assert_eq!(eligible_batch(&db_path, &root, None, 8).unwrap(), vec![a]);
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn keyset_batching_only_walks_enabled_collection_members() {
        let db_path = temp_db("batch");
        let root = root("batch");
        let a = add_image(&db_path, &root, "people/a.jpg");
        let b = add_image(&db_path, &root, "people/b.jpg");
        let _texture = add_image(&db_path, &root, "textures/z.jpg");

        let people = db::create_collection(&db_path, "People").unwrap();
        db::add_collection_folders(&db_path, people.id, &[root.join("people")]).unwrap();
        set_collection_enabled(&db_path, people.id, true).unwrap();

        let textures = db::create_collection(&db_path, "Textures").unwrap();
        db::add_collection_folders(&db_path, textures.id, &[root.join("textures")]).unwrap();

        let first = eligible_batch(&db_path, &root, None, 1).unwrap();
        assert_eq!(first, vec![a.clone()]);
        let second = eligible_batch(&db_path, &root, Some(&a), 1).unwrap();
        assert_eq!(second, vec![b]);
        let _ = std::fs::remove_file(db_path);
    }
}
