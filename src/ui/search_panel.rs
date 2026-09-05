use super::{ImageSearchApp, SearchMode};
use eframe::egui;

impl ImageSearchApp {
    fn activate_search_mode(&mut self, mode: SearchMode) {
        if self.search_mode == mode {
            return;
        }
        match mode {
            SearchMode::Text => {
                self.similarity_results = None;
                self.query_image = None;
                self.clear_face_search_result_state();
                self.selected_paths.clear();
            }
            SearchMode::SimilarImage => {
                self.search_text.clear();
                if self.face_search_active() {
                    self.similarity_results = None;
                    self.query_image = None;
                    self.clear_face_search_result_state();
                    self.selected_paths.clear();
                }
            }
            SearchMode::Face => {
                self.open_face_search();
                return;
            }
        }
        self.search_mode = mode;
    }

    pub(super) fn show_search_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("search_sidebar")
            .resizable(true)
            .default_width(310.0)
            .min_width(270.0)
            .max_width(430.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Search");
                    ui.small("One workspace for text, visual similarity and face identity search.");
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(self.search_mode == SearchMode::Text, "Text")
                            .clicked()
                        {
                            self.activate_search_mode(SearchMode::Text);
                        }
                        if ui
                            .selectable_label(
                                self.search_mode == SearchMode::SimilarImage,
                                "Similar Image",
                            )
                            .clicked()
                        {
                            self.activate_search_mode(SearchMode::SimilarImage);
                        }
                        if ui
                            .selectable_label(self.search_mode == SearchMode::Face, "Face")
                            .clicked()
                        {
                            self.activate_search_mode(SearchMode::Face);
                        }
                    });
                    ui.add_space(8.0);

                    match self.search_mode {
                        SearchMode::Text => {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.search_text)
                                    .hint_text("Search filename, path, description, keywords…")
                                    .desired_width(f32::INFINITY),
                            );
                            if self.text_search_pending {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.small("Searching indexed text…");
                                });
                            }
                        }
                        SearchMode::SimilarImage => {
                            if let Some(query) = self.query_image.clone() {
                                ui.group(|ui| {
                                    ui.strong("Similarity query");
                                    ui.add_space(4.0);
                                    if let Some(texture) = self.thumbnail(&query) {
                                        super::views::show_query_preview(ui, &texture, 220.0);
                                    } else {
                                        ui.add_sized(
                                            [220.0, 140.0],
                                            egui::Label::new("Loading preview…"),
                                        );
                                    }
                                    ui.small(super::views::truncate_middle(
                                        &query.display().to_string(),
                                        42,
                                    ))
                                    .on_hover_text(query.display().to_string());
                                    ui.horizontal_wrapped(|ui| {
                                        if ui
                                            .add_enabled(
                                                self.can_run_similarity_search(),
                                                egui::Button::new("Change image"),
                                            )
                                            .clicked()
                                        {
                                            self.choose_similarity_image();
                                        }
                                        if ui.button("Clear query").clicked() {
                                            self.similarity_results = None;
                                            self.query_image = None;
                                            self.selected_paths.clear();
                                        }
                                    });
                                });
                            } else {
                                if ui
                                    .add_enabled(
                                        self.can_run_similarity_search(),
                                        egui::Button::new("Choose query image…"),
                                    )
                                    .clicked()
                                {
                                    self.choose_similarity_image();
                                }
                                ui.small(
                                    "Choose an image to find visually similar indexed results.",
                                );
                            }

                            if self.indexing && !self.index_paused {
                                ui.small("Pause indexing to search committed images.");
                            } else if self.indexing && self.index_paused {
                                ui.small("Indexing is paused; search uses committed images.");
                            }

                            if self.query_image.is_some() {
                                ui.add_space(8.0);
                                egui::CollapsingHeader::new("Advanced similarity")
                                    .default_open(false)
                                    .show(ui, |ui| {
                                        ui.small("Weights are normalized automatically.");
                                        ui.add(
                                            egui::Slider::new(
                                                &mut self
                                                    .similarity_settings
                                                    .color_distribution_weight,
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
                                            "Reject strong color mismatches",
                                        );
                                        if self.similarity_settings.strict_color_rejection {
                                            ui.add(
                                                egui::Slider::new(
                                                    &mut self
                                                        .similarity_settings
                                                        .min_color_distribution_match,
                                                    0.0..=100.0,
                                                )
                                                .text("Min color match")
                                                .suffix("%"),
                                            );
                                            ui.add(
                                                egui::Slider::new(
                                                    &mut self
                                                        .similarity_settings
                                                        .max_dominant_color_difference,
                                                    5.0..=100.0,
                                                )
                                                .text("Max color difference")
                                                .suffix("%"),
                                            );
                                        }
                                        ui.horizontal_wrapped(|ui| {
                                            if ui.button("Reset weights").clicked() {
                                                self.similarity_settings =
                                                    crate::indexer::SimilaritySettings::default();
                                            }
                                            if ui
                                                .add_enabled(
                                                    self.can_run_similarity_search(),
                                                    egui::Button::new("Apply / re-run"),
                                                )
                                                .clicked()
                                            {
                                                self.rerun_similarity_search();
                                            }
                                        });
                                    });
                            }
                        }
                        SearchMode::Face => {
                            self.show_face_search_sidebar(ui);
                        }
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.strong("Filters");
                    self.show_collection_filter(ui);
                    self.show_people_filter(ui);

                    egui::CollapsingHeader::new(if self.color_enabled {
                        "Color filter · active"
                    } else {
                        "Color filter"
                    })
                    .default_open(self.color_enabled)
                    .show(ui, |ui| {
                        ui.checkbox(&mut self.color_enabled, "Filter by dominant color");
                        if self.color_enabled {
                            ui.horizontal(|ui| {
                                ui.color_edit_button_srgb(&mut self.target_color);
                                ui.add(
                                    egui::Slider::new(&mut self.color_tolerance, 0.03..=0.70)
                                        .text("Tolerance"),
                                );
                            });
                        }
                    });
                });
            });
    }
}
