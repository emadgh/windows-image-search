from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


# db.rs: replace per-file lookup helper with one-shot state cache loading.
path = Path("src/db.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "use rusqlite::{params, Connection, OptionalExtension};\nuse std::path::{Path, PathBuf};\n",
    "use rusqlite::{params, Connection};\nuse std::collections::HashMap;\nuse std::path::{Path, PathBuf};\n",
    "db imports",
)

old = '''pub fn existing_file_state(conn: &Connection, path: &Path) -> Result<Option<(u64, i64, bool)>> {
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
'''
new = '''#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileState {
    pub size: u64,
    pub modified: i64,
    pub has_embedding: bool,
}

pub fn load_file_states(conn: &Connection) -> Result<HashMap<PathBuf, FileState>> {
    let mut stmt = conn.prepare(
        "SELECT path, size, modified, embedding IS NOT NULL FROM images",
    )?;
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
'''
text = replace_once(text, old, new, "replace per-file state query")

if "fn load_file_states_returns_all_persisted_rows()" not in text:
    text += r'''

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
'''

path.write_text(text, encoding="utf-8")


# indexer.rs: load persisted states once, then use O(1) HashMap lookups per candidate.
path = Path("src/indexer.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    '''    let indexing_settings = indexing_settings.sanitized();
    let mut conn = db::open(db_path)?;
    let mut candidates: Vec<(PathBuf, PathBuf)> = Vec::new();

    let _ = tx.send(WorkerMessage::Status(
        "Scanning folders recursively…".to_owned(),
    ));
''',
    '''    let indexing_settings = indexing_settings.sanitized();
    let mut conn = db::open(db_path)?;
    let existing_file_states = db::load_file_states(&conn)?;
    let mut candidates: Vec<(PathBuf, PathBuf)> = Vec::new();

    let _ = tx.send(WorkerMessage::Status(format!(
        "Scanning folders recursively… {} persisted file states cached in memory",
        existing_file_states.len()
    )));
''',
    "load file-state cache once",
)
text = replace_once(
    text,
    '''        let unchanged = db::existing_file_state(&conn, path)?
            .map(|(old_size, old_modified, _)| old_size == size && old_modified == modified)
            .unwrap_or(false);
''',
    '''        let unchanged = existing_file_states
            .get(path)
            .is_some_and(|state| state.size == size && state.modified == modified);
''',
    "replace per-file SQLite lookup",
)

path.write_text(text, encoding="utf-8")
print("Rescan file-state cache patch applied")
