mod collections;
mod face_runtime;
mod face_search_panel;
mod people_filter;
mod people_manager;
mod settings_window;
mod texture_lru;
mod thumbnails;
mod views;

use crate::db::{self, ImageSummary};
use crate::embedding::EmbeddingService;
use crate::face_settings::{self, FaceEmbeddingSettings};
use crate::face_sface_adapter::SFaceExecutionProvider;
use crate::fs_watch::{FsWatchMessage, FsWatchService};
use crate::indexer::{self, WorkerMessage};
use crate::portable;
use crate::settings::{self, ClipExecutionProvider, IndexingSettings};
use crate::text_search::TextSearchService;
use crate::thumbnail_cache;
use eframe::egui;
use egui::{ColorImage, TextureHandle, TextureOptions};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};
use texture_lru::{TextureLru, DEFAULT_GPU_TEXTURE_CAPACITY};
use thumbnails::{ThumbnailPool, ThumbnailResult};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ViewMode {
    Grid,
    Details,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ThumbnailFit {
    Contain,
    Cover,
}

enum StartupMessage {
    Stage {
        status: String,
        done: usize,
        total: usize,
    },
    Ready {
        roots: Vec<PathBuf>,
        images: Vec<ImageSummary>,
        collections: collections::CollectionsState,
        root_counts: HashMap<PathBuf, (usize, usize)>,
        warnings: Vec<String>,
    },
    Error(String),
}

pub struct ImageSearchApp {
    pub(super) db_path: PathBuf,
    embedding_service: EmbeddingService,
    fs_watch_service: FsWatchService,
    pending_fs_paths: HashSet<PathBuf>,
    watcher_reconcile_required: Option<String>,
    pub(super) roots: Vec<PathBuf>,
    pub(super) images: Vec<ImageSummary>,
    image_positions: HashMap<PathBuf, usize>,
    pub(super) similarity_results: Option<Vec<ImageSummary>>,
    pub(super) query_image: Option<PathBuf>,
    pub(super) similarity_settings: indexer::SimilaritySettings,
    pub(super) indexing_settings: IndexingSettings,
    settings_path: PathBuf,
    face_embedding_settings: FaceEmbeddingSettings,
    face_settings_path: PathBuf,
    face_runtime: face_runtime::FaceRuntimeState,
    face_search_ui: face_search_panel::FaceSearchUiState,
    people_filter_ui: people_filter::PeopleFilterUiState,
    people_manager_ui: people_manager::PeopleManagerUiState,
    collections: collections::CollectionsState,
    pub(super) search_text: String,
    text_search_service: TextSearchService,
    text_search_matches: Option<HashSet<PathBuf>>,
    text_search_observed: String,
    text_search_due: Option<Instant>,
    text_search_generation: u64,
    text_search_pending: bool,
    pub(super) color_enabled: bool,
    pub(super) target_color: [u8; 3],
    pub(super) color_tolerance: f32,
    pub(super) view_mode: ViewMode,
    pub(super) thumb_size: f32,
    pub(super) thumb_fit: ThumbnailFit,
    pub(super) textures: HashMap<PathBuf, TextureHandle>,
    texture_lru: TextureLru,
    pub(super) selected_paths: HashSet<PathBuf>,
    thumb_pool: ThumbnailPool,
    pub(super) tx: Sender<WorkerMessage>,
    pub(super) rx: Receiver<WorkerMessage>,
    startup_rx: Receiver<StartupMessage>,
    index_control: Option<indexer::IndexControl>,
    index_paused: bool,
    searching: bool,
    current_file: Option<String>,
    root_counts: HashMap<PathBuf, (usize, usize)>,
    pub(super) busy: bool,
    pub(super) indexing: bool,
    pub(super) status: String,
    pub(super) progress: Option<(usize, usize)>,
    pub(super) last_error: Option<String>,
    settings_open: bool,
    close_confirmation_open: bool,
    allow_close: bool,
}

impl ImageSearchApp {
    pub fn new(db_path: PathBuf, model_cache: PathBuf) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let (startup_tx, startup_rx) = std::sync::mpsc::channel();
        let app_data_dir = db_path.parent().unwrap_or_else(|| Path::new("."));
        let thumbnail_cache = thumbnail_cache::cache_dir_for_db(&db_path);
        let settings_path = app_data_dir.join("performance-settings.ini");
        let indexing_settings = settings::load(&settings_path);
        let face_settings_path = app_data_dir.join("face-embedding-settings.ini");
        let face_embedding_settings = face_settings::load(&face_settings_path);
        let face_runtime = face_runtime::FaceRuntimeState::new(app_data_dir);
        let face_search_ui = face_search_panel::FaceSearchUiState::default();
        let people_filter_ui = people_filter::PeopleFilterUiState::default();
        let people_manager_ui = people_manager::PeopleManagerUiState::default();
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
            face_embedding_settings,
            face_settings_path,
            face_runtime,
            face_search_ui,
            people_filter_ui,
            people_manager_ui,
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
            searching: false,
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

    fn process_startup_messages(&mut self) {
        while let Ok(message) = self.startup_rx.try_recv() {
            match message {
                StartupMessage::Stage {
                    status,
                    done,
                    total,
                } => {
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
                    self.status = warnings
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "Ready".to_owned());
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

    fn process_worker_messages(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                WorkerMessage::Status(status) => self.status = status,
                WorkerMessage::CurrentFile(file_name) => self.current_file = Some(file_name),
                WorkerMessage::Progress { done, total } => self.progress = Some((done, total)),
                WorkerMessage::IndexedBatch(records) => {
                    self.merge_indexed_batch(records);
                    self.refresh_live_root_indexed_counts();
                    self.refresh_collection_effective_membership();
                    self.refresh_text_search_after_data_change();
                }
                WorkerMessage::RemovedPaths(paths) => {
                    self.remove_indexed_paths(paths);
                    self.refresh_live_root_indexed_counts();
                    self.refresh_collection_effective_membership();
                    self.refresh_text_search_after_data_change();
                }
                WorkerMessage::Reload => {
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
                WorkerMessage::SimilarityResults(results) => {
                    self.similarity_results = Some(results);
                    if !self.indexing {
                        self.progress = None;
                    }
                }
                WorkerMessage::RootCounts(counts) => {
                    self.root_counts = counts;
                    self.refresh_live_root_indexed_counts();
                    let _ = self.collections.refresh_discovered_counts(&self.db_path);
                }
                WorkerMessage::Warning(warning) => {
                    self.last_error = Some(warning.clone());
                    self.status = warning;
                }
                WorkerMessage::Error(error) => {
                    self.last_error = Some(error.clone());
                    self.status = error;
                }
                WorkerMessage::SearchIdle => {
                    self.searching = false;
                    self.busy = self.indexing;
                }
                WorkerMessage::Idle => {
                    self.indexing = false;
                    self.index_paused = false;
                    self.index_control = None;
                    self.current_file = None;
                    self.progress = None;
                    self.close_confirmation_open = false;
                    self.busy = self.searching;
                    self.schedule_face_pipeline_after_base_index();
                }
            }
        }

        if !self.busy && !self.pending_fs_paths.is_empty() {
            let paths: Vec<PathBuf> = self.pending_fs_paths.drain().collect();
            self.start_incremental_update(paths);
        }
    }

    fn rebuild_image_positions(&mut self) {
        self.image_positions.clear();
        self.image_positions.extend(
            self.images
                .iter()
                .enumerate()
                .map(|(index, record)| (record.path.clone(), index)),
        );
    }

    fn refresh_live_root_indexed_counts(&mut self) {
        let mut indexed = HashMap::<PathBuf, usize>::new();
        for image in &self.images {
            *indexed.entry(image.root.clone()).or_default() += 1;
        }
        for root in &self.roots {
            let indexed_count = indexed.get(root).copied().unwrap_or(0);
            let discovered = self
                .root_counts
                .get(root)
                .map(|counts| counts.0)
                .unwrap_or(0)
                .max(indexed_count);
            self.root_counts
                .insert(root.clone(), (discovered, indexed_count));
        }
    }

    fn merge_indexed_batch(&mut self, records: Vec<ImageSummary>) {
        for record in records {
            self.textures.remove(&record.path);
            self.texture_lru.remove(&record.path);
            if let Some(&index) = self.image_positions.get(&record.path) {
                self.images[index] = record;
            } else {
                let index = self.images.len();
                self.image_positions.insert(record.path.clone(), index);
                self.images.push(record);
            }
        }
    }

    fn remove_indexed_paths(&mut self, paths: Vec<PathBuf>) {
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
            self.texture_lru.remove(path);
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
        let control = indexer::IndexControl::default();
        self.index_control = Some(control.clone());
        self.index_paused = false;
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
            control,
            self.tx.clone(),
        );
    }

    fn process_thumbnail_messages(&mut self, ctx: &egui::Context) {
        let mut received = false;
        while let Some(message) = self.thumb_pool.try_recv() {
            received = true;
            if let ThumbnailResult::Ready {
                path,
                width,
                height,
                rgba,
            } = message
            {
                let image = ColorImage::from_rgba_unmultiplied([width, height], &rgba);
                let texture = ctx.load_texture(
                    format!("thumb:{}", path.display()),
                    image,
                    TextureOptions::LINEAR,
                );
                self.textures.insert(path.clone(), texture);
                self.texture_lru.register(&path);
            }
        }
        if received {
            self.evict_gpu_textures();
            ctx.request_repaint();
        }
    }

    fn evict_gpu_textures(&mut self) {
        if self.textures.len() <= self.texture_lru.capacity() {
            return;
        }

        let residents: Vec<PathBuf> = self.textures.keys().cloned().collect();
        let mut protected = HashSet::new();
        if let Some(query) = &self.query_image {
            if self.textures.contains_key(query) {
                protected.insert(query.clone());
            }
        }

        for path in self.texture_lru.eviction_victims(&residents, &protected) {
            self.textures.remove(&path);
        }
    }

    fn start_rescan(&mut self) {
        if self.busy || self.roots.is_empty() {
            return;
        }
        self.busy = true;
        self.indexing = true;
        self.watcher_reconcile_required = None;
        self.allow_close = false;
        self.close_confirmation_open = false;
        self.progress = None;
        self.last_error = None;
        self.similarity_results = None;
        self.selected_paths.clear();
        let control = indexer::IndexControl::default();
        self.index_control = Some(control.clone());
        self.index_paused = false;
        self.status = "Starting recursive rescan…".into();
        indexer::spawn_rescan(
            self.db_path.clone(),
            self.roots.clone(),
            self.indexing_settings,
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
        if self.busy {
            return;
        }
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        match portable::attach_root(&self.db_path, &folder) {
            Ok(outcome) => {
                self.roots = db::load_roots(&self.db_path).unwrap_or_default();
                self.root_counts = db::load_root_counts(&self.db_path).unwrap_or_default();
                self.thumb_pool.set_roots(self.roots.clone());
                self.fs_watch_service.set_roots(self.roots.clone());
                self.images = db::load_image_summaries(&self.db_path).unwrap_or_default();
                self.rebuild_image_positions();
                self.refresh_collection_effective_membership();
                self.refresh_text_search_after_data_change();
                self.status = if outcome.reused_existing_index {
                    format!(
                        "Attached portable index: {} ({} cached image records; no rescan required)",
                        folder.display(),
                        outcome.images
                    )
                } else if outcome.migrated_legacy_rows {
                    format!(
                        "Migrated {} image records into {}/.imagesearch",
                        outcome.images,
                        folder.display()
                    )
                } else {
                    format!(
                        "Portable index initialized: {} — run Rescan to index images",
                        folder.display()
                    )
                };
            }
            Err(err) => self.last_error = Some(format!("Cannot attach folder: {err:#}")),
        }
    }

    fn remove_folder(&mut self, folder: &Path) {
        if self.busy {
            return;
        }
        match db::remove_root(&self.db_path, folder) {
            Ok(()) => {
                self.roots = db::load_roots(&self.db_path).unwrap_or_default();
                self.root_counts = db::load_root_counts(&self.db_path).unwrap_or_default();
                self.thumb_pool.set_roots(self.roots.clone());
                self.images = db::load_image_summaries(&self.db_path).unwrap_or_default();
                self.rebuild_image_positions();
                self.fs_watch_service.set_roots(self.roots.clone());
                self.similarity_results = None;
                self.selected_paths.clear();
                self.refresh_collection_effective_membership();
                self.refresh_text_search_after_data_change();
            }
            Err(err) => self.last_error = Some(format!("Cannot remove folder: {err:#}")),
        }
    }

    fn can_run_similarity_search(&self) -> bool {
        !self.searching
            && !self.images.is_empty()
            && ((!self.busy && !self.indexing) || (self.indexing && self.index_paused))
    }

    fn choose_similarity_image(&mut self) {
        if !self.can_run_similarity_search() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["jpg", "jpeg", "png", "tif", "tiff"])
            .pick_file()
        else {
            return;
        };
        self.run_similarity_search(path);
    }

    fn rerun_similarity_search(&mut self) {
        if let Some(path) = self.query_image.clone() {
            self.run_similarity_search(path);
        }
    }

    fn run_similarity_search(&mut self, path: PathBuf) {
        if self.searching || (self.indexing && !self.index_paused) || (!self.indexing && self.busy)
        {
            return;
        }
        let allow_descriptor_backfill = !self.indexing;
        self.clear_face_search_result_state();
        self.searching = true;
        self.busy = true;
        self.last_error = None;
        self.query_image = Some(path.clone());
        self.selected_paths.clear();
        self.status = "Starting image search with current controls…".into();
        indexer::spawn_similarity_search(
            self.db_path.clone(),
            path,
            self.similarity_settings,
            self.indexing_settings,
            self.embedding_service.clone(),
            allow_descriptor_backfill,
            self.tx.clone(),
        );
    }

    pub(super) fn source(&self) -> &[ImageSummary] {
        self.similarity_results.as_deref().unwrap_or(&self.images)
    }

    pub(super) fn visible_indices(&self) -> Vec<usize> {
        let text_filter_active = !self.search_text.trim().is_empty();
        self.source()
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                if !self.collection_filter_matches(&record.path) {
                    return false;
                }
                if !self.people_filter_matches(&record.path) {
                    return false;
                }
                if text_filter_active {
                    let Some(matches) = &self.text_search_matches else {
                        return false;
                    };
                    if !matches.contains(&record.path) {
                        return false;
                    }
                }
                !self.color_enabled
                    || views::color_distance(record.dominant, self.target_color)
                        <= self.color_tolerance
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn observe_text_search_input(&mut self) {
        if self.search_text == self.text_search_observed {
            return;
        }
        self.text_search_observed = self.search_text.clone();
        self.text_search_generation = self.text_search_generation.wrapping_add(1);
        self.text_search_matches = None;

        if self.search_text.trim().is_empty() {
            self.text_search_due = None;
            self.text_search_pending = false;
        } else {
            self.text_search_due = Some(Instant::now() + Duration::from_millis(160));
            self.text_search_pending = true;
        }
    }

    fn refresh_text_search_after_data_change(&mut self) {
        if self.search_text.trim().is_empty() {
            return;
        }
        self.text_search_generation = self.text_search_generation.wrapping_add(1);
        self.text_search_due = Some(Instant::now() + Duration::from_millis(220));
        self.text_search_pending = true;
    }

    fn dispatch_text_search_if_due(&mut self) {
        let Some(due) = self.text_search_due else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        self.text_search_due = None;
        self.text_search_service
            .request(self.text_search_generation, self.search_text.clone());
    }

    fn process_text_search_results(&mut self) {
        while let Some(result) = self.text_search_service.try_recv() {
            if result.generation != self.text_search_generation || result.query != self.search_text
            {
                continue;
            }
            match result.paths {
                Ok(paths) => {
                    let count = paths.len();
                    self.text_search_matches = Some(paths);
                    self.text_search_pending = false;
                    self.status = format!(
                        "Indexed text search: {count} match{} in {} ms",
                        if count == 1 { "" } else { "es" },
                        result.elapsed_ms
                    );
                }
                Err(err) => {
                    self.text_search_matches = Some(HashSet::new());
                    self.text_search_pending = false;
                    self.last_error = Some(format!("Text search failed: {err}"));
                }
            }
        }
    }

    pub(super) fn thumbnail(&mut self, path: &Path) -> Option<TextureHandle> {
        if let Some(texture) = self.textures.get(path).cloned() {
            self.texture_lru.touch(path);
            return Some(texture);
        }
        self.thumb_pool.request(path);
        None
    }

    pub(super) fn select_path(&mut self, path: &Path, additive: bool) {
        if additive {
            if !self.selected_paths.insert(path.to_path_buf()) {
                self.selected_paths.remove(path);
            }
        } else {
            self.selected_paths.clear();
            self.selected_paths.insert(path.to_path_buf());
        }
    }

    fn clear_thumbnail_cache(&mut self) {
        self.textures.clear();
        self.texture_lru.clear();
        self.thumb_pool.clear_cache();
        self.status = "Thumbnail cache cleared".into();
    }

    fn show_settings_window(&mut self, ctx: &egui::Context) {
        settings_window::show(self, ctx);
    }

    fn show_search_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("search_sidebar")
            .resizable(true)
            .default_width(330.0)
            .min_width(280.0)
            .max_width(470.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Search");
                    self.show_collection_filter(ui);
                    self.show_people_filter(ui);
                    ui.add_space(6.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search_text)
                            .hint_text("filename, path, description, keywords…")
                            .desired_width(f32::INFINITY),
                    );
                    if self.text_search_pending {
                        ui.small("Searching indexed text…");
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                self.can_run_similarity_search(),
                                egui::Button::new("◉ Search by image"),
                            )
                            .clicked()
                        {
                            self.choose_similarity_image();
                        }
                        if self.similarity_results.is_some()
                            && ui.button("Clear image search").clicked()
                        {
                            self.similarity_results = None;
                            self.query_image = None;
                            self.selected_paths.clear();
                            self.clear_face_search_result_state();
                        }
                        if ui
                            .add_enabled(!self.busy, egui::Button::new("👤 Face Search"))
                            .clicked()
                        {
                            self.open_face_search();
                        }
                    });
                    if self.indexing && !self.index_paused {
                        ui.small("Pause indexing to search the already committed images.");
                    } else if self.indexing && self.index_paused {
                        ui.small("Indexing is paused; image search uses committed data only.");
                    }

                    if let Some(query) = self.query_image.clone() {
                        ui.add_space(8.0);
                        ui.strong("Similarity query");
                        if let Some(texture) = self.thumbnail(&query) {
                            views::show_query_preview(ui, &texture, 220.0);
                        } else {
                            ui.add_sized([220.0, 150.0], egui::Label::new("Loading preview…"));
                        }
                        ui.small(views::truncate_middle(&query.display().to_string(), 46))
                            .on_hover_text(query.display().to_string());
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.strong("Color filter");
                    ui.checkbox(&mut self.color_enabled, "Enable explicit color filter");
                    if self.color_enabled {
                        ui.horizontal(|ui| {
                            ui.color_edit_button_srgb(&mut self.target_color);
                            ui.add(
                                egui::Slider::new(&mut self.color_tolerance, 0.03..=0.70)
                                    .text("Tolerance"),
                            );
                        });
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.strong("Similarity weights");
                    ui.small("Relative weights are normalized automatically.");
                    ui.add(
                        egui::Slider::new(
                            &mut self.similarity_settings.color_distribution_weight,
                            0.0..=100.0,
                        )
                        .text("Color distribution")
                        .suffix("%"),
                    );
                    ui.add(
                        egui::Slider::new(
                            &mut self.similarity_settings.texture_weight,
                            0.0..=100.0,
                        )
                        .text("Texture / pattern")
                        .suffix("%"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.similarity_settings.clip_weight, 0.0..=100.0)
                            .text("CLIP semantic")
                            .suffix("%"),
                    );
                    ui.add(
                        egui::Slider::new(
                            &mut self.similarity_settings.dominant_color_weight,
                            0.0..=100.0,
                        )
                        .text("Dominant color")
                        .suffix("%"),
                    );
                    ui.checkbox(
                        &mut self.similarity_settings.strict_color_rejection,
                        "Reject color mismatches",
                    );
                    if self.similarity_settings.strict_color_rejection {
                        ui.add(
                            egui::Slider::new(
                                &mut self.similarity_settings.min_color_distribution_match,
                                0.0..=100.0,
                            )
                            .text("Min color-distribution match")
                            .suffix("%"),
                        );
                        ui.add(
                            egui::Slider::new(
                                &mut self.similarity_settings.max_dominant_color_difference,
                                5.0..=100.0,
                            )
                            .text("Max dominant-color difference")
                            .suffix("%"),
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Reset 44 / 31 / 20 / 5").clicked() {
                            self.similarity_settings = indexer::SimilaritySettings::default();
                        }
                        if ui
                            .add_enabled(
                                self.query_image.is_some() && self.can_run_similarity_search(),
                                egui::Button::new("Apply / re-run"),
                            )
                            .clicked()
                        {
                            self.rerun_similarity_search();
                        }
                    });

                    if !self.selected_paths.is_empty() {
                        ui.add_space(8.0);
                        ui.small(format!("{} selected", self.selected_paths.len()));
                    }
                    if let Some(error) = &self.last_error {
                        ui.add_space(8.0);
                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                    }
                });
            });
    }

    fn show_close_confirmation(&mut self, ctx: &egui::Context) {
        if !self.close_confirmation_open {
            return;
        }

        let mut keep_indexing = false;
        let mut close_anyway = false;
        egui::Window::new("Indexing in progress")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("Images are still being indexed and committed in small batches.");
                ui.label(
                    "Already committed images are safe. Closing now stops the current batch and remaining work.",
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Keep indexing").clicked() {
                        keep_indexing = true;
                    }
                    if ui.button("Close anyway").clicked() {
                        close_anyway = true;
                    }
                });
            });

        if keep_indexing {
            self.close_confirmation_open = false;
        }
        if close_anyway {
            self.allow_close = true;
            self.close_confirmation_open = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for ImageSearchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_startup_messages();
        self.process_worker_messages();
        self.process_face_runtime_messages();
        self.process_face_search_messages();
        self.process_people_filter_messages();
        self.process_fs_watch_messages();
        self.process_thumbnail_messages(ctx);
        self.observe_text_search_input();
        self.dispatch_text_search_if_due();
        self.process_text_search_results();

        if self.text_search_pending
            || self.text_search_due.is_some()
            || self.people_filter_work_pending()
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        if ctx.input(|input| input.viewport().close_requested())
            && self.indexing
            && !self.allow_close
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_confirmation_open = true;
        }
        self.show_close_confirmation(ctx);

        if self.busy {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Windows Image Search");
                if ui.button("⚙ Settings").clicked() {
                    self.settings_open = true;
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("👥 People"))
                    .clicked()
                {
                    self.open_people_manager();
                }
                if ui
                    .add_enabled(
                        !self.busy && !self.roots.is_empty(),
                        egui::Button::new("⟳ Rescan"),
                    )
                    .clicked()
                {
                    self.start_rescan();
                }
                ui.separator();
                ui.small(format!("{} indexed images", self.images.len()));
                if self.indexing {
                    ui.small("Indexing… committed results appear live");
                }
            });
        });

        self.show_search_sidebar(ctx);
        self.show_settings_window(ctx);
        self.show_face_search_window(ctx);
        self.show_people_manager_window(ctx);

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.busy {
                    ui.spinner();
                }
                ui.small(views::truncate_middle(&self.status, 96))
                    .on_hover_text(&self.status);
                if let Some(file_name) = &self.current_file {
                    ui.separator();
                    ui.small(file_name);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.indexing && self.index_control.is_some() {
                        let label = if self.index_paused {
                            "▶ Resume"
                        } else {
                            "⏸ Pause"
                        };
                        if ui
                            .add_enabled(!self.searching, egui::Button::new(label))
                            .on_hover_text(if self.searching {
                                "Finish the paused-index image search before resuming indexing"
                            } else {
                                "Pause or resume indexing"
                            })
                            .clicked()
                        {
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

        egui::CentralPanel::default().show(ctx, |ui| {
            let visible = self.visible_indices();
            ui.horizontal(|ui| {
                ui.strong(format!(
                    "{} result{}",
                    visible.len(),
                    if visible.len() == 1 { "" } else { "s" }
                ));
                if self.face_search_active() {
                    ui.small("Face identity similarity order");
                } else if self.similarity_results.is_some() {
                    ui.small("Hybrid similarity order using current weights");
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let view_label = match self.view_mode {
                        ViewMode::Grid => "▦ Grid",
                        ViewMode::Details => "☷ Details",
                    };
                    if ui.button(view_label).clicked() {
                        self.view_mode = match self.view_mode {
                            ViewMode::Grid => ViewMode::Details,
                            ViewMode::Details => ViewMode::Grid,
                        };
                    }

                    let fit_label = match self.thumb_fit {
                        ThumbnailFit::Contain => "Contain",
                        ThumbnailFit::Cover => "Cover",
                    };
                    if ui.button(fit_label).clicked() {
                        self.thumb_fit = match self.thumb_fit {
                            ThumbnailFit::Contain => ThumbnailFit::Cover,
                            ThumbnailFit::Cover => ThumbnailFit::Contain,
                        };
                    }

                    ui.add(
                        egui::Slider::new(&mut self.thumb_size, 96.0..=512.0)
                            .text("Thumbnail")
                            .suffix(" px"),
                    );
                });
            });
            ui.separator();
            if visible.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(if self.images.is_empty() {
                        "No indexed images yet. Open Settings to add a folder, then Rescan."
                    } else {
                        "No images match the current filters."
                    });
                });
            } else if self.view_mode == ViewMode::Grid {
                self.show_grid(ui, &visible);
            } else {
                self.show_details(ui, &visible);
            }
        });
    }
}
