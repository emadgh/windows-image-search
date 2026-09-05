include!("core.rs");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeFailureState {
    pub root: PathBuf,
    pub size: u64,
    pub modified: i64,
    pub reason: String,
}

pub fn ensure_decode_failure_store(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS decode_failures (
            path TEXT PRIMARY KEY NOT NULL,
            root TEXT NOT NULL,
            size INTEGER NOT NULL,
            modified INTEGER NOT NULL,
            reason TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_decode_failures_root ON decode_failures(root);
        "#,
    )?;
    Ok(())
}

pub fn load_decode_failure_states(
    conn: &Connection,
) -> Result<HashMap<PathBuf, DecodeFailureState>> {
    ensure_decode_failure_store(conn)?;
    let mut stmt = conn.prepare(
        "SELECT path, root, size, modified, reason FROM decode_failures ORDER BY path COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            PathBuf::from(row.get::<_, String>(0)?),
            DecodeFailureState {
                root: PathBuf::from(row.get::<_, String>(1)?),
                size: row.get::<_, i64>(2)?.max(0) as u64,
                modified: row.get(3)?,
                reason: row.get(4)?,
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

pub fn record_decode_failure(
    conn: &Connection,
    path: &Path,
    root: &Path,
    size: u64,
    modified: i64,
    reason: &str,
) -> Result<()> {
    ensure_decode_failure_store(conn)?;
    conn.execute(
        r#"
        INSERT INTO decode_failures(path, root, size, modified, reason)
        VALUES(?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(path) DO UPDATE SET
            root = excluded.root,
            size = excluded.size,
            modified = excluded.modified,
            reason = excluded.reason
        "#,
        params![
            path.to_string_lossy().to_string(),
            root.to_string_lossy().to_string(),
            size as i64,
            modified,
            reason,
        ],
    )?;
    Ok(())
}

pub fn clear_decode_failure(conn: &Connection, path: &Path) -> Result<()> {
    conn.execute(
        "DELETE FROM decode_failures WHERE path = ?1",
        params![path.to_string_lossy().to_string()],
    )?;
    Ok(())
}

pub fn delete_decode_failure_path_tree(conn: &Connection, target: &Path) -> Result<usize> {
    ensure_decode_failure_store(conn)?;
    Ok(conn.execute(
        "DELETE FROM decode_failures WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\' COLLATE NOCASE",
        params![
            target.to_string_lossy().to_string(),
            like_prefix_pattern(target)
        ],
    )?)
}

pub fn prune_decode_failures_not_discovered(conn: &Connection, root: &Path) -> Result<usize> {
    ensure_decode_failure_store(conn)?;
    Ok(conn.execute(
        r#"
        DELETE FROM decode_failures
        WHERE root = ?1
          AND path NOT IN (SELECT path FROM discovered_images WHERE root = ?1)
        "#,
        params![root.to_string_lossy().to_string()],
    )?)
}

pub fn delete_discovered_path_tree(conn: &Connection, target: &Path) -> Result<usize> {
    let _ = delete_decode_failure_path_tree(conn, target)?;
    let target_text = target.to_string_lossy().to_string();
    let prefix = like_prefix_pattern(target);
    Ok(conn.execute(
        "DELETE FROM discovered_images WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\' COLLATE NOCASE",
        params![target_text, prefix],
    )?)
}

#[cfg(test)]
mod live_discovery_tests {
    use super::*;

    #[test]
    fn live_discovery_tree_removal_preserves_unrelated_paths() {
        let db_path = std::env::temp_dir().join(format!(
            "windows-image-search-live-discovery-remove-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = std::env::temp_dir().join("windows-image-search-live-discovery-root");
        let folder = root.join("folder");
        let nested = folder.join("nested").join("a.jpg");
        let direct = folder.join("b.jpg");
        let sibling = root.join("sibling.jpg");
        {
            let mut conn = open(&db_path).unwrap();
            let generation = next_scan_generation(&conn).unwrap();
            mark_discovered_paths_seen(
                &mut conn,
                generation,
                &[
                    (root.clone(), nested.clone()),
                    (root.clone(), direct.clone()),
                    (root.clone(), sibling.clone()),
                ],
            )
            .unwrap();
            assert_eq!(delete_discovered_path_tree(&conn, &folder).unwrap(), 2);
        }
        let discovered = load_discovered_paths(&db_path).unwrap();
        assert_eq!(discovered, vec![sibling]);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }

    #[test]
    fn decode_failure_state_persists_clears_and_prunes() {
        let db_path = std::env::temp_dir().join(format!(
            "windows-image-search-decode-failure-state-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = std::env::temp_dir().join("windows-image-search-decode-failure-root");
        let present = root.join("present.jpg");
        let stale = root.join("stale.jpg");
        {
            let mut conn = open(&db_path).unwrap();
            ensure_decode_failure_store(&conn).unwrap();
            record_decode_failure(&conn, &present, &root, 111, 222, "invalid JPEG data").unwrap();
            record_decode_failure(&conn, &stale, &root, 333, 444, "truncated image").unwrap();

            let states = load_decode_failure_states(&conn).unwrap();
            assert_eq!(states[&present].size, 111);
            assert_eq!(states[&present].modified, 222);
            assert_eq!(states[&present].reason, "invalid JPEG data");

            clear_decode_failure(&conn, &present).unwrap();
            assert!(!load_decode_failure_states(&conn)
                .unwrap()
                .contains_key(&present));
            record_decode_failure(&conn, &present, &root, 111, 222, "invalid JPEG data").unwrap();

            let generation = next_scan_generation(&conn).unwrap();
            mark_discovered_paths_seen(&mut conn, generation, &[(root.clone(), present.clone())])
                .unwrap();
            prune_decode_failures_not_discovered(&conn, &root).unwrap();
            let states = load_decode_failure_states(&conn).unwrap();
            assert!(states.contains_key(&present));
            assert!(!states.contains_key(&stale));
        }
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }
}
