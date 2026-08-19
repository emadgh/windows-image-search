from pathlib import Path
import re


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


# -----------------------------------------------------------------------------
# src/settings.rs
# -----------------------------------------------------------------------------
Path("src/settings.rs").write_text(
    '''use anyhow::{Context, Result};
use std::path::Path;

pub const MAX_BATCH_SIZE: usize = 256;
const MAX_CONFIGURED_THREADS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexingSettings {
    pub decode_workers: usize,
    pub clip_threads: usize,
    pub batch_size: usize,
}

impl Default for IndexingSettings {
    fn default() -> Self {
        let logical = logical_parallelism();
        Self {
            decode_workers: logical.saturating_sub(1).max(1).min(2),
            clip_threads: logical.saturating_sub(1).max(1).min(4),
            batch_size: 16,
        }
    }
}

impl IndexingSettings {
    pub fn sanitized(self) -> Self {
        Self {
            decode_workers: self.decode_workers.clamp(1, max_decode_workers()),
            clip_threads: self.clip_threads.clamp(1, max_clip_threads()),
            batch_size: self.batch_size.clamp(1, MAX_BATCH_SIZE),
        }
    }
}

pub fn logical_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4)
        .max(1)
}

pub fn max_decode_workers() -> usize {
    logical_parallelism().min(MAX_CONFIGURED_THREADS).max(1)
}

pub fn max_clip_threads() -> usize {
    logical_parallelism().min(MAX_CONFIGURED_THREADS).max(1)
}

pub fn load(path: &Path) -> IndexingSettings {
    let mut settings = IndexingSettings::default();
    let Ok(content) = std::fs::read_to_string(path) else {
        return settings;
    };

    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Ok(value) = value.trim().parse::<usize>() else {
            continue;
        };
        match key.trim() {
            "decode_workers" => settings.decode_workers = value,
            "clip_threads" => settings.clip_threads = value,
            "batch_size" => settings.batch_size = value,
            _ => {}
        }
    }

    settings.sanitized()
}

pub fn save(path: &Path, settings: IndexingSettings) -> Result<()> {
    let settings = settings.sanitized();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating settings directory {}", parent.display()))?;
    }
    let content = format!(
        "decode_workers={}\\nclip_threads={}\\nbatch_size={}\\n",
        settings.decode_workers, settings.clip_threads, settings.batch_size
    );
    std::fs::write(path, content)
        .with_context(|| format!("writing performance settings {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_settings_path(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "windows-image-search-{label}-{}-{nonce}.ini",
            std::process::id()
        ))
    }

    #[test]
    fn sanitized_settings_stay_inside_safe_ranges() {
        let settings = IndexingSettings {
            decode_workers: usize::MAX,
            clip_threads: 0,
            batch_size: usize::MAX,
        }
        .sanitized();

        assert_eq!(settings.decode_workers, max_decode_workers());
        assert_eq!(settings.clip_threads, 1);
        assert_eq!(settings.batch_size, MAX_BATCH_SIZE);
    }

    #[test]
    fn saved_settings_round_trip() {
        let path = temp_settings_path("round-trip");
        let expected = IndexingSettings {
            decode_workers: max_decode_workers().min(3),
            clip_threads: max_clip_threads().min(4),
            batch_size: 48,
        }
        .sanitized();

        save(&path, expected).unwrap();
        assert_eq!(load(&path), expected);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_values_are_clamped() {
        let path = temp_settings_path("invalid");
        std::fs::write(
            &path,
            "decode_workers=0\\nclip_threads=999999\\nbatch_size=0\\nunknown=12\\n",
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.decode_workers, 1);
        assert_eq!(loaded.clip_threads, max_clip_threads());
        assert_eq!(loaded.batch_size, 1);
        let _ = std::fs::remove_file(path);
    }
}
''',
    encoding="utf-8",
)


# -----------------------------------------------------------------------------
# src/main.rs
# -----------------------------------------------------------------------------
path = Path("src/main.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "mod metadata;\nmod ui;\n",
    "mod metadata;\nmod settings;\nmod ui;\n",
    "main settings module",
)
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# src/indexer.rs
# -----------------------------------------------------------------------------
path = Path("src/indexer.rs")
text = path.read_text(encoding="utf-8")

text = replace_once(
    text,
    "use crate::metadata;\n",
    "use crate::metadata;\nuse crate::settings::IndexingSettings;\n",
    "indexer settings import",
)
text = replace_once(
    text,
    "const COLOR_HISTOGRAM_BINS: usize = 64;\nconst INDEX_DECODE_WORKERS_CAP: usize = 2;\nconst INDEX_COMMIT_BATCH: usize = 16;\n",
    "const COLOR_HISTOGRAM_BINS: usize = 64;\n",
    "hardcoded indexing constants",
)

text, count = re.subn(
    r"fn background_worker_count\(\) -> usize \{.*?\n\}\n\nfn clip_worker_count\(\) -> usize \{.*?\n\}\n\n",
    "",
    text,
    count=1,
    flags=re.S,
)
if count != 1:
    raise SystemExit(f"worker helper functions: expected one block, found {count}")

text = replace_once(
    text,
    '''pub fn spawn_rescan(
    db_path: PathBuf,
    model_cache: PathBuf,
    roots: Vec<PathBuf>,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let result = rescan(&db_path, &model_cache, &roots, &tx);
''',
    '''pub fn spawn_rescan(
    db_path: PathBuf,
    model_cache: PathBuf,
    roots: Vec<PathBuf>,
    indexing_settings: IndexingSettings,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let result = rescan(&db_path, &model_cache, &roots, indexing_settings, &tx);
''',
    "spawn_rescan signature",
)

text = replace_once(
    text,
    '''fn rescan(
    db_path: &Path,
    model_cache: &Path,
    roots: &[PathBuf],
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let mut conn = db::open(db_path)?;
''',
    '''fn rescan(
    db_path: &Path,
    model_cache: &Path,
    roots: &[PathBuf],
    indexing_settings: IndexingSettings,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let indexing_settings = indexing_settings.sanitized();
    let mut conn = db::open(db_path)?;
''',
    "rescan signature",
)

text = replace_once(
    text,
    '''    let changed_total = pending.len();
    let workers = background_worker_count();
    let _ = tx.send(WorkerMessage::Status(format!(
        "Preparing {changed_total} changed image{} with {workers} HDD-friendly decode worker{}; committing every {INDEX_COMMIT_BATCH} images…",
        if changed_total == 1 { "" } else { "s" },
        if workers == 1 { "" } else { "s" },
    )));
''',
    '''    let changed_total = pending.len();
    let workers = indexing_settings.decode_workers;
    let batch_size = indexing_settings.batch_size;
    let _ = tx.send(WorkerMessage::Status(format!(
        "Preparing {changed_total} changed image{} with {workers} decode worker{}; committing every {batch_size} images…",
        if changed_total == 1 { "" } else { "s" },
        if workers == 1 { "" } else { "s" },
    )));
''',
    "rescan configurable values",
)
text = replace_once(
    text,
    "    for batch in pending.chunks(INDEX_COMMIT_BATCH) {\n",
    "    for batch in pending.chunks(batch_size) {\n",
    "base indexing batch size",
)

visual_call = "        build_visual_descriptors(&conn, &missing_visual, tx)?;\n"
if text.count(visual_call) != 2:
    raise SystemExit(
        f"build_visual_descriptors calls: expected 2, found {text.count(visual_call)}"
    )
text = text.replace(
    visual_call,
    '''        build_visual_descriptors(
            &conn,
            &missing_visual,
            indexing_settings.decode_workers,
            tx,
        )?;
''',
)

text = replace_once(
    text,
    '''fn build_visual_descriptors(
    conn: &rusqlite::Connection,
    paths: &[PathBuf],
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let total = paths.len();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(background_worker_count())
''',
    '''fn build_visual_descriptors(
    conn: &rusqlite::Connection,
    paths: &[PathBuf],
    workers: usize,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let total = paths.len();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers.max(1))
''',
    "visual descriptor workers",
)

text = replace_once(
    text,
    '''fn build_embeddings(
    conn: &mut rusqlite::Connection,
    model_cache: &Path,
    paths: &[PathBuf],
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    std::fs::create_dir_all(model_cache)?;
''',
    '''fn build_embeddings(
    conn: &mut rusqlite::Connection,
    model_cache: &Path,
    paths: &[PathBuf],
    indexing_settings: IndexingSettings,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let indexing_settings = indexing_settings.sanitized();
    std::fs::create_dir_all(model_cache)?;
''',
    "build_embeddings signature",
)

build_start = text.index("fn build_embeddings(")
build_end = text.index("\npub fn spawn_similarity_search", build_start)
build = text[build_start:build_end]
build = replace_once(
    build,
    "        .with_intra_threads(clip_worker_count());\n",
    "        .with_intra_threads(indexing_settings.clip_threads);\n",
    "embedding CLIP threads",
)
build = replace_once(
    build,
    "    let total = paths.len();\n    for (batch_index, batch) in paths.chunks(16).enumerate() {\n",
    "    let total = paths.len();\n    let batch_size = indexing_settings.batch_size;\n    for (batch_index, batch) in paths.chunks(batch_size).enumerate() {\n",
    "embedding batch loop",
)
build = replace_once(
    build,
    ".embed(batch_paths, Some(16))",
    ".embed(batch_paths, Some(batch_size))",
    "embedding model batch",
)
build = replace_once(
    build,
    "        let done = ((batch_index + 1) * 16).min(total);\n",
    "        let done = ((batch_index + 1) * batch_size).min(total);\n",
    "embedding progress batch",
)
text = text[:build_start] + build + text[build_end:]

text = replace_once(
    text,
    "        if let Err(err) = build_embeddings(&mut conn, model_cache, &missing, tx) {\n",
    "        if let Err(err) = build_embeddings(&mut conn, model_cache, &missing, indexing_settings, tx) {\n",
    "rescan embedding settings",
)

text = replace_once(
    text,
    '''pub fn spawn_similarity_search(
    db_path: PathBuf,
    model_cache: PathBuf,
    query_path: PathBuf,
    settings: SimilaritySettings,
    tx: Sender<WorkerMessage>,
) {
''',
    '''pub fn spawn_similarity_search(
    db_path: PathBuf,
    model_cache: PathBuf,
    query_path: PathBuf,
    settings: SimilaritySettings,
    indexing_settings: IndexingSettings,
    tx: Sender<WorkerMessage>,
) {
''',
    "spawn similarity signature",
)
text = replace_once(
    text,
    "        match similarity_search(&db_path, &model_cache, &query_path, settings, &tx) {\n",
    '''        match similarity_search(
            &db_path,
            &model_cache,
            &query_path,
            settings,
            indexing_settings,
            &tx,
        ) {
''',
    "spawn similarity call",
)

text = replace_once(
    text,
    '''fn similarity_search(
    db_path: &Path,
    model_cache: &Path,
    query_path: &Path,
    settings: SimilaritySettings,
    tx: &Sender<WorkerMessage>,
) -> Result<Vec<ImageRecord>> {
    let conn = db::open(db_path)?;
''',
    '''fn similarity_search(
    db_path: &Path,
    model_cache: &Path,
    query_path: &Path,
    settings: SimilaritySettings,
    indexing_settings: IndexingSettings,
    tx: &Sender<WorkerMessage>,
) -> Result<Vec<ImageRecord>> {
    let indexing_settings = indexing_settings.sanitized();
    let conn = db::open(db_path)?;
''',
    "similarity search signature",
)

text = replace_once(
    text,
    "    let query_embedding = match query_clip_embedding(model_cache, query_path) {\n",
    '''    let query_embedding = match query_clip_embedding(
        model_cache,
        query_path,
        indexing_settings.clip_threads,
    ) {
''',
    "query embedding settings",
)

text = replace_once(
    text,
    "fn query_clip_embedding(model_cache: &Path, query_path: &Path) -> Result<Vec<f32>> {\n",
    '''fn query_clip_embedding(
    model_cache: &Path,
    query_path: &Path,
    clip_threads: usize,
) -> Result<Vec<f32>> {
''',
    "query embedding signature",
)
query_start = text.index("fn query_clip_embedding(")
query_end = text.index("\nfn passes_color_gate", query_start)
query_block = text[query_start:query_end]
query_block = replace_once(
    query_block,
    "        .with_intra_threads(clip_worker_count());\n",
    "        .with_intra_threads(clip_threads.max(1));\n",
    "query CLIP threads",
)
text = text[:query_start] + query_block + text[query_end:]

for forbidden in (
    "background_worker_count",
    "clip_worker_count",
    "INDEX_DECODE_WORKERS_CAP",
    "INDEX_COMMIT_BATCH",
):
    if forbidden in text:
        raise SystemExit(f"hardcoded performance symbol remains: {forbidden}")

path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# src/ui/mod.rs
# -----------------------------------------------------------------------------
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")

text = replace_once(
    text,
    "use crate::indexer::{self, WorkerMessage};\n",
    "use crate::indexer::{self, WorkerMessage};\nuse crate::settings::{self, IndexingSettings};\n",
    "UI settings import",
)
text = replace_once(
    text,
    "    pub(super) similarity_settings: indexer::SimilaritySettings,\n",
    "    pub(super) similarity_settings: indexer::SimilaritySettings,\n    pub(super) indexing_settings: IndexingSettings,\n    settings_path: PathBuf,\n",
    "UI indexing settings fields",
)

text = replace_once(
    text,
    '''        let thumbnail_cache = db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("thumbnail-cache");
''',
    '''        let app_data_dir = db_path.parent().unwrap_or_else(|| Path::new("."));
        let thumbnail_cache = app_data_dir.join("thumbnail-cache");
        let settings_path = app_data_dir.join("performance-settings.ini");
        let indexing_settings = settings::load(&settings_path);
''',
    "UI settings path/load",
)
text = replace_once(
    text,
    "            similarity_settings: indexer::SimilaritySettings::default(),\n",
    "            similarity_settings: indexer::SimilaritySettings::default(),\n            indexing_settings,\n            settings_path,\n",
    "UI constructor settings",
)

text = replace_once(
    text,
    '''        indexer::spawn_rescan(
            self.db_path.clone(),
            self.model_cache.clone(),
            self.roots.clone(),
            self.tx.clone(),
        );
''',
    '''        indexer::spawn_rescan(
            self.db_path.clone(),
            self.model_cache.clone(),
            self.roots.clone(),
            self.indexing_settings,
            self.tx.clone(),
        );
''',
    "UI rescan settings",
)
text = replace_once(
    text,
    '''        indexer::spawn_similarity_search(
            self.db_path.clone(),
            self.model_cache.clone(),
            path,
            self.similarity_settings,
            self.tx.clone(),
        );
''',
    '''        indexer::spawn_similarity_search(
            self.db_path.clone(),
            self.model_cache.clone(),
            path,
            self.similarity_settings,
            self.indexing_settings,
            self.tx.clone(),
        );
''',
    "UI similarity settings",
)

text = replace_once(
    text,
    "        let mut clear_cache = false;\n",
    "        let mut clear_cache = false;\n        let mut save_performance_settings = false;\n",
    "UI settings save flag",
)

text = replace_once(
    text,
    '''                ui.add_space(12.0);
                ui.separator();
                ui.heading("Thumbnail cache");
''',
    '''                ui.add_space(12.0);
                ui.separator();
                ui.heading("Indexing performance");
                ui.label(
                    "Tune disk/CPU pressure. Changes are saved immediately and apply to the next indexing or image-search operation.",
                );
                let logical_threads = settings::logical_parallelism();
                ui.small(format!(
                    "Detected {logical_threads} logical CPU thread{}. Safe defaults: Decode 2 / CLIP up to 4 / Batch 16.",
                    if logical_threads == 1 { "" } else { "s" }
                ));
                ui.add_enabled_ui(!self.busy, |ui| {
                    let decode_changed = ui
                        .add(
                            egui::Slider::new(
                                &mut self.indexing_settings.decode_workers,
                                1..=settings::max_decode_workers(),
                            )
                            .text("Image decode workers"),
                        )
                        .changed();
                    let clip_changed = ui
                        .add(
                            egui::Slider::new(
                                &mut self.indexing_settings.clip_threads,
                                1..=settings::max_clip_threads(),
                            )
                            .text("CLIP CPU threads"),
                        )
                        .changed();
                    let batch_changed = ui
                        .add(
                            egui::Slider::new(
                                &mut self.indexing_settings.batch_size,
                                1..=settings::MAX_BATCH_SIZE,
                            )
                            .text("Index / embedding batch size"),
                        )
                        .changed();

                    if decode_changed || clip_changed || batch_changed {
                        self.indexing_settings = self.indexing_settings.sanitized();
                        save_performance_settings = true;
                    }
                    if ui.button("Reset safe defaults").clicked() {
                        self.indexing_settings = IndexingSettings::default();
                        save_performance_settings = true;
                    }
                });
                if self.busy {
                    ui.small("Performance controls are locked while a worker operation is active.");
                }

                ui.add_space(12.0);
                ui.separator();
                ui.heading("Thumbnail cache");
''',
    "performance settings UI",
)

text = replace_once(
    text,
    '''        if clear_cache {
            self.clear_thumbnail_cache();
        }
    }
''',
    '''        if clear_cache {
            self.clear_thumbnail_cache();
        }
        if save_performance_settings {
            self.indexing_settings = self.indexing_settings.sanitized();
            match settings::save(&self.settings_path, self.indexing_settings) {
                Ok(()) => {
                    self.status = format!(
                        "Performance settings saved: decode {}, CLIP {}, batch {}",
                        self.indexing_settings.decode_workers,
                        self.indexing_settings.clip_threads,
                        self.indexing_settings.batch_size
                    );
                }
                Err(err) => {
                    self.last_error = Some(format!("Cannot save performance settings: {err:#}"));
                }
            }
        }
    }
''',
    "performance settings persistence",
)

path.write_text(text, encoding="utf-8")

print("Indexing performance settings patch applied successfully")
