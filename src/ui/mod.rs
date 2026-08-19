mod views;

use crate::db::{self, ImageRecord};
use crate::indexer::{self, WorkerMessage};
use eframe::egui;
use egui::{ColorImage, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ViewMode {
    Grid,
    Details,
}

pub struct ImageSearchApp {
    pub(super) db_path: PathBuf,
    pub(super) model_cache: PathBuf,
    pub(super) roots: Vec<PathBuf>,
    pub(super) images: Vec<ImageRecord>,
    pub(super) similarity_results: Option<Vec<ImageRecord>>,
    pub(super) query_image: Option<PathBuf>,
    pub(super) search_text: String,
    pub(super) color_enabled: bool,
    pub(super) target_color: [u8; 3],
    pub(super) color_tolerance: f32,
    pub(super) view_mode: ViewMode,
    pub(super) thumb_size: f32,
    pub(super) textures: HashMap<PathBuf, TextureHandle>,
    pub(super) tx: Sender<WorkerMessage>,
    pub(super) rx: Receiver<WorkerMessage>,
    pub(super) busy: bool,
    pub(super) status: String,
    pub(super) progress: Option<(usize, usize)>,
    pub(super) last_error: Option<String>,
}

impl ImageSearchApp {
    pub fn new(db_path: PathBuf, model_cache: PathBuf) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            roots: db::load_roots(&db_path).unwrap_or_default(),
            images: db::load_images(&db_path).unwrap_or_default(),
            db_path,
            model_cache,
            similarity_results: None,
            query_image: None,
            search_text: String::new(),
            color_enabled: false,
            target_color: [128, 128, 128],
            color_tolerance: 0.22,
            view_mode: ViewMode::Grid,
            thumb_size: 168.0,
            textures: HashMap::new(),
            tx,
            rx,
            busy: false,
            status: "Ready".into(),
            progress: None,
            last_error: None,
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

    fn start_rescan(&mut self) {
        if self.busy || self.roots.is_empty() {
            return;
        }
        self.busy = true;
        self.progress = None;
        self.last_error = None;
        self.query_image = None;
        self.similarity_results = None;
        self.status = "Starting rescan…".into();
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
                self.query_image = None;
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
        self.busy = true;
        self.last_error = None;
        self.query_image = Some(path.clone());
        indexer::spawn_similarity_search(
            self.db_path.clone(),
            self.model_cache.clone(),
            path,
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

    pub(super) fn thumbnail(&mut self, ctx: &egui::Context, path: &Path) -> Option<TextureHandle> {
        if let Some(texture) = self.textures.get(path) {
            return Some(texture.clone());
        }
        let image = image::ImageReader::open(path)
            .ok()?
            .with_guessed_format()
            .ok()?
            .decode()
            .ok()?;
        let thumb = image.thumbnail(360, 360).to_rgba8();
        let size = [thumb.width() as usize, thumb.height() as usize];
        let pixels = thumb.into_raw();
        let texture = ctx.load_texture(
            path.to_string_lossy(),
            ColorImage::from_rgba_unmultiplied(size, &pixels),
            TextureOptions::LINEAR,
        );
        self.textures.insert(path.to_path_buf(), texture.clone());
        Some(texture)
    }
}

impl eframe::App for ImageSearchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_worker_messages();
        if self.busy {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("＋ Add folder").clicked() {
                    self.add_folder();
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
                if ui
                    .add_enabled(
                        !self.busy && !self.images.is_empty(),
                        egui::Button::new("◉ Search by image"),
                    )
                    .clicked()
                {
                    self.choose_similarity_image();
                }
                if self.similarity_results.is_some() && ui.button("Clear image search").clicked() {
                    self.similarity_results = None;
                    self.query_image = None;
                }
                ui.separator();
                ui.label("Search:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.search_text)
                        .hint_text("filename, path, description, keywords…")
                        .desired_width(320.0),
                );
                ui.checkbox(&mut self.color_enabled, "Color");
                if self.color_enabled {
                    ui.color_edit_button_srgb(&mut self.target_color);
                    ui.add(
                        egui::Slider::new(&mut self.color_tolerance, 0.03..=0.70).text("Tolerance"),
                    );
                }
            });
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.view_mode, ViewMode::Grid, "▦ Grid");
                ui.selectable_value(&mut self.view_mode, ViewMode::Details, "☷ Details");
                if self.view_mode == ViewMode::Grid {
                    ui.add(egui::Slider::new(&mut self.thumb_size, 96.0..=280.0).text("Thumbnail"));
                }
            });
        });

        egui::SidePanel::left("folders")
            .resizable(true)
            .default_width(245.0)
            .show(ctx, |ui| {
                ui.heading("Indexed folders");
                ui.small(format!("{} images", self.images.len()));
                ui.separator();
                let mut remove = None;
                for root in &self.roots {
                    ui.horizontal(|ui| {
                        ui.label(views::truncate_middle(&root.display().to_string(), 30))
                            .on_hover_text(root.display().to_string());
                        if ui.small_button("×").clicked() {
                            remove = Some(root.clone());
                        }
                    });
                }
                if let Some(root) = remove {
                    self.remove_folder(&root);
                }
                if self.roots.is_empty() {
                    ui.label("Add a folder, then run Rescan.");
                }
                if let Some(query) = &self.query_image {
                    ui.separator();
                    ui.strong("Similarity query");
                    ui.label(views::truncate_middle(&query.display().to_string(), 32));
                }
                if let Some(error) = &self.last_error {
                    ui.separator();
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
            });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.busy {
                    ui.spinner();
                }
                ui.label(&self.status);
                if let Some((done, total)) = self.progress.filter(|(_, total)| *total > 0) {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total as f32)
                            .desired_width(180.0)
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
                    ui.small("CLIP similarity order");
                }
            });
            ui.separator();
            if visible.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(if self.images.is_empty() {
                        "No indexed images yet."
                    } else {
                        "No images match the current filters."
                    });
                });
            } else if self.view_mode == ViewMode::Grid {
                self.show_grid(ui, ctx, &visible);
            } else {
                self.show_details(ui, ctx, &visible);
            }
        });
    }
}
