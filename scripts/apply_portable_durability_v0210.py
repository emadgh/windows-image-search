from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:180]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


# Stable fingerprint and immediate durable mirroring of committed batches.
replace_once(
    "src/indexer.rs",
    "use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};\nuse std::hash::Hasher;\n",
    "use std::collections::{HashMap, HashSet};\n",
)
replace_once(
    "src/indexer.rs",
    "    let thumbnail_cache_dir = thumbnail_cache::cache_dir_for_db(db_path);\n    let mut conn = db::open(db_path)?;\n    let existing_states = db::load_file_states(&conn)?;\n",
    "    let mut conn = db::open(db_path)?;\n",
)
replace_once(
    "src/indexer.rs",
    '''            transaction.commit()?;\n        }\n        let live_records = prepared.iter().map(PreparedImage::to_summary).collect();''',
    '''            transaction.commit()?;\n        }\n        let prepared_paths: Vec<PathBuf> = prepared.iter().map(|item| item.path.clone()).collect();\n        portable::sync_paths_from_session(&mut conn, &prepared_paths)?;\n        let live_records = prepared.iter().map(PreparedImage::to_summary).collect();''',
)
replace_once(
    "src/indexer.rs",
    '''            transaction.commit()?;\n        }\n\n        changed += prepared.len();''',
    '''            transaction.commit()?;\n        }\n        let prepared_paths: Vec<PathBuf> = prepared.iter().map(|item| item.path.clone()).collect();\n        portable::sync_paths_from_session(&mut conn, &prepared_paths)?;\n\n        changed += prepared.len();''',
)
replace_once(
    "src/indexer.rs",
    '''        let descriptors: Vec<(PathBuf, u64, Vec<f32>, Vec<f32>)> = pool.install(|| {\n            batch\n                .par_iter()\n                .filter_map(|path| {\n                    let result = decode_image(path).map(|image| visual_descriptor(&image));''',
    '''        let descriptors: Vec<(PathBuf, u64, Vec<f32>, Vec<f32>, u64)> = pool.install(|| {\n            batch\n                .par_iter()\n                .filter_map(|path| {\n                    let result = decode_image(path).map(|image| {\n                        let fingerprint = decoded_content_fingerprint(&image);\n                        let (_, visual_hash, color_histogram, material_texture) = visual_descriptor(&image);\n                        (visual_hash, color_histogram, material_texture, fingerprint)\n                    });''',
)
replace_once(
    "src/indexer.rs",
    '''                    match result {\n                        Ok((_, visual_hash, color_histogram, material_texture)) => {\n                            Some((path.clone(), visual_hash, color_histogram, material_texture))\n                        }''',
    '''                    match result {\n                        Ok((visual_hash, color_histogram, material_texture, fingerprint)) => Some((\n                            path.clone(),\n                            visual_hash,\n                            color_histogram,\n                            material_texture,\n                            fingerprint,\n                        )),''',
)
replace_once(
    "src/indexer.rs",
    '''            for (path, visual_hash, color_histogram, material_texture) in &descriptors {\n                db::set_visual_descriptor(&transaction, path, *visual_hash, color_histogram)?;\n                db::set_material_texture(&transaction, path, material_texture)?;\n            }\n            transaction.commit()?;\n        }\n\n        committed += descriptors.len();''',
    '''            for (path, visual_hash, color_histogram, material_texture, fingerprint) in &descriptors {\n                db::set_visual_descriptor(&transaction, path, *visual_hash, color_histogram)?;\n                db::set_material_texture(&transaction, path, material_texture)?;\n                db::set_content_fingerprint(&transaction, path, *fingerprint)?;\n            }\n            transaction.commit()?;\n        }\n        let descriptor_paths: Vec<PathBuf> = descriptors\n            .iter()\n            .map(|(path, _, _, _, _)| path.clone())\n            .collect();\n        portable::sync_paths_from_session(conn, &descriptor_paths)?;\n\n        committed += descriptors.len();''',
)
replace_once(
    "src/indexer.rs",
    '''fn decoded_content_fingerprint(image: &DynamicImage) -> u64 {\n    let mut hasher = DefaultHasher::new();\n    hasher.write_u32(image.width());\n    hasher.write_u32(image.height());\n    hasher.write(image.as_bytes());\n    hasher.finish()\n}''',
    '''fn decoded_content_fingerprint(image: &DynamicImage) -> u64 {\n    // Stable FNV-1a fingerprint. Unlike DefaultHasher, this value is defined by\n    // us and remains comparable across Rust/application upgrades. Hash decoded\n    // pixels while they are already resident so verification adds no HDD read.\n    const OFFSET: u64 = 0xcbf29ce484222325;\n    const PRIME: u64 = 0x100000001b3;\n    let mut hash = OFFSET;\n    for byte in image\n        .width()\n        .to_le_bytes()\n        .into_iter()\n        .chain(image.height().to_le_bytes())\n        .chain(image.as_bytes().iter().copied())\n    {\n        hash ^= byte as u64;\n        hash = hash.wrapping_mul(PRIME);\n    }\n    hash\n}''',
)

# Expose deterministic/lazy HNSW preparation for any database, including root-local DBs.
replace_once(
    "src/ann.rs",
    '''pub fn default_benchmark_queries() -> usize {\n    DEFAULT_BENCHMARK_QUERIES\n}\n''',
    '''pub fn default_benchmark_queries() -> usize {\n    DEFAULT_BENCHMARK_QUERIES\n}\n\npub fn prepare_index(db_path: &Path) -> Result<bool> {\n    let index_dir = index_dir_for_db(db_path);\n    let (_, rebuilt) = ensure_manifest(db_path, &index_dir)?;\n    Ok(rebuilt)\n}\n''',
)

# Portable root migration also migrates existing thumbnails without decoding sources,
# and keeps a root-local ANN dump ready/rebuildable from the stored embeddings.
replace_once(
    "src/portable.rs",
    "use crate::db;\n",
    "use crate::{ann, db, thumbnail_cache};\n",
)
replace_once(
    "src/portable.rs",
    '''    if !ready {\n        if legacy_count > 0 {\n            replace_root_from_session(session_db_path, root)?;\n            migrated = true;\n        } else if portable_count > 0 {''',
    '''    if !ready {\n        if legacy_count > 0 {\n            replace_root_from_session(session_db_path, root)?;\n            migrate_legacy_thumbnails(session_db_path, root);\n            migrated = true;\n        } else if portable_count > 0 {''',
)
replace_once(
    "src/portable.rs",
    '''    let images = import_root_into_session(session_db_path, root, &library_id)?;\n    Ok(AttachOutcome {''',
    '''    let images = import_root_into_session(session_db_path, root, &library_id)?;\n    // ANN is a derived cache; a missing/stale dump must never make attachment\n    // fail because exact search can still use stored embeddings.\n    let _ = refresh_ann(root);\n    Ok(AttachOutcome {''',
)
replace_once(
    "src/portable.rs",
    '''pub fn remove_absolute_paths(roots: &[PathBuf], paths: &[PathBuf]) -> Result<()> {''',
    '''pub fn refresh_ann(root: &Path) -> Result<bool> {\n    ann::prepare_index(&index_db_path(root))\n}\n\nfn migrate_legacy_thumbnails(session_db_path: &Path, root: &Path) {\n    let fallback = thumbnail_cache::cache_dir_for_db(session_db_path);\n    let Ok(conn) = db::open(session_db_path) else {\n        return;\n    };\n    let Ok(mut stmt) = conn.prepare("SELECT path FROM images WHERE root = ?1") else {\n        return;\n    };\n    let Ok(rows) = stmt.query_map(params![root.to_string_lossy().to_string()], |row| {\n        row.get::<_, String>(0)\n    }) else {\n        return;\n    };\n    let sources: Vec<PathBuf> = rows\n        .filter_map(|row| row.ok())\n        .map(PathBuf::from)\n        .collect();\n    drop(stmt);\n\n    for source in sources {\n        let old = thumbnail_cache::cache_path(&fallback, &source);\n        if !old.is_file() {\n            continue;\n        }\n        let Ok(new) = thumbnail_cache::cache_path_for_root(root, &source) else {\n            continue;\n        };\n        if new.exists() {\n            continue;\n        }\n        if let Some(parent) = new.parent() {\n            let _ = std::fs::create_dir_all(parent);\n        }\n        let _ = std::fs::copy(old, new);\n    }\n}\n\npub fn remove_absolute_paths(roots: &[PathBuf], paths: &[PathBuf]) -> Result<()> {''',
)

# The legacy fallback cache path is now only needed for non-indexed query images.
