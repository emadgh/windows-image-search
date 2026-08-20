from pathlib import Path


def read(path: str) -> str:
    return Path(path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    Path(path).write_text(text, encoding="utf-8")


def replace_once(path: str, old: str, new: str) -> None:
    text = read(path)
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:180]!r}")
    write(path, text.replace(old, new, 1))


def insert_once(path: str, marker: str, addition: str, *, before: bool = True) -> None:
    text = read(path)
    if addition in text:
        return
    count = text.count(marker)
    if count != 1:
        raise SystemExit(f"{path}: expected one marker, found {count}: {marker[:180]!r}")
    replacement = addition + marker if before else marker + addition
    write(path, text.replace(marker, replacement, 1))


# ---------------------------------------------------------------------------
# Cargo / Windows subsystem
# ---------------------------------------------------------------------------
replace_once("Cargo.toml", 'version = "0.3.0-alpha.3"', 'version = "0.3.0-alpha.4"')
insert_once(
    "Cargo.toml",
    "\n[profile.release]\n",
    "\n[target.'cfg(windows)'.dependencies]\nwindows = { version = \"0.61\", features = [\"Win32_Foundation\", \"Win32_System_Com\", \"Win32_System_Console\", \"Win32_UI_Shell\", \"Win32_UI_WindowsAndMessaging\"] }\n",
)

main = read("src/main.rs")
if not main.startswith("#![cfg_attr(target_os = \"windows\", windows_subsystem = \"windows\")]\n"):
    main = '#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]\n\n' + main
if "mod windows_shell;\n" not in main:
    main = main.replace("mod ui;\n", "mod ui;\nmod windows_shell;\n", 1)
write("src/main.rs", main)

insert_once(
    "src/main.rs",
    "fn main() -> eframe::Result<()> {\n",
    '''#[cfg(target_os = "windows")]
fn attach_parent_console_for_cli() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(target_os = "windows"))]
fn attach_parent_console_for_cli() {}

''',
)
replace_once(
    "src/main.rs",
    '''fn main() -> eframe::Result<()> {
    let mode = startup_mode();
    if matches!(&mode, StartupMode::Version) {
''',
    '''fn main() -> eframe::Result<()> {
    let mode = startup_mode();
    if !matches!(&mode, StartupMode::Gui) {
        attach_parent_console_for_cli();
    }
    if matches!(&mode, StartupMode::Version) {
''',
)
replace_once(
    "src/main.rs",
    '''    let _ = db::open(&db_path);

    // GUI startup performs portable-root hydration in ImageSearchApp::new so it
    // can surface unavailable-drive warnings. CLI diagnostics have no UI layer,
    // so hydrate the rebuildable aggregate session here exactly once for them.
    if !matches!(&mode, StartupMode::Gui) {
        let registered_roots = db::load_roots(&db_path).unwrap_or_default();
        let _ = portable::prepare_registered_roots(&db_path, &registered_roots);
    }
''',
    '''    // Keep GUI launch lightweight: database open/migration, portable-root hydration,
    // and the initial image list are loaded by ImageSearchApp on a background thread.
    // CLI modes still prepare the database synchronously before running diagnostics.
    if !matches!(&mode, StartupMode::Gui) {
        let _ = db::open(&db_path);
        let registered_roots = db::load_roots(&db_path).unwrap_or_default();
        let _ = portable::prepare_registered_roots(&db_path, &registered_roots);
    }
''',
)

# ---------------------------------------------------------------------------
# Persistent discovery inventory + lightweight counts
# ---------------------------------------------------------------------------
replace_once(
    "src/db.rs",
    '''        CREATE INDEX IF NOT EXISTS idx_images_file_name ON images(file_name);

        CREATE TABLE IF NOT EXISTS collections (
''',
    '''        CREATE INDEX IF NOT EXISTS idx_images_file_name ON images(file_name);

        CREATE TABLE IF NOT EXISTS discovered_images (
            path TEXT PRIMARY KEY NOT NULL,
            root TEXT NOT NULL,
            last_seen_scan INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_discovered_images_root
            ON discovered_images(root);
        CREATE INDEX IF NOT EXISTS idx_discovered_images_root_scan
            ON discovered_images(root, last_seen_scan);

        CREATE TABLE IF NOT EXISTS collections (
''',
)
replace_once(
    "src/db.rs",
    '''    tx.execute("DELETE FROM roots WHERE path = ?1", params![root_text])?;
    tx.execute("DELETE FROM images WHERE root = ?1", params![root_text])?;
''',
    '''    tx.execute("DELETE FROM roots WHERE path = ?1", params![root_text])?;
    tx.execute("DELETE FROM images WHERE root = ?1", params![root_text])?;
    tx.execute("DELETE FROM discovered_images WHERE root = ?1", params![root_text])?;
''',
)
replace_once(
    "src/db.rs",
    '''pub struct FileState {
    pub size: u64,
    pub modified: i64,
    pub has_embedding: bool,
}
''',
    '''pub struct FileState {
    pub size: u64,
    pub modified: i64,
    pub width: u32,
    pub height: u32,
    pub content_fingerprint: Option<u64>,
    pub has_embedding: bool,
}
''',
)
replace_once(
    "src/db.rs",
    '''    let mut stmt =
        conn.prepare("SELECT path, size, modified, embedding IS NOT NULL FROM images")?;
''',
    '''    let mut stmt = conn.prepare(
        "SELECT path, size, modified, width, height, content_fingerprint, embedding IS NOT NULL FROM images",
    )?;
''',
)
replace_once(
    "src/db.rs",
    '''        let has_embedding = row.get::<_, bool>(3)?;
        Ok((
            path,
            FileState {
                size,
                modified,
                has_embedding,
            },
        ))
''',
    '''        let width = row.get::<_, i64>(3)?.max(0) as u32;
        let height = row.get::<_, i64>(4)?.max(0) as u32;
        let content_fingerprint = row.get::<_, Option<i64>>(5)?.map(|value| value as u64);
        let has_embedding = row.get::<_, bool>(6)?;
        Ok((
            path,
            FileState {
                size,
                modified,
                width,
                height,
                content_fingerprint,
                has_embedding,
            },
        ))
''',
)
replace_once(
    "src/db.rs",
    '''pub fn next_scan_generation(conn: &Connection) -> Result<i64> {
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
''',
    '''pub fn next_scan_generation(conn: &Connection) -> Result<i64> {
    let current: i64 = conn.query_row(
        "SELECT MAX(value) FROM (SELECT COALESCE(MAX(last_seen_scan), 0) AS value FROM images UNION ALL SELECT COALESCE(MAX(last_seen_scan), 0) AS value FROM discovered_images)",
        [],
        |row| row.get::<_, Option<i64>>(0).map(|value| value.unwrap_or(0)),
    )?;
    if current == i64::MAX {
        conn.execute("UPDATE images SET last_seen_scan = 0", [])?;
        conn.execute("UPDATE discovered_images SET last_seen_scan = 0", [])?;
        Ok(1)
    } else {
        Ok((current + 1).max(1))
    }
}
''',
)
insert_once(
    "src/db.rs",
    "pub fn load_image_summaries(db_path: &Path) -> Result<Vec<ImageSummary>> {\n",
    '''pub fn mark_discovered_paths_seen(
    conn: &mut Connection,
    generation: i64,
    candidates: &[(PathBuf, PathBuf)],
) -> Result<usize> {
    let tx = conn.transaction()?;
    let mut updated = 0usize;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO discovered_images(path, root, last_seen_scan) VALUES(?1, ?2, ?3) ON CONFLICT(path) DO UPDATE SET root = excluded.root, last_seen_scan = excluded.last_seen_scan",
        )?;
        for (root, path) in candidates {
            updated += stmt.execute(params![
                path.to_string_lossy().to_string(),
                root.to_string_lossy().to_string(),
                generation,
            ])?;
        }
    }
    tx.commit()?;
    Ok(updated)
}

pub fn delete_stale_discovered_for_root(
    conn: &Connection,
    root: &Path,
    generation: i64,
) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM discovered_images WHERE root = ?1 AND last_seen_scan <> ?2",
        params![root.to_string_lossy().to_string(), generation],
    )?)
}

pub fn load_discovered_paths(db_path: &Path) -> Result<Vec<PathBuf>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare("SELECT path FROM discovered_images ORDER BY path COLLATE NOCASE")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.filter_map(|row| row.ok()).map(PathBuf::from).collect())
}

pub fn load_root_counts(db_path: &Path) -> Result<HashMap<PathBuf, (usize, usize)>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT r.path,
               (SELECT COUNT(*) FROM discovered_images d WHERE d.root = r.path),
               (SELECT COUNT(*) FROM images i WHERE i.root = r.path)
        FROM roots r
        ORDER BY r.path COLLATE NOCASE
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            PathBuf::from(row.get::<_, String>(0)?),
            row.get::<_, i64>(1)?.max(0) as usize,
            row.get::<_, i64>(2)?.max(0) as usize,
        ))
    })?;
    let mut counts = HashMap::new();
    for row in rows {
        let (root, discovered, indexed) = row?;
        counts.insert(root, (discovered.max(indexed), indexed));
    }
    Ok(counts)
}

''',
)

# ---------------------------------------------------------------------------
# Thumbnail cache validation helper for force-rescan and CLIP inputs
# ---------------------------------------------------------------------------
insert_once(
    "src/thumbnail_cache.rs",
    "pub fn store_from_decoded(\n",
    '''pub fn valid_cache_path_for_root(root: &Path, source: &Path) -> Option<PathBuf> {
    let path = cache_path_for_root(root, source).ok()?;
    load_cached_path(path.clone()).map(|_| path)
}

''',
)

# ---------------------------------------------------------------------------
# Collection total/discovered counts
# ---------------------------------------------------------------------------
replace_once(
    "src/ui/collections.rs",
    '''    effective: HashMap<i64, HashSet<PathBuf>>,
    face_detection: HashMap<i64, bool>,
''',
    '''    effective: HashMap<i64, HashSet<PathBuf>>,
    discovered_counts: HashMap<i64, usize>,
    face_detection: HashMap<i64, bool>,
''',
)
replace_once(
    "src/ui/collections.rs",
    '''        self.rebuild_effective(images);
        Ok(())
    }

    pub(super) fn rebuild_effective(&mut self, images: &[ImageSummary]) {
''',
    '''        self.rebuild_effective(images);
        let discovered = db::load_discovered_paths(db_path)?;
        self.rebuild_discovered_counts(&discovered);
        Ok(())
    }

    fn rebuild_discovered_counts(&mut self, discovered: &[PathBuf]) {
        self.discovered_counts.clear();
        for item in &self.items {
            let Some(membership) = self.memberships.get(&item.id) else {
                self.discovered_counts.insert(item.id, 0);
                continue;
            };
            let manual: HashSet<&Path> = membership.files.iter().map(PathBuf::as_path).collect();
            let count = discovered
                .iter()
                .filter(|path| {
                    manual.contains(path.as_path())
                        || membership.folders.iter().any(|folder| path.starts_with(folder))
                })
                .count();
            self.discovered_counts.insert(item.id, count);
        }
    }

    pub(super) fn refresh_discovered_counts(&mut self, db_path: &Path) -> anyhow::Result<()> {
        let discovered = db::load_discovered_paths(db_path)?;
        self.rebuild_discovered_counts(&discovered);
        Ok(())
    }

    pub(super) fn rebuild_effective(&mut self, images: &[ImageSummary]) {
''',
)
replace_once(
    "src/ui/collections.rs",
    '''    fn count(&self, id: i64) -> usize {
        self.effective.get(&id).map_or(0, HashSet::len)
    }
''',
    '''    fn count(&self, id: i64) -> usize {
        self.effective.get(&id).map_or(0, HashSet::len)
    }

    fn total_count(&self, id: i64) -> usize {
        self.discovered_counts
            .get(&id)
            .copied()
            .unwrap_or(0)
            .max(self.count(id))
    }
''',
)
replace_once(
    "src/ui/collections.rs",
    '''                                format!("{}  ·  {count}", item.name),
''',
    '''                                format!(
                                    "{}  ·  {count}/{} indexed",
                                    item.name,
                                    self.collections.total_count(item.id)
                                ),
''',
)
replace_once(
    "src/ui/collections.rs",
    '''                ui.small(format!(
                    "{} effective indexed image{}",
                    self.collections.count(id),
                    if self.collections.count(id) == 1 { "" } else { "s" }
                ));
''',
    '''                ui.small(format!(
                    "{} indexed / {} discovered image{}",
                    self.collections.count(id),
                    self.collections.total_count(id),
                    if self.collections.total_count(id) == 1 { "" } else { "s" }
                ));
''',
)

# ---------------------------------------------------------------------------
# Index worker lifecycle / pause / force-rescan
# ---------------------------------------------------------------------------
replace_once(
    "src/indexer.rs",
    '''use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
''',
    '''use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
''',
)
insert_once(
    "src/indexer.rs",
    "#[derive(Clone)]\nstruct PendingImage {\n",
    '''#[derive(Clone, Default)]
pub struct IndexControl {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

impl IndexControl {
    pub fn pause(&self) {
        let (lock, _) = &*self.inner;
        *lock.lock().unwrap_or_else(|err| err.into_inner()) = true;
    }

    pub fn resume(&self) {
        let (lock, condvar) = &*self.inner;
        *lock.lock().unwrap_or_else(|err| err.into_inner()) = false;
        condvar.notify_all();
    }

    pub fn is_paused(&self) -> bool {
        let (lock, _) = &*self.inner;
        *lock.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn wait_if_paused(&self) {
        let (lock, condvar) = &*self.inner;
        let mut paused = lock.lock().unwrap_or_else(|err| err.into_inner());
        while *paused {
            paused = condvar.wait(paused).unwrap_or_else(|err| err.into_inner());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RescanMode {
    ChangedOnly,
    ForcePreferThumbnail,
}

''',
)
replace_once(
    "src/indexer.rs",
    '''struct PendingImage {
    root: PathBuf,
    path: PathBuf,
    size: u64,
    modified: i64,
}
''',
    '''struct PendingImage {
    root: PathBuf,
    path: PathBuf,
    size: u64,
    modified: i64,
    previous_width: u32,
    previous_height: u32,
    previous_fingerprint: Option<u64>,
    prefer_thumbnail: bool,
}
''',
)
replace_once(
    "src/indexer.rs",
    '''    Status(String),
    Progress { done: usize, total: usize },
    Reload,
''',
    '''    Status(String),
    CurrentFile(String),
    Progress { done: usize, total: usize },
    Reload,
    ReplaceImages(Vec<ImageSummary>),
''',
)

# Replace spawn_rescan with mode-aware shared worker.
old_spawn = '''pub fn spawn_rescan(
    db_path: PathBuf,
    roots: Vec<PathBuf>,
    indexing_settings: IndexingSettings,
    embedding_service: EmbeddingService,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let result = rescan(&db_path, &roots, indexing_settings, &embedding_service, &tx);
        if let Err(err) = result {
            let _ = tx.send(WorkerMessage::Error(format!("Indexing failed: {err:#}")));
        }
        let _ = tx.send(WorkerMessage::Reload);
        let _ = tx.send(WorkerMessage::Idle);
    });
}
'''
new_spawn = '''pub fn spawn_rescan(
    db_path: PathBuf,
    roots: Vec<PathBuf>,
    indexing_settings: IndexingSettings,
    embedding_service: EmbeddingService,
    control: IndexControl,
    tx: Sender<WorkerMessage>,
) {
    spawn_rescan_with_mode(
        db_path,
        roots,
        indexing_settings,
        embedding_service,
        control,
        RescanMode::ChangedOnly,
        tx,
    );
}

pub fn spawn_force_rescan(
    db_path: PathBuf,
    roots: Vec<PathBuf>,
    indexing_settings: IndexingSettings,
    embedding_service: EmbeddingService,
    control: IndexControl,
    tx: Sender<WorkerMessage>,
) {
    spawn_rescan_with_mode(
        db_path,
        roots,
        indexing_settings,
        embedding_service,
        control,
        RescanMode::ForcePreferThumbnail,
        tx,
    );
}

fn spawn_rescan_with_mode(
    db_path: PathBuf,
    roots: Vec<PathBuf>,
    indexing_settings: IndexingSettings,
    embedding_service: EmbeddingService,
    control: IndexControl,
    mode: RescanMode,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let result = rescan(
            &db_path,
            &roots,
            indexing_settings,
            &embedding_service,
            mode,
            &control,
            &tx,
        );
        if let Err(err) = result {
            let _ = tx.send(WorkerMessage::Error(format!("Indexing failed: {err:#}")));
        }
        match db::load_image_summaries(&db_path) {
            Ok(images) => {
                let _ = tx.send(WorkerMessage::ReplaceImages(images));
            }
            Err(err) => {
                let _ = tx.send(WorkerMessage::Error(format!("Final index reload failed: {err:#}")));
            }
        }
        let _ = tx.send(WorkerMessage::Idle);
    });
}
'''
replace_once("src/indexer.rs", old_spawn, new_spawn)

replace_once(
    "src/indexer.rs",
    '''    embedding_service: EmbeddingService,
    tx: Sender<WorkerMessage>,
) {
''',
    '''    embedding_service: EmbeddingService,
    control: IndexControl,
    tx: Sender<WorkerMessage>,
) {
''',
)
replace_once(
    "src/indexer.rs",
    '''            indexing_settings,
            &embedding_service,
            &tx,
        );
''',
    '''            indexing_settings,
            &embedding_service,
            &control,
            &tx,
        );
''',
)
replace_once(
    "src/indexer.rs",
    '''    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
''',
    '''    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    control: &IndexControl,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
''',
)
# Incremental pending fields.
replace_once(
    "src/indexer.rs",
    '''        pending.push(PendingImage {
            root,
            path,
            size,
            modified,
        });
''',
    '''        pending.push(PendingImage {
            root,
            path,
            size,
            modified,
            previous_width: 0,
            previous_height: 0,
            previous_fingerprint: None,
            prefer_thumbnail: false,
        });
''',
)
# Pause at incremental batch and emit filenames.
replace_once(
    "src/indexer.rs",
    '''    for batch in pending.chunks(batch_size) {
        let prepared: Vec<PreparedImage> = pool.install(|| {
''',
    '''    for batch in pending.chunks(batch_size) {
        control.wait_if_paused();
        let prepared: Vec<PreparedImage> = pool.install(|| {
''',
)
replace_once(
    "src/indexer.rs",
    '''                .filter_map(|item| {
                    let result = inspect_image(&item.path, &item.root).map(
''',
    '''                .filter_map(|item| {
                    control.wait_if_paused();
                    let _ = tx.send(WorkerMessage::CurrentFile(
                        item.path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned(),
                    ));
                    let result = inspect_pending_image(item).map(
''',
)
# First build_embeddings call (incremental).
replace_once(
    "src/indexer.rs",
    '''            indexing_settings,
            embedding_service,
            tx,
        ) {
''',
    '''            indexing_settings,
            embedding_service,
            roots,
            false,
            control,
            tx,
        ) {
''',
)

# Rescan signature and mode.
replace_once(
    "src/indexer.rs",
    '''fn rescan(
    db_path: &Path,
    roots: &[PathBuf],
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
''',
    '''fn rescan(
    db_path: &Path,
    roots: &[PathBuf],
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    mode: RescanMode,
    control: &IndexControl,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
''',
)
replace_once(
    "src/indexer.rs",
    '''    let existing_file_states = db::load_file_states(&conn)?;
    let mut candidates: Vec<(PathBuf, PathBuf)> = Vec::new();
''',
    '''    let existing_file_states = db::load_file_states(&conn)?;
    let force_rescan = mode == RescanMode::ForcePreferThumbnail;
    let mut candidates: Vec<(PathBuf, PathBuf)> = Vec::new();
''',
)
# Add discovery inventory immediately after traversal.
replace_once(
    "src/indexer.rs",
    '''    let total = candidates.len();
    let mut pending = Vec::<PendingImage>::new();
''',
    '''    let total = candidates.len();
    let scan_generation = db::next_scan_generation(&conn)?;
    let discovered_marked = db::mark_discovered_paths_seen(&mut conn, scan_generation, &candidates)?;
    for root in &prunable_roots {
        let _ = db::delete_stale_discovered_for_root(&conn, root, scan_generation)?;
    }
    let _ = tx.send(WorkerMessage::Status(format!(
        "Discovered {discovered_marked}/{total} image paths; checking index state…"
    )));
    let mut pending = Vec::<PendingImage>::new();
''',
)
replace_once(
    "src/indexer.rs",
    '''        let unchanged = existing_file_states
            .get(path)
            .is_some_and(|state| state.size == size && state.modified == modified);

        if !unchanged {
            pending.push(PendingImage {
                root: root.clone(),
                path: path.clone(),
                size,
                modified,
            });
        }
''',
    '''        let previous = existing_file_states.get(path);
        let unchanged = previous.is_some_and(|state| state.size == size && state.modified == modified);

        if force_rescan || !unchanged {
            pending.push(PendingImage {
                root: root.clone(),
                path: path.clone(),
                size,
                modified,
                previous_width: previous.map_or(0, |state| state.width),
                previous_height: previous.map_or(0, |state| state.height),
                previous_fingerprint: previous.and_then(|state| state.content_fingerprint),
                prefer_thumbnail: force_rescan && unchanged,
            });
        }
''',
)
replace_once(
    "src/indexer.rs",
    '''    let _ = tx.send(WorkerMessage::Status(format!(
        "Preparing {changed_total} changed image{} with {workers} decode worker{}; committing every {batch_size} images…",
''',
    '''    let _ = tx.send(WorkerMessage::Status(format!(
        "Preparing {changed_total} image{} with {workers} decode worker{}; committing every {batch_size} images…",
''',
)
# Rescan batch occurrence after first incremental one already replaced: use unique prepared_count snippet.
replace_once(
    "src/indexer.rs",
    '''    for batch in pending.chunks(batch_size) {
        let prepared: Vec<PreparedImage> = pool.install(|| {
            batch
                .par_iter()
                .filter_map(|item| {
                    let result = inspect_image(&item.path, &item.root).map(
''',
    '''    for batch in pending.chunks(batch_size) {
        control.wait_if_paused();
        let prepared: Vec<PreparedImage> = pool.install(|| {
            batch
                .par_iter()
                .filter_map(|item| {
                    control.wait_if_paused();
                    let _ = tx.send(WorkerMessage::CurrentFile(
                        item.path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned(),
                    ));
                    let result = inspect_pending_image(item).map(
''',
)
# Progress instead of verbose duplicate decoding status.
replace_once(
    "src/indexer.rs",
    '''                    if done % 8 == 0 || done == changed_total {
                        let _ = tx.send(WorkerMessage::Status(format!(
                            "Decoding/reading metadata: {done}/{changed_total}; committed {changed}"
                        )));
                    }
''',
    '''                    if done % 8 == 0 || done == changed_total {
                        let _ = tx.send(WorkerMessage::Progress {
                            done,
                            total: changed_total,
                        });
                    }
''',
)
# Reuse scan_generation rather than creating a second generation.
replace_once(
    "src/indexer.rs",
    '''    let scan_generation = db::next_scan_generation(&conn)?;
    let marked = db::mark_paths_seen(
''',
    '''    let marked = db::mark_paths_seen(
''',
)
# Visual descriptor backfill gets pause control.
replace_once(
    "src/indexer.rs",
    '''        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, tx)?;
''',
    '''        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, control, tx)?;
''',
)
# Rescan embedding call.
replace_once(
    "src/indexer.rs",
    '''            indexing_settings,
            embedding_service,
            tx,
        ) {
            let _ = tx.send(WorkerMessage::Error(format!(
                "Texture/color index is ready, but CLIP indexing is unavailable: {err:#}"
''',
    '''            indexing_settings,
            embedding_service,
            roots,
            force_rescan,
            control,
            tx,
        ) {
            let _ = tx.send(WorkerMessage::Error(format!(
                "Texture/color index is ready, but CLIP indexing is unavailable: {err:#}"
''',
)
# build_visual_descriptors signature/pause.
replace_once(
    "src/indexer.rs",
    '''fn build_visual_descriptors(
    conn: &mut rusqlite::Connection,
    paths: &[PathBuf],
    indexing_settings: IndexingSettings,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
''',
    '''fn build_visual_descriptors(
    conn: &mut rusqlite::Connection,
    paths: &[PathBuf],
    indexing_settings: IndexingSettings,
    control: &IndexControl,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
''',
)
replace_once(
    "src/indexer.rs",
    '''    for batch in paths.chunks(batch_size) {
        let committed_before_batch = committed;
''',
    '''    for batch in paths.chunks(batch_size) {
        control.wait_if_paused();
        let committed_before_batch = committed;
''',
)
replace_once(
    "src/indexer.rs",
    '''                .filter_map(|path| {
                    let result = decode_image(path).map(|image| {
''',
    '''                .filter_map(|path| {
                    control.wait_if_paused();
                    let _ = tx.send(WorkerMessage::CurrentFile(
                        path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned(),
                    ));
                    let result = decode_image(path).map(|image| {
''',
)
# build_embeddings signature/input mapping.
replace_once(
    "src/indexer.rs",
    '''fn build_embeddings(
    conn: &mut rusqlite::Connection,
    paths: &[PathBuf],
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
''',
    '''fn build_embeddings(
    conn: &mut rusqlite::Connection,
    paths: &[PathBuf],
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    roots: &[PathBuf],
    prefer_thumbnails: bool,
    control: &IndexControl,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
''',
)
replace_once(
    "src/indexer.rs",
    '''    for (batch_index, batch) in paths.chunks(batch_size).enumerate() {
        let response = embedding_service
            .embed_with_provider(
                batch.to_vec(),
''',
    '''    for (batch_index, batch) in paths.chunks(batch_size).enumerate() {
        control.wait_if_paused();
        let input_paths: Vec<PathBuf> = batch
            .iter()
            .map(|path| {
                if prefer_thumbnails {
                    indexed_root_for_path(path, roots)
                        .and_then(|root| thumbnail_cache::valid_cache_path_for_root(root, path))
                        .unwrap_or_else(|| path.clone())
                } else {
                    path.clone()
                }
            })
            .collect();
        if let Some(path) = batch.first() {
            let _ = tx.send(WorkerMessage::CurrentFile(
                path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned(),
            ));
        }
        let response = embedding_service
            .embed_with_provider(
                input_paths,
''',
)
replace_once(
    "src/indexer.rs",
    '''        let done = ((batch_index + 1) * batch_size).min(total);
        let _ = tx.send(WorkerMessage::Status(format!(
            "Building CLIP index: {done}/{total} (committed; persistent model)"
        )));
''',
    '''        let done = ((batch_index + 1) * batch_size).min(total);
        let _ = tx.send(WorkerMessage::Progress { done, total });
        let _ = tx.send(WorkerMessage::Status(if prefer_thumbnails {
            format!("Building CLIP index from cached thumbnails: {done}/{total}")
        } else {
            format!("Building CLIP index: {done}/{total}")
        }));
''',
)
# Add force-thumbnail inspection helper before inspect_image.
insert_once(
    "src/indexer.rs",
    "fn inspect_image(\n",
    '''fn inspect_pending_image(
    item: &PendingImage,
) -> Result<(u32, u32, [u8; 3], u64, Vec<f32>, Vec<f32>, u64)> {
    if item.prefer_thumbnail {
        if let (Some(image), Some(fingerprint)) = (
            thumbnail_cache::load_cached_for_root(&item.root, &item.path),
            item.previous_fingerprint,
        ) {
            let (dominant, visual_hash, color_histogram, material_texture) = visual_descriptor(&image);
            let width = if item.previous_width > 0 { item.previous_width } else { image.width() };
            let height = if item.previous_height > 0 { item.previous_height } else { image.height() };
            return Ok((
                width,
                height,
                dominant,
                visual_hash,
                color_histogram,
                material_texture,
                fingerprint,
            ));
        }
    }
    inspect_image(&item.path, &item.root)
}

''',
)

# Indexer tests for pause and thumbnail force path.
insert_once(
    "src/indexer.rs",
    "    #[test]\n",
    '''    #[test]
    fn index_control_pause_resume_is_cooperative() {
        let control = IndexControl::default();
        control.pause();
        assert!(control.is_paused());
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_control = control.clone();
        std::thread::spawn(move || {
            worker_control.wait_if_paused();
            let _ = tx.send(());
        });
        assert!(rx.recv_timeout(std::time::Duration::from_millis(80)).is_err());
        control.resume();
        rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
        assert!(!control.is_paused());
    }

    #[test]
    fn force_inspection_reuses_valid_thumbnail_and_preserves_source_identity() {
        use image::{ImageBuffer, Rgb};
        let root = std::env::temp_dir().join(format!(
            "wis-force-thumb-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(root.join("tiles")).unwrap();
        let source = root.join("tiles").join("large.png");
        let original = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(900, 700, Rgb([20, 40, 60])));
        original.save(&source).unwrap();
        thumbnail_cache::store_from_decoded_for_root(&root, &source, &original).unwrap();
        let pending = PendingImage {
            root: root.clone(),
            path: source,
            size: 1,
            modified: 1,
            previous_width: 900,
            previous_height: 700,
            previous_fingerprint: Some(123456789),
            prefer_thumbnail: true,
        };
        let (width, height, _, _, _, _, fingerprint) = inspect_pending_image(&pending).unwrap();
        assert_eq!((width, height), (900, 700));
        assert_eq!(fingerprint, 123456789);
        let _ = std::fs::remove_dir_all(root);
    }

''',
)

# ---------------------------------------------------------------------------
# UI state, async startup, settings bounds, progress/Pause
# ---------------------------------------------------------------------------
replace_once(
    "src/ui/mod.rs",
    '''use std::sync::mpsc::{Receiver, Sender};
''',
    '''use std::sync::mpsc::{Receiver, Sender};
''',
)
insert_once(
    "src/ui/mod.rs",
    "pub struct ImageSearchApp {\n",
    '''enum StartupMessage {
    Stage { status: String, done: usize, total: usize },
    Ready {
        roots: Vec<PathBuf>,
        images: Vec<ImageSummary>,
        collections: collections::CollectionsState,
        root_counts: HashMap<PathBuf, (usize, usize)>,
        warnings: Vec<String>,
    },
    Error(String),
}

''',
)
replace_once(
    "src/ui/mod.rs",
    '''    pub(super) tx: Sender<WorkerMessage>,
    pub(super) rx: Receiver<WorkerMessage>,
    pub(super) busy: bool,
''',
    '''    pub(super) tx: Sender<WorkerMessage>,
    pub(super) rx: Receiver<WorkerMessage>,
    startup_rx: Receiver<StartupMessage>,
    index_control: Option<indexer::IndexControl>,
    index_paused: bool,
    current_file: Option<String>,
    root_counts: HashMap<PathBuf, (usize, usize)>,
    pub(super) busy: bool,
''',
)

# Replace synchronous constructor body wholesale using exact section from current file.
start = read("src/ui/mod.rs")
old_begin = '''    pub fn new(db_path: PathBuf, model_cache: PathBuf) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let app_data_dir = db_path.parent().unwrap_or_else(|| Path::new("."));
        let thumbnail_cache = thumbnail_cache::cache_dir_for_db(&db_path);
        let settings_path = app_data_dir.join("performance-settings.ini");
        let indexing_settings = settings::load(&settings_path);
        let embedding_service = EmbeddingService::new(model_cache);
        let text_search_service = TextSearchService::new(db_path.clone());
        let roots = db::load_roots(&db_path).unwrap_or_default();
        let portable_warnings = portable::prepare_registered_roots(&db_path, &roots);
        let fs_watch_service = FsWatchService::new(roots.clone());
        let images = db::load_image_summaries(&db_path).unwrap_or_default();
        let image_positions = images
            .iter()
            .enumerate()
            .map(|(index, record)| (record.path.clone(), index))
            .collect();
        let collections =
            collections::CollectionsState::load(&db_path, &images).unwrap_or_default();
        let thumb_pool = ThumbnailPool::new(thumbnail_cache, roots.clone());
        let initial_status = portable_warnings
            .first()
            .cloned()
            .unwrap_or_else(|| "Ready".to_owned());
        Self {
            roots,
            images,
            image_positions,
            db_path,
            embedding_service,
            fs_watch_service,
            pending_fs_paths: HashSet::new(),
            watcher_reconcile_required: None,
            similarity_results: None,
            query_image: None,
            similarity_settings: indexer::SimilaritySettings::default(),
            indexing_settings,
            settings_path,
            collections,
            search_text: String::new(),
            text_search_service,
            text_search_matches: None,
            text_search_observed: String::new(),
            text_search_due: None,
            text_search_generation: 0,
            text_search_pending: false,
            color_enabled: false,
            target_color: [128, 128, 128],
            color_tolerance: 0.22,
            view_mode: ViewMode::Grid,
            thumb_size: 168.0,
            thumb_fit: ThumbnailFit::Contain,
            textures: HashMap::new(),
            texture_lru: TextureLru::new(DEFAULT_GPU_TEXTURE_CAPACITY),
            selected_paths: HashSet::new(),
            thumb_pool,
            tx,
            rx,
            busy: false,
            indexing: false,
            status: initial_status,
            progress: None,
            last_error: None,
            settings_open: false,
            close_confirmation_open: false,
            allow_close: false,
        }
    }
'''
new_begin = '''    pub fn new(db_path: PathBuf, model_cache: PathBuf) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let (startup_tx, startup_rx) = std::sync::mpsc::channel();
        let app_data_dir = db_path.parent().unwrap_or_else(|| Path::new("."));
        let thumbnail_cache = thumbnail_cache::cache_dir_for_db(&db_path);
        let settings_path = app_data_dir.join("performance-settings.ini");
        let indexing_settings = settings::load(&settings_path);
        let embedding_service = EmbeddingService::new(model_cache);
        let text_search_service = TextSearchService::new(db_path.clone());
        let fs_watch_service = FsWatchService::new(Vec::new());
        let thumb_pool = ThumbnailPool::new(thumbnail_cache, Vec::new());

        let startup_db = db_path.clone();
        std::thread::spawn(move || {
            let send_stage = |status: &str, done: usize| {
                let _ = startup_tx.send(StartupMessage::Stage {
                    status: status.to_owned(),
                    done,
                    total: 4,
                });
            };
            let result = (|| -> anyhow::Result<_> {
                send_stage("Opening index database…", 1);
                let _ = db::open(&startup_db)?;
                send_stage("Loading indexed roots…", 2);
                let roots = db::load_roots(&startup_db)?;
                send_stage("Attaching portable indexes…", 3);
                let warnings = portable::prepare_registered_roots(&startup_db, &roots);
                send_stage("Loading indexed image catalog…", 4);
                let images = db::load_image_summaries(&startup_db)?;
                let collections = collections::CollectionsState::load(&startup_db, &images)?;
                let root_counts = db::load_root_counts(&startup_db)?;
                Ok((roots, images, collections, root_counts, warnings))
            })();
            match result {
                Ok((roots, images, collections, root_counts, warnings)) => {
                    let _ = startup_tx.send(StartupMessage::Ready {
                        roots,
                        images,
                        collections,
                        root_counts,
                        warnings,
                    });
                }
                Err(err) => {
                    let _ = startup_tx.send(StartupMessage::Error(format!(
                        "Startup index load failed: {err:#}"
                    )));
                }
            }
        });

        Self {
            roots: Vec::new(),
            images: Vec::new(),
            image_positions: HashMap::new(),
            db_path,
            embedding_service,
            fs_watch_service,
            pending_fs_paths: HashSet::new(),
            watcher_reconcile_required: None,
            similarity_results: None,
            query_image: None,
            similarity_settings: indexer::SimilaritySettings::default(),
            indexing_settings,
            settings_path,
            collections: collections::CollectionsState::default(),
            search_text: String::new(),
            text_search_service,
            text_search_matches: None,
            text_search_observed: String::new(),
            text_search_due: None,
            text_search_generation: 0,
            text_search_pending: false,
            color_enabled: false,
            target_color: [128, 128, 128],
            color_tolerance: 0.22,
            view_mode: ViewMode::Grid,
            thumb_size: 168.0,
            thumb_fit: ThumbnailFit::Contain,
            textures: HashMap::new(),
            texture_lru: TextureLru::new(DEFAULT_GPU_TEXTURE_CAPACITY),
            selected_paths: HashSet::new(),
            thumb_pool,
            tx,
            rx,
            startup_rx,
            index_control: None,
            index_paused: false,
            current_file: None,
            root_counts: HashMap::new(),
            busy: true,
            indexing: false,
            status: "Starting application…".to_owned(),
            progress: Some((0, 4)),
            last_error: None,
            settings_open: false,
            close_confirmation_open: false,
            allow_close: false,
        }
    }
'''
if new_begin not in start:
    if old_begin not in start:
        raise SystemExit("src/ui/mod.rs: constructor block not found")
    write("src/ui/mod.rs", start.replace(old_begin, new_begin, 1))

insert_once(
    "src/ui/mod.rs",
    "    fn process_worker_messages(&mut self) {\n",
    '''    fn process_startup_messages(&mut self) {
        while let Ok(message) = self.startup_rx.try_recv() {
            match message {
                StartupMessage::Stage { status, done, total } => {
                    self.status = status;
                    self.progress = Some((done, total));
                }
                StartupMessage::Ready {
                    roots,
                    images,
                    collections,
                    root_counts,
                    warnings,
                } => {
                    self.roots = roots;
                    self.images = images;
                    self.rebuild_image_positions();
                    self.collections = collections;
                    self.root_counts = root_counts;
                    self.thumb_pool.set_roots(self.roots.clone());
                    self.fs_watch_service.set_roots(self.roots.clone());
                    self.busy = false;
                    self.progress = None;
                    self.status = warnings.first().cloned().unwrap_or_else(|| "Ready".to_owned());
                }
                StartupMessage::Error(error) => {
                    self.busy = false;
                    self.progress = None;
                    self.status = "Startup load failed".to_owned();
                    self.last_error = Some(error);
                }
            }
        }
    }

''',
)
replace_once(
    "src/ui/mod.rs",
    '''                WorkerMessage::Status(status) => self.status = status,
                WorkerMessage::Progress { done, total } => self.progress = Some((done, total)),
                WorkerMessage::IndexedBatch(records) => {
''',
    '''                WorkerMessage::Status(status) => self.status = status,
                WorkerMessage::CurrentFile(file_name) => self.current_file = Some(file_name),
                WorkerMessage::Progress { done, total } => self.progress = Some((done, total)),
                WorkerMessage::IndexedBatch(records) => {
''',
)
replace_once(
    "src/ui/mod.rs",
    '''                WorkerMessage::Reload => {
                    match db::load_image_summaries(&self.db_path) {
                        Ok(images) => {
                            self.images = images;
                            self.rebuild_image_positions();
                        }
                        Err(err) => self.last_error = Some(format!("Reload failed: {err:#}")),
                    }
                    self.progress = None;
                    self.refresh_collection_effective_membership();
                    self.refresh_text_search_after_data_change();
                }
''',
    '''                WorkerMessage::Reload => {
                    // Legacy message retained for compatibility; avoid blocking the UI on a
                    // synchronous full catalog reload. New workers send ReplaceImages instead.
                    self.progress = None;
                }
                WorkerMessage::ReplaceImages(images) => {
                    self.images = images;
                    self.rebuild_image_positions();
                    self.refresh_collection_effective_membership();
                    let _ = self.collections.refresh_discovered_counts(&self.db_path);
                    self.root_counts = db::load_root_counts(&self.db_path).unwrap_or_default();
                    self.refresh_text_search_after_data_change();
                }
''',
)
replace_once(
    "src/ui/mod.rs",
    '''                WorkerMessage::Idle => {
                    self.busy = false;
                    self.indexing = false;
                    self.close_confirmation_open = false;
                }
''',
    '''                WorkerMessage::Idle => {
                    self.busy = false;
                    self.indexing = false;
                    self.index_paused = false;
                    self.index_control = None;
                    self.current_file = None;
                    self.progress = None;
                    self.close_confirmation_open = false;
                }
''',
)
# start incremental control.
replace_once(
    "src/ui/mod.rs",
    '''        self.status = format!(
            "Live filesystem update: {} changed path{} queued",
''',
    '''        let control = indexer::IndexControl::default();
        self.index_control = Some(control.clone());
        self.index_paused = false;
        self.status = format!(
            "Live filesystem update: {} changed path{} queued",
''',
)
replace_once(
    "src/ui/mod.rs",
    '''            self.indexing_settings,
            self.embedding_service.clone(),
            self.tx.clone(),
        );
''',
    '''            self.indexing_settings,
            self.embedding_service.clone(),
            control,
            self.tx.clone(),
        );
''',
)
# start_rescan control.
replace_once(
    "src/ui/mod.rs",
    '''        self.status = "Starting recursive rescan…".into();
        indexer::spawn_rescan(
''',
    '''        let control = indexer::IndexControl::default();
        self.index_control = Some(control.clone());
        self.index_paused = false;
        self.status = "Starting recursive rescan…".into();
        indexer::spawn_rescan(
''',
)
replace_once(
    "src/ui/mod.rs",
    '''            self.indexing_settings,
            self.embedding_service.clone(),
            self.tx.clone(),
        );
    }

    fn add_folder(&mut self) {
''',
    '''            self.indexing_settings,
            self.embedding_service.clone(),
            control,
            self.tx.clone(),
        );
    }

    fn start_force_rescan(&mut self) {
        if self.busy || self.roots.is_empty() {
            return;
        }
        self.busy = true;
        self.indexing = true;
        self.allow_close = false;
        self.close_confirmation_open = false;
        self.progress = None;
        self.last_error = None;
        self.similarity_results = None;
        self.selected_paths.clear();
        let control = indexer::IndexControl::default();
        self.index_control = Some(control.clone());
        self.index_paused = false;
        self.status = "Force rescanning all images; valid thumbnails will be preferred…".into();
        indexer::spawn_force_rescan(
            self.db_path.clone(),
            self.roots.clone(),
            self.indexing_settings,
            self.embedding_service.clone(),
            control,
            self.tx.clone(),
        );
    }

    fn toggle_index_pause(&mut self) {
        let Some(control) = self.index_control.clone() else {
            return;
        };
        if control.is_paused() {
            control.resume();
            self.index_paused = false;
            self.status = "Indexing resumed".to_owned();
        } else {
            control.pause();
            self.index_paused = true;
            self.status = "Indexing paused".to_owned();
        }
    }

    fn add_folder(&mut self) {
''',
)
# Root count refresh after attach/remove.
replace_once(
    "src/ui/mod.rs",
    '''                self.roots = db::load_roots(&self.db_path).unwrap_or_default();
                self.thumb_pool.set_roots(self.roots.clone());
''',
    '''                self.roots = db::load_roots(&self.db_path).unwrap_or_default();
                self.root_counts = db::load_root_counts(&self.db_path).unwrap_or_default();
                self.thumb_pool.set_roots(self.roots.clone());
''',
)
# The same root reload occurs in remove_folder; replace if still present once.
text = read("src/ui/mod.rs")
needle = '''                self.roots = db::load_roots(&self.db_path).unwrap_or_default();
                self.thumb_pool.set_roots(self.roots.clone());
'''
if needle in text:
    text = text.replace(
        needle,
        '''                self.roots = db::load_roots(&self.db_path).unwrap_or_default();
                self.root_counts = db::load_root_counts(&self.db_path).unwrap_or_default();
                self.thumb_pool.set_roots(self.roots.clone());
''',
        1,
    )
    write("src/ui/mod.rs", text)
# Settings bounded scroll + force button.
replace_once(
    "src/ui/mod.rs",
    '''            .resizable(true)
            .default_width(920.0)
            .show(ctx, |ui| {
                ui.heading("Indexed folders");
''',
    '''            .resizable(true)
            .default_width(920.0)
            .default_height(700.0)
            .max_height((ctx.available_rect().height() - 48.0).max(320.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                ui.heading("Indexed folders");
''',
)
replace_once(
    "src/ui/mod.rs",
    '''                    if ui
                        .add_enabled(
                            !self.busy && !self.roots.is_empty(),
                            egui::Button::new("⟳ Rescan all folders"),
                        )
                        .clicked()
                    {
                        self.start_rescan();
                    }
''',
    '''                    if ui
                        .add_enabled(
                            !self.busy && !self.roots.is_empty(),
                            egui::Button::new("⟳ Rescan changed"),
                        )
                        .clicked()
                    {
                        self.start_rescan();
                    }
                    if ui
                        .add_enabled(
                            !self.busy && !self.roots.is_empty(),
                            egui::Button::new("⟳ Force rescan all"),
                        )
                        .on_hover_text("Rebuild all visual/CLIP descriptors; valid cached thumbnails are used instead of large originals when safe.")
                        .clicked()
                    {
                        self.start_force_rescan();
                    }
''',
)
replace_once(
    "src/ui/mod.rs",
    '''                        ui.horizontal(|ui| {
                            ui.label(root.display().to_string());
                            if portable::is_indexed_root(root) {
''',
    '''                        ui.horizontal(|ui| {
                            ui.label(root.display().to_string());
                            let (discovered, indexed) = self
                                .root_counts
                                .get(root)
                                .copied()
                                .unwrap_or((0, self.images.iter().filter(|image| &image.root == root).count()));
                            ui.small(format!("{indexed}/{discovered} indexed"));
                            if portable::is_indexed_root(root) {
''',
)
# Close ScrollArea before window closure at unique thumbnail-cache tail.
replace_once(
    "src/ui/mod.rs",
    '''                if ui.button("Clear thumbnail cache").clicked() {
                    clear_cache = true;
                }
            });

        self.settings_open = open;
''',
    '''                if ui.button("Clear thumbnail cache").clicked() {
                    clear_cache = true;
                }
                    });
            });

        self.settings_open = open;
''',
)
# Startup processing at start of update.
replace_once(
    "src/ui/mod.rs",
    '''    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_worker_messages();
''',
    '''    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_startup_messages();
        self.process_worker_messages();
''',
)
# Footer right-aligned progress + pause, filename only.
replace_once(
    "src/ui/mod.rs",
    '''        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.busy {
                    ui.spinner();
                }
                ui.label(&self.status);
                if let Some((done, total)) = self.progress.filter(|(_, total)| *total > 0) {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total as f32)
                            .desired_width(220.0)
                            .text(format!("{done}/{total}")),
                    );
                }
            });
        });
''',
    '''        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.busy {
                    ui.spinner();
                }
                ui.label(&self.status);
                if let Some(file_name) = &self.current_file {
                    ui.separator();
                    ui.small(file_name);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.indexing && self.index_control.is_some() {
                        let label = if self.index_paused { "▶ Resume" } else { "⏸ Pause" };
                        if ui.button(label).clicked() {
                            self.toggle_index_pause();
                        }
                    }
                    if let Some((done, total)) = self.progress.filter(|(_, total)| *total > 0) {
                        ui.add(
                            egui::ProgressBar::new(done as f32 / total as f32)
                                .desired_width(260.0)
                                .text(format!("{done}/{total}")),
                        );
                    }
                });
            });
        });
''',
)

# ---------------------------------------------------------------------------
# Grid width/scrollbar + native Explorer context menu
# ---------------------------------------------------------------------------
replace_once(
    "src/ui/views.rs",
    '''        egui::ScrollArea::vertical().show_rows(ui, row_height, rows, |ui, row_range| {
''',
    '''        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_height, rows, |ui, row_range| {
''',
)
# Closing style remains syntactically valid because method chain only changes opening.
text = read("src/ui/views.rs")
text = text.replace('                                response.context_menu(|ui| file_context_menu(ui, &record.path));\n', '')
text = text.replace('                                label.context_menu(|ui| file_context_menu(ui, &record.path));\n', '')
text = text.replace('                        response.context_menu(|ui| file_context_menu(ui, &record.path));\n', '')
text = text.replace('                                name.context_menu(|ui| file_context_menu(ui, &record.path));\n', '')
write("src/ui/views.rs", text)
replace_once(
    "src/ui/views.rs",
    '''        if response.secondary_clicked() && !self.selected_paths.contains(path) {
            self.select_path(path, false);
        }
''',
    '''        if response.secondary_clicked() {
            if !self.selected_paths.contains(path) {
                self.select_path(path, false);
            }
            crate::windows_shell::show_context_menu(path.to_path_buf());
        }
''',
)
# Remove old fake context-menu helper block.
views = read("src/ui/views.rs")
start_marker = "fn file_context_menu(ui: &mut egui::Ui, path: &Path) {\n"
end_marker = "fn swatch(ui: &mut egui::Ui, rgb: [u8; 3]) {\n"
if start_marker in views:
    start_i = views.index(start_marker)
    end_i = views.index(end_marker, start_i)
    views = views[:start_i] + views[end_i:]
    write("src/ui/views.rs", views)

print("alpha4 UI stabilization patch applied")
