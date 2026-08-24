use super::photo_grid::{self, PhotoGridSpec, PhotoTileMode};
use super::ImageSearchApp;
use crate::face_detection::FaceBox;
use crate::face_search::{self, IndexedFaceSuggestion};
use crate::people_effective::{self, EffectivePeopleCatalog, EffectivePersonSource};
use crate::people_management;
use eframe::egui;
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Default)]
pub(super) struct PeopleManagerUiState {
    open: bool,
    catalog: EffectivePeopleCatalog,
    selected_person_id: Option<String>,
    merge_selection: BTreeSet<String>,
    selected_face: Option<(String, String)>,
    rename_text: String,
    split_name: String,
    merge_name: String,
    move_target: Option<String>,
    preview_cache: HashMap<(String, String), Option<IndexedFaceSuggestion>>,
    confirm_delete_person: Option<String>,
}

#[derive(Clone, Debug)]
enum PeopleAction {
    Refresh,
    Rename {
        person_id: String,
        name: String,
    },
    Merge {
        person_ids: Vec<String>,
        name: Option<String>,
    },
    SplitToNew {
        library_id: String,
        face_id: String,
        name: String,
    },
    MoveFace {
        library_id: String,
        face_id: String,
        manual_person_id: String,
    },
    Detach {
        library_id: String,
        face_id: String,
    },
    Ignore {
        library_id: String,
        face_id: String,
    },
    Restore {
        library_id: String,
        face_id: String,
    },
    SetRepresentative {
        person_id: String,
        library_id: String,
        face_id: String,
    },
    DeleteManualPerson {
        person_id: String,
    },
    ShowImages {
        person_id: String,
    },
}

impl ImageSearchApp {
    pub(super) fn open_people_manager(&mut self) {
        self.people_manager_ui.open = true;
        self.refresh_people_manager();
    }

    fn refresh_people_manager(&mut self) {
        match crate::db::open(&self.db_path).and_then(|conn| people_effective::load(&conn)) {
            Ok(catalog) => {
                self.people_manager_ui.catalog = catalog;
                self.people_manager_ui.preview_cache.clear();
                let selected_exists = self
                    .people_manager_ui
                    .selected_person_id
                    .as_ref()
                    .is_some_and(|id| {
                        self.people_manager_ui
                            .catalog
                            .people
                            .iter()
                            .any(|person| &person.person_id == id)
                    });
                if !selected_exists {
                    self.people_manager_ui.selected_person_id = self
                        .people_manager_ui
                        .catalog
                        .people
                        .first()
                        .map(|person| person.person_id.clone());
                }
                let valid_person_ids = self
                    .people_manager_ui
                    .catalog
                    .people
                    .iter()
                    .map(|person| person.person_id.clone())
                    .collect::<HashSet<_>>();
                self.people_manager_ui
                    .merge_selection
                    .retain(|id| valid_person_ids.contains(id));
                self.sync_people_editor_fields();
            }
            Err(err) => {
                self.last_error = Some(format!("Cannot load People catalog: {err:#}"));
            }
        }
    }

    fn sync_people_editor_fields(&mut self) {
        let selected = self
            .people_manager_ui
            .selected_person_id
            .as_ref()
            .and_then(|id| {
                self.people_manager_ui
                    .catalog
                    .people
                    .iter()
                    .find(|person| &person.person_id == id)
            });
        self.people_manager_ui.rename_text = selected
            .and_then(|person| person.display_name.clone())
            .unwrap_or_default();
        self.people_manager_ui.selected_face = None;
        self.people_manager_ui.move_target = None;
    }

    fn people_preview(&mut self, library_id: &str, face_id: &str) -> Option<IndexedFaceSuggestion> {
        let key = (library_id.to_owned(), face_id.to_owned());
        if let Some(value) = self.people_manager_ui.preview_cache.get(&key) {
            return value.clone();
        }
        let value = face_search::resolve_searchable_face(&self.roots, library_id, face_id)
            .ok()
            .flatten();
        self.people_manager_ui
            .preview_cache
            .insert(key, value.clone());
        value
    }

    fn apply_people_action(&mut self, action: PeopleAction) {
        let result = (|| -> anyhow::Result<()> {
            match action {
                PeopleAction::Refresh => {}
                PeopleAction::Rename { person_id, name } => {
                    let conn = crate::db::open(&self.db_path)?;
                    let id = people_management::rename_effective_person(&conn, &person_id, &name)?;
                    self.people_manager_ui.selected_person_id = Some(id);
                    self.status = "Person name saved".to_owned();
                }
                PeopleAction::Merge { person_ids, name } => {
                    let mut conn = crate::db::open(&self.db_path)?;
                    let id = people_management::merge_effective_people(
                        &mut conn,
                        &person_ids,
                        name.as_deref(),
                    )?;
                    self.people_manager_ui.selected_person_id = Some(id);
                    self.people_manager_ui.merge_selection.clear();
                    self.status = "People groups merged".to_owned();
                }
                PeopleAction::SplitToNew {
                    library_id,
                    face_id,
                    name,
                } => {
                    let conn = crate::db::open(&self.db_path)?;
                    let id = people_management::split_face_to_new_person(
                        &conn,
                        &library_id,
                        &face_id,
                        &name,
                    )?;
                    self.people_manager_ui.selected_person_id = Some(id);
                    self.people_manager_ui.split_name.clear();
                    self.status = "Face split into a new Person".to_owned();
                }
                PeopleAction::MoveFace {
                    library_id,
                    face_id,
                    manual_person_id,
                } => {
                    let conn = crate::db::open(&self.db_path)?;
                    people_management::move_face_to_person(
                        &conn,
                        &library_id,
                        &face_id,
                        &manual_person_id,
                    )?;
                    self.people_manager_ui.selected_person_id = Some(manual_person_id);
                    self.status = "Face moved to Person".to_owned();
                }
                PeopleAction::Detach {
                    library_id,
                    face_id,
                } => {
                    let conn = crate::db::open(&self.db_path)?;
                    people_management::detach_face(&conn, &library_id, &face_id)?;
                    self.status = "Face removed from People grouping".to_owned();
                }
                PeopleAction::Ignore {
                    library_id,
                    face_id,
                } => {
                    let conn = crate::db::open(&self.db_path)?;
                    people_management::ignore_face(&conn, &library_id, &face_id)?;
                    self.status = "Face ignored for People grouping".to_owned();
                }
                PeopleAction::Restore {
                    library_id,
                    face_id,
                } => {
                    let conn = crate::db::open(&self.db_path)?;
                    let _ =
                        people_management::restore_automatic_face(&conn, &library_id, &face_id)?;
                    self.status = "Face restored to automatic clustering".to_owned();
                }
                PeopleAction::SetRepresentative {
                    person_id,
                    library_id,
                    face_id,
                } => {
                    let conn = crate::db::open(&self.db_path)?;
                    let manual_id =
                        people_management::materialize_effective_person(&conn, &person_id, None)?;
                    people_management::set_person_representative(
                        &conn,
                        &manual_id,
                        &library_id,
                        &face_id,
                    )?;
                    self.people_manager_ui.selected_person_id = Some(manual_id);
                    self.status = "Representative face updated".to_owned();
                }
                PeopleAction::DeleteManualPerson { person_id } => {
                    let conn = crate::db::open(&self.db_path)?;
                    let _ = people_management::delete_manual_person(&conn, &person_id)?;
                    self.people_manager_ui.selected_person_id = None;
                    self.people_manager_ui.confirm_delete_person = None;
                    self.status = "Manual Person reverted to automatic clustering".to_owned();
                }
                PeopleAction::ShowImages { person_id } => {
                    self.show_effective_person_images(&person_id)?;
                }
            }
            Ok(())
        })();
        if let Err(err) = result {
            self.last_error = Some(format!("People correction failed: {err:#}"));
        }
        self.refresh_people_manager();
        self.refresh_face_suggestions();
        self.refresh_people_filter_catalog();
    }

    fn show_effective_person_images(&mut self, person_id: &str) -> anyhow::Result<()> {
        let members = self
            .people_manager_ui
            .catalog
            .members
            .iter()
            .filter(|member| member.person_id.as_deref() == Some(person_id))
            .map(|member| (member.library_id.clone(), member.face_id.clone()))
            .collect::<Vec<_>>();
        let mut paths = HashSet::new();
        for (library_id, face_id) in members {
            if let Some(preview) = self.people_preview(&library_id, &face_id) {
                paths.insert(preview.image_path);
            }
        }
        let mut results = self
            .images
            .iter()
            .filter(|image| paths.contains(&image.path))
            .cloned()
            .collect::<Vec<_>>();
        results.sort_by(|left, right| left.path.cmp(&right.path));
        self.similarity_results = Some(results);
        self.query_image = None;
        self.selected_paths.clear();
        self.clear_face_search_result_state();
        self.status = format!("Showing {} image(s) for selected Person", paths.len());
        Ok(())
    }

    pub(super) fn show_people_manager_window(&mut self, ctx: &egui::Context) {
        if !self.people_manager_ui.open {
            return;
        }

        let mut open = self.people_manager_ui.open;
        let mut action = None;
        let mut selection_changed = false;
        egui::Window::new("People")
            .open(&mut open)
            .resizable(true)
            .default_size([1080.0, 720.0])
            .min_width(820.0)
            .min_height(540.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("People");
                    if ui.button("⟳ Refresh").clicked() {
                        action = Some(PeopleAction::Refresh);
                    }
                    ui.separator();
                    ui.small(format!(
                        "{} effective group{}",
                        self.people_manager_ui.catalog.people.len(),
                        if self.people_manager_ui.catalog.people.len() == 1 {
                            ""
                        } else {
                            "s"
                        }
                    ));
                });
                ui.small("Automatic groups are derived. Names, merges, splits, ignored faces and representative choices are stored as durable manual overrides.");
                ui.separator();

                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(340.0, ui.available_height()),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.strong("People groups");
                            ui.small("Check multiple groups, then merge them from the editor pane.");
                            ui.add_space(6.0);
                            egui::ScrollArea::vertical()
                                .id_salt("people-manager-list")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    let people = self.people_manager_ui.catalog.people.clone();
                                    for person in people {
                                        let selected = self
                                            .people_manager_ui
                                            .selected_person_id
                                            .as_ref()
                                            .is_some_and(|id| id == &person.person_id);
                                        let mut merge_checked = self
                                            .people_manager_ui
                                            .merge_selection
                                            .contains(&person.person_id);
                                        ui.horizontal(|ui| {
                                            if ui.checkbox(&mut merge_checked, "").changed() {
                                                if merge_checked {
                                                    self.people_manager_ui
                                                        .merge_selection
                                                        .insert(person.person_id.clone());
                                                } else {
                                                    self.people_manager_ui
                                                        .merge_selection
                                                        .remove(&person.person_id);
                                                }
                                            }
                                            if let Some((library_id, face_id)) = person
                                                .representative_library_id
                                                .as_deref()
                                                .zip(person.representative_face_id.as_deref())
                                            {
                                                if let Some(preview) =
                                                    self.people_preview(library_id, face_id)
                                                {
                                                    if let Some(texture) =
                                                        self.thumbnail(&preview.image_path)
                                                    {
                                                        let response = photo_grid::photo_tile(ui, &texture, egui::vec2(58.0, 58.0), PhotoTileMode::Face(preview.bbox), selected, egui::Sense::click());
                                                        if response.clicked() {
                                                            self.people_manager_ui
                                                                .selected_person_id =
                                                                Some(person.person_id.clone());
                                                            selection_changed = true;
                                                        }
                                                    }
                                                }
                                            }
                                            let name = person
                                                .display_name
                                                .clone()
                                                .unwrap_or_else(|| "Unnamed person".to_owned());
                                            let source = match person.source {
                                                EffectivePersonSource::Automatic => "Auto",
                                                EffectivePersonSource::Manual => "Manual",
                                            };
                                            let response = ui.selectable_label(
                                                selected,
                                                format!(
                                                    "{name}\n{} face{} · {source}",
                                                    person.member_count,
                                                    if person.member_count == 1 { "" } else { "s" }
                                                ),
                                            );
                                            if response.clicked() {
                                                self.people_manager_ui.selected_person_id =
                                                    Some(person.person_id.clone());
                                                selection_changed = true;
                                            }
                                        });
                                        ui.add_space(3.0);
                                    }
                                });

                            let exceptions = self
                                .people_manager_ui
                                .catalog
                                .members
                                .iter()
                                .filter(|member| {
                                    member.person_id.is_none() && (member.detached || member.ignored)
                                })
                                .cloned()
                                .collect::<Vec<_>>();
                            if !exceptions.is_empty() {
                                ui.add_space(8.0);
                                ui.separator();
                                ui.strong(format!("Manual exceptions ({})", exceptions.len()));
                                ui.small("Detached/ignored faces stay here so every correction can be restored.");
                                egui::ScrollArea::vertical()
                                    .id_salt("people-manager-exceptions")
                                    .max_height(180.0)
                                    .show(ui, |ui| {
                                        for member in exceptions {
                                            ui.horizontal(|ui| {
                                                if let Some(preview) = self.people_preview(
                                                    &member.library_id,
                                                    &member.face_id,
                                                ) {
                                                    if let Some(texture) = self.thumbnail(&preview.image_path) {
                                                        let _ = photo_grid::photo_tile(ui, &texture, egui::vec2(42.0, 42.0), PhotoTileMode::Face(preview.bbox), false, egui::Sense::click());
                                                    }
                                                }
                                                ui.vertical(|ui| {
                                                    ui.small(if member.ignored { "Ignored face" } else { "Detached face" });
                                                    ui.small(&member.face_id);
                                                });
                                                if ui.small_button("Restore").clicked() {
                                                    action = Some(PeopleAction::Restore {
                                                        library_id: member.library_id.clone(),
                                                        face_id: member.face_id.clone(),
                                                    });
                                                }
                                            });
                                        }
                                    });
                            }
                        },
                    );
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_min_width(430.0);
                        let selected_person = self
                            .people_manager_ui
                            .selected_person_id
                            .clone()
                            .and_then(|id| {
                                self.people_manager_ui
                                    .catalog
                                    .people
                                    .iter()
                                    .find(|person| person.person_id == id)
                                    .cloned()
                            });
                        let Some(person) = selected_person else {
                            ui.heading("Select a Person");
                            ui.label("Choose a People group from the list to edit it.");
                            return;
                        };

                        let members = self
                            .people_manager_ui
                            .catalog
                            .members
                            .iter()
                            .filter(|member| {
                                member.person_id.as_deref() == Some(&person.person_id)
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        let mut parent_images = HashSet::new();
                        for member in &members {
                            if let Some(preview) =
                                self.people_preview(&member.library_id, &member.face_id)
                            {
                                parent_images.insert(preview.image_path);
                            }
                        }
                        let title = person
                            .display_name
                            .clone()
                            .unwrap_or_else(|| "Unnamed person".to_owned());
                        ui.heading(title);
                        ui.small(format!(
                            "{} · {} face{} · {} image{}",
                            match person.source {
                                EffectivePersonSource::Automatic => "Automatic group",
                                EffectivePersonSource::Manual => "Manual identity",
                            },
                            person.member_count,
                            if person.member_count == 1 { "" } else { "s" },
                            parent_images.len(),
                            if parent_images.len() == 1 { "" } else { "s" }
                        ));

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label("Name");
                            ui.add(
                                egui::TextEdit::singleline(
                                    &mut self.people_manager_ui.rename_text,
                                )
                                .desired_width(250.0)
                                .hint_text("Person name"),
                            );
                            if ui.button("Save name").clicked() {
                                action = Some(PeopleAction::Rename {
                                    person_id: person.person_id.clone(),
                                    name: self.people_manager_ui.rename_text.clone(),
                                });
                            }
                        });

                        ui.horizontal(|ui| {
                            if ui.button("Show member images").clicked() {
                                action = Some(PeopleAction::ShowImages {
                                    person_id: person.person_id.clone(),
                                });
                            }
                            if person.source == EffectivePersonSource::Manual
                                && ui.button("Revert manual Person…").clicked()
                            {
                                self.people_manager_ui.confirm_delete_person =
                                    Some(person.person_id.clone());
                            }
                        });

                        ui.add_space(10.0);
                        ui.separator();
                        ui.strong("Merge selected People");
                        let merge_ids = self
                            .people_manager_ui
                            .merge_selection
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>();
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(
                                    &mut self.people_manager_ui.merge_name,
                                )
                                .desired_width(220.0)
                                .hint_text("Optional merged name"),
                            );
                            if ui
                                .add_enabled(
                                    merge_ids.len() >= 2,
                                    egui::Button::new(format!(
                                        "Merge {} selected",
                                        merge_ids.len()
                                    )),
                                )
                                .clicked()
                            {
                                let name = self.people_manager_ui.merge_name.trim();
                                action = Some(PeopleAction::Merge {
                                    person_ids: merge_ids,
                                    name: (!name.is_empty()).then(|| name.to_owned()),
                                });
                            }
                        });

                        ui.add_space(10.0);
                        ui.separator();
                        ui.strong("Faces in this Person");
                        let selected_face = self.people_manager_ui.selected_face.clone();
                        let member_grid = PhotoGridSpec::new(
                            "people-manager-members",
                            94.0,
                            108.0,
                        );
                        photo_grid::show(ui, members.len(), member_grid, |ui, index| {
                            let member = &members[index];
                            let key = (member.library_id.clone(), member.face_id.clone());
                            let is_selected = selected_face.as_ref() == Some(&key);
                            let response = if let Some(preview) =
                                self.people_preview(&member.library_id, &member.face_id)
                            {
                                if let Some(texture) = self.thumbnail(&preview.image_path) {
                                    photo_grid::photo_tile(
                                        ui,
                                        &texture,
                                        egui::vec2(82.0, 82.0),
                                        PhotoTileMode::Face(preview.bbox),
                                        is_selected,
                                        egui::Sense::click(),
                                    )
                                } else {
                                    ui.add_sized([82.0, 82.0], egui::Button::new("Loading…"))
                                }
                            } else {
                                ui.add_sized([82.0, 82.0], egui::Button::new("Unavailable"))
                            };
                            if response.clicked() {
                                self.people_manager_ui.selected_face = Some(key);
                            }
                            let mut flags = Vec::new();
                            if member.explicit_manual_assignment {
                                flags.push("manual");
                            }
                            if member.detached {
                                flags.push("detached");
                            }
                            if member.ignored {
                                flags.push("ignored");
                            }
                            if !flags.is_empty() {
                                ui.small(flags.join(" · "));
                            }
                        });


                        if let Some((library_id, face_id)) =
                            self.people_manager_ui.selected_face.clone()
                        {
                            ui.add_space(8.0);
                            ui.strong("Selected face correction");
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Set representative").clicked() {
                                    action = Some(PeopleAction::SetRepresentative {
                                        person_id: person.person_id.clone(),
                                        library_id: library_id.clone(),
                                        face_id: face_id.clone(),
                                    });
                                }
                                if ui.button("Remove from group").clicked() {
                                    action = Some(PeopleAction::Detach {
                                        library_id: library_id.clone(),
                                        face_id: face_id.clone(),
                                    });
                                }
                                if ui.button("Ignore face").clicked() {
                                    action = Some(PeopleAction::Ignore {
                                        library_id: library_id.clone(),
                                        face_id: face_id.clone(),
                                    });
                                }
                                if ui.button("Restore automatic").clicked() {
                                    action = Some(PeopleAction::Restore {
                                        library_id: library_id.clone(),
                                        face_id: face_id.clone(),
                                    });
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(
                                        &mut self.people_manager_ui.split_name,
                                    )
                                    .desired_width(190.0)
                                    .hint_text("New Person name"),
                                );
                                if ui.button("Split to new Person").clicked() {
                                    action = Some(PeopleAction::SplitToNew {
                                        library_id: library_id.clone(),
                                        face_id: face_id.clone(),
                                        name: self.people_manager_ui.split_name.clone(),
                                    });
                                }
                            });

                            let manual_people = self
                                .people_manager_ui
                                .catalog
                                .people
                                .iter()
                                .filter(|item| {
                                    item.source == EffectivePersonSource::Manual
                                        && item.person_id != person.person_id
                                })
                                .cloned()
                                .collect::<Vec<_>>();
                            if !manual_people.is_empty() {
                                ui.horizontal(|ui| {
                                    egui::ComboBox::from_label("Move to")
                                        .selected_text(
                                            self.people_manager_ui
                                                .move_target
                                                .as_ref()
                                                .and_then(|id| {
                                                    manual_people.iter().find(|item| {
                                                        &item.person_id == id
                                                    })
                                                })
                                                .and_then(|item| item.display_name.clone())
                                                .unwrap_or_else(|| "Choose manual Person".to_owned()),
                                        )
                                        .show_ui(ui, |ui| {
                                            for target in &manual_people {
                                                ui.selectable_value(
                                                    &mut self.people_manager_ui.move_target,
                                                    Some(target.person_id.clone()),
                                                    target.display_name.clone().unwrap_or_else(|| {
                                                        "Unnamed manual Person".to_owned()
                                                    }),
                                                );
                                            }
                                        });
                                    if let Some(target) =
                                        self.people_manager_ui.move_target.clone()
                                    {
                                        if ui.button("Move face").clicked() {
                                            action = Some(PeopleAction::MoveFace {
                                                library_id: library_id.clone(),
                                                face_id: face_id.clone(),
                                                manual_person_id: target,
                                            });
                                        }
                                    }
                                });
                            }
                        }
                    });
                });
            });
        self.people_manager_ui.open = open;

        if selection_changed {
            self.sync_people_editor_fields();
        }
        if let Some(action) = action {
            self.apply_people_action(action);
        }

        if let Some(person_id) = self.people_manager_ui.confirm_delete_person.clone() {
            let mut keep = true;
            egui::Window::new("Revert manual Person")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("Remove this manual identity and all of its manual face assignments?");
                    ui.label("Automatic clustering data is not deleted and will become visible again where applicable.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                            self.people_manager_ui.confirm_delete_person = None;
                        }
                        if ui.button("Revert manual Person").clicked() {
                            keep = false;
                            self.apply_people_action(PeopleAction::DeleteManualPerson {
                                person_id: person_id.clone(),
                            });
                        }
                    });
                });
            if !keep {
                self.people_manager_ui.confirm_delete_person = None;
            }
        }
    }
}
