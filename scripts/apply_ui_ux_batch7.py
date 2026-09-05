from pathlib import Path


def once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


def replace_between(text: str, start: str, end: str, replacement: str, label: str) -> str:
    a = text.find(start)
    if a < 0:
        raise SystemExit(f"missing start target: {label}")
    b = text.find(end, a)
    if b < 0:
        raise SystemExit(f"missing end target: {label}")
    return text[:a] + replacement + text[b:]


# Search mode state lives with the main UI state.
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    """#[derive(Clone, Copy, PartialEq, Eq)]\npub(super) enum ViewMode {\n""",
    """#[derive(Clone, Copy, PartialEq, Eq)]\npub(super) enum SearchMode {\n    Text,\n    SimilarImage,\n    Face,\n}\n\n#[derive(Clone, Copy, PartialEq, Eq)]\npub(super) enum ViewMode {\n""",
    "search mode enum",
)
text = once(
    text,
    """    pub(super) search_text: String,\n    text_search_service: TextSearchService,\n""",
    """    pub(super) search_mode: SearchMode,\n    pub(super) search_text: String,\n    text_search_service: TextSearchService,\n""",
    "search mode field",
)
text = once(
    text,
    """            search_text: String::new(),\n            text_search_service: TextSearchService::new(db_path.clone()),\n""",
    """            search_mode: SearchMode::Text,\n            search_text: String::new(),\n            text_search_service: TextSearchService::new(db_path.clone()),\n""",
    "search mode default",
)
text = once(
    text,
    """        self.clear_face_search_result_state();\n        self.searching = true;\n""",
    """        self.clear_face_search_result_state();\n        self.search_mode = SearchMode::SimilarImage;\n        self.searching = true;\n""",
    "similarity activates mode",
)
text = text.replace("        self.show_face_search_window(ctx);\n", "", 1)
path.write_text(text, encoding="utf-8")


# Primary Search sidebar becomes a real mode selector; common filters stay stable.
path = Path("src/ui/search_panel.rs")
text = path.read_text(encoding="utf-8")
text = once(text, "use super::ImageSearchApp;\n", "use super::{ImageSearchApp, SearchMode};\n", "search panel mode import")
start = "                    ui.add_space(8.0);\n\n                    ui.add(\n                        egui::TextEdit::singleline(&mut self.search_text)"
end = "                    if self.indexing && !self.index_paused {\n"
replacement = '''                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let text_clicked = ui
                            .selectable_label(self.search_mode == SearchMode::Text, "Text")
                            .clicked();
                        let image_clicked = ui
                            .selectable_label(
                                self.search_mode == SearchMode::SimilarImage,
                                "Similar Image",
                            )
                            .clicked();
                        let face_clicked = ui
                            .selectable_label(self.search_mode == SearchMode::Face, "Face")
                            .clicked();
                        if text_clicked {
                            self.activate_search_mode(SearchMode::Text);
                        }
                        if image_clicked {
                            self.activate_search_mode(SearchMode::SimilarImage);
                        }
                        if face_clicked {
                            self.open_face_search();
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
                            if self.query_image.is_none() || self.face_search_active() {
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
                        }
                        SearchMode::Face => {
                            self.show_face_search_sidebar(ui);
                        }
                    }

'''
text = replace_between(text, start, end, replacement, "mode selector block")
# Do not show the generic query card for Face mode; its compact query UI is inline.
text = once(
    text,
    """                    if let Some(query) = self.query_image.clone() {\n""",
    """                    if self.search_mode != SearchMode::Face {\n                        if let Some(query) = self.query_image.clone() {\n""",
    "query card face guard",
)
# Close the new guard immediately after the existing query-card block.
needle = """                        });\n                    }\n\n                    ui.add_space(10.0);\n                    ui.separator();\n                    ui.strong(\"Filters\");\n"""
replacement2 = """                        });\n                        }\n                    }\n\n                    ui.add_space(10.0);\n                    ui.separator();\n                    ui.strong(\"Filters\");\n"""
text = once(text, needle, replacement2, "query guard close")
# Advanced similarity is tied to the Similar Image mode, not inferred from Face state.
text = once(
    text,
    """                    if self.query_image.is_some() && !self.face_search_active() {\n""",
    """                    if self.search_mode == SearchMode::SimilarImage && self.query_image.is_some() {\n""",
    "advanced similarity mode guard",
)
# Add coherent mode transition helper before show_search_sidebar closes impl.
insert_marker = """    pub(super) fn show_search_sidebar(&mut self, ctx: &egui::Context) {\n"""
# Add helper before the public renderer to avoid disturbing its body.
helper = '''    fn activate_search_mode(&mut self, mode: SearchMode) {
        if self.search_mode == mode {
            return;
        }
        self.search_mode = mode;
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
            SearchMode::Face => {}
        }
    }

'''
text = once(text, insert_marker, helper + insert_marker, "search mode transition helper")
path.write_text(text, encoding="utf-8")


# Face Search is rendered inline inside Search mode instead of opening another window.
path = Path("src/ui/face_search_panel.rs")
text = path.read_text(encoding="utf-8")
text = once(text, "use super::ImageSearchApp;\n", "use super::{ImageSearchApp, SearchMode};\n", "face panel mode import")
text = once(text, "    open: bool,\n", "", "remove face window state")
text = once(text, "            open: false,\n", "", "remove face window default")
old_open = '''    pub(super) fn open_face_search(&mut self) {
        self.face_search_ui.open = true;
        if self.face_search_ui.suggestions.is_empty() && !self.face_search_ui.loading {
            self.refresh_face_suggestions();
        }
    }
'''
new_open = '''    pub(super) fn open_face_search(&mut self) {
        if self.search_mode != SearchMode::Face {
            self.similarity_results = None;
            self.query_image = None;
            self.clear_face_search_result_state();
            self.selected_paths.clear();
        }
        self.search_mode = SearchMode::Face;
        if self.face_search_ui.suggestions.is_empty() && !self.face_search_ui.loading {
            self.refresh_face_suggestions();
        }
    }
'''
text = once(text, old_open, new_open, "inline face activation")
# Applying Face results keeps the mode explicit.
text = once(
    text,
    """        self.face_search_ui.active = true;\n        self.face_search_ui.active_query = active_query;\n""",
    """        self.search_mode = SearchMode::Face;\n        self.face_search_ui.active = true;\n        self.face_search_ui.active_query = active_query;\n""",
    "face result activates mode",
)
start = "    pub(super) fn show_face_search_window(&mut self, ctx: &egui::Context) {\n"
end = "}\n\nfn prepare_external_faces(\n"
inline = r'''    pub(super) fn show_face_search_sidebar(&mut self, ui: &mut egui::Ui) {
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
            ui.small("No searchable faces yet. Enable face detection for a Collection and configure YuNet + SFace in Settings.");
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
                            let label = ui.selectable_label(selected, truncate(&title, 24));
                            clicked |= label.clicked();
                            double_clicked |= label.double_clicked();
                            if let Some(group_size) = face.group_size {
                                ui.small(format!("{group_size} indexed face{}", if group_size == 1 { "" } else { "s" }));
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
                if self.face_search_ui.last_rows_considered == 1 { "" } else { "s" }
            ));
        }
    }
}

'''
text = replace_between(text, start, end, inline, "replace face search window with inline sidebar")
path.write_text(text, encoding="utf-8")
