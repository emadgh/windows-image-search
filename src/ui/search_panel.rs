use super::{theme, ImageSearchApp, SearchMode};
use eframe::egui;

impl ImageSearchApp {
    fn activate_search_mode(&mut self, mode: SearchMode) {
        if self.search_mode == mode {
            return;
        }
        self.cancel_visual_search();
        self.cancel_face_search();
        match mode {
            SearchMode::Text => {
                self.similarity_results = None;
                self.query_image = None;
                self.clear_face_search_result_state();
                self.selected_paths.clear();
            }
            SearchMode::SimilarImage => {
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
            SearchMode::Semantic => {
                self.similarity_results = None;
                self.query_image = None;
                self.clear_face_search_result_state();
            }
        }
        self.search_mode = mode;
    }

    pub(super) fn show_search_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("search_sidebar")
            .resizable(true)
            .default_width(theme::SEARCH_SIDEBAR_DEFAULT)
            .min_width(theme::SEARCH_SIDEBAR_MIN)
            .max_width(theme::SEARCH_SIDEBAR_MAX)
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
                    if ui.selectable_label(self.search_mode == SearchMode::Semantic, "Describe an image").clicked() {
                        self.activate_search_mode(SearchMode::Semantic);
                    }
                    ui.add_space(8.0);

                    match self.search_mode {
                        SearchMode::Semantic => {
                            ui.label("Describe what the image looks like");
                            ui.text_edit_multiline(&mut self.semantic_description);
                            ui.small("Uses the matching CLIP text model. First use downloads it; cached inference is local. English descriptions usually work best.");
                            if ui.add_enabled(self.can_run_similarity_search() && !self.semantic_description.trim().is_empty(), egui::Button::new("Find images")).clicked() {
                                self.run_description_search();
                            }
                            if self.searching && ui.button("Cancel search").clicked() { self.cancel_visual_search(); }
                        }
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
                            ui.collapsing("Search a region", |ui| {
                                ui.checkbox(&mut self.query_region_enabled, "Use selected rectangle");
                                if self.query_region_enabled {
                                    ui.add(egui::Slider::new(&mut self.query_region.x,0.0..=0.99).text("Left"));
                                    ui.add(egui::Slider::new(&mut self.query_region.y,0.0..=0.99).text("Top"));
                                    self.query_region.width = self.query_region.width.min(1.0-self.query_region.x);
                                    self.query_region.height = self.query_region.height.min(1.0-self.query_region.y);
                                    ui.add(egui::Slider::new(&mut self.query_region.width,0.01..=1.0-self.query_region.x).text("Width"));
                                    ui.add(egui::Slider::new(&mut self.query_region.height,0.01..=1.0-self.query_region.y).text("Height"));
                                    if let Some(path) = self.query_image.clone() {
                                        if let Some(texture) = self.thumbnail(&path) {
                                            let response = ui.add(egui::Image::new(&texture).max_size(egui::vec2(ui.available_width(), 200.0)));
                                            let region = self.query_region;
                                            let rectangle = egui::Rect::from_min_max(
                                                response.rect.min + response.rect.size() * egui::vec2(region.x, region.y),
                                                response.rect.min + response.rect.size() * egui::vec2(region.x + region.width, region.y + region.height));
                                            ui.painter().rect_stroke(rectangle, 0.0, egui::Stroke::new(2.0, egui::Color32::YELLOW), egui::StrokeKind::Inside);
                                        }
                                    }
                                    ui.small("Coordinates are fractions of the image. Re-run applies the crop without modifying the source.");
                                }
                            });
                            if self.searching && ui.button("Cancel search").clicked() {
                                self.cancel_visual_search();
                            }
                            ui.add_enabled_ui(!self.searching, |ui| {
                                let selected = crate::indexer::SimilarityPreset::matching(
                                    self.similarity_settings,
                                );
                                egui::ComboBox::from_id_salt("similarity_preset")
                                    .selected_text(selected.map_or("Custom", |preset| preset.label()))
                                    .show_ui(ui, |ui| {
                                        for preset in crate::indexer::SimilarityPreset::ALL {
                                            if ui
                                                .selectable_label(selected == Some(preset), preset.label())
                                                .clicked()
                                            {
                                                self.similarity_settings = preset.settings();
                                            }
                                        }
                                    });
                                ui.small("Presets change controls; re-run to update results. Possible duplicates require review.");
                            });
                            if let Some(reason) = self
                                .similarity_semantic_unavailable
                                .as_ref()
                                .filter(|_| self.similarity_results.is_some())
                            {
                                ui.group(|ui| {
                                    ui.strong("Semantic model unavailable");
                                    ui.label("These results use texture and color only.");
                                    ui.collapsing("Details", |ui| {
                                        ui.label(reason);
                                    });
                                });
                            }
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
                                ui.small("Search is available while indexing continues. Re-run to include newly indexed images.");
                            } else if self.indexing && self.index_paused {
                                ui.small("Indexing is paused; search uses committed images.");
                            }

                            if self.query_image.is_some() {
                                ui.add_space(8.0);
                                ui.small(format!(
                                    "Mix · Color {:.0}% · Texture {:.0}% · Semantic {:.0}% · Dominant {:.0}%{}",
                                    self.similarity_settings.color_distribution_weight,
                                    self.similarity_settings.texture_weight,
                                    self.similarity_settings.clip_weight,
                                    self.similarity_settings.dominant_color_weight,
                                    if self.similarity_settings.strict_color_rejection {
                                        " · strict color"
                                    } else {
                                        ""
                                    }
                                ));
                                ui.horizontal(|ui| {
                                    if ui
                                        .add_enabled(
                                            self.can_run_similarity_search(),
                                            egui::Button::new("Re-run search"),
                                        )
                                        .clicked()
                                    {
                                        self.rerun_similarity_search();
                                    }
                                });
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
                    if self.search_mode != SearchMode::Text {
                        ui.add(egui::TextEdit::singleline(&mut self.search_text).hint_text("Filename / tags / People name filter"));
                    }
                    if let Some(timing) = &self.similarity_timings {
                        ui.collapsing("Last search timing", |ui| {
                            ui.small(format!("Decode {:.0} ms · Inference {:.0} ms · Retrieval {:.0} ms · Ranking {:.0} ms · Total {:.0} ms", timing.decode_ms,timing.inference_ms,timing.retrieval_ms,timing.ranking_ms,timing.total_ms));
                            ui.small(format!("Result metadata {:.1} ms", timing.metadata_ms));
                            ui.small(format!("Descriptors: {} / {:.1} MiB retained (64 MiB budget)", if timing.descriptor_cache_hit { "cache hit" } else { "loaded" }, timing.descriptor_cache_bytes as f64 / (1024.0 * 1024.0)));
                        });
                    }
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
