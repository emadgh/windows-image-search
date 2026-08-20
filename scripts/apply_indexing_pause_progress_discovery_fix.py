from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if new in text:
        return text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label} anchor count={count}")
    return text.replace(old, new, 1)

# --- db.rs: keep discovered_images correct for live removals ---
path = Path("src/db.rs")
text = path.read_text(encoding="utf-8")
old = '''pub fn delete_stale_discovered_for_root(\n    conn: &Connection,\n    root: &Path,\n    generation: i64,\n) -> Result<usize> {\n    Ok(conn.execute(\n        "DELETE FROM discovered_images WHERE root = ?1 AND last_seen_scan <> ?2",\n        params![root.to_string_lossy().to_string(), generation],\n    )?)\n}\n\npub fn load_discovered_paths'''
new = '''pub fn delete_stale_discovered_for_root(\n    conn: &Connection,\n    root: &Path,\n    generation: i64,\n) -> Result<usize> {\n    Ok(conn.execute(\n        "DELETE FROM discovered_images WHERE root = ?1 AND last_seen_scan <> ?2",\n        params![root.to_string_lossy().to_string(), generation],\n    )?)\n}\n\npub fn delete_discovered_path_tree(conn: &Connection, target: &Path) -> Result<usize> {\n    let target_text = target.to_string_lossy().to_string();\n    let prefix = like_prefix_pattern(target);\n    Ok(conn.execute(\n        "DELETE FROM discovered_images WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\\\' COLLATE NOCASE",\n        params![target_text, prefix],\n    )?)\n}\n\npub fn load_discovered_paths'''
text = replace_once(text, old, new, "delete discovered tree")

anchor = '''    #[test]\n    fn collections_persist_deduplicate_recursive_membership_and_delete_safely() {\n'''
test = '''    #[test]\n    fn live_discovery_tree_removal_preserves_unrelated_paths() {\n        let db_path = temp_db_path("live-discovery-remove");\n        let root = std::env::temp_dir().join("windows-image-search-live-discovery-root");\n        let folder = root.join("folder");\n        let nested = folder.join("nested").join("a.jpg");\n        let direct = folder.join("b.jpg");\n        let sibling = root.join("sibling.jpg");\n        {\n            let mut conn = open(&db_path).unwrap();\n            let generation = next_scan_generation(&conn).unwrap();\n            mark_discovered_paths_seen(\n                &mut conn,\n                generation,\n                &[\n                    (root.clone(), nested.clone()),\n                    (root.clone(), direct.clone()),\n                    (root.clone(), sibling.clone()),\n                ],\n            )\n            .unwrap();\n            assert_eq!(delete_discovered_path_tree(&conn, &folder).unwrap(), 2);\n        }\n        let discovered = load_discovered_paths(&db_path).unwrap();\n        assert_eq!(discovered, vec![sibling]);\n        let _ = std::fs::remove_file(&db_path);\n        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));\n        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));\n    }\n\n'''
if test not in text:
    if text.count(anchor) != 1:
        raise SystemExit(f"db test anchor count={text.count(anchor)}")
    text = text.replace(anchor, test + anchor, 1)
path.write_text(text, encoding="utf-8")

# --- indexer.rs: actual pause checkpoints, concise decode warnings, live counts, search completion isolation ---
path = Path("src/indexer.rs")
text = path.read_text(encoding="utf-8")
old = '''    SimilarityResults(Vec<ImageSummary>),\n    Error(String),\n    Idle,\n}'''
new = '''    SimilarityResults(Vec<ImageSummary>),\n    RootCounts(HashMap<PathBuf, (usize, usize)>),\n    Warning(String),\n    Error(String),\n    SearchIdle,\n    Idle,\n}'''
text = replace_once(text, old, new, "worker variants")

old = '''    for changed in unique_paths {\n        let Some(root) = indexed_root_for_path(&changed, roots) else {'''
new = '''    for changed in unique_paths {\n        control.wait_if_paused();\n        let Some(root) = indexed_root_for_path(&changed, roots) else {'''
text = replace_once(text, old, new, "incremental changed pause")

old = '''                {\n                    match entry {\n                        Ok(entry)\n                            if entry.file_type().is_file() && is_supported_image(entry.path()) =>'''
new = '''                {\n                    control.wait_if_paused();\n                    match entry {\n                        Ok(entry)\n                            if entry.file_type().is_file() && is_supported_image(entry.path()) =>'''
text = replace_once(text, old, new, "incremental traversal pause")

old = '''    let mut removed_paths = Vec::<PathBuf>::new();\n    for target in removed_targets {\n        removed_paths.extend(db::delete_path_tree(&conn, &target)?);\n    }'''
new = '''    let discovery_generation = db::next_scan_generation(&conn)?;\n    let discovered_candidates: Vec<(PathBuf, PathBuf)> = candidates\n        .iter()\n        .map(|(path, root)| (root.clone(), path.clone()))\n        .collect();\n    if !discovered_candidates.is_empty() {\n        db::mark_discovered_paths_seen(\n            &mut conn,\n            discovery_generation,\n            &discovered_candidates,\n        )?;\n    }\n\n    let mut removed_paths = Vec::<PathBuf>::new();\n    for target in removed_targets {\n        control.wait_if_paused();\n        let _ = db::delete_discovered_path_tree(&conn, &target)?;\n        removed_paths.extend(db::delete_path_tree(&conn, &target)?);\n    }'''
text = replace_once(text, old, new, "incremental discovery accounting")

old = '''    if !removed_paths.is_empty() {\n        portable::remove_absolute_paths(roots, &removed_paths)?;\n        let _ = tx.send(WorkerMessage::RemovedPaths(removed_paths.clone()));\n    }\n\n    let mut pending = Vec::<PendingImage>::new();\n    for (path, root) in candidates {'''
new = '''    if !removed_paths.is_empty() {\n        portable::remove_absolute_paths(roots, &removed_paths)?;\n        let _ = tx.send(WorkerMessage::RemovedPaths(removed_paths.clone()));\n    }\n    let _ = tx.send(WorkerMessage::RootCounts(db::load_root_counts(db_path)?));\n\n    let mut pending = Vec::<PendingImage>::new();\n    for (path, root) in candidates {\n        control.wait_if_paused();'''
text = replace_once(text, old, new, "incremental root counts")

old = '''                        Err(err) => {\n                            let _ = tx.send(WorkerMessage::Error(format!(\n                                "Cannot decode changed image {}: {err:#}",\n                                item.path.display()\n                            )));\n                            None\n                        }'''
new = '''                        Err(err) => {\n                            let _ = tx.send(WorkerMessage::Warning(compact_decode_failure(\n                                &item.path,\n                                &err,\n                            )));\n                            None\n                        }'''
text = replace_once(text, old, new, "incremental compact decode")

# Full rescan pause checkpoints.
old = '''    for root in roots {\n        if !root.exists() {'''
new = '''    for root in roots {\n        control.wait_if_paused();\n        if !root.exists() {'''
text = replace_once(text, old, new, "rescan root pause")

old = '''        {\n            match entry {\n                Ok(entry) => {\n                    if entry.file_type().is_file() && is_supported_image(entry.path()) {'''
new = '''        {\n            control.wait_if_paused();\n            match entry {\n                Ok(entry) => {\n                    if entry.file_type().is_file() && is_supported_image(entry.path()) {'''
text = replace_once(text, old, new, "rescan traversal pause")

old = '''    let _ = tx.send(WorkerMessage::Status(format!(\n        "Discovered {discovered_marked}/{total} image paths; checking index state…"\n    )));\n    let mut pending = Vec::<PendingImage>::new();'''
new = '''    let _ = tx.send(WorkerMessage::Status(format!(\n        "Discovered {discovered_marked}/{total} image paths; checking index state…"\n    )));\n    let _ = tx.send(WorkerMessage::RootCounts(db::load_root_counts(db_path)?));\n    let mut pending = Vec::<PendingImage>::new();'''
text = replace_once(text, old, new, "rescan discovery snapshot")

old = '''    for (index, (root, path)) in candidates.iter().enumerate() {\n        let meta = match std::fs::metadata(path) {'''
new = '''    for (index, (root, path)) in candidates.iter().enumerate() {\n        control.wait_if_paused();\n        let meta = match std::fs::metadata(path) {'''
text = replace_once(text, old, new, "rescan state-check pause")

old = '''    for batch in pending.chunks(batch_size) {\n        let prepared: Vec<PreparedImage> = pool.install(|| {\n            batch\n                .par_iter()\n                .filter_map(|item| {\n                    let result = inspect_image(&item.path, &item.root).map('''
new = '''    for batch in pending.chunks(batch_size) {\n        control.wait_if_paused();\n        let prepared: Vec<PreparedImage> = pool.install(|| {\n            batch\n                .par_iter()\n                .filter_map(|item| {\n                    control.wait_if_paused();\n                    let _ = tx.send(WorkerMessage::CurrentFile(\n                        item.path\n                            .file_name()\n                            .and_then(|name| name.to_str())\n                            .unwrap_or_default()\n                            .to_owned(),\n                    ));\n                    let result = inspect_image(&item.path, &item.root).map('''
text = replace_once(text, old, new, "rescan decode pause")

old = '''                        Err(err) => {\n                            let _ = tx.send(WorkerMessage::Error(format!(\n                                "Cannot decode {}: {err:#}",\n                                item.path.display()\n                            )));\n                            None\n                        }'''
new = '''                        Err(err) => {\n                            let _ = tx.send(WorkerMessage::Warning(compact_decode_failure(\n                                &item.path,\n                                &err,\n                            )));\n                            None\n                        }'''
text = replace_once(text, old, new, "rescan compact decode")

old = '''                        Err(err) => {\n                            failed.fetch_add(1, Ordering::Relaxed);\n                            let _ = tx.send(WorkerMessage::Error(format!(\n                                "Cannot build visual descriptor for {}: {err:#}",\n                                path.display()\n                            )));\n                            None\n                        }'''
new = '''                        Err(err) => {\n                            failed.fetch_add(1, Ordering::Relaxed);\n                            let _ = tx.send(WorkerMessage::Warning(compact_decode_failure(\n                                path,\n                                &err,\n                            )));\n                            None\n                        }'''
text = replace_once(text, old, new, "visual compact decode")

old = '''    for root in roots {\n        if root.exists() {\n            portable::replace_root_from_session(db_path, root)?;\n        }\n    }'''
new = '''    for root in roots {\n        control.wait_if_paused();\n        if root.exists() {\n            portable::replace_root_from_session(db_path, root)?;\n        }\n    }'''
text = replace_once(text, old, new, "portable replace pause")

# Compact non-fatal decoder status helper.
anchor = '''fn indexed_root_for_path<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {\n'''
helper = '''fn compact_decode_failure(path: &Path, err: &anyhow::Error) -> String {\n    let detail = format!("{err:#}");\n    let lower = detail.to_ascii_lowercase();\n    let reason = if lower.contains("illegal start bytes")\n        || lower.contains("format error decoding jpeg")\n        || lower.contains("jpeg") && lower.contains("format error")\n    {\n        "invalid JPEG data"\n    } else if lower.contains("unexpected eof")\n        || lower.contains("unexpected end")\n        || lower.contains("end of file")\n        || lower.contains("truncated")\n    {\n        "truncated image"\n    } else if lower.contains("unsupported") {\n        "unsupported image format"\n    } else if lower.contains("permission denied") || lower.contains("access is denied") {\n        "access denied"\n    } else {\n        "decode error"\n    };\n    let name = path\n        .file_name()\n        .and_then(|value| value.to_str())\n        .unwrap_or("image");\n    format!("Decode failed: {name} — {reason}")\n}\n\n'''
if helper not in text:
    if text.count(anchor) != 1:
        raise SystemExit(f"compact helper anchor count={text.count(anchor)}")
    text = text.replace(anchor, helper + anchor, 1)

# Search worker can coexist with a paused index without sending the index Idle signal.
old = '''pub fn spawn_similarity_search(\n    db_path: PathBuf,\n    query_path: PathBuf,\n    settings: SimilaritySettings,\n    indexing_settings: IndexingSettings,\n    embedding_service: EmbeddingService,\n    tx: Sender<WorkerMessage>,\n) {'''
new = '''pub fn spawn_similarity_search(\n    db_path: PathBuf,\n    query_path: PathBuf,\n    settings: SimilaritySettings,\n    indexing_settings: IndexingSettings,\n    embedding_service: EmbeddingService,\n    allow_descriptor_backfill: bool,\n    tx: Sender<WorkerMessage>,\n) {'''
text = replace_once(text, old, new, "search spawn signature")

old = '''            indexing_settings,\n            &embedding_service,\n            &tx,\n        ) {'''
new = '''            indexing_settings,\n            &embedding_service,\n            allow_descriptor_backfill,\n            &tx,\n        ) {'''
text = replace_once(text, old, new, "search allow backfill call")

old = '''        let _ = tx.send(WorkerMessage::Idle);\n    });\n}\n\n#[derive(Clone, Copy, Debug)]\nstruct SimilarityMetrics'''
new = '''        let _ = tx.send(WorkerMessage::SearchIdle);\n    });\n}\n\n#[derive(Clone, Copy, Debug)]\nstruct SimilarityMetrics'''
text = replace_once(text, old, new, "search idle isolation")

old = '''fn similarity_search(\n    db_path: &Path,\n    query_path: &Path,\n    settings: SimilaritySettings,\n    indexing_settings: IndexingSettings,\n    embedding_service: &EmbeddingService,\n    tx: &Sender<WorkerMessage>,\n) -> Result<Vec<ImageSummary>> {'''
new = '''fn similarity_search(\n    db_path: &Path,\n    query_path: &Path,\n    settings: SimilaritySettings,\n    indexing_settings: IndexingSettings,\n    embedding_service: &EmbeddingService,\n    allow_descriptor_backfill: bool,\n    tx: &Sender<WorkerMessage>,\n) -> Result<Vec<ImageSummary>> {'''
text = replace_once(text, old, new, "search signature")

old = '''    let missing_visual = db::paths_missing_visual_descriptor(&conn)?;\n    if !missing_visual.is_empty() {\n        let _ = tx.send(WorkerMessage::Status(format!(\n            "Upgrading texture/color index: {} image{}…",\n            missing_visual.len(),\n            if missing_visual.len() == 1 { "" } else { "s" }\n        )));\n        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, None, tx)?;\n    }'''
new = '''    let missing_visual = db::paths_missing_visual_descriptor(&conn)?;\n    if !missing_visual.is_empty() && allow_descriptor_backfill {\n        let _ = tx.send(WorkerMessage::Status(format!(\n            "Upgrading texture/color index: {} image{}…",\n            missing_visual.len(),\n            if missing_visual.len() == 1 { "" } else { "s" }\n        )));\n        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, None, tx)?;\n    } else if !missing_visual.is_empty() {\n        let _ = tx.send(WorkerMessage::Status(format!(\n            "Searching committed index; {} pending descriptor{} skipped while indexing is paused",\n            missing_visual.len(),\n            if missing_visual.len() == 1 { "" } else { "s" }\n        )));\n    }'''
text = replace_once(text, old, new, "paused search no backfill")

# Add concise-reason regression test; pause control already has a blocking test.
anchor = '''    #[test]\n    fn force_inspection_reuses_valid_thumbnail_and_preserves_source_identity() {\n'''
test = '''    #[test]\n    fn compact_decode_failure_hides_nested_decoder_chain() {\n        let err = anyhow::anyhow!(\n            "Format error decoding Jpeg: Error parsing image. Illegal start bytes:3842"\n        );\n        let message = compact_decode_failure(Path::new("R:/tiles/_1791925316.jpg"), &err);\n        assert_eq!(\n            message,\n            "Decode failed: _1791925316.jpg — invalid JPEG data"\n        );\n        assert!(!message.contains("Illegal start bytes"));\n    }\n\n'''
if test not in text:
    if text.count(anchor) != 1:
        raise SystemExit(f"indexer test anchor count={text.count(anchor)}")
    text = text.replace(anchor, test + anchor, 1)

path.write_text(text, encoding="utf-8")

# --- ui/mod.rs: distinct search state, paused search enablement, live root counts, settings progress ---
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
old = '''    index_control: Option<indexer::IndexControl>,\n    index_paused: bool,\n    current_file: Option<String>,'''
new = '''    index_control: Option<indexer::IndexControl>,\n    index_paused: bool,\n    searching: bool,\n    current_file: Option<String>,'''
text = replace_once(text, old, new, "searching field")

old = '''            index_control: None,\n            index_paused: false,\n            current_file: None,'''
new = '''            index_control: None,\n            index_paused: false,\n            searching: false,\n            current_file: None,'''
text = replace_once(text, old, new, "searching init")

old = '''                WorkerMessage::IndexedBatch(records) => {\n                    self.merge_indexed_batch(records);\n                    self.refresh_collection_effective_membership();\n                    self.refresh_text_search_after_data_change();\n                }'''
new = '''                WorkerMessage::IndexedBatch(records) => {\n                    self.merge_indexed_batch(records);\n                    self.refresh_live_root_indexed_counts();\n                    self.refresh_collection_effective_membership();\n                    self.refresh_text_search_after_data_change();\n                }'''
text = replace_once(text, old, new, "indexed batch live counts")

old = '''                WorkerMessage::RemovedPaths(paths) => {\n                    self.remove_indexed_paths(paths);\n                    self.refresh_collection_effective_membership();\n                    self.refresh_text_search_after_data_change();\n                }'''
new = '''                WorkerMessage::RemovedPaths(paths) => {\n                    self.remove_indexed_paths(paths);\n                    self.refresh_live_root_indexed_counts();\n                    self.refresh_collection_effective_membership();\n                    self.refresh_text_search_after_data_change();\n                }'''
text = replace_once(text, old, new, "removed live counts")

old = '''                WorkerMessage::SimilarityResults(results) => {\n                    self.similarity_results = Some(results);\n                    self.progress = None;\n                }\n                WorkerMessage::Error(error) => {\n                    self.last_error = Some(error.clone());\n                    self.status = error;\n                }\n                WorkerMessage::Idle => {\n                    self.busy = false;\n                    self.indexing = false;\n                    self.index_paused = false;\n                    self.index_control = None;\n                    self.current_file = None;\n                    self.progress = None;\n                    self.close_confirmation_open = false;\n                }'''
new = '''                WorkerMessage::SimilarityResults(results) => {\n                    self.similarity_results = Some(results);\n                    if !self.indexing {\n                        self.progress = None;\n                    }\n                }\n                WorkerMessage::RootCounts(counts) => {\n                    self.root_counts = counts;\n                    self.refresh_live_root_indexed_counts();\n                    let _ = self.collections.refresh_discovered_counts(&self.db_path);\n                }\n                WorkerMessage::Warning(warning) => {\n                    self.last_error = Some(warning.clone());\n                    self.status = warning;\n                }\n                WorkerMessage::Error(error) => {\n                    self.last_error = Some(error.clone());\n                    self.status = error;\n                }\n                WorkerMessage::SearchIdle => {\n                    self.searching = false;\n                    self.busy = self.indexing;\n                }\n                WorkerMessage::Idle => {\n                    self.indexing = false;\n                    self.index_paused = false;\n                    self.index_control = None;\n                    self.current_file = None;\n                    self.progress = None;\n                    self.close_confirmation_open = false;\n                    self.busy = self.searching;\n                }'''
text = replace_once(text, old, new, "worker completion handling")

anchor = '''    fn merge_indexed_batch(&mut self, records: Vec<ImageSummary>) {\n'''
helper = '''    fn refresh_live_root_indexed_counts(&mut self) {\n        let mut indexed = HashMap::<PathBuf, usize>::new();\n        for image in &self.images {\n            *indexed.entry(image.root.clone()).or_default() += 1;\n        }\n        for root in &self.roots {\n            let indexed_count = indexed.get(root).copied().unwrap_or(0);\n            let discovered = self\n                .root_counts\n                .get(root)\n                .map(|counts| counts.0)\n                .unwrap_or(0)\n                .max(indexed_count);\n            self.root_counts\n                .insert(root.clone(), (discovered, indexed_count));\n        }\n    }\n\n'''
if helper not in text:
    if text.count(anchor) != 1:
        raise SystemExit(f"root count helper anchor count={text.count(anchor)}")
    text = text.replace(anchor, helper + anchor, 1)

# Allow image search only when idle or the index worker is paused.
old = '''    fn choose_similarity_image(&mut self) {\n        if self.busy || self.indexing {\n            return;\n        }'''
new = '''    fn can_run_similarity_search(&self) -> bool {\n        !self.searching\n            && !self.images.is_empty()\n            && ((!self.busy && !self.indexing) || (self.indexing && self.index_paused))\n    }\n\n    fn choose_similarity_image(&mut self) {\n        if !self.can_run_similarity_search() {\n            return;\n        }'''
text = replace_once(text, old, new, "paused search eligibility")

old = '''    fn run_similarity_search(&mut self, path: PathBuf) {\n        if self.busy || self.indexing {\n            return;\n        }\n        self.busy = true;\n        self.last_error = None;'''
new = '''    fn run_similarity_search(&mut self, path: PathBuf) {\n        if self.searching\n            || (self.indexing && !self.index_paused)\n            || (!self.indexing && self.busy)\n        {\n            return;\n        }\n        let allow_descriptor_backfill = !self.indexing;\n        self.searching = true;\n        self.busy = true;\n        self.last_error = None;'''
text = replace_once(text, old, new, "run paused search")

old = '''            self.indexing_settings,\n            self.embedding_service.clone(),\n            self.tx.clone(),\n        );\n    }\n\n    pub(super) fn source'''
new = '''            self.indexing_settings,\n            self.embedding_service.clone(),\n            allow_descriptor_backfill,\n            self.tx.clone(),\n        );\n    }\n\n    pub(super) fn source'''
text = replace_once(text, old, new, "search spawn ui call")

# Settings receives the same progress/current-file/pause state as the bottom bar.
old = '''                });\n                ui.separator();\n                if self.roots.is_empty() {'''
new = '''                });\n                if self.indexing || self.searching || self.progress.is_some() {\n                    ui.add_space(6.0);\n                    ui.group(|ui| {\n                        ui.horizontal(|ui| {\n                            if self.indexing && !self.index_paused {\n                                ui.spinner();\n                            }\n                            if self.index_paused {\n                                ui.strong("Indexing paused");\n                            } else if self.indexing {\n                                ui.strong("Indexing");\n                            } else if self.searching {\n                                ui.strong("Image search");\n                            }\n                            if self.indexing && self.index_control.is_some() {\n                                let label = if self.index_paused { "▶ Resume" } else { "⏸ Pause" };\n                                if ui\n                                    .add_enabled(!self.searching, egui::Button::new(label).small())\n                                    .clicked()\n                                {\n                                    self.toggle_index_pause();\n                                }\n                            }\n                        });\n                        if let Some((done, total)) = self.progress.filter(|(_, total)| *total > 0) {\n                            ui.add(\n                                egui::ProgressBar::new(done as f32 / total as f32)\n                                    .desired_width(ui.available_width().min(620.0))\n                                    .text(format!("{done}/{total}")),\n                            );\n                        }\n                        if let Some(file_name) = &self.current_file {\n                            ui.small(format!("Current: {file_name}"));\n                        }\n                        ui.small(views::truncate_middle(&self.status, 96))\n                            .on_hover_text(&self.status);\n                    });\n                }\n                ui.separator();\n                if self.roots.is_empty() {'''
text = replace_once(text, old, new, "settings progress")

# Search controls use paused-index eligibility.
old = '''                            .add_enabled(\n                                !self.busy && !self.indexing && !self.images.is_empty(),\n                                egui::Button::new("◉ Search by image"),\n                            )'''
new = '''                            .add_enabled(\n                                self.can_run_similarity_search(),\n                                egui::Button::new("◉ Search by image"),\n                            )'''
text = replace_once(text, old, new, "search button eligibility")

old = '''                    if self.indexing {\n                        ui.small("Image similarity search is disabled while indexing.");\n                    }'''
new = '''                    if self.indexing && !self.index_paused {\n                        ui.small("Pause indexing to search the already committed images.");\n                    } else if self.indexing && self.index_paused {\n                        ui.small("Indexing is paused; image search uses committed data only.");\n                    }'''
text = replace_once(text, old, new, "paused search hint")

old = '''                            .add_enabled(\n                                !self.busy && !self.indexing && self.query_image.is_some(),\n                                egui::Button::new("Apply / re-run"),\n                            )'''
new = '''                            .add_enabled(\n                                self.query_image.is_some() && self.can_run_similarity_search(),\n                                egui::Button::new("Apply / re-run"),\n                            )'''
text = replace_once(text, old, new, "rerun eligibility")

# Keep bottom status compact and do not allow Resume while a paused-index search is running.
old = '''                ui.label(&self.status);'''
new = '''                ui.small(views::truncate_middle(&self.status, 96))\n                    .on_hover_text(&self.status);'''
text = replace_once(text, old, new, "compact bottom status")

old = '''                        if ui.button(label).clicked() {\n                            self.toggle_index_pause();\n                        }'''
new = '''                        if ui\n                            .add_enabled(!self.searching, egui::Button::new(label))\n                            .on_hover_text(if self.searching {\n                                "Finish the paused-index image search before resuming indexing"\n                            } else {\n                                "Pause or resume indexing"\n                            })\n                            .clicked()\n                        {\n                            self.toggle_index_pause();\n                        }'''
text = replace_once(text, old, new, "pause button search guard")

path.write_text(text, encoding="utf-8")

print("indexing pause/progress/discovery regression fix applied")
