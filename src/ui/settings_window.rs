use super::{AppearanceMode, ImageSearchApp};
use crate::face_settings;
use crate::face_sface_adapter::SFaceExecutionProvider;
use crate::portable;
use crate::settings::{self, ClipExecutionProvider, IndexingSettings};
use eframe::egui;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsCategory {
    Appearance,
    SearchClip,
    FacesPeople,
    Performance,
    Storage,
}

impl SettingsCategory {
    const ALL: [Self; 5] = [
        Self::Appearance,
        Self::SearchClip,
        Self::FacesPeople,
        Self::Performance,
        Self::Storage,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::SearchClip => "Search / CLIP",
            Self::FacesPeople => "Faces / People",
            Self::Performance => "Performance",
            Self::Storage => "Storage",
        }
    }

    fn index(self) -> u8 {
        match self {
            Self::Appearance => 0,
            Self::SearchClip => 1,
            Self::FacesPeople => 2,
            Self::Performance => 3,
            Self::Storage => 4,
        }
    }

    fn from_index(index: u8) -> Self {
        match index {
            1 => Self::SearchClip,
            2 => Self::FacesPeople,
            3 => Self::Performance,
            4 => Self::Storage,
            _ => Self::Appearance,
        }
    }
}

#[derive(Default)]
struct Effects {
    add_folder: bool,
    remove_folder: Option<PathBuf>,
    clear_cache: bool,
    save_performance_settings: bool,
    save_face_settings: bool,
}

pub(super) fn show(app: &mut ImageSearchApp, ctx: &egui::Context) {
    if !app.settings_open {
        return;
    }
    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        app.settings_open = false;
        return;
    }

    let category_id = egui::Id::new("preferences-category");
    let mut category = SettingsCategory::from_index(
        ctx.data_mut(|data| data.get_temp::<u8>(category_id).unwrap_or(0)),
    );
    let mut open = app.settings_open;
    let mut effects = Effects::default();

    egui::Window::new("Preferences")
        .open(&mut open)
        .resizable(true)
        .default_size([920.0, 640.0])
        .min_width(580.0)
        .min_height(460.0)
        .max_height((ctx.available_rect().height() - 48.0).max(320.0))
        .show(ctx, |ui| {
            let height = ui.available_height().max(440.0);
            let sidebar_width = 168.0;

            // Consume one fixed outer region, then let the right-to-left layout reserve
            // the sidebar/separator before the content takes the exact remainder. Avoid
            // deriving a child width from the parent's current width and then adding
            // spacing/separator back on top: that creates a frame-to-frame growth loop.
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), height),
                egui::Layout::right_to_left(egui::Align::Min),
                |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(sidebar_width, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.heading("Preferences");
                            ui.add_space(5.0);
                            for item in SettingsCategory::ALL {
                                if ui
                                    .selectable_label(category == item, item.label())
                                    .clicked()
                                {
                                    category = item;
                                }
                            }

                            ui.add_space(14.0);
                            ui.separator();
                            ui.small(format!("{} indexed images", app.images.len()));
                            ui.small(format!(
                                "{} indexed root{}",
                                app.roots.len(),
                                if app.roots.len() == 1 { "" } else { "s" }
                            ));
                            if app.busy {
                                ui.add_space(4.0);
                                ui.spinner();
                                ui.small("Background work active");
                            }
                        },
                    );

                    ui.separator();

                    let content_width = ui.available_width().max(1.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(content_width, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("preferences-category-content")
                                .auto_shrink([false, false])
                                .show(ui, |ui| match category {
                                    SettingsCategory::Appearance => settings_appearance(app, ui),
                                    SettingsCategory::SearchClip => {
                                        settings_search_clip(app, ui, &mut effects)
                                    }
                                    SettingsCategory::FacesPeople => {
                                        settings_faces_people(app, ui, &mut effects)
                                    }
                                    SettingsCategory::Performance => {
                                        settings_performance(app, ui, &mut effects)
                                    }
                                    SettingsCategory::Storage => {
                                        settings_storage(app, ui, &mut effects)
                                    }
                                });
                        },
                    );
                },
            );
        });

    ctx.data_mut(|data| data.insert_temp(category_id, category.index()));
    app.settings_open = open;
    apply_effects(app, effects);
}

fn section_title(ui: &mut egui::Ui, title: &str, description: &str) {
    ui.heading(title);
    ui.separator();
    ui.add_space(4.0);
    if !description.is_empty() {
        ui.label(description);
        ui.add_space(6.0);
    }
}

fn settings_library_indexing(app: &mut ImageSearchApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(
        ui,
        "Library / Indexing",
        "Manage portable indexed roots and reconcile changes. Every root is scanned recursively.",
    );

    ui.horizontal(|ui| {
        if ui
            .add_enabled(!app.busy, egui::Button::new("Add folder"))
            .clicked()
        {
            effects.add_folder = true;
        }
        if ui
            .add_enabled(
                !app.busy && !app.roots.is_empty(),
                egui::Button::new("Rescan changed"),
            )
            .clicked()
        {
            app.start_rescan();
        }
        if ui
            .add_enabled(
                !app.busy && !app.roots.is_empty(),
                egui::Button::new("Force rescan all"),
            )
            .on_hover_text(
                "Rebuild all visual/CLIP descriptors; valid cached thumbnails are used instead of large originals when safe.",
            )
            .clicked()
        {
            app.start_force_rescan();
        }
    });

    show_worker_activity(app, ui);

    ui.add_space(10.0);
    ui.strong("Indexed folders");
    if app.roots.is_empty() {
        ui.label("No folders configured.");
    } else {
        for root in app.roots.clone() {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(root.display().to_string());
                        let (discovered, indexed) =
                            app.root_counts.get(&root).copied().unwrap_or((
                                0,
                                app.images.iter().filter(|image| image.root == root).count(),
                            ));
                        ui.small(format!("{indexed}/{discovered} indexed"));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!app.busy, egui::Button::new("Remove").small())
                            .clicked()
                        {
                            effects.remove_folder = Some(root.clone());
                        }
                        if portable::is_indexed_root(&root) {
                            ui.small("✓ .imagesearch");
                        }
                    });
                });
            });
        }
    }

    ui.add_space(12.0);
    ui.strong("Live indexing");
    ui.label(
        "Filesystem watching is enabled. Create, modify, rename and delete events are debounced and indexed without a full root rescan.",
    );
    ui.small("Rescan changed remains the reconciliation fallback for missed watcher events.");
    if let Some(reason) = &app.watcher_reconcile_required {
        ui.colored_label(egui::Color32::LIGHT_RED, reason);
    }
}

fn settings_appearance(app: &mut ImageSearchApp, ui: &mut egui::Ui) {
    section_title(
        ui,
        "Appearance",
        "Choose a compact application theme. System follows the appearance captured when the app starts.",
    );

    ui.horizontal_wrapped(|ui| {
        for mode in [
            AppearanceMode::System,
            AppearanceMode::Light,
            AppearanceMode::Dark,
        ] {
            if ui
                .selectable_label(app.appearance_mode == mode, mode.label())
                .clicked()
            {
                app.appearance_mode = mode;
            }
        }
    });
    ui.add_space(8.0);
    ui.small("The same spacing, typography, selection treatment and control geometry are used in both light and dark themes.");
}

fn settings_search_clip(app: &mut ImageSearchApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(
        ui,
        "Search / CLIP",
        "Choose the execution backend used for CLIP image embeddings and similarity search.",
    );

    ui.add_enabled_ui(!app.busy, |ui| {
        let provider_before = app.indexing_settings.clip_execution_provider;
        egui::ComboBox::from_label("CLIP execution provider")
            .selected_text(app.indexing_settings.clip_execution_provider.label())
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut app.indexing_settings.clip_execution_provider,
                    ClipExecutionProvider::Cpu,
                    "CPU (safe default)",
                );
                ui.selectable_value(
                    &mut app.indexing_settings.clip_execution_provider,
                    ClipExecutionProvider::DirectMl,
                    "DirectML (Windows GPU)",
                );
            });
        if provider_before != app.indexing_settings.clip_execution_provider {
            effects.save_performance_settings = true;
        }
        ui.small(
            "DirectML uses the same CLIP model and falls back to CPU automatically if the GPU provider cannot initialize.",
        );
    });

    if app.busy {
        ui.small("Search/model controls are locked while a worker operation is active.");
    }

    ui.add_space(12.0);
    ui.strong("Search behavior");
    ui.small(
        "Similarity weights, explicit color filtering and the active query remain in the Search sidebar because they are per-search controls rather than application preferences.",
    );
}

fn settings_faces_people(app: &mut ImageSearchApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(
        ui,
        "Faces / People",
        "Configure face detection, SFace identity embeddings and automatic People grouping.",
    );

    app.show_face_detector_settings(ui);

    ui.add_space(12.0);
    ui.separator();
    ui.heading("Face identity (SFace)");
    ui.label(
        "The verified managed SFace model is the default. Browse an ONNX file below only to use a custom identity model.",
    );
    ui.add_enabled_ui(!app.busy && !app.face_model_download_running(), |ui| {
        ui.horizontal(|ui| {
            let mut model_path = app
                .face_embedding_settings
                .model_path
                .to_string_lossy()
                .into_owned();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut model_path)
                        .hint_text("Path to external SFace .onnx")
                        .desired_width(520.0),
                )
                .changed()
            {
                app.face_embedding_settings.model_path = PathBuf::from(model_path.trim());
                effects.save_face_settings = true;
            }
            if ui.button("Browse…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("ONNX model", &["onnx"])
                    .pick_file()
                {
                    app.face_embedding_settings.model_path = path;
                    effects.save_face_settings = true;
                }
            }
        });

        let provider_before = app.face_embedding_settings.provider;
        egui::ComboBox::from_label("SFace execution provider")
            .selected_text(app.face_embedding_settings.provider_label())
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut app.face_embedding_settings.provider,
                    SFaceExecutionProvider::Cpu,
                    "CPU",
                );
                ui.selectable_value(
                    &mut app.face_embedding_settings.provider,
                    SFaceExecutionProvider::DirectMl,
                    "DirectML (Windows GPU)",
                );
            });
        if provider_before != app.face_embedding_settings.provider {
            effects.save_face_settings = true;
        }
    });

    if !app.face_embedding_settings.configured() {
        ui.small(
            "No SFace model configured. Face embedding remains disabled until an external ONNX path is selected.",
        );
    } else if app.face_embedding_settings.model_path.is_file() {
        ui.small(format!(
            "Configured: {} on {}",
            app.face_embedding_settings.model_path.display(),
            app.face_embedding_settings.provider_label()
        ));
    } else {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            "Configured SFace model path is not currently available.",
        );
    }
}

fn settings_performance(app: &mut ImageSearchApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(
        ui,
        "Performance",
        "Tune disk, CPU and embedding pressure. Changes apply to the next indexing or image-search operation.",
    );

    let logical_threads = settings::logical_parallelism();
    ui.small(format!(
        "Detected {logical_threads} logical CPU thread{}. Safe defaults: Decode 2 / CLIP up to 4 / Batch 16 / Device CPU / Direct source ceiling 256 MiB.",
        if logical_threads == 1 { "" } else { "s" }
    ));

    ui.add_enabled_ui(!app.busy, |ui| {
        let decode_changed = ui
            .add(
                egui::Slider::new(
                    &mut app.indexing_settings.decode_workers,
                    1..=settings::max_decode_workers(),
                )
                .text("Image decode workers"),
            )
            .changed();
        let clip_changed = ui
            .add(
                egui::Slider::new(
                    &mut app.indexing_settings.clip_threads,
                    1..=settings::max_clip_threads(),
                )
                .text("CLIP CPU threads"),
            )
            .changed();
        let batch_changed = ui
            .add(
                egui::Slider::new(
                    &mut app.indexing_settings.batch_size,
                    1..=settings::MAX_BATCH_SIZE,
                )
                .text("Index / embedding batch size"),
            )
            .changed();
        let max_file_size_changed = ui
            .add(
                egui::Slider::new(
                    &mut app.indexing_settings.max_file_size_mib,
                    1..=settings::MAX_FILE_SIZE_MIB,
                )
                .text("Maximum source file size (MiB)"),
            )
            .changed();
        if app.indexing_settings.max_file_size_mib <= settings::DIRECT_DECODE_MAX_FILE_SIZE_MIB {
            ui.small(format!(
                "Files larger than {} MiB are skipped before metadata, image decode, visual descriptors and CLIP work. Direct decoding is hard-capped at 256 MiB.",
                app.indexing_settings.max_file_size_mib
            ));
        } else {
            ui.small(format!(
                "Sources up to 256 MiB use the normal direct path. Sources above 256 MiB and up to {} MiB are forced through a bounded 2048 px resized-preview path; larger sources are skipped. Oversized non-JPEG sources are safely skipped until a bounded decoder is available.",
                app.indexing_settings.max_file_size_mib
            ));
        }

        if decode_changed || clip_changed || batch_changed || max_file_size_changed {
            app.indexing_settings = app.indexing_settings.sanitized();
            effects.save_performance_settings = true;
        }
        if ui.button("Reset safe defaults").clicked() {
            app.indexing_settings = IndexingSettings::default();
            effects.save_performance_settings = true;
        }
    });

    if app.busy {
        ui.small("Performance controls are locked while a worker operation is active.");
    }
}

fn settings_storage(app: &mut ImageSearchApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(
        ui,
        "Storage",
        "Inspect local application state and manage disposable thumbnail cache data.",
    );

    ui.strong("Face model attribution");
    ui.small("YuNet 2026may — OpenCV Zoo — MIT. SFace 2021dec — OpenCV Zoo — Apache-2.0. Managed downloads are checksum-pinned; custom model paths remain user-owned.");
    ui.add_space(12.0);
    ui.strong("Session database");
    ui.label(app.db_path.display().to_string());
    ui.small(
        "Portable image indexes remain in each root's .imagesearch folder; central session state coordinates attached roots and derived People data.",
    );

    ui.add_space(12.0);
    ui.strong("Thumbnail cache");
    ui.label(format!(
        "Location: {}",
        app.thumb_pool.cache_dir().display()
    ));
    ui.label("Cached previews are generated at up to 512 px on background worker threads.");
    ui.small(format!(
        "GPU/UI thumbnail textures: {} / {} active (LRU bounded; disk cache survives eviction).",
        app.textures.len(),
        app.texture_lru.capacity()
    ));
    if ui.button("Clear thumbnail cache").clicked() {
        effects.clear_cache = true;
    }
}

fn show_worker_activity(app: &mut ImageSearchApp, ui: &mut egui::Ui) {
    if !(app.indexing || app.searching || app.progress.is_some()) {
        return;
    }

    ui.add_space(8.0);
    ui.group(|ui| {
        ui.horizontal(|ui| {
            if app.indexing && !app.index_paused {
                ui.spinner();
            }
            if app.index_paused {
                ui.strong("Indexing paused");
            } else if app.indexing {
                ui.strong("Indexing");
            } else if app.searching {
                ui.strong("Image search");
            }
            if app.indexing && app.index_control.is_some() {
                let label = if app.index_paused { "Resume" } else { "Pause" };
                if ui
                    .add_enabled(!app.searching, egui::Button::new(label).small())
                    .clicked()
                {
                    app.toggle_index_pause();
                }
            }
        });

        if let Some((done, total)) = app.progress.filter(|(_, total)| *total > 0) {
            ui.add(
                egui::ProgressBar::new(done as f32 / total as f32)
                    .desired_width(ui.available_width().min(620.0))
                    .text(format!("{done}/{total}")),
            );
        }
        if let Some(file_name) = &app.current_file {
            ui.small(format!("Current: {file_name}"));
        }
        ui.small(super::views::truncate_middle(&app.status, 96))
            .on_hover_text(&app.status);
    });
}

fn apply_effects(app: &mut ImageSearchApp, effects: Effects) {
    if effects.add_folder {
        app.add_folder();
    }
    if let Some(root) = effects.remove_folder {
        app.remove_folder(&root);
    }
    if effects.clear_cache {
        app.clear_thumbnail_cache();
    }

    if effects.save_performance_settings {
        app.indexing_settings = app.indexing_settings.sanitized();
        match settings::save(&app.settings_path, app.indexing_settings) {
            Ok(()) => {
                app.status = format!(
                    "Performance settings saved: decode {}, CLIP {} threads on {}, batch {}, max source {} MiB",
                    app.indexing_settings.decode_workers,
                    app.indexing_settings.clip_threads,
                    app.indexing_settings.clip_execution_provider.label(),
                    app.indexing_settings.batch_size,
                    app.indexing_settings.max_file_size_mib
                );
            }
            Err(err) => {
                app.last_error = Some(format!("Cannot save performance settings: {err:#}"));
            }
        }
    }

    if effects.save_face_settings {
        match face_settings::save(&app.face_settings_path, &app.face_embedding_settings) {
            Ok(()) => {
                app.status = if app.face_embedding_settings.configured() {
                    format!(
                        "SFace settings saved: {} on {}",
                        app.face_embedding_settings.model_path.display(),
                        app.face_embedding_settings.provider_label()
                    )
                } else {
                    "SFace settings cleared".to_owned()
                };
            }
            Err(err) => {
                app.last_error = Some(format!("Cannot save SFace settings: {err:#}"));
            }
        }
    }
}
