use super::{ImageSearchApp, ThumbnailFit};
use crate::people_filter::{self, NamedPersonOption, PeopleFilterMode, ResolvedPeopleFilter};
use eframe::egui;
use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender};

#[derive(Debug)]
enum PeopleFilterMessage {
    Catalog {
        generation: u64,
        result: Result<Vec<NamedPersonOption>, String>,
    },
    Resolved {
        generation: u64,
        result: Result<ResolvedPeopleFilter, String>,
    },
}

pub(super) struct PeopleFilterUiState {
    options: Vec<NamedPersonOption>,
    selected_person_ids: BTreeSet<String>,
    mode: PeopleFilterMode,
    resolved: ResolvedPeopleFilter,
    name_query: String,
    catalog_generation: u64,
    resolve_generation: u64,
    catalog_loaded: bool,
    catalog_loading: bool,
    resolving: bool,
    tx: Sender<PeopleFilterMessage>,
    rx: Receiver<PeopleFilterMessage>,
}

impl Default for PeopleFilterUiState {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            options: Vec::new(),
            selected_person_ids: BTreeSet::new(),
            mode: PeopleFilterMode::Any,
            resolved: ResolvedPeopleFilter::default(),
            name_query: String::new(),
            catalog_generation: 0,
            resolve_generation: 0,
            catalog_loaded: false,
            catalog_loading: false,
            resolving: false,
            tx,
            rx,
        }
    }
}

impl ImageSearchApp {
    pub(super) fn process_people_filter_messages(&mut self) {
        while let Ok(message) = self.people_filter_ui.rx.try_recv() {
            match message {
                PeopleFilterMessage::Catalog { generation, result } => {
                    if generation != self.people_filter_ui.catalog_generation {
                        continue;
                    }
                    self.people_filter_ui.catalog_loading = false;
                    self.people_filter_ui.catalog_loaded = true;
                    match result {
                        Ok(options) => {
                            let valid_ids = options
                                .iter()
                                .map(|person| person.person_id.as_str())
                                .collect::<HashSet<_>>();
                            self.people_filter_ui
                                .selected_person_ids
                                .retain(|id| valid_ids.contains(id.as_str()));
                            self.people_filter_ui.options = options;
                            self.request_people_filter_resolution();
                        }
                        Err(error) => {
                            self.last_error = Some(format!(
                                "Cannot load named People for search filters: {error}"
                            ));
                        }
                    }
                }
                PeopleFilterMessage::Resolved { generation, result } => {
                    if generation != self.people_filter_ui.resolve_generation {
                        continue;
                    }
                    self.people_filter_ui.resolving = false;
                    match result {
                        Ok(resolved) => {
                            self.people_filter_ui.selected_person_ids = resolved
                                .selected_person_ids
                                .iter()
                                .cloned()
                                .collect::<BTreeSet<_>>();
                            self.people_filter_ui.resolved = resolved;
                        }
                        Err(error) => {
                            self.last_error = Some(format!(
                                "Cannot resolve People filter against portable indexes: {error}"
                            ));
                        }
                    }
                }
            }
        }
    }

    pub(super) fn refresh_people_filter_catalog(&mut self) {
        self.people_filter_ui.catalog_generation =
            self.people_filter_ui.catalog_generation.wrapping_add(1);
        let generation = self.people_filter_ui.catalog_generation;
        self.people_filter_ui.catalog_loading = true;
        let db_path = self.db_path.clone();
        let roots = self.roots.clone();
        let tx = self.people_filter_ui.tx.clone();
        std::thread::spawn(move || {
            let result = people_filter::load_named_people(&db_path, &roots)
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(PeopleFilterMessage::Catalog { generation, result });
        });
    }

    fn ensure_people_filter_catalog(&mut self) {
        if !self.people_filter_ui.catalog_loaded && !self.people_filter_ui.catalog_loading {
            self.refresh_people_filter_catalog();
        }
    }

    fn request_people_filter_resolution(&mut self) {
        self.people_filter_ui.resolve_generation =
            self.people_filter_ui.resolve_generation.wrapping_add(1);
        let generation = self.people_filter_ui.resolve_generation;
        let selected = self
            .people_filter_ui
            .selected_person_ids
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mode = self.people_filter_ui.mode;
        if selected.is_empty() {
            self.people_filter_ui.resolving = false;
            self.people_filter_ui.resolved = ResolvedPeopleFilter {
                mode,
                ..ResolvedPeopleFilter::default()
            };
            return;
        }

        self.people_filter_ui.resolving = true;
        let db_path = self.db_path.clone();
        let roots = self.roots.clone();
        let tx = self.people_filter_ui.tx.clone();
        std::thread::spawn(move || {
            let result = people_filter::resolve_filter(&db_path, &roots, &selected, mode)
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(PeopleFilterMessage::Resolved { generation, result });
        });
    }

    pub(super) fn people_filter_matches(&self, path: &Path) -> bool {
        self.people_filter_ui.resolved.matches(path)
    }

    pub(super) fn people_filter_selected_count(&self) -> usize {
        self.people_filter_ui.selected_person_ids.len()
    }

    pub(super) fn clear_people_filter(&mut self) {
        if self.people_filter_ui.selected_person_ids.is_empty() {
            return;
        }
        self.people_filter_ui.selected_person_ids.clear();
        self.request_people_filter_resolution();
    }

    pub(super) fn people_filter_work_pending(&self) -> bool {
        self.people_filter_ui.catalog_loading || self.people_filter_ui.resolving
    }

    pub(super) fn show_people_filter(&mut self, ui: &mut egui::Ui) {
        self.ensure_people_filter_catalog();

        ui.add_space(8.0);
        ui.separator();
        let selected_count = self.people_filter_ui.selected_person_ids.len();
        egui::CollapsingHeader::new(if selected_count == 0 {
            "People filter".to_owned()
        } else {
            format!("People filter ({selected_count} selected)")
        })
        .default_open(selected_count > 0)
        .show(ui, |ui| {
            ui.small("Combines with collection, text, color and image-search results.");
            ui.horizontal(|ui| {
                if ui.button("Manage People…").clicked() {
                    self.open_people_manager();
                }
                if ui
                    .add_enabled(selected_count > 0, egui::Button::new("Clear"))
                    .clicked()
                {
                    self.people_filter_ui.selected_person_ids.clear();
                    self.request_people_filter_resolution();
                }
                if ui.small_button("⟳").on_hover_text("Refresh named People").clicked() {
                    self.refresh_people_filter_catalog();
                }
            });

            if self.people_filter_ui.catalog_loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.small("Loading named People…");
                });
            }

            if self.people_filter_ui.catalog_loaded && self.people_filter_ui.options.is_empty() {
                ui.small("No named People yet. Name a group in People Manager to make it available here.");
                return;
            }

            if !self.people_filter_ui.options.is_empty() {
                ui.add(
                    egui::TextEdit::singleline(&mut self.people_filter_ui.name_query)
                        .hint_text("Filter people by name…")
                        .desired_width(f32::INFINITY),
                );
                let query = self.people_filter_ui.name_query.trim().to_lowercase();
                let options = self
                    .people_filter_ui
                    .options
                    .iter()
                    .filter(|person| {
                        query.is_empty() || person.display_name.to_lowercase().contains(&query)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let selected_people = self
                    .people_filter_ui
                    .options
                    .iter()
                    .filter(|person| {
                        self.people_filter_ui
                            .selected_person_ids
                            .contains(&person.person_id)
                    })
                    .map(|person| (person.person_id.clone(), person.display_name.clone()))
                    .collect::<Vec<_>>();
                let mut selection_changed = false;
                if !selected_people.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for (person_id, display_name) in selected_people {
                            if ui.small_button(format!("{display_name}  ×")).clicked() {
                                self.people_filter_ui.selected_person_ids.remove(&person_id);
                                selection_changed = true;
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
                egui::ScrollArea::vertical()
                    .id_salt("people-filter-options")
                    .max_height(230.0)
                    .show(ui, |ui| {
                        for person in options {
                            let selected = self
                                .people_filter_ui
                                .selected_person_ids
                                .contains(&person.person_id);
                            let mut clicked = false;
                            ui.horizontal(|ui| {
                                let avatar_size = egui::vec2(36.0, 36.0);
                                if let Some(image_path) = person.representative_image.as_ref() {
                                    if let Some(texture) = self.thumbnail(image_path) {
                                        let response = super::views::thumbnail_widget(
                                            ui,
                                            &texture,
                                            avatar_size,
                                            ThumbnailFit::Cover,
                                            selected,
                                            egui::Sense::click(),
                                        );
                                        clicked |= response.clicked();
                                    } else {
                                        ui.add_sized(avatar_size, egui::Spinner::new());
                                    }
                                } else {
                                    let initial = person
                                        .display_name
                                        .chars()
                                        .next()
                                        .map(|value| value.to_uppercase().to_string())
                                        .unwrap_or_else(|| "?".to_owned());
                                    let (rect, response) =
                                        ui.allocate_exact_size(avatar_size, egui::Sense::click());
                                    ui.painter().circle_filled(
                                        rect.center(),
                                        18.0,
                                        ui.visuals().widgets.inactive.bg_fill,
                                    );
                                    ui.painter().text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        initial,
                                        egui::FontId::proportional(14.0),
                                        ui.visuals().text_color(),
                                    );
                                    clicked |= response.clicked();
                                }

                                let label = format!(
                                    "{}  ·  {} face{}",
                                    person.display_name,
                                    person.member_count,
                                    if person.member_count == 1 { "" } else { "s" }
                                );
                                clicked |= ui.selectable_label(selected, label).clicked();
                            });
                            if clicked {
                                selection_changed = true;
                                if selected {
                                    self.people_filter_ui
                                        .selected_person_ids
                                        .remove(&person.person_id);
                                } else {
                                    self.people_filter_ui
                                        .selected_person_ids
                                        .insert(person.person_id);
                                }
                            }
                        }
                    });

                if self.people_filter_ui.selected_person_ids.len() > 1 {
                    ui.horizontal(|ui| {
                        ui.label("Match");
                        let any_changed = ui
                            .radio_value(
                                &mut self.people_filter_ui.mode,
                                PeopleFilterMode::Any,
                                "ANY",
                            )
                            .on_hover_text("Image contains at least one selected Person")
                            .changed();
                        let all_changed = ui
                            .radio_value(
                                &mut self.people_filter_ui.mode,
                                PeopleFilterMode::All,
                                "ALL",
                            )
                            .on_hover_text("Image contains every selected Person")
                            .changed();
                        selection_changed |= any_changed || all_changed;
                    });
                }

                if selection_changed {
                    self.request_people_filter_resolution();
                }
            }

            if self.people_filter_ui.resolving {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.small("Updating People filter…");
                });
            } else if self.people_filter_ui.resolved.active() {
                ui.small(format!(
                    "{} matching image{} · {}",
                    self.people_filter_ui.resolved.matching_images.len(),
                    if self.people_filter_ui.resolved.matching_images.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    self.people_filter_ui.resolved.mode.label()
                ));
                if self.people_filter_ui.resolved.unavailable_faces > 0 {
                    ui.small(format!(
                        "{} selected face reference{} currently unavailable in portable indexes",
                        self.people_filter_ui.resolved.unavailable_faces,
                        if self.people_filter_ui.resolved.unavailable_faces == 1 {
                            " is"
                        } else {
                            "s are"
                        }
                    ));
                }
            }
        });
    }
}
