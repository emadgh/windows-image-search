from pathlib import Path

INDEXER = Path("src/indexer.rs")
DB_MOD = Path("src/db/mod.rs")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def replace_exact_count(text: str, old: str, new: str, expected: int, label: str) -> str:
    count = text.count(old)
    if count != expected:
        raise RuntimeError(f"{label}: expected {expected} matches, found {count}")
    return text.replace(old, new)


def patch_db_mod() -> None:
    text = DB_MOD.read_text(encoding="utf-8")
    if "pub struct DecodeFailureState" in text:
        print("db decode failure store already patched")
        return

    store = r'''

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
'''
    text = replace_once(text, 'include!("core.rs");\n', 'include!("core.rs");' + store + '\n', "insert decode failure store")

    text = replace_once(
        text,
        '''pub fn delete_discovered_path_tree(conn: &Connection, target: &Path) -> Result<usize> {\n    let target_text = target.to_string_lossy().to_string();''',
        '''pub fn delete_discovered_path_tree(conn: &Connection, target: &Path) -> Result<usize> {\n    let _ = delete_decode_failure_path_tree(conn, target)?;\n    let target_text = target.to_string_lossy().to_string();''',
        "delete failure state with discovered subtree",
    )

    insert_at = text.rfind("\n}")
    if insert_at < 0:
        raise RuntimeError("db tests module closing brace not found")
    tests = r'''

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
            assert!(!load_decode_failure_states(&conn).unwrap().contains_key(&present));
            record_decode_failure(&conn, &present, &root, 111, 222, "invalid JPEG data").unwrap();

            let generation = next_scan_generation(&conn).unwrap();
            mark_discovered_paths_seen(
                &mut conn,
                generation,
                &[(root.clone(), present.clone())],
            )
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
'''
    text = text[:insert_at] + tests + text[insert_at:]
    DB_MOD.write_text(text, encoding="utf-8")


def patch_indexer() -> None:
    text = INDEXER.read_text(encoding="utf-8")
    if "struct DecodeFailureRecord" in text:
        print("indexer scan fixes already patched")
        return

    record_struct = r'''

#[derive(Clone, Debug)]
struct DecodeFailureRecord {
    root: PathBuf,
    path: PathBuf,
    size: u64,
    modified: i64,
    reason: String,
}
'''
    text = replace_once(
        text,
        '''struct PreparedImage {\n    root: PathBuf,''',
        record_struct + '''\nstruct PreparedImage {\n    root: PathBuf,''',
        "insert decode failure record",
    )

    text = replace_once(
        text,
        '''    let mut conn = db::open(db_path)?;\n    let unique_paths: HashSet<PathBuf> = changed_paths.iter().cloned().collect();''',
        '''    let mut conn = db::open(db_path)?;\n    db::ensure_decode_failure_store(&conn)?;\n    let unique_paths: HashSet<PathBuf> = changed_paths.iter().cloned().collect();''',
        "ensure failure store for incremental update",
    )

    batch_start = '''    for batch in pending.chunks(batch_size) {\n        control.wait_if_paused();\n        let prepared: Vec<PreparedImage> = pool.install(|| {'''
    batch_replacement = '''    for batch in pending.chunks(batch_size) {\n        control.wait_if_paused();\n        let decode_failures = Arc::new(Mutex::new(Vec::<DecodeFailureRecord>::new()));\n        let prepared: Vec<PreparedImage> = pool.install(|| {'''
    text = replace_exact_count(text, batch_start, batch_replacement, 2, "capture base-index decode failures")

    old_err = '''                        Err(err) => {\n                            let _ = tx.send(WorkerMessage::Warning(compact_decode_failure(\n                                &item.path, &err,\n                            )));\n                            None\n                        }'''
    new_err = '''                        Err(err) => {\n                            if let Some(reason) = durable_decode_failure_reason(&err) {\n                                decode_failures\n                                    .lock()\n                                    .unwrap_or_else(|poisoned| poisoned.into_inner())\n                                    .push(DecodeFailureRecord {\n                                        root: item.root.clone(),\n                                        path: item.path.clone(),\n                                        size: item.size,\n                                        modified: item.modified,\n                                        reason,\n                                    });\n                            }\n                            let _ = tx.send(WorkerMessage::Warning(compact_decode_failure(\n                                &item.path, &err,\n                            )));\n                            None\n                        }'''
    text = replace_exact_count(text, old_err, new_err, 2, "record durable base-index decode failures")

    prepared_end = '''        });\n\n        if prepared.is_empty() {'''
    prepared_replacement = '''        });\n\n        let failures = {\n            let mut locked = decode_failures\n                .lock()\n                .unwrap_or_else(|poisoned| poisoned.into_inner());\n            std::mem::take(&mut *locked)\n        };\n        persist_decode_failures(&conn, roots, failures, tx)?;\n\n        if prepared.is_empty() {'''
    text = replace_exact_count(text, prepared_end, prepared_replacement, 2, "persist captured decode failures")

    provenance = '''                persist_descriptor_provenance(&transaction, &item.path, item.size)?;'''
    provenance_new = provenance + '''\n                db::clear_decode_failure(&transaction, &item.path)?;'''
    text = replace_exact_count(text, provenance, provenance_new, 2, "clear remembered failures after successful decode")

    text = replace_once(
        text,
        '''fn compact_decode_failure(path: &Path, err: &anyhow::Error) -> String {''',
        r'''fn durable_decode_failure_reason(err: &anyhow::Error) -> Option<String> {
    let detail = format!("{err:#}");
    let lower = detail.to_ascii_lowercase();
    if lower.contains("permission denied")
        || lower.contains("access is denied")
        || lower.contains("sharing violation")
    {
        return None;
    }
    let durable = lower.contains("illegal start bytes")
        || lower.contains("format error")
        || lower.contains("decoding")
        || lower.contains("unexpected eof")
        || lower.contains("unexpected end")
        || lower.contains("end of file")
        || lower.contains("truncated")
        || lower.contains("unsupported");
    if !durable {
        return None;
    }
    let reason = if lower.contains("illegal start bytes")
        || lower.contains("format error decoding jpeg")
        || lower.contains("jpeg") && lower.contains("format error")
    {
        "invalid JPEG data"
    } else if lower.contains("unexpected eof")
        || lower.contains("unexpected end")
        || lower.contains("end of file")
        || lower.contains("truncated")
    {
        "truncated image"
    } else if lower.contains("unsupported") {
        "unsupported image format"
    } else {
        "decode error"
    };
    Some(reason.to_owned())
}

fn persist_decode_failures(
    conn: &rusqlite::Connection,
    roots: &[PathBuf],
    failures: Vec<DecodeFailureRecord>,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    if failures.is_empty() {
        return Ok(());
    }
    let mut invalidated_paths = Vec::<PathBuf>::new();
    for failure in failures {
        db::record_decode_failure(
            conn,
            &failure.path,
            &failure.root,
            failure.size,
            failure.modified,
            &failure.reason,
        )?;
        invalidated_paths.extend(db::delete_path_tree(conn, &failure.path)?);
    }
    invalidated_paths.sort();
    invalidated_paths.dedup();
    if !invalidated_paths.is_empty() {
        portable::remove_absolute_paths(roots, &invalidated_paths)?;
        let _ = tx.send(WorkerMessage::RemovedPaths(invalidated_paths));
    }
    Ok(())
}

fn unchanged_known_decode_failure(
    state: Option<&db::DecodeFailureState>,
    size: u64,
    modified: i64,
) -> bool {
    state.is_some_and(|state| state.size == size && state.modified == modified)
}

fn compact_decode_failure(path: &Path, err: &anyhow::Error) -> String {''',
        "insert durable decode failure helpers",
    )

    text = replace_once(
        text,
        '''    let mut conn = db::open(db_path)?;\n    let existing_file_states = db::load_file_states(&conn)?;\n    let force_rescan = mode == RescanMode::ForcePreferThumbnail;''',
        '''    let mut conn = db::open(db_path)?;\n    let existing_file_states = db::load_file_states(&conn)?;\n    let existing_decode_failures = db::load_decode_failure_states(&conn)?;\n    let force_rescan = mode == RescanMode::ForcePreferThumbnail;''',
        "load known decode failures",
    )

    text = replace_once(
        text,
        '''    for root in &prunable_roots {\n        let _ = db::delete_stale_discovered_for_root(&conn, root, scan_generation)?;\n    }''',
        '''    for root in &prunable_roots {\n        let _ = db::delete_stale_discovered_for_root(&conn, root, scan_generation)?;\n        let _ = db::prune_decode_failures_not_discovered(&conn, root)?;\n    }''',
        "prune stale decode failure states",
    )

    text = replace_once(
        text,
        '''    let _ = tx.send(WorkerMessage::RootCounts(db::load_root_counts(db_path)?));\n    let mut pending = Vec::<PendingImage>::new();''',
        '''    let _ = tx.send(WorkerMessage::RootCounts(db::load_root_counts(db_path)?));\n    let mut pending = Vec::<PendingImage>::new();\n    let mut known_decode_failures_skipped = 0usize;''',
        "track skipped known failures",
    )

    old_pending = '''        let previous = existing_file_states.get(path);\n        let unchanged =\n            previous.is_some_and(|state| state.size == size && state.modified == modified);\n\n        if force_rescan || !unchanged {\n            pending.push(PendingImage {\n                root: root.clone(),\n                path: path.clone(),\n                size,\n                modified,\n                previous_width: previous.map_or(0, |state| state.width),\n                previous_height: previous.map_or(0, |state| state.height),\n                previous_fingerprint: previous.and_then(|state| state.content_fingerprint),\n                prefer_thumbnail: force_rescan && unchanged,\n            });\n        }'''
    new_pending = '''        let previous = existing_file_states.get(path);\n        let unchanged =\n            previous.is_some_and(|state| state.size == size && state.modified == modified);\n        let failed_unchanged = unchanged_known_decode_failure(\n            existing_decode_failures.get(path),\n            size,\n            modified,\n        );\n\n        if !force_rescan && failed_unchanged {\n            known_decode_failures_skipped += 1;\n        } else if force_rescan || !unchanged {\n            pending.push(PendingImage {\n                root: root.clone(),\n                path: path.clone(),\n                size,\n                modified,\n                previous_width: previous.map_or(0, |state| state.width),\n                previous_height: previous.map_or(0, |state| state.height),\n                previous_fingerprint: previous.and_then(|state| state.content_fingerprint),\n                prefer_thumbnail: force_rescan && unchanged,\n            });\n        }'''
    text = replace_once(text, old_pending, new_pending, "skip unchanged known decode failures")

    text = replace_once(
        text,
        '''    let changed_total = pending.len();''',
        '''    if known_decode_failures_skipped > 0 {\n        let _ = tx.send(WorkerMessage::Status(format!(\n            "Skipped {known_decode_failures_skipped} unchanged image{} with remembered decode failures; retry requires a source change or Force Rescan",\n            if known_decode_failures_skipped == 1 { "" } else { "s" }\n        )));\n    }\n\n    let changed_total = pending.len();''',
        "report known decode failure skips",
    )

    text = replace_once(
        text,
        '''            roots,\n            false,\n            control,\n            tx,''',
        '''            roots,\n            true,\n            control,\n            tx,''',
        "prefer thumbnails for incremental CLIP",
    )
    text = replace_once(
        text,
        '''            roots,\n            force_rescan,\n            control,\n            tx,''',
        '''            roots,\n            true,\n            control,\n            tx,''',
        "prefer thumbnails for scan CLIP",
    )

    marker = '''    #[test]\n    fn normalized_query_similarity_matches_legacy_candidate_fallback()'''
    tests = r'''    #[test]
    fn clip_embedding_prefers_cached_thumbnail_and_falls_back_to_source() {
        use image::{ImageBuffer, Rgb};
        let root = std::env::temp_dir().join(format!(
            "wis-clip-thumb-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("tiles")).unwrap();
        let source = root.join("tiles").join("large.png");
        DynamicImage::ImageRgb8(ImageBuffer::from_pixel(900, 700, Rgb([20, 40, 60])))
            .save(&source)
            .unwrap();
        let settings = IndexingSettings::default().sanitized();

        let without_cache = safe_embedding_input_path(
            &source,
            std::slice::from_ref(&root),
            settings,
            true,
        )
        .unwrap();
        assert_eq!(without_cache, source);

        let decoded = image::ImageReader::open(&source).unwrap().decode().unwrap();
        let cached = thumbnail_cache::store_from_decoded_for_root(&root, &source, &decoded).unwrap();
        let with_cache = safe_embedding_input_path(
            &source,
            std::slice::from_ref(&root),
            settings,
            true,
        )
        .unwrap();
        assert_eq!(with_cache, cached);
        assert_ne!(with_cache, source);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unchanged_decode_failure_is_skipped_until_changed_or_forced() {
        let state = db::DecodeFailureState {
            root: PathBuf::from("C:/images"),
            size: 123,
            modified: 456,
            reason: "invalid JPEG data".to_owned(),
        };
        assert!(unchanged_known_decode_failure(Some(&state), 123, 456));
        assert!(!unchanged_known_decode_failure(Some(&state), 124, 456));
        assert!(!unchanged_known_decode_failure(Some(&state), 123, 457));
        assert!(!unchanged_known_decode_failure(None, 123, 456));

        let force_rescan = true;
        assert!(force_rescan || !unchanged_known_decode_failure(Some(&state), 123, 456));
    }

'''
    text = replace_once(text, marker, tests + marker, "add scan regression tests")

    INDEXER.write_text(text, encoding="utf-8")


patch_db_mod()
patch_indexer()
print("scan indexing fixes applied")
