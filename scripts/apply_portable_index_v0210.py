from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:160]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


def replace_all(path: str, old: str, new: str, expected: int) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text and old not in text:
        return
    count = text.count(old)
    if count != expected:
        raise SystemExit(f"{path}: expected {expected} matches, found {count}: {old[:160]!r}")
    p.write_text(text.replace(old, new), encoding="utf-8")


replace_once("Cargo.toml", 'version = "0.2.9"', 'version = "0.2.10"')
replace_once("src/main.rs", "mod preview_benchmark;\n", "mod portable;\nmod preview_benchmark;\n")

# Database schema: portable copies preserve a lightweight decoded-content fingerprint.
replace_once(
    "src/db.rs",
    "            embedding_normalized INTEGER NOT NULL DEFAULT 0,\n            last_seen_scan INTEGER NOT NULL DEFAULT 0\n",
    "            embedding_normalized INTEGER NOT NULL DEFAULT 0,\n            last_seen_scan INTEGER NOT NULL DEFAULT 0,\n            content_fingerprint INTEGER\n",
)
replace_once(
    "src/db.rs",
    '''    ensure_column(\n        &conn,\n        "images",\n        "last_seen_scan",\n        "INTEGER NOT NULL DEFAULT 0",\n    )?;\n    conn.execute(''',
    '''    ensure_column(\n        &conn,\n        "images",\n        "last_seen_scan",\n        "INTEGER NOT NULL DEFAULT 0",\n    )?;\n    ensure_column(&conn, "images", "content_fingerprint", "INTEGER")?;\n    conn.execute(''',
)
replace_once(
    "src/db.rs",
    '''            embedding = NULL,\n            embedding_dim = NULL,\n            embedding_normalized = 0\n''',
    '''            embedding = NULL,\n            embedding_dim = NULL,\n            embedding_normalized = 0,\n            content_fingerprint = NULL\n''',
)
replace_once(
    "src/db.rs",
    "pub fn paths_missing_visual_descriptor(conn: &Connection) -> Result<Vec<PathBuf>> {",
    '''pub fn set_content_fingerprint(conn: &Connection, path: &Path, fingerprint: u64) -> Result<()> {\n    conn.execute(\n        "UPDATE images SET content_fingerprint = ?2 WHERE path = ?1",\n        params![path.to_string_lossy().to_string(), fingerprint as i64],\n    )?;\n    Ok(())\n}\n\npub fn paths_missing_visual_descriptor(conn: &Connection) -> Result<Vec<PathBuf>> {''',
)

# Filesystem events caused by the portable database/cache itself must never recurse into indexing.
replace_once(
    "src/fs_watch.rs",
    "use notify::{Event, EventKind, RecursiveMode, Watcher};\n",
    "use crate::portable;\nuse notify::{Event, EventKind, RecursiveMode, Watcher};\n",
)
replace_once(
    "src/fs_watch.rs",
    '''                if event_kind_needs_indexing(&event.kind) {\n                    pending_paths.extend(event.paths);\n                    flush_at = Some(Instant::now() + EVENT_DEBOUNCE);\n                }''',
    '''                if event_kind_needs_indexing(&event.kind) {\n                    pending_paths.extend(event.paths.into_iter().filter(|path| {\n                        !watched_roots\n                            .iter()\n                            .any(|root| portable::is_internal_path(root, path))\n                    }));\n                    if !pending_paths.is_empty() {\n                        flush_at = Some(Instant::now() + EVENT_DEBOUNCE);\n                    }\n                }''',
)

# Indexer writes the source-derived data to the aggregate/session DB, then mirrors committed
# records to each root's portable DB. Thumbnail creation goes directly to the root cache.
replace_once(
    "src/indexer.rs",
    "use crate::metadata;\n",
    "use crate::metadata;\nuse crate::portable;\n",
)
replace_once(
    "src/indexer.rs",
    "use std::collections::{HashMap, HashSet};\n",
    "use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};\nuse std::hash::Hasher;\n",
)
replace_once(
    "src/indexer.rs",
    "    material_texture: Vec<f32>,\n}",
    "    material_texture: Vec<f32>,\n    content_fingerprint: u64,\n}",
)
replace_once(
    "src/indexer.rs",
    "    let thumbnail_cache_dir = thumbnail_cache::cache_dir_for_db(db_path);\n    let mut conn = db::open(db_path)?;\n    let existing_states = db::load_file_states(&conn)?;\n",
    "    let mut conn = db::open(db_path)?;\n",
)
replace_once(
    "src/indexer.rs",
    '''        let unchanged = existing_states\n            .get(&path)\n            .is_some_and(|state| state.size == size && state.modified == modified);\n        if !unchanged {\n            pending.push(PendingImage {\n                root,\n                path,\n                size,\n                modified,\n            });\n        }''',
    '''        // A filesystem watcher explicitly reported this path. Reindex it even when\n        // size/mtime were preserved by a copy/replace operation.\n        pending.push(PendingImage {\n            root,\n            path,\n            size,\n            modified,\n        });''',
)
replace_once(
    "src/indexer.rs",
    '''    removed_paths.sort();\n    removed_paths.dedup();\n    if !removed_paths.is_empty() {\n        let _ = tx.send(WorkerMessage::RemovedPaths(removed_paths.clone()));\n    }''',
    '''    removed_paths.sort();\n    removed_paths.dedup();\n    if !removed_paths.is_empty() {\n        portable::remove_absolute_paths(roots, &removed_paths)?;\n        let _ = tx.send(WorkerMessage::RemovedPaths(removed_paths.clone()));\n    }''',
)
replace_all(
    "src/indexer.rs",
    "WalkDir::new(&changed).follow_links(false).into_iter()",
    "WalkDir::new(&changed)\n                    .follow_links(false)\n                    .into_iter()\n                    .filter_entry(|entry| entry.file_name() != portable::INDEX_DIR_NAME)",
    1,
)
replace_all(
    "src/indexer.rs",
    "WalkDir::new(root).follow_links(false).into_iter()",
    "WalkDir::new(root)\n            .follow_links(false)\n            .into_iter()\n            .filter_entry(|entry| entry.file_name() != portable::INDEX_DIR_NAME)",
    1,
)
replace_all(
    "src/indexer.rs",
    "inspect_image(&item.path, &thumbnail_cache_dir)",
    "inspect_image(&item.path, &item.root)",
    2,
)
replace_all(
    "src/indexer.rs",
    '''                            material_texture,\n                        )| {''',
    '''                            material_texture,\n                            content_fingerprint,\n                        )| {''',
    2,
)
replace_all(
    "src/indexer.rs",
    '''                                color_histogram,\n                                material_texture,\n                            }''',
    '''                                color_histogram,\n                                material_texture,\n                                content_fingerprint,\n                            }''',
    2,
)
replace_all(
    "src/indexer.rs",
    "                db::set_material_texture(&transaction, &item.path, &item.material_texture)?;\n",
    "                db::set_material_texture(&transaction, &item.path, &item.material_texture)?;\n                db::set_content_fingerprint(&transaction, &item.path, item.content_fingerprint)?;\n",
    2,
)
replace_once(
    "src/indexer.rs",
    '''fn indexed_root_for_path<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {\n    roots\n        .iter()\n        .filter(|root| path.starts_with(root))\n        .max_by_key(|root| root.components().count())\n}''',
    '''fn indexed_root_for_path<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {\n    portable::indexed_root_for_path(path, roots)\n}''',
)
replace_once(
    "src/indexer.rs",
    "    let thumbnail_cache_dir = thumbnail_cache::cache_dir_for_db(db_path);\n    let mut conn = db::open(db_path)?;\n",
    "    let mut conn = db::open(db_path)?;\n",
)
replace_once(
    "src/indexer.rs",
    '''    if !committed_paths.is_empty() {\n        if let Err(err) = build_embeddings(''',
    '''    if !committed_paths.is_empty() {\n        if let Err(err) = build_embeddings(''',
)
replace_once(
    "src/indexer.rs",
    '''    let _ = tx.send(WorkerMessage::Status(format!(\n        "Live index synchronized: {} changed, {} removed",\n        committed_paths.len(),''',
    '''    portable::sync_paths_from_session(&mut conn, &committed_paths)?;\n\n    let _ = tx.send(WorkerMessage::Status(format!(\n        "Live index synchronized: {} changed, {} removed",\n        committed_paths.len(),''',
)
replace_once(
    "src/indexer.rs",
    '''    let _ = tx.send(WorkerMessage::Status(format!(\n        "Index ready: {total} image{} (recursive scan, {traversal_errors} traversal error{})",''',
    '''    for root in roots {\n        if root.exists() {\n            portable::replace_root_from_session(db_path, root)?;\n        }\n    }\n\n    let _ = tx.send(WorkerMessage::Status(format!(\n        "Index ready: {total} image{} (recursive scan, {traversal_errors} traversal error{})",''',
)
replace_once(
    "src/indexer.rs",
    '''            transaction.commit()?;\n        }\n        let done = ((batch_index + 1) * batch_size).min(total);''',
    '''            transaction.commit()?;\n        }\n        portable::sync_paths_from_session(conn, batch)?;\n        let done = ((batch_index + 1) * batch_size).min(total);''',
)
replace_once(
    "src/indexer.rs",
    '''fn inspect_image(\n    path: &Path,\n    thumbnail_cache_dir: &Path,\n) -> Result<(u32, u32, [u8; 3], u64, Vec<f32>, Vec<f32>)> {\n    let image = decode_image(path)?;\n    let (width, height) = image.dimensions();\n\n    // Seed the exact same persistent cache used by the UI while the original\n    // file is already decoded. Thumbnail cache failures never invalidate the\n    // authoritative image index; the UI can still rebuild the preview later.\n    let _ = thumbnail_cache::store_from_decoded(thumbnail_cache_dir, path, &image);\n\n    let (dominant, visual_hash, color_histogram, material_texture) = visual_descriptor(&image);\n    Ok((\n        width,\n        height,\n        dominant,\n        visual_hash,\n        color_histogram,\n        material_texture,\n    ))\n}''',
    '''fn inspect_image(\n    path: &Path,\n    root: &Path,\n) -> Result<(u32, u32, [u8; 3], u64, Vec<f32>, Vec<f32>, u64)> {\n    let image = decode_image(path)?;\n    let (width, height) = image.dimensions();\n\n    // Seed the portable cache while the original file is already decoded. The\n    // cache identity uses the root-relative path, so changing drive letters does\n    // not invalidate thumbnails.\n    let _ = thumbnail_cache::store_from_decoded_for_root(root, path, &image);\n\n    let content_fingerprint = decoded_content_fingerprint(&image);\n    let (dominant, visual_hash, color_histogram, material_texture) = visual_descriptor(&image);\n    Ok((\n        width,\n        height,\n        dominant,\n        visual_hash,\n        color_histogram,\n        material_texture,\n        content_fingerprint,\n    ))\n}\n\nfn decoded_content_fingerprint(image: &DynamicImage) -> u64 {\n    let mut hasher = DefaultHasher::new();\n    hasher.write_u32(image.width());\n    hasher.write_u32(image.height());\n    hasher.write(image.as_bytes());\n    hasher.finish()\n}''',
)

# UI attaches/migrates portable roots before loading the aggregate session and keeps
# the thumbnail scheduler's root map synchronized with Settings changes.
replace_once(
    "src/ui/mod.rs",
    "use crate::indexer::{self, WorkerMessage};\n",
    "use crate::indexer::{self, WorkerMessage};\nuse crate::portable;\n",
)
replace_once(
    "src/ui/mod.rs",
    '''        let roots = db::load_roots(&db_path).unwrap_or_default();\n        let fs_watch_service = FsWatchService::new(roots.clone());\n        let images = db::load_image_summaries(&db_path).unwrap_or_default();''',
    '''        let roots = db::load_roots(&db_path).unwrap_or_default();\n        let portable_warnings = portable::prepare_registered_roots(&db_path, &roots);\n        let fs_watch_service = FsWatchService::new(roots.clone());\n        let images = db::load_image_summaries(&db_path).unwrap_or_default();''',
)
replace_once(
    "src/ui/mod.rs",
    '''        let collections =\n            collections::CollectionsState::load(&db_path, &images).unwrap_or_default();\n        Self {''',
    '''        let collections =\n            collections::CollectionsState::load(&db_path, &images).unwrap_or_default();\n        let thumb_pool = ThumbnailPool::new(thumbnail_cache, roots.clone());\n        let initial_status = portable_warnings\n            .first()\n            .cloned()\n            .unwrap_or_else(|| "Ready".to_owned());\n        Self {''',
)
replace_once(
    "src/ui/mod.rs",
    "            thumb_pool: ThumbnailPool::new(thumbnail_cache),\n",
    "            thumb_pool,\n",
)
replace_once(
    "src/ui/mod.rs",
    '            status: "Ready".into(),\n',
    "            status: initial_status,\n",
)
replace_once(
    "src/ui/mod.rs",
    '''        match db::add_root(&self.db_path, &folder) {\n            Ok(()) => {\n                self.roots = db::load_roots(&self.db_path).unwrap_or_default();\n                self.fs_watch_service.set_roots(self.roots.clone());\n                self.status = format!("Added {}", folder.display());\n            }\n            Err(err) => self.last_error = Some(format!("Cannot add folder: {err:#}")),\n        }''',
    '''        match portable::attach_root(&self.db_path, &folder) {\n            Ok(outcome) => {\n                self.roots = db::load_roots(&self.db_path).unwrap_or_default();\n                self.thumb_pool.set_roots(self.roots.clone());\n                self.fs_watch_service.set_roots(self.roots.clone());\n                self.images = db::load_image_summaries(&self.db_path).unwrap_or_default();\n                self.rebuild_image_positions();\n                self.refresh_collection_effective_membership();\n                self.refresh_text_search_after_data_change();\n                self.status = if outcome.reused_existing_index {\n                    format!(\n                        "Attached portable index: {} ({} cached image records; no rescan required)",\n                        folder.display(),\n                        outcome.images\n                    )\n                } else if outcome.migrated_legacy_rows {\n                    format!(\n                        "Migrated {} image records into {}/.imagesearch",\n                        outcome.images,\n                        folder.display()\n                    )\n                } else {\n                    format!("Portable index initialized: {} — run Rescan to index images", folder.display())\n                };\n            }\n            Err(err) => self.last_error = Some(format!("Cannot attach folder: {err:#}")),\n        }''',
)
replace_once(
    "src/ui/mod.rs",
    '''                self.roots = db::load_roots(&self.db_path).unwrap_or_default();\n                self.images = db::load_image_summaries(&self.db_path).unwrap_or_default();''',
    '''                self.roots = db::load_roots(&self.db_path).unwrap_or_default();\n                self.thumb_pool.set_roots(self.roots.clone());\n                self.images = db::load_image_summaries(&self.db_path).unwrap_or_default();''',
)
replace_once(
    "src/ui/mod.rs",
    '''                            ui.label(root.display().to_string());\n                            if ui''',
    '''                            ui.label(root.display().to_string());\n                            if portable::is_indexed_root(root) {\n                                ui.small("✓ .imagesearch");\n                            }\n                            if ui''',
)

# Keep the normal recursive traversal away from the portable marker even if this
# code path is exercised outside the UI watcher.
