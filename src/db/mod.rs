include!("core.rs");

pub fn delete_discovered_path_tree(conn: &Connection, target: &Path) -> Result<usize> {
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
}
