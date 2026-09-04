use super::ImageSearchApp;
use eframe::egui;
use std::path::PathBuf;

impl ImageSearchApp {
    pub(super) fn handle_result_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() || self.settings_open || self.close_confirmation_open {
            return;
        }

        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.selected_paths.clear();
        }

        if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::A)) {
            let source = self.source();
            let paths = self
                .visible_indices()
                .into_iter()
                .filter_map(|index| source.get(index).map(|record| record.path.clone()))
                .collect::<Vec<_>>();
            self.selected_paths.clear();
            self.selected_paths.extend(paths);
        }

        if ctx.input(|input| input.key_pressed(egui::Key::Enter)) {
            if let Some(path) = self.selected_path() {
                let _ = open::that(path);
            }
        }
    }

    pub(super) fn show_error_banner(&mut self, ctx: &egui::Context) {
        let Some(error) = self.last_error.clone() else {
            return;
        };
        egui::TopBottomPanel::top("global-error-banner").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Something needs attention");
                ui.label(super::views::truncate_middle(&error, 140))
                    .on_hover_text(&error);
                if ui.button("Dismiss").clicked() {
                    self.last_error = None;
                }
            });
        });
    }

    pub(super) fn show_active_filter_chips(&mut self, ui: &mut egui::Ui) {
        let collection = self.collection_filter_chip();
        let people_count = self.people_filter_selected_count();
        let text = self.search_text.trim().to_owned();
        let color_active = self.color_enabled;
        let query = self.query_image.as_ref().map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("query image")
                .to_owned()
        });
        let face_active = self.face_search_active();
        let any_active = collection.is_some()
            || people_count > 0
            || !text.is_empty()
            || color_active
            || query.is_some();
        if !any_active {
            return;
        }

        ui.horizontal_wrapped(|ui| {
            ui.small("Active:");
            if let Some(label) = collection {
                if ui.small_button(format!("{label}  ×")).clicked() {
                    self.clear_collection_filter();
                }
            }
            if people_count > 0
                && ui
                    .small_button(format!("People: {people_count}  ×"))
                    .clicked()
            {
                self.clear_people_filter();
            }
            if !text.is_empty()
                && ui
                    .small_button(format!(
                        "Text: {}  ×",
                        super::views::truncate_middle(&text, 28)
                    ))
                    .clicked()
            {
                self.search_text.clear();
            }
            if color_active {
                let [r, g, b] = self.target_color;
                if ui
                    .small_button(format!("Color #{r:02X}{g:02X}{b:02X}  ×"))
                    .clicked()
                {
                    self.color_enabled = false;
                }
            }
            if let Some(query) = query {
                let prefix = if face_active { "Face" } else { "Similar" };
                if ui
                    .small_button(format!(
                        "{prefix}: {}  ×",
                        super::views::truncate_middle(&query, 24)
                    ))
                    .clicked()
                {
                    self.similarity_results = None;
                    self.query_image = None;
                    self.clear_face_search_result_state();
                }
            }
            ui.separator();
            if ui.small_button("Clear all").clicked() {
                self.clear_all_search_constraints();
            }
        });
        ui.add_space(4.0);
    }

    pub(super) fn show_selection_bar(&mut self, ui: &mut egui::Ui) {
        if self.selected_paths.is_empty() {
            return;
        }
        let count = self.selected_paths.len();
        let single = (count == 1)
            .then(|| self.selected_paths.iter().next().cloned())
            .flatten();
        let all_paths = self
            .selected_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();

        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!(
                    "{count} selected{}",
                    if count == 1 { "" } else { " images" }
                ));
                if ui
                    .add_enabled(single.is_some(), egui::Button::new("Open"))
                    .clicked()
                {
                    if let Some(path) = &single {
                        let _ = open::that(path);
                    }
                }
                if ui
                    .add_enabled(single.is_some(), egui::Button::new("Open folder"))
                    .clicked()
                {
                    if let Some(parent) = single.as_ref().and_then(|path| path.parent()) {
                        let _ = open::that(parent);
                    }
                }
                if ui.button("Copy path(s)").clicked() {
                    ui.ctx().copy_text(all_paths.join("\n"));
                }
                if ui.button("Clear selection").clicked() {
                    self.selected_paths.clear();
                }
            });
        });
        ui.add_space(5.0);
    }

    pub(super) fn show_empty_state(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.14).min(90.0));
            if self.images.is_empty() {
                ui.heading("Build your image library");
                ui.add_space(4.0);
                ui.label(
                    "Add a folder to create or reuse its portable local image index. Your source files stay where they are.",
                );
                ui.add_space(12.0);
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Add folder"))
                    .clicked()
                {
                    self.prompt_add_library_folder();
                }
                if !self.roots.is_empty()
                    && ui
                        .add_enabled(!self.busy, egui::Button::new("Rescan library"))
                        .clicked()
                {
                    self.start_rescan();
                }
                ui.add_space(8.0);
                ui.small("Existing .imagesearch indexes are attached and reused automatically.");
            } else {
                ui.heading("No matching images");
                ui.add_space(4.0);
                ui.label("Try removing one or more active filters or search constraints.");
                ui.add_space(10.0);
                if ui.button("Clear all filters").clicked() {
                    self.clear_all_search_constraints();
                }
            }
        });
    }

    fn clear_all_search_constraints(&mut self) {
        self.clear_collection_filter();
        self.clear_people_filter();
        self.search_text.clear();
        self.color_enabled = false;
        self.similarity_results = None;
        self.query_image = None;
        self.clear_face_search_result_state();
        self.selected_paths.clear();
    }

    pub(super) fn selected_path(&self) -> Option<PathBuf> {
        (self.selected_paths.len() == 1)
            .then(|| self.selected_paths.iter().next().cloned())
            .flatten()
    }
}
