mod thumbnails;
mod views;

use crate::db::{self, ImageRecord};
use crate::indexer::{self, WorkerMessage};
use eframe::egui;
use egui::{ColorImage, TextureHandle, TextureOptions};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;
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

pub struct ImageSearchApp {
    pub(super) db_path: PathBuf,
    pub(super) model_cache: PathBuf,
    pub(super) roots: Vec<PathBuf>,
    pub(super) images: Vec<ImageRecord>,
    pub(super) similarity_results: Option<Vec<ImageRecord>>,
    pub(super) query_image: Option<PathBuf>,
    pub(super) similarity_settings: indexer::SimilaritySettings,
    pub(super) search_text: String,
    pub(super) color_enabled: bool,
    pub(super) target_color: [u8; 3],
    pub(super) color_tolerance: f32,
    pub(super) view_mode: ViewMode,
    pub(super) thumb_size: f32,
    pub(super) thumb_fit: ThumbnailFit,
    pub(super) textures: HashMap<PathBuf, TextureHandle>,
    pub(super) selected_paths: HashSet<PathBuf>,
    thumb_pool: ThumbnailPool,
    pub(super) tx: Sender<WorkerMessage>,
    pub(super) rx: Receiver<WorkerMessage>,
    pub(super) busy: bool,
    pub(super) status: String,
    pub(super) progress: Option<(usize, usize)>,
    pub(super) last_error: Option<String>,
    settings_open: bool,
}

impl ImageSearchApp {
    pub fn new(db_path: PathBuf, model_cache: PathBuf) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let thumbnail_cache = db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("thumbnail-cache");
        Self {
            roots: db::load_roots(&db_path).unwrap_or_default(),
            images: db::load_images(&db_path).unwrap_or_default(),
            db_path,
            model_cache,
            similarity_results: None,
            query_image: None,
            similarity_settings: indexer::SimilaritySettings::default(),
            search_text: String::new(),
            color_enabled: false,
            target_color: [128, 128, 128],
            color_tolerance: 0.22,
            view_mode: ViewMode::Grid,
            thumb_size: 168.0,
            thumb_fit: ThumbnailFit::Contain,
            textures: HashMap::new(),
            selected_paths: HashSet::new(),
            thumb_pool: ThumbnailPool::new(thumbnail_cache),
            tx,
            rx,
            busy: false,
            status: "Ready".into(),
            progress: None,
            last_error: None,
            settings_open: false,
        }
    }

    fn process_worker_messages(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                WorkerMessage::Status(status) => self.status = status,
                WorkerMessage::Progress { done, total } => self.progress = Some((done, total)),
                WorkerMessage::Reload => {
                    match db::load_images(&self.db_path) {
                        Ok(images) => self.images = images,
                        Err(err) => self.last_error = Some(format!("Reload failed: {err:#}")),
                    }
                    self.progress = None;
                }
                WorkerMessage::SimilarityResults(results) => {
                    self.similarity_results = Some(results);
                    self.progress = None;
                }
                WorkerMessage::Error(error) => {
                    self.last_error = Some(error.clone());
                    self.status = error;
                }
                WorkerMessage::Idle => self.busy = false,
            }
        }
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
                self.textures.insert(path, texture);
            }
        }
        if received {
            ctx.request_repaint();
        }
    }

    fn start_rescan(&mut self) {
        if self.busy || self.roots.is_empty() {
            return;
        }
        self.busy = true;
        self.progress = None;
        self.last_error = None;
        self.similarity_results = None;
        self.selected_paths.clear();
        self.status = "Starting recursive rescan…".into();
        indexer::spawn_rescan(
            self.db_path.clone(),
            self.model_cache.clone(),
            self.roots.clone(),
            self.tx.clone(),
        );
    }

    fn add_folder(&mut self) {
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        match db::add_root(&self.db_path, &folder) {
            Ok(()) => {
                self.roots = db::load_roots(&self.db_path).unwrap_or_default();
                self.status = format!("Added {}", folder.display());
            }
            Err(err) => self.last_error = Some(format!("Cannot add folder: {err:#}")),
        }
    }

    fn remove_folder(&mut self, folder: &Path) {
        match db::remove_root(&self.db_path, folder) {
            Ok(()) => {
                self.roots = db::load_roots(&self.db_path).unwrap_or_default();
                self.images = db::load_images(&self.db_path).unwrap_or_default();
                self.similarity_results = None;
                self.selected_paths.clear();
            }
            Err(err) => self.last_error = Some(format!("Cannot remove folder: {err:#}")),
        }
    }

    fn choose_similarity_image(&mut self) {
        if self.busy {
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
        if self.busy {
            return;
        }
        self.busy = true;
        self.last_error = None;
        self.query_image = Some(path.clone());
        self.selected_paths.clear();
        self.status = "Starting image search with current controls…".into();
        indexer::spawn_similarity_search(
            self.db_path.clone(),
            self.model_cache.clone(),
            path,
            self.similarity_settings,
            self.tx.clone(),
        );
    }

    pub(super) fn source(&self) -> &[ImageRecord] {
        self.similarity_results.as_deref().unwrap_or(&self.images)
    }

    pub(super) fn visible_indices(&self) -> Vec<usize> {
        let tokens: Vec<String> = self
            .search_text
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
        self.source()
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                if !tokens.is_empty() {
                    let haystack = format!(
                        "{} {} {} {}",
                        record.file_name,
                        record.path.display(),
                        record.description,
                        record.keywords
                    )
                    .to_ascii_lowercase();
                    if !tokens.iter().all(|token| haystack.contains(token)) {
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

    pub(super) fn thumbnail(&mut self, path: &Path) -> Option<TextureHandle> {
        if let Some(texture) = self.textures.get(path) {
            return Some(texture.clone());
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
        self.thumb_pool.clear_cache();
        self.status = "Thumbnail cache cleared".into();
    }

    fn show_settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }

        let mut open = self.settings_open;
        let mut add_folder = false;
        let mut remove_folder = None;
        let mut clear_cache = false;

        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(true)
            .default_width(620.0)
            .show(ctx, |ui| {
                ui.heading("Indexed folders");
                ui.label("Recursive indexing is enabled for every root and includes all subfolders.");
                ui.horizontal(|ui| {
                    if ui.button("＋ Add folder").clicked() {
                        add_folder = true;
                    }
                    if ui
                        .add_enabled(
                            !self.busy && !self.roots.is_empty(),
                            egui::Button::new("⟳ Rescan all folders"),
                        )
                        .clicked()
                    {
                        self.start_rescan();
                    }
                });
                ui.separator();
                if self.roots.is_empty() {
                    ui.label("No folders configured.");
                } else {
                    for root in &self.roots {
                        ui.horizontal(|ui| {
                            ui.label(root.display().to_string());
                            if ui.small_button("Remove").clicked() {
                                remove_folder = Some(root.clone());
                            }
                        });
                    }
                }

                ui.add_space(12.0);
                ui.separator();
                ui.heading("Thumbnail cache");
                ui.label(format!("Location: {}", self.thumb_pool.cache_dir().display()));
                ui.label("Cached previews are generated at up to 512 px on background worker threads.");
                if ui.button("Clear thumbnail cache").clicked() {
                    clear_cache = true;
                }
            });

        self.settings_open = open;
        if add_folder {
            self.add_folder();
        }
        if let Some(root) = remove_folder {
            self.remove_folder(&root);
        }
        if clear_cache {
            self.clear_thumbnail_cache();
        }
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
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search_text)
                            .hint_text("filename, path, description, keywords…")
                            .desired_width(f32::INFINITY),
                    );

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !self.busy && !self.images.is_empty(),
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
                        }
                    });

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
                        egui::Slider::new(
                            &mut self.similarity_settings.clip_weight,
                            0.0..=100.0,
                        )
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
                                !self.busy && self.query_image.is_some(),
                                egui::Button::new("Apply / re-run"),
                            )
                            .clicked()
                        {
                            self.rerun_similarity_search();
                        }
                    });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.strong("View");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.view_mode, ViewMode::Grid, "▦ Grid");
                        ui.selectable_value(&mut self.view_mode, ViewMode::Details, "☷ Details");
                    });
                    if self.view_mode == ViewMode::Grid {
                        ui.add(
                            egui::Slider::new(&mut self.thumb_size, 96.0..=512.0)
                                .text("Thumbnail")
                                .suffix(" px"),
                        );
                    }
                    ui.horizontal(|ui| {
                        ui.label("Image fit:");
                        ui.selectable_value(&mut self.thumb_fit, ThumbnailFit::Contain, "Contain");
                        ui.selectable_value(&mut self.thumb_fit, ThumbnailFit::Cover, "Cover");
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
}

impl eframe::App for ImageSearchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_worker_messages();
        self.process_thumbnail_messages(ctx);
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
            });
        });

        self.show_search_sidebar(ctx);
        self.show_settings_window(ctx);

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
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

        egui::CentralPanel::default().show(ctx, |ui| {
            let visible = self.visible_indices();
            ui.horizontal(|ui| {
                ui.strong(format!(
                    "{} result{}",
                    visible.len(),
                    if visible.len() == 1 { "" } else { "s" }
                ));
                if self.similarity_results.is_some() {
                    ui.small("Hybrid similarity order using current weights");
                }
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
