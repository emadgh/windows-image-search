from pathlib import Path


def replace_exact(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    count = text.count(old)
    if count == 0:
        if new in text:
            return
        raise SystemExit(f"{path}: missing expected text: {old[:180]!r}")
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:180]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_exact(
    "src/indexer.rs",
    '''    let indexing_settings = indexing_settings.sanitized();\n    let thumbnail_cache_dir = thumbnail_cache::cache_dir_for_db(db_path);\n    let mut conn = db::open(db_path)?;\n    let existing_states = db::load_file_states(&conn)?;\n    let unique_paths: HashSet<PathBuf> = changed_paths.iter().cloned().collect();''',
    '''    let indexing_settings = indexing_settings.sanitized();\n    let mut conn = db::open(db_path)?;\n    let unique_paths: HashSet<PathBuf> = changed_paths.iter().cloned().collect();''',
)
replace_exact(
    "src/indexer.rs",
    '''    let indexing_settings = indexing_settings.sanitized();\n    let thumbnail_cache_dir = thumbnail_cache::cache_dir_for_db(db_path);\n    let mut conn = db::open(db_path)?;\n    let existing_file_states = db::load_file_states(&conn)?;''',
    '''    let indexing_settings = indexing_settings.sanitized();\n    let mut conn = db::open(db_path)?;\n    let existing_file_states = db::load_file_states(&conn)?;''',
)

replace_exact(
    "src/main.rs",
    '''    let _ = db::open(&db_path);\n\n    if let StartupMode::AnnBenchmark(query_count) = &mode {''',
    '''    let _ = db::open(&db_path);\n    // The AppData database is only the attached multi-root session/catalog cache.\n    // Hydrate it from every available portable root before GUI or CLI diagnostics.\n    let registered_roots = db::load_roots(&db_path).unwrap_or_default();\n    let _ = portable::prepare_registered_roots(&db_path, &registered_roots);\n\n    if let StartupMode::AnnBenchmark(query_count) = &mode {''',
)

replace_exact(
    "src/portable.rs",
    '''        tx.execute(\n            "INSERT INTO portable_root_registry(library_id, root_path) VALUES(?1, ?2) \\\n             ON CONFLICT(library_id) DO UPDATE SET root_path = excluded.root_path",\n            params![library_id, root_text],\n        )?;''',
    '''        // A Windows drive letter/root path can later belong to another portable\n        // library. Keep a one-to-one registry so stale library ids cannot claim the\n        // new drive while the durable library id remains stored on the drive itself.\n        tx.execute(\n            "DELETE FROM portable_root_registry WHERE root_path = ?1 AND library_id <> ?2",\n            params![root_text, library_id],\n        )?;\n        tx.execute(\n            "INSERT INTO portable_root_registry(library_id, root_path) VALUES(?1, ?2) \\\n             ON CONFLICT(library_id) DO UPDATE SET root_path = excluded.root_path",\n            params![library_id, root_text],\n        )?;''',
)

replace_exact(
    "src/thumbnail_cache.rs",
    '''fn portable_key_for_state(identity: &Path, size: u64, modified_secs: u64, modified_nanos: u32) -> u64 {''',
    '''#[cfg(test)]\nfn portable_key_for_state(identity: &Path, size: u64, modified_secs: u64, modified_nanos: u32) -> u64 {''',
)
