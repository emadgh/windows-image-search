from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


# -----------------------------------------------------------------------------
# Cargo.toml
# -----------------------------------------------------------------------------
path = Path("Cargo.toml")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    'kamadak-exif = "0.6"\nopen = "5"\n',
    'kamadak-exif = "0.6"\nnotify = "8.2"\nopen = "5"\n',
    "notify dependency",
)
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# fs_watch.rs
# -----------------------------------------------------------------------------
Path("src/fs_watch.rs").write_text(
    r'''use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

const EVENT_DEBOUNCE: Duration = Duration::from_millis(420);

#[derive(Debug)]
pub enum FsWatchMessage {
    PathsChanged(Vec<PathBuf>),
    ReconcileRequired(String),
    Status(String),
}

enum ControlMessage {
    SetRoots(Vec<PathBuf>),
}

pub struct FsWatchService {
    control_tx: Sender<ControlMessage>,
    result_rx: Receiver<FsWatchMessage>,
}

impl FsWatchService {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        let (control_tx, control_rx) = mpsc::channel::<ControlMessage>();
        let (result_tx, result_rx) = mpsc::channel::<FsWatchMessage>();

        std::thread::Builder::new()
            .name("filesystem-watch-service".to_owned())
            .spawn(move || run_watcher(roots, control_rx, result_tx))
            .expect("creating filesystem watcher worker");

        Self {
            control_tx,
            result_rx,
        }
    }

    pub fn set_roots(&self, roots: Vec<PathBuf>) {
        let _ = self.control_tx.send(ControlMessage::SetRoots(roots));
    }

    pub fn try_recv(&self) -> Option<FsWatchMessage> {
        self.result_rx.try_recv().ok()
    }
}

fn run_watcher(
    initial_roots: Vec<PathBuf>,
    control_rx: Receiver<ControlMessage>,
    result_tx: Sender<FsWatchMessage>,
) {
    let (event_tx, event_rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = match notify::recommended_watcher(event_tx) {
        Ok(watcher) => watcher,
        Err(err) => {
            let _ = result_tx.send(FsWatchMessage::ReconcileRequired(format!(
                "Live filesystem watcher could not start: {err}"
            )));
            return;
        }
    };

    let mut watched_roots = Vec::<PathBuf>::new();
    replace_roots(
        &mut watcher,
        &mut watched_roots,
        initial_roots,
        &result_tx,
    );

    let mut pending_paths = HashSet::<PathBuf>::new();
    let mut flush_at: Option<Instant> = None;

    loop {
        while let Ok(control) = control_rx.try_recv() {
            match control {
                ControlMessage::SetRoots(roots) => replace_roots(
                    &mut watcher,
                    &mut watched_roots,
                    roots,
                    &result_tx,
                ),
            }
        }

        let timeout = flush_at
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_millis(100))
            .min(Duration::from_millis(100));

        match event_rx.recv_timeout(timeout) {
            Ok(Ok(event)) => {
                if event_kind_needs_indexing(&event.kind) {
                    pending_paths.extend(event.paths);
                    flush_at = Some(Instant::now() + EVENT_DEBOUNCE);
                }
            }
            Ok(Err(err)) => {
                let _ = result_tx.send(FsWatchMessage::ReconcileRequired(format!(
                    "Filesystem watcher reported an error; run Rescan to reconcile the index: {err}"
                )));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let _ = result_tx.send(FsWatchMessage::ReconcileRequired(
                    "Filesystem watcher event channel stopped; run Rescan to reconcile the index"
                        .to_owned(),
                ));
                return;
            }
        }

        if flush_at.is_some_and(|deadline| Instant::now() >= deadline) {
            flush_at = None;
            if !pending_paths.is_empty() {
                let mut paths: Vec<PathBuf> = pending_paths.drain().collect();
                paths.sort();
                let _ = result_tx.send(FsWatchMessage::PathsChanged(paths));
            }
        }
    }
}

fn replace_roots(
    watcher: &mut notify::RecommendedWatcher,
    watched_roots: &mut Vec<PathBuf>,
    new_roots: Vec<PathBuf>,
    result_tx: &Sender<FsWatchMessage>,
) {
    for root in watched_roots.drain(..) {
        let _ = watcher.unwatch(&root);
    }

    let mut active = Vec::new();
    for root in new_roots {
        if !root.exists() {
            let _ = result_tx.send(FsWatchMessage::ReconcileRequired(format!(
                "Cannot live-watch missing indexed root: {}",
                root.display()
            )));
            continue;
        }
        match watcher.watch(&root, RecursiveMode::Recursive) {
            Ok(()) => active.push(root),
            Err(err) => {
                let _ = result_tx.send(FsWatchMessage::ReconcileRequired(format!(
                    "Cannot live-watch {}: {err}",
                    root.display()
                )));
            }
        }
    }

    *watched_roots = active;
    let _ = result_tx.send(FsWatchMessage::Status(format!(
        "Live filesystem watching: {} indexed root{}",
        watched_roots.len(),
        if watched_roots.len() == 1 { "" } else { "s" }
    )));
}

fn event_kind_needs_indexing(kind: &EventKind) -> bool {
    !matches!(kind, EventKind::Access(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, ModifyKind, RemoveKind};

    #[test]
    fn access_events_are_ignored_but_content_changes_are_kept() {
        assert!(!event_kind_needs_indexing(&EventKind::Access(AccessKind::Any)));
        assert!(event_kind_needs_indexing(&EventKind::Create(CreateKind::Any)));
        assert!(event_kind_needs_indexing(&EventKind::Modify(ModifyKind::Any)));
        assert!(event_kind_needs_indexing(&EventKind::Remove(RemoveKind::Any)));
    }
}
''',
    encoding="utf-8",
)


# -----------------------------------------------------------------------------
# main.rs
# -----------------------------------------------------------------------------
path = Path("src/main.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "mod embedding;\nmod indexer;\n",
    "mod embedding;\nmod fs_watch;\nmod indexer;\n",
    "filesystem watcher module",
)
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# db.rs: targeted delete for a removed file or removed directory tree.
# -----------------------------------------------------------------------------
path = Path("src/db.rs")
text = path.read_text(encoding="utf-8")
insert_marker = "pub fn next_scan_generation(conn: &Connection) -> Result<i64> {\n"
delete_helper = r'''fn like_prefix_pattern(path: &Path) -> String {
    let mut text = path.to_string_lossy().to_string();
    if !text.ends_with(std::path::MAIN_SEPARATOR) {
        text.push(std::path::MAIN_SEPARATOR);
    }
    let escaped = text
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("{escaped}%")
}

pub fn delete_path_tree(conn: &Connection, target: &Path) -> Result<Vec<PathBuf>> {
    let target_text = target.to_string_lossy().to_string();
    let prefix = like_prefix_pattern(target);
    let mut stmt = conn.prepare(
        "SELECT path FROM images WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\' COLLATE NOCASE",
    )?;
    let paths: Vec<PathBuf> = stmt
        .query_map(params![target_text, prefix], |row| row.get::<_, String>(0))?
        .filter_map(|row| row.ok())
        .map(PathBuf::from)
        .collect();
    drop(stmt);

    if !paths.is_empty() {
        conn.execute(
            "DELETE FROM images WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\' COLLATE NOCASE",
            params![target.to_string_lossy().to_string(), like_prefix_pattern(target)],
        )?;
    }
    Ok(paths)
}

'''
text = replace_once(text, insert_marker, delete_helper + insert_marker, "targeted delete helper")

# Regression test for removed directory tree.
test_marker = '''    #[test]
    fn fts_text_search_supports_substrings_and_and_semantics() {
'''
test_block = r'''    #[test]
    fn delete_path_tree_removes_only_the_requested_subtree() {
        let db_path = temp_db_path("delete-path-tree");
        let root = std::env::temp_dir().join("windows-image-search-delete-root");
        let folder = root.join("folder");
        let first = folder.join("first.jpg");
        let second = folder.join("nested").join("second.jpg");
        let keep = root.join("keep.jpg");

        {
            let conn = open(&db_path).unwrap();
            for path in [&first, &second, &keep] {
                let name = path.file_name().unwrap().to_string_lossy();
                upsert_image(
                    &conn,
                    path,
                    &root,
                    &name,
                    "jpg",
                    1,
                    1,
                    8,
                    8,
                    "",
                    "",
                    [1, 2, 3],
                    1,
                    &[1.0],
                )
                .unwrap();
            }
            let removed = delete_path_tree(&conn, &folder).unwrap();
            assert_eq!(removed.len(), 2);
            let states = load_file_states(&conn).unwrap();
            assert_eq!(states.len(), 1);
            assert!(states.contains_key(&keep));
        }

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }

    #[test]
    fn fts_text_search_supports_substrings_and_and_semantics() {
'''
text = replace_once(text, test_marker, test_block, "targeted delete regression test")
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# indexer.rs: incremental indexing entry point for watcher paths.
# -----------------------------------------------------------------------------
path = Path("src/indexer.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "use std::path::{Path, PathBuf};\n",
    "use std::collections::{HashMap, HashSet};\nuse std::path::{Path, PathBuf};\n",
    "incremental collection imports",
)
text = replace_once(
    text,
    "    IndexedBatch(Vec<ImageSummary>),\n    SimilarityResults(Vec<ImageSummary>),\n",
    "    IndexedBatch(Vec<ImageSummary>),\n    RemovedPaths(Vec<PathBuf>),\n    SimilarityResults(Vec<ImageSummary>),\n",
    "removed paths worker message",
)

insert_before = "fn rescan(\n"
incremental_code = r'''pub fn spawn_incremental_update(
    db_path: PathBuf,
    roots: Vec<PathBuf>,
    changed_paths: Vec<PathBuf>,
    indexing_settings: IndexingSettings,
    embedding_service: EmbeddingService,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let result = incremental_update(
            &db_path,
            &roots,
            &changed_paths,
            indexing_settings,
            &embedding_service,
            &tx,
        );
        if let Err(err) = result {
            let _ = tx.send(WorkerMessage::Error(format!(
                "Live indexing failed: {err:#}"
            )));
        }
        let _ = tx.send(WorkerMessage::Idle);
    });
}

fn incremental_update(
    db_path: &Path,
    roots: &[PathBuf],
    changed_paths: &[PathBuf],
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let indexing_settings = indexing_settings.sanitized();
    let mut conn = db::open(db_path)?;
    let existing_states = db::load_file_states(&conn)?;
    let unique_paths: HashSet<PathBuf> = changed_paths.iter().cloned().collect();
    let mut candidates = HashMap::<PathBuf, PathBuf>::new();
    let mut removed_targets = Vec::<PathBuf>::new();

    for changed in unique_paths {
        let Some(root) = indexed_root_for_path(&changed, roots) else {
            continue;
        };
        if changed.exists() {
            if changed.is_file() {
                if is_supported_image(&changed) {
                    candidates.insert(changed, root.clone());
                }
            } else if changed.is_dir() {
                for entry in WalkDir::new(&changed).follow_links(false).into_iter() {
                    match entry {
                        Ok(entry) if entry.file_type().is_file() && is_supported_image(entry.path()) => {
                            candidates.insert(entry.into_path(), root.clone());
                        }
                        Ok(_) => {}
                        Err(err) => {
                            let _ = tx.send(WorkerMessage::Error(format!(
                                "Live subtree scan could not access {}: {err}",
                                changed.display()
                            )));
                        }
                    }
                }
            }
        } else {
            removed_targets.push(changed);
        }
    }

    let mut removed_paths = Vec::<PathBuf>::new();
    for target in removed_targets {
        removed_paths.extend(db::delete_path_tree(&conn, &target)?);
    }
    removed_paths.sort();
    removed_paths.dedup();
    if !removed_paths.is_empty() {
        let _ = tx.send(WorkerMessage::RemovedPaths(removed_paths.clone()));
    }

    let mut pending = Vec::<PendingImage>::new();
    for (path, root) in candidates {
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let size = meta.len();
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        let unchanged = existing_states
            .get(&path)
            .is_some_and(|state| state.size == size && state.modified == modified);
        if !unchanged {
            pending.push(PendingImage {
                root,
                path,
                size,
                modified,
            });
        }
    }

    if pending.is_empty() {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Live index synchronized: 0 changed, {} removed",
            removed_paths.len()
        )));
        return Ok(());
    }

    let workers = indexing_settings.decode_workers;
    let batch_size = indexing_settings.batch_size;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|index| format!("live-image-index-{index}"))
        .build()
        .context("creating live image indexing worker pool")?;
    let mut committed_paths = Vec::<PathBuf>::new();
    let total = pending.len();
    let done = AtomicUsize::new(0);

    for batch in pending.chunks(batch_size) {
        let prepared: Vec<PreparedImage> = pool.install(|| {
            batch
                .par_iter()
                .filter_map(|item| {
                    let result = inspect_image(&item.path).map(
                        |(width, height, dominant, visual_hash, color_histogram)| {
                            let text = metadata::extract(&item.path);
                            PreparedImage {
                                root: item.root.clone(),
                                path: item.path.clone(),
                                file_name: item
                                    .path
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or_default()
                                    .to_owned(),
                                extension: item
                                    .path
                                    .extension()
                                    .and_then(|ext| ext.to_str())
                                    .unwrap_or_default()
                                    .to_ascii_lowercase(),
                                size: item.size,
                                modified: item.modified,
                                width,
                                height,
                                description: text.description,
                                keywords: text.keywords,
                                dominant,
                                visual_hash,
                                color_histogram,
                            }
                        },
                    );
                    let current = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if current == total || current % 8 == 0 {
                        let _ = tx.send(WorkerMessage::Status(format!(
                            "Live indexing changed files: {current}/{total}"
                        )));
                    }
                    match result {
                        Ok(value) => Some(value),
                        Err(err) => {
                            let _ = tx.send(WorkerMessage::Error(format!(
                                "Cannot decode changed image {}: {err:#}",
                                item.path.display()
                            )));
                            None
                        }
                    }
                })
                .collect()
        });

        if prepared.is_empty() {
            continue;
        }
        {
            let transaction = conn.transaction()?;
            for item in &prepared {
                db::upsert_image(
                    &transaction,
                    &item.path,
                    &item.root,
                    &item.file_name,
                    &item.extension,
                    item.size,
                    item.modified,
                    item.width,
                    item.height,
                    &item.description,
                    &item.keywords,
                    item.dominant,
                    item.visual_hash,
                    &item.color_histogram,
                )?;
                committed_paths.push(item.path.clone());
            }
            transaction.commit()?;
        }
        let live_records = prepared.iter().map(PreparedImage::to_summary).collect();
        let _ = tx.send(WorkerMessage::IndexedBatch(live_records));
    }

    if !committed_paths.is_empty() {
        if let Err(err) = build_embeddings(
            &mut conn,
            &committed_paths,
            indexing_settings,
            embedding_service,
            tx,
        ) {
            let _ = tx.send(WorkerMessage::Error(format!(
                "Live metadata/visual index is ready, but CLIP update failed: {err:#}"
            )));
        }
    }

    let _ = tx.send(WorkerMessage::Status(format!(
        "Live index synchronized: {} changed, {} removed",
        committed_paths.len(),
        removed_paths.len()
    )));
    Ok(())
}

fn indexed_root_for_path<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
}

'''
text = replace_once(text, insert_before, incremental_code + insert_before, "incremental indexer insertion")
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# ui/mod.rs: own watcher, queue events while busy, live-delete/update UI records.
# -----------------------------------------------------------------------------
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "use crate::embedding::EmbeddingService;\n",
    "use crate::embedding::EmbeddingService;\nuse crate::fs_watch::{FsWatchMessage, FsWatchService};\n",
    "UI watcher import",
)
text = replace_once(
    text,
    '''    embedding_service: EmbeddingService,
    pub(super) roots: Vec<PathBuf>,
''',
    '''    embedding_service: EmbeddingService,
    fs_watch_service: FsWatchService,
    pending_fs_paths: HashSet<PathBuf>,
    watcher_reconcile_required: Option<String>,
    pub(super) roots: Vec<PathBuf>,
''',
    "UI watcher fields",
)
text = replace_once(
    text,
    '''        let indexing_settings = settings::load(&settings_path);
        let embedding_service = EmbeddingService::new(model_cache);
        let text_search_service = TextSearchService::new(db_path.clone());
        let images = db::load_image_summaries(&db_path).unwrap_or_default();
''',
    '''        let indexing_settings = settings::load(&settings_path);
        let embedding_service = EmbeddingService::new(model_cache);
        let text_search_service = TextSearchService::new(db_path.clone());
        let roots = db::load_roots(&db_path).unwrap_or_default();
        let fs_watch_service = FsWatchService::new(roots.clone());
        let images = db::load_image_summaries(&db_path).unwrap_or_default();
''',
    "create filesystem watcher",
)
text = replace_once(
    text,
    '''        Self {
            roots: db::load_roots(&db_path).unwrap_or_default(),
            images,
''',
    '''        Self {
            roots,
            images,
''',
    "use preloaded roots",
)
text = replace_once(
    text,
    '''            db_path,
            embedding_service,
            similarity_results: None,
''',
    '''            db_path,
            embedding_service,
            fs_watch_service,
            pending_fs_paths: HashSet::new(),
            watcher_reconcile_required: None,
            similarity_results: None,
''',
    "initialize watcher state",
)

# Worker message removal handling and launch queued watch updates after Idle.
text = replace_once(
    text,
    '''                WorkerMessage::IndexedBatch(records) => {
                    self.merge_indexed_batch(records);
                    self.refresh_text_search_after_data_change();
                }
                WorkerMessage::Reload => {
''',
    '''                WorkerMessage::IndexedBatch(records) => {
                    self.merge_indexed_batch(records);
                    self.refresh_text_search_after_data_change();
                }
                WorkerMessage::RemovedPaths(paths) => {
                    self.remove_indexed_paths(paths);
                    self.refresh_text_search_after_data_change();
                }
                WorkerMessage::Reload => {
''',
    "UI removed paths worker message",
)
text = replace_once(
    text,
    '''                WorkerMessage::Idle => {
                    self.busy = false;
                    self.indexing = false;
                    self.close_confirmation_open = false;
                }
            }
        }
    }
''',
    '''                WorkerMessage::Idle => {
                    self.busy = false;
                    self.indexing = false;
                    self.close_confirmation_open = false;
                }
            }
        }

        if !self.busy && !self.pending_fs_paths.is_empty() {
            let paths: Vec<PathBuf> = self.pending_fs_paths.drain().collect();
            self.start_incremental_update(paths);
        }
    }
''',
    "launch queued watcher changes",
)

# Ensure changed thumbnails are invalidated.
text = replace_once(
    text,
    '''    fn merge_indexed_batch(&mut self, records: Vec<ImageSummary>) {
        for record in records {
            if let Some(&index) = self.image_positions.get(&record.path) {
''',
    '''    fn merge_indexed_batch(&mut self, records: Vec<ImageSummary>) {
        for record in records {
            self.textures.remove(&record.path);
            if let Some(&index) = self.image_positions.get(&record.path) {
''',
    "invalidate changed thumbnails",
)

insert_before_ui = "    fn process_thumbnail_messages(&mut self, ctx: &egui::Context) {\n"
watcher_methods = r'''    fn remove_indexed_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let removed: HashSet<PathBuf> = paths.into_iter().collect();
        self.images.retain(|record| !removed.contains(&record.path));
        if let Some(results) = &mut self.similarity_results {
            results.retain(|record| !removed.contains(&record.path));
        }
        for path in &removed {
            self.textures.remove(path);
            self.selected_paths.remove(path);
        }
        self.rebuild_image_positions();
    }

    fn process_fs_watch_messages(&mut self) {
        while let Some(message) = self.fs_watch_service.try_recv() {
            match message {
                FsWatchMessage::PathsChanged(paths) => {
                    if self.busy {
                        self.pending_fs_paths.extend(paths);
                    } else {
                        self.start_incremental_update(paths);
                    }
                }
                FsWatchMessage::ReconcileRequired(reason) => {
                    self.watcher_reconcile_required = Some(reason.clone());
                    self.status = reason;
                }
                FsWatchMessage::Status(status) => self.status = status,
            }
        }
    }

    fn start_incremental_update(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if self.busy {
            self.pending_fs_paths.extend(paths);
            return;
        }
        self.busy = true;
        self.indexing = true;
        self.allow_close = false;
        self.close_confirmation_open = false;
        self.similarity_results = None;
        self.selected_paths.clear();
        self.status = format!(
            "Live filesystem update: {} changed path{} queued",
            paths.len(),
            if paths.len() == 1 { "" } else { "s" }
        );
        indexer::spawn_incremental_update(
            self.db_path.clone(),
            self.roots.clone(),
            paths,
            self.indexing_settings,
            self.embedding_service.clone(),
            self.tx.clone(),
        );
    }

'''
text = replace_once(text, insert_before_ui, watcher_methods + insert_before_ui, "UI watcher methods")

# Full rescan clears reconciliation warning.
text = replace_once(
    text,
    '''        self.busy = true;
        self.indexing = true;
        self.allow_close = false;
''',
    '''        self.busy = true;
        self.indexing = true;
        self.watcher_reconcile_required = None;
        self.allow_close = false;
''',
    "clear reconcile flag on full rescan",
)

# Update watched roots after add/remove; refresh text after remove.
text = replace_once(
    text,
    '''            Ok(()) => {
                self.roots = db::load_roots(&self.db_path).unwrap_or_default();
                self.status = format!("Added {}", folder.display());
            }
''',
    '''            Ok(()) => {
                self.roots = db::load_roots(&self.db_path).unwrap_or_default();
                self.fs_watch_service.set_roots(self.roots.clone());
                self.status = format!("Added {}", folder.display());
            }
''',
    "watch newly added root",
)
text = replace_once(
    text,
    '''                self.images = db::load_image_summaries(&self.db_path).unwrap_or_default();
                self.rebuild_image_positions();
                self.similarity_results = None;
                self.selected_paths.clear();
''',
    '''                self.images = db::load_image_summaries(&self.db_path).unwrap_or_default();
                self.rebuild_image_positions();
                self.fs_watch_service.set_roots(self.roots.clone());
                self.similarity_results = None;
                self.selected_paths.clear();
                self.refresh_text_search_after_data_change();
''',
    "unwatch removed root",
)

# Settings live-watcher section before performance settings.
settings_marker = '''                ui.add_space(12.0);
                ui.separator();
                ui.heading("Indexing performance");
'''
settings_live = '''                ui.add_space(12.0);
                ui.separator();
                ui.heading("Live indexing");
                ui.label(
                    "Filesystem watching is ON. Create, modify, rename and delete events are debounced and indexed without a full root rescan.",
                );
                ui.small("Manual Rescan remains the reconciliation fallback for missed watcher events.");
                if let Some(reason) = &self.watcher_reconcile_required {
                    ui.colored_label(egui::Color32::LIGHT_RED, reason);
                }

                ui.add_space(12.0);
                ui.separator();
                ui.heading("Indexing performance");
'''
text = replace_once(text, settings_marker, settings_live, "live indexing settings section")

# Drive watcher each frame before search/rendering.
text = replace_once(
    text,
    '''        self.process_worker_messages();
        self.process_thumbnail_messages(ctx);
        self.observe_text_search_input();
''',
    '''        self.process_worker_messages();
        self.process_fs_watch_messages();
        self.process_thumbnail_messages(ctx);
        self.observe_text_search_input();
''',
    "process watcher messages",
)
path.write_text(text, encoding="utf-8")

print("Live filesystem watcher patch applied")
