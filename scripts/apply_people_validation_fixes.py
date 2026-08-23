from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def replace_between(text: str, start: str, end: str, replacement: str, label: str) -> str:
    i = text.find(start)
    if i < 0:
        raise SystemExit(f"{label}: start marker missing")
    j = text.find(end, i)
    if j < 0:
        raise SystemExit(f"{label}: end marker missing")
    return text[:i] + replacement + text[j:]


people_path = Path("src/ui/people_manager.rs")
people = people_path.read_text(encoding="utf-8")
people = replace_once(
    people,
    "    confirm_delete_person: Option<String>,\n",
    "    confirm_delete_person: Option<String>,\n    filter_text: String,\n",
    "people filter state",
)
people = replace_once(
    people,
    ".min_width(820.0)\n            .min_height(540.0)",
    ".min_width(760.0)\n            .min_height(540.0)\n            .max_width(ctx.available_rect().width().max(760.0))",
    "people window constraints",
)
people = replace_once(
    people,
    "                ui.horizontal_top(|ui| {\n                    ui.allocate_ui_with_layout(\n                        egui::vec2(340.0, ui.available_height()),",
    "                ui.horizontal_top(|ui| {\n                    let total_width = ui.available_width();\n                    let list_width = (total_width * 0.34).clamp(260.0, 340.0);\n                    let editor_width = (total_width - list_width - 18.0).max(360.0);\n                    ui.allocate_ui_with_layout(\n                        egui::vec2(list_width, ui.available_height()),",
    "people pane widths",
)

list_start = '                            ui.strong("People groups");'
list_end = "\n\n                            let exceptions = self"
list_replacement = '''                            ui.strong("People groups");
                            ui.small("Check multiple groups, then merge them from the editor pane.");
                            ui.add_space(6.0);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.people_manager_ui.filter_text)
                                    .hint_text("Filter named people…")
                                    .desired_width(f32::INFINITY),
                            );
                            ui.add_space(5.0);

                            let filter = self.people_manager_ui.filter_text.trim().to_lowercase();
                            let people = self
                                .people_manager_ui
                                .catalog
                                .people
                                .iter()
                                .filter(|person| {
                                    filter.is_empty()
                                        || person
                                            .display_name
                                            .as_deref()
                                            .is_some_and(|name| name.to_lowercase().contains(&filter))
                                })
                                .cloned()
                                .collect::<Vec<_>>();
                            if !filter.is_empty() {
                                ui.small(format!(
                                    "{} of {} named/visible groups",
                                    people.len(),
                                    self.people_manager_ui.catalog.people.len()
                                ));
                            }

                            egui::ScrollArea::vertical()
                                .id_salt("people-manager-list")
                                .auto_shrink([false, false])
                                .show_rows(ui, 66.0, people.len(), |ui, row_range| {
                                    for row in row_range {
                                        let person = people[row].clone();
                                        let selected = self
                                            .people_manager_ui
                                            .selected_person_id
                                            .as_ref()
                                            .is_some_and(|id| id == &person.person_id);
                                        let mut merge_checked = self
                                            .people_manager_ui
                                            .merge_selection
                                            .contains(&person.person_id);
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(ui.available_width(), 64.0),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
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

                                                let preview_response = if let Some((library_id, face_id)) = person
                                                    .representative_library_id
                                                    .as_deref()
                                                    .zip(person.representative_face_id.as_deref())
                                                {
                                                    if let Some(preview) = self.people_preview(library_id, face_id) {
                                                        if let Some(texture) = self.thumbnail(&preview.image_path) {
                                                            face_crop_widget(
                                                                ui,
                                                                &texture,
                                                                preview.bbox,
                                                                egui::vec2(58.0, 58.0),
                                                                selected,
                                                            )
                                                        } else {
                                                            ui.add_sized(
                                                                [58.0, 58.0],
                                                                egui::Button::new("…"),
                                                            )
                                                        }
                                                    } else {
                                                        ui.add_sized(
                                                            [58.0, 58.0],
                                                            egui::Button::new("—"),
                                                        )
                                                    }
                                                } else {
                                                    ui.add_sized(
                                                        [58.0, 58.0],
                                                        egui::Button::new("—"),
                                                    )
                                                };
                                                if preview_response.clicked() {
                                                    self.people_manager_ui.selected_person_id =
                                                        Some(person.person_id.clone());
                                                    selection_changed = true;
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
                                                        "{name}\\n{} face{} · {source}",
                                                        person.member_count,
                                                        if person.member_count == 1 { "" } else { "s" }
                                                    ),
                                                );
                                                if response.clicked() {
                                                    self.people_manager_ui.selected_person_id =
                                                        Some(person.person_id.clone());
                                                    selection_changed = true;
                                                }
                                            },
                                        );
                                    }
                                });'''
people = replace_between(people, list_start, list_end, list_replacement, "people list")
people = replace_once(
    people,
    "                    ui.separator();\n                    ui.vertical(|ui| {\n                        ui.set_min_width(430.0);",
    "                    ui.separator();\n                    ui.allocate_ui_with_layout(\n                        egui::vec2(editor_width, ui.available_height()),\n                        egui::Layout::top_down(egui::Align::Min),\n                        |ui| {",
    "bounded people editor",
)

members_start = '''                        egui::ScrollArea::vertical()
                            .id_salt("people-manager-members")'''
members_end = '''

                        if let Some((library_id, face_id)) =
                            self.people_manager_ui.selected_face.clone()'''
members_replacement = '''                        let card_width = 98.0;
                        let columns = ((ui.available_width() / card_width).floor() as usize).max(1);
                        let rows = members.len().div_ceil(columns);
                        egui::ScrollArea::vertical()
                            .id_salt("people-manager-members")
                            .max_height(300.0)
                            .auto_shrink([false, false])
                            .show_rows(ui, 108.0, rows, |ui, row_range| {
                                for row in row_range {
                                    ui.horizontal(|ui| {
                                        for column in 0..columns {
                                            let index = row * columns + column;
                                            if index >= members.len() {
                                                break;
                                            }
                                            let member = members[index].clone();
                                            let key =
                                                (member.library_id.clone(), member.face_id.clone());
                                            let is_selected = selected_face.as_ref() == Some(&key);
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(card_width, 104.0),
                                                egui::Layout::top_down(egui::Align::Center),
                                                |ui| {
                                                    let response = if let Some(preview) = self
                                                        .people_preview(&member.library_id, &member.face_id)
                                                    {
                                                        if let Some(texture) =
                                                            self.thumbnail(&preview.image_path)
                                                        {
                                                            face_crop_widget(
                                                                ui,
                                                                &texture,
                                                                preview.bbox,
                                                                egui::vec2(82.0, 82.0),
                                                                is_selected,
                                                            )
                                                        } else {
                                                            ui.add_sized(
                                                                [82.0, 82.0],
                                                                egui::Button::new("Loading…"),
                                                            )
                                                        }
                                                    } else {
                                                        ui.add_sized(
                                                            [82.0, 82.0],
                                                            egui::Button::new("Unavailable"),
                                                        )
                                                    };
                                                    if response.clicked() {
                                                        self.people_manager_ui.selected_face =
                                                            Some(key.clone());
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
                                                },
                                            );
                                        }
                                    });
                                }
                            });'''
people = replace_between(people, members_start, members_end, members_replacement, "member grid")

people = replace_once(
    people,
    "        let mut action = None;\n        let mut selection_changed = false;",
    "        let mut action = None;\n        let mut selection_changed = false;\n        let mut search_face = None;\n        let mut search_image = None;",
    "search action state",
)

selected_marker = '''                            ui.add_space(8.0);
                            ui.strong("Selected face correction");
                            ui.horizontal_wrapped(|ui| {'''
selected_replacement = '''                            let selected_preview =
                                self.people_preview(&library_id, &face_id);
                            ui.add_space(8.0);
                            ui.strong("Selected face actions");
                            ui.horizontal_wrapped(|ui| {
                                if let Some(preview) = selected_preview.clone() {
                                    if ui.button("Search this face").clicked() {
                                        search_face = Some(preview.clone());
                                    }
                                    if ui.button("Search by image").clicked() {
                                        search_image = Some(preview.image_path.clone());
                                    }
                                }
                            });
                            ui.add_space(5.0);
                            ui.strong("Selected face correction");
                            ui.horizontal_wrapped(|ui| {'''
people = replace_once(people, selected_marker, selected_replacement, "selected face search actions")

people = replace_once(
    people,
    "        self.people_manager_ui.open = open;\n\n        if selection_changed {",
    "        self.people_manager_ui.open = open;\n\n        if let Some(query) = search_face {\n            self.start_indexed_face_search(query);\n        } else if let Some(path) = search_image {\n            self.run_similarity_search(path);\n        }\n\n        if selection_changed {",
    "run selected face searches",
)
people_path.write_text(people, encoding="utf-8")

face_path = Path("src/ui/face_search_panel.rs")
face = face_path.read_text(encoding="utf-8")
face = replace_once(
    face,
    "                        Ok(suggestions) => {\n                            self.face_search_ui.suggestions = suggestions;",
    "                        Ok(suggestions) => {\n                            let suggestion_count = suggestions.len();\n                            self.face_search_ui.suggestions = suggestions;",
    "suggestion count",
)
face = replace_once(
    face,
    "                            if !selected_exists {\n                                self.face_search_ui.selected_face_id = None;\n                            }\n                        }\n                        Err(error) => {\n                            self.last_error = Some(error);\n                        }",
    "                            if !selected_exists {\n                                self.face_search_ui.selected_face_id = None;\n                            }\n                            self.status = if suggestion_count == 0 {\n                                \"No searchable faces are currently available\".to_owned()\n                            } else {\n                                format!(\n                                    \"Loaded {suggestion_count} People / searchable face suggestion{}\",\n                                    if suggestion_count == 1 { \"\" } else { \"s\" }\n                                )\n                            };\n                        }\n                        Err(error) => {\n                            self.status = \"Could not load People / searchable face suggestions\".to_owned();\n                            self.last_error = Some(error);\n                        }",
    "terminal suggestion status",
)
face = replace_once(
    face,
    "    fn start_indexed_face_search(&mut self, query: IndexedFaceSuggestion) {",
    "    pub(super) fn start_indexed_face_search(&mut self, query: IndexedFaceSuggestion) {",
    "face search visibility",
)
face_path.write_text(face, encoding="utf-8")
