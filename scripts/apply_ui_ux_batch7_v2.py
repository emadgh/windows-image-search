from pathlib import Path


def once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


def replace_method(text: str, signature: str, replacement: str, label: str) -> str:
    start = text.find(signature)
    if start < 0:
        raise SystemExit(f"missing method: {label}")
    brace = text.find("{", start)
    if brace < 0:
        raise SystemExit(f"missing method brace: {label}")
    depth = 0
    i = brace
    while i < len(text):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[:start] + replacement + text[i + 1 :]
        i += 1
    raise SystemExit(f"unbalanced method: {label}")


# --- Main UI search-mode state -------------------------------------------------
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    "#[derive(Clone, Copy, PartialEq, Eq)]\npub(super) enum ViewMode {\n",
    "#[derive(Clone, Copy, PartialEq, Eq)]\npub(super) enum SearchMode {\n    Text,\n    SimilarImage,\n    Face,\n}\n\n#[derive(Clone, Copy, PartialEq, Eq)]\npub(super) enum ViewMode {\n",
    "SearchMode enum",
)
text = once(
    text,
    "    pub(super) search_text: String,\n",
    "    pub(super) search_mode: SearchMode,\n    pub(super) search_text: String,\n",
    "SearchMode field",
)
text = once(
    text,
    "            collections_open: false,\n            search_text: String::new(),\n",
    "            collections_open: false,\n            search_mode: SearchMode::Text,\n            search_text: String::new(),\n",
    "SearchMode constructor",
)
run_sig = "    fn run_similarity_search(&mut self, path: PathBuf) {"
run_start = text.find(run_sig)
if run_start < 0:
    raise SystemExit("missing run_similarity_search")
run_end = text.find("    pub(super) fn source", run_start)
if run_end < 0:
    raise SystemExit("missing source after run_similarity_search")
run_block = text[run_start:run_end]
run_block = once(
    run_block,
    "        self.clear_face_search_result_state();\n        self.searching = true;\n",
    "        self.clear_face_search_result_state();\n        self.search_mode = SearchMode::SimilarImage;\n        self.search_text.clear();\n        self.searching = true;\n",
    "similarity mode activation",
)
text = text[:run_start] + run_block + text[run_end:]
text = text.replace("        self.show_face_search_window(ctx);\n", "", 1)
path.write_text(text, encoding="utf-8")


# --- Search sidebar: one stable workspace, three query modes -------------------
Path("src/ui/search_panel.rs").write_text(
    r'''use super::{ImageSearchApp, SearchMode};
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
                                ui.small("Choose an image to find visually similar indexed results.");
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
                                                &mut self
                                                    .similarity_settings
                                                    .dominant_color_weight,
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
''',
    encoding="utf-8",
)


# --- Face Search: keep backend behavior, render its primary workflow inline -----
path = Path("src/ui/face_search_panel.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    "use super::ImageSearchApp;\n",
    "use super::{ImageSearchApp, SearchMode};\n",
    "face SearchMode import",
)
text = once(text, "    open: bool,\n", "", "face open field")
text = once(text, "            open: false,\n", "", "face open default")
text = replace_method(
    text,
    "    pub(super) fn open_face_search(&mut self)",
    '''    pub(super) fn open_face_search(&mut self) {
        if self.search_mode != SearchMode::Face {
            self.search_text.clear();
            self.similarity_results = None;
            self.query_image = None;
            self.clear_face_search_result_state();
            self.selected_paths.clear();
        }
        self.search_mode = SearchMode::Face;
        if self.face_search_ui.suggestions.is_empty() && !self.face_search_ui.loading {
            self.refresh_face_suggestions();
        }
    }''',
    "open_face_search",
)
text = once(
    text,
    "        self.face_search_ui.active = true;\n        self.face_search_ui.active_query = active_query;\n",
    "        self.search_mode = SearchMode::Face;\n        self.face_search_ui.active = true;\n        self.face_search_ui.active_query = active_query;\n",
    "face result mode",
)
inline_method = r'''    pub(super) fn show_face_search_sidebar(&mut self, ui: &mut egui::Ui) {
        if self.face_search_ui.suggestions.is_empty() && !self.face_search_ui.loading {
            self.refresh_face_suggestions();
        }

        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.face_search_ui.loading && !self.busy,
                    egui::Button::new("Refresh faces"),
                )
                .clicked()
            {
                self.refresh_face_suggestions();
            }
            if ui
                .add_enabled(
                    !self.face_search_ui.external_loading && !self.busy,
                    egui::Button::new("Face from file…"),
                )
                .clicked()
            {
                self.choose_external_face_file();
            }
        });

        if self.face_search_ui.loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.small("Reading indexed People / faces…");
            });
        }
        if self.face_search_ui.external_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.small("Detecting faces in query image…");
            });
        }

        egui::CollapsingHeader::new("Advanced face search")
            .default_open(false)
            .show(ui, |ui| {
                ui.label("Minimum similarity");
                ui.add(
                    egui::Slider::new(
                        &mut self.face_search_ui.options.min_similarity,
                        0.0..=1.0,
                    )
                    .fixed_decimals(2),
                );
                ui.horizontal(|ui| {
                    ui.label("Top-K");
                    ui.add(
                        egui::DragValue::new(&mut self.face_search_ui.options.limit)
                            .range(1..=5_000)
                            .speed(5.0),
                    );
                });
                self.face_search_ui.options = self.face_search_ui.options.sanitized();
            });

        if !self.face_search_ui.external_faces.is_empty() {
            ui.add_space(6.0);
            ui.strong("Faces from file");
            if let Some(path) = &self.face_search_ui.external_source {
                ui.small(super::views::truncate_middle(&path.display().to_string(), 38))
                    .on_hover_text(path.display().to_string());
            }
            let faces = self.face_search_ui.external_faces.clone();
            ui.horizontal_wrapped(|ui| {
                for face in faces {
                    let selected = self
                        .face_search_ui
                        .selected_external_ordinal
                        .is_some_and(|ordinal| ordinal == face.ordinal);
                    ui.vertical(|ui| {
                        let response = if let Some(texture) = self.thumbnail(&face.image_path) {
                            photo_grid::photo_tile(
                                ui,
                                &texture,
                                egui::vec2(70.0, 70.0),
                                PhotoTileMode::Face(face.bbox),
                                selected,
                                egui::Sense::click(),
                            )
                        } else {
                            photo_grid::loading_tile(
                                ui,
                                egui::vec2(70.0, 70.0),
                                selected,
                                egui::Sense::click(),
                            )
                        };
                        if response.clicked() {
                            self.face_search_ui.selected_external_ordinal = Some(face.ordinal);
                        }
                        if response.double_clicked() && !self.busy {
                            self.start_external_face_search(face.clone());
                        }
                        ui.small(format!("Face {}", face.ordinal + 1));
                    });
                }
            });
            let selected_external = self
                .face_search_ui
                .selected_external_ordinal
                .and_then(|ordinal| {
                    self.face_search_ui
                        .external_faces
                        .iter()
                        .find(|face| face.ordinal == ordinal)
                })
                .cloned();
            if ui
                .add_enabled(
                    selected_external.is_some() && !self.busy,
                    egui::Button::new("Search selected file face"),
                )
                .clicked()
            {
                if let Some(query) = selected_external {
                    self.start_external_face_search(query);
                }
            }
            ui.separator();
        }

        ui.strong("People / indexed faces");
        ui.add(
            egui::TextEdit::singleline(&mut self.face_search_ui.filter_text)
                .hint_text("Filter named people…")
                .desired_width(f32::INFINITY),
        );

        if self.face_search_ui.suggestions.is_empty() && !self.face_search_ui.loading {
            ui.small(
                "No searchable faces yet. Enable face detection for a Collection and configure YuNet + SFace in Settings.",
            );
            return;
        }

        let filter = self.face_search_ui.filter_text.trim().to_lowercase();
        let suggestions = self
            .face_search_ui
            .suggestions
            .iter()
            .filter(|face| {
                filter.is_empty()
                    || self
                        .face_search_ui
                        .suggestion_names
                        .get(&face.face_id)
                        .is_some_and(|name| name.to_lowercase().contains(&filter))
            })
            .cloned()
            .collect::<Vec<_>>();

        egui::ScrollArea::vertical()
            .id_salt("inline-face-search-suggestions")
            .max_height(260.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for face in suggestions {
                    let selected = self
                        .face_search_ui
                        .selected_face_id
                        .as_ref()
                        .is_some_and(|id| id == &face.face_id);
                    let mut clicked = false;
                    let mut double_clicked = false;
                    ui.horizontal(|ui| {
                        let response = if let Some(texture) = self.thumbnail(&face.image_path) {
                            photo_grid::photo_tile(
                                ui,
                                &texture,
                                egui::vec2(48.0, 48.0),
                                PhotoTileMode::Face(face.bbox),
                                selected,
                                egui::Sense::click(),
                            )
                        } else {
                            photo_grid::loading_tile(
                                ui,
                                egui::vec2(48.0, 48.0),
                                selected,
                                egui::Sense::click(),
                            )
                        };
                        clicked |= response.clicked();
                        double_clicked |= response.double_clicked();

                        ui.vertical(|ui| {
                            let title = self
                                .face_search_ui
                                .suggestion_names
                                .get(&face.face_id)
                                .cloned()
                                .unwrap_or_else(|| {
                                    face.image_path
                                        .file_name()
                                        .and_then(|name| name.to_str())
                                        .unwrap_or("Indexed face")
                                        .to_owned()
                                });
                            let response = ui.selectable_label(selected, truncate(&title, 24));
                            clicked |= response.clicked();
                            double_clicked |= response.double_clicked();
                            if let Some(group_size) = face.group_size {
                                ui.small(format!(
                                    "{group_size} indexed face{}",
                                    if group_size == 1 { "" } else { "s" }
                                ));
                            } else {
                                ui.small(format!("Confidence {:.0}%", face.confidence * 100.0));
                            }
                        });
                    });
                    if clicked {
                        self.face_search_ui.selected_face_id = Some(face.face_id.clone());
                    }
                    if double_clicked && !self.busy {
                        self.start_indexed_face_search(face.clone());
                    }
                }
            });

        let selected = self
            .face_search_ui
            .selected_face_id
            .as_ref()
            .and_then(|id| {
                self.face_search_ui
                    .suggestions
                    .iter()
                    .find(|face| &face.face_id == id)
            })
            .cloned();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    selected.is_some() && !self.busy,
                    egui::Button::new("Search selected person / face"),
                )
                .clicked()
            {
                if let Some(query) = selected {
                    self.start_indexed_face_search(query);
                }
            }
            if self.face_search_ui.searching {
                ui.spinner();
                ui.small("Comparing face embeddings…");
            }
        });
        if self.face_search_ui.active {
            ui.small(format!(
                "{} compatible face embedding{} inspected in the last search",
                self.face_search_ui.last_rows_considered,
                if self.face_search_ui.last_rows_considered == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        }
    }'''
text = replace_method(
    text,
    "    pub(super) fn show_face_search_window(&mut self, ctx: &egui::Context)",
    inline_method,
    "show_face_search_window",
)
path.write_text(text, encoding="utf-8")


# Keyboard shortcuts should not operate behind the first-class Collections workspace.
path = Path("src/ui/ux.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    "        if ctx.wants_keyboard_input() || self.settings_open || self.close_confirmation_open {\n",
    "        if ctx.wants_keyboard_input()\n            || self.settings_open\n            || self.collections_open\n            || self.close_confirmation_open\n        {\n",
    "shortcut workspace guard",
)
path.write_text(text, encoding="utf-8")
