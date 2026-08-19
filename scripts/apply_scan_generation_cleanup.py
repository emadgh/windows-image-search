from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


# -----------------------------------------------------------------------------
# db.rs
# -----------------------------------------------------------------------------
path = Path("src/db.rs")
text = path.read_text(encoding="utf-8")

text = replace_once(
    text,
    '''            embedding BLOB,
            embedding_dim INTEGER
        );
''',
    '''            embedding BLOB,
            embedding_dim INTEGER,
            last_seen_scan INTEGER NOT NULL DEFAULT 0
        );
''',
    "fresh schema scan generation column",
)

text = replace_once(
    text,
    '''    ensure_column(&conn, "images", "visual_hash", "INTEGER")?;
    ensure_column(&conn, "images", "color_histogram", "BLOB")?;
    ensure_column(&conn, "images", "color_histogram_dim", "INTEGER")?;

    Ok(conn)
''',
    '''    ensure_column(&conn, "images", "visual_hash", "INTEGER")?;
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
''',
    "scan generation migration/index",
)

old = '''pub fn delete_missing_for_root(conn: &Connection, root: &Path, seen: &[PathBuf]) -> Result<usize> {
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
'''
new = '''pub fn next_scan_generation(conn: &Connection) -> Result<i64> {
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

pub fn mark_paths_seen<'a, I>(
    conn: &mut Connection,
    generation: i64,
    paths: I,
) -> Result<usize>
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

pub fn delete_stale_for_root(
    conn: &Connection,
    root: &Path,
    generation: i64,
) -> Result<usize> {
    let root_text = root.to_string_lossy().to_string();
    Ok(conn.execute(
        "DELETE FROM images WHERE root = ?1 AND last_seen_scan <> ?2",
        params![root_text, generation],
    )?)
}
'''
text = replace_once(text, old, new, "replace seen HashSet cleanup")

text = replace_once(
    text,
    '''    #[test]
    fn load_file_states_returns_all_persisted_rows() {
''',
    '''    #[test]
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
    fn load_file_states_returns_all_persisted_rows() {
''',
    "insert scan generation regression test",
)

path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# indexer.rs
# -----------------------------------------------------------------------------
path = Path("src/indexer.rs")
text = path.read_text(encoding="utf-8")

text = replace_once(
    text,
    '''    let mut traversal_errors = 0usize;
    for root in roots {
        if !root.exists() {
            traversal_errors += 1;
            let _ = tx.send(WorkerMessage::Error(format!(
                "Indexed root does not exist: {}",
                root.display()
            )));
            continue;
        }

        for entry in WalkDir::new(root).follow_links(false).into_iter() {
            match entry {
                Ok(entry) => {
                    if entry.file_type().is_file() && is_supported_image(entry.path()) {
                        candidates.push((root.clone(), entry.into_path()));
                    }
                }
                Err(err) => {
                    traversal_errors += 1;
                    if traversal_errors <= 8 {
                        let _ = tx.send(WorkerMessage::Error(format!(
                            "Recursive scan could not access an entry under {}: {err}",
                            root.display()
                        )));
                    }
                }
            }
        }
    }

    let total = candidates.len();
    let mut seen_by_root: std::collections::HashMap<PathBuf, Vec<PathBuf>> =
        std::collections::HashMap::new();
    let mut pending = Vec::<PendingImage>::new();
''',
    '''    let mut traversal_errors = 0usize;
    let mut prunable_roots = Vec::<PathBuf>::new();
    for root in roots {
        if !root.exists() {
            traversal_errors += 1;
            let _ = tx.send(WorkerMessage::Error(format!(
                "Indexed root does not exist; stale cleanup skipped for {}",
                root.display()
            )));
            continue;
        }

        let root_errors_before = traversal_errors;
        for entry in WalkDir::new(root).follow_links(false).into_iter() {
            match entry {
                Ok(entry) => {
                    if entry.file_type().is_file() && is_supported_image(entry.path()) {
                        candidates.push((root.clone(), entry.into_path()));
                    }
                }
                Err(err) => {
                    traversal_errors += 1;
                    if traversal_errors <= 8 {
                        let _ = tx.send(WorkerMessage::Error(format!(
                            "Recursive scan could not access an entry under {}: {err}",
                            root.display()
                        )));
                    }
                }
            }
        }

        if traversal_errors == root_errors_before {
            prunable_roots.push(root.clone());
        } else {
            let _ = tx.send(WorkerMessage::Status(format!(
                "Stale cleanup skipped for {} because traversal was incomplete",
                root.display()
            )));
        }
    }

    let total = candidates.len();
    let mut pending = Vec::<PendingImage>::new();
''',
    "track only safely prunable roots",
)

text = replace_once(
    text,
    '''    for (index, (root, path)) in candidates.iter().enumerate() {
        seen_by_root
            .entry(root.clone())
            .or_default()
            .push(path.clone());

        let meta = match std::fs::metadata(path) {
''',
    '''    for (index, (root, path)) in candidates.iter().enumerate() {
        let meta = match std::fs::metadata(path) {
''',
    "remove duplicate seen_by_root allocation",
)

text = replace_once(
    text,
    '''    let mut removed = 0usize;
    for root in roots {
        let seen = seen_by_root.get(root).cloned().unwrap_or_default();
        removed += db::delete_missing_for_root(&conn, root, &seen)?;
    }
''',
    '''    let scan_generation = db::next_scan_generation(&conn)?;
    let marked = db::mark_paths_seen(
        &mut conn,
        scan_generation,
        candidates.iter().map(|(_, path)| path),
    )?;
    let mut removed = 0usize;
    for root in &prunable_roots {
        removed += db::delete_stale_for_root(&conn, root, scan_generation)?;
    }
    let _ = tx.send(WorkerMessage::Status(format!(
        "Scan generation {scan_generation}: {marked} persisted paths marked present; {removed} stale rows removed across {}/{} safe roots",
        prunable_roots.len(),
        roots.len()
    )));
''',
    "generation-based stale cleanup",
)

path.write_text(text, encoding="utf-8")
print("Scan-generation cleanup patch applied")
