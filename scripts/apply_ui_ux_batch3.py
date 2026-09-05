from pathlib import Path


def once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


# Enrich named People with a best-effort representative image path.
path = Path("src/people_filter.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    """pub struct NamedPersonOption {\n    pub person_id: String,\n    pub display_name: String,\n    pub member_count: usize,\n}\n""",
    """pub struct NamedPersonOption {\n    pub person_id: String,\n    pub display_name: String,\n    pub member_count: usize,\n    pub representative_image: Option<PathBuf>,\n}\n""",
    "named person representative image field",
)
text = once(
    text,
    """pub fn load_named_people(session_db_path: &Path) -> Result<Vec<NamedPersonOption>> {\n    let conn = db::open(session_db_path)\n        .with_context(|| format!(\"opening People catalog {}\", session_db_path.display()))?;\n    let catalog = people_effective::load(&conn)?;\n    let mut people = catalog\n        .people\n        .into_iter()\n        .filter_map(|person| {\n            let display_name = person.display_name?.trim().to_owned();\n            (!display_name.is_empty()).then_some(NamedPersonOption {\n                person_id: person.person_id,\n                display_name,\n                member_count: person.member_count,\n            })\n        })\n        .collect::<Vec<_>>();\n    people.sort_by(|left, right| {\n        left.display_name\n            .to_lowercase()\n            .cmp(&right.display_name.to_lowercase())\n            .then_with(|| left.person_id.cmp(&right.person_id))\n    });\n    Ok(people)\n}\n""",
    """pub fn load_named_people(\n    session_db_path: &Path,\n    roots: &[PathBuf],\n) -> Result<Vec<NamedPersonOption>> {\n    let conn = db::open(session_db_path)\n        .with_context(|| format!(\"opening People catalog {}\", session_db_path.display()))?;\n    let catalog = people_effective::load(&conn)?;\n    let mut people = Vec::new();\n    for person in catalog.people {\n        let Some(display_name) = person\n            .display_name\n            .as_deref()\n            .map(str::trim)\n            .filter(|name| !name.is_empty())\n            .map(str::to_owned)\n        else {\n            continue;\n        };\n        let representative_image = person\n            .representative_library_id\n            .as_deref()\n            .zip(person.representative_face_id.as_deref())\n            .and_then(|(library_id, face_id)| {\n                crate::face_search::resolve_searchable_face(roots, library_id, face_id)\n                    .ok()\n                    .flatten()\n                    .map(|face| face.image_path)\n            });\n        people.push(NamedPersonOption {\n            person_id: person.person_id,\n            display_name,\n            member_count: person.member_count,\n            representative_image,\n        });\n    }\n    people.sort_by(|left, right| {\n        left.display_name\n            .to_lowercase()\n            .cmp(&right.display_name.to_lowercase())\n            .then_with(|| left.person_id.cmp(&right.person_id))\n    });\n    Ok(people)\n}\n""",
    "named people loader",
)
path.write_text(text, encoding="utf-8")


# Make the shared thumbnail painter reusable by visual People rows.
path = Path("src/ui/views.rs")
text = path.read_text(encoding="utf-8")
text = once(text, "fn thumbnail_widget(\n", "pub(super) fn thumbnail_widget(\n", "thumbnail widget visibility")
path.write_text(text, encoding="utf-8")


# Turn People checkboxes into visual avatar rows and named selection chips.
path = Path("src/ui/people_filter.rs")
text = path.read_text(encoding="utf-8")
text = once(text, "use super::ImageSearchApp;\n", "use super::{ImageSearchApp, ThumbnailFit};\n", "people UI imports")
text = once(
    text,
    """        let db_path = self.db_path.clone();\n        let tx = self.people_filter_ui.tx.clone();\n        std::thread::spawn(move || {\n            let result =\n                people_filter::load_named_people(&db_path).map_err(|err| format!(\"{err:#}\"));\n            let _ = tx.send(PeopleFilterMessage::Catalog { generation, result });\n        });\n""",
    """        let db_path = self.db_path.clone();\n        let roots = self.roots.clone();\n        let tx = self.people_filter_ui.tx.clone();\n        std::thread::spawn(move || {\n            let result = people_filter::load_named_people(&db_path, &roots)\n                .map_err(|err| format!(\"{err:#}\"));\n            let _ = tx.send(PeopleFilterMessage::Catalog { generation, result });\n        });\n""",
    "people catalog roots",
)
old = """                let query = self.people_filter_ui.name_query.trim().to_lowercase();\n                let options = self\n                    .people_filter_ui\n                    .options\n                    .iter()\n                    .filter(|person| {\n                        query.is_empty() || person.display_name.to_lowercase().contains(&query)\n                    })\n                    .cloned()\n                    .collect::<Vec<_>>();\n                let mut selection_changed = false;\n                egui::ScrollArea::vertical()\n                    .id_salt(\"people-filter-options\")\n                    .max_height(180.0)\n                    .show(ui, |ui| {\n                        for person in options {\n                            let mut selected = self\n                                .people_filter_ui\n                                .selected_person_ids\n                                .contains(&person.person_id);\n                            let label = format!(\n                                \"{}  ·  {} face{}\",\n                                person.display_name,\n                                person.member_count,\n                                if person.member_count == 1 { \"\" } else { \"s\" }\n                            );\n                            if ui.checkbox(&mut selected, label).changed() {\n                                selection_changed = true;\n                                if selected {\n                                    self.people_filter_ui\n                                        .selected_person_ids\n                                        .insert(person.person_id);\n                                } else {\n                                    self.people_filter_ui\n                                        .selected_person_ids\n                                        .remove(&person.person_id);\n                                }\n                            }\n                        }\n                    });\n"""
new = """                let query = self.people_filter_ui.name_query.trim().to_lowercase();\n                let options = self\n                    .people_filter_ui\n                    .options\n                    .iter()\n                    .filter(|person| {\n                        query.is_empty() || person.display_name.to_lowercase().contains(&query)\n                    })\n                    .cloned()\n                    .collect::<Vec<_>>();\n                let selected_people = self\n                    .people_filter_ui\n                    .options\n                    .iter()\n                    .filter(|person| {\n                        self.people_filter_ui\n                            .selected_person_ids\n                            .contains(&person.person_id)\n                    })\n                    .map(|person| (person.person_id.clone(), person.display_name.clone()))\n                    .collect::<Vec<_>>();\n                let mut selection_changed = false;\n                if !selected_people.is_empty() {\n                    ui.horizontal_wrapped(|ui| {\n                        for (person_id, display_name) in selected_people {\n                            if ui.small_button(format!(\"{display_name}  ×\")).clicked() {\n                                self.people_filter_ui.selected_person_ids.remove(&person_id);\n                                selection_changed = true;\n                            }\n                        }\n                    });\n                    ui.add_space(4.0);\n                }\n                egui::ScrollArea::vertical()\n                    .id_salt(\"people-filter-options\")\n                    .max_height(230.0)\n                    .show(ui, |ui| {\n                        for person in options {\n                            let selected = self\n                                .people_filter_ui\n                                .selected_person_ids\n                                .contains(&person.person_id);\n                            let mut clicked = false;\n                            ui.horizontal(|ui| {\n                                let avatar_size = egui::vec2(36.0, 36.0);\n                                if let Some(image_path) = person.representative_image.as_ref() {\n                                    if let Some(texture) = self.thumbnail(image_path) {\n                                        let response = super::views::thumbnail_widget(\n                                            ui,\n                                            &texture,\n                                            avatar_size,\n                                            ThumbnailFit::Cover,\n                                            selected,\n                                            egui::Sense::click(),\n                                        );\n                                        clicked |= response.clicked();\n                                    } else {\n                                        ui.add_sized(avatar_size, egui::Spinner::new());\n                                    }\n                                } else {\n                                    let initial = person\n                                        .display_name\n                                        .chars()\n                                        .next()\n                                        .map(|value| value.to_uppercase().to_string())\n                                        .unwrap_or_else(|| \"?\".to_owned());\n                                    let (rect, response) =\n                                        ui.allocate_exact_size(avatar_size, egui::Sense::click());\n                                    ui.painter().circle_filled(\n                                        rect.center(),\n                                        18.0,\n                                        ui.visuals().widgets.inactive.bg_fill,\n                                    );\n                                    ui.painter().text(\n                                        rect.center(),\n                                        egui::Align2::CENTER_CENTER,\n                                        initial,\n                                        egui::FontId::proportional(14.0),\n                                        ui.visuals().text_color(),\n                                    );\n                                    clicked |= response.clicked();\n                                }\n\n                                let label = format!(\n                                    \"{}  ·  {} face{}\",\n                                    person.display_name,\n                                    person.member_count,\n                                    if person.member_count == 1 { \"\" } else { \"s\" }\n                                );\n                                clicked |= ui.selectable_label(selected, label).clicked();\n                            });\n                            if clicked {\n                                selection_changed = true;\n                                if selected {\n                                    self.people_filter_ui\n                                        .selected_person_ids\n                                        .remove(&person.person_id);\n                                } else {\n                                    self.people_filter_ui\n                                        .selected_person_ids\n                                        .insert(person.person_id);\n                                }\n                            }\n                        }\n                    });\n"""
text = once(text, old, new, "visual people options")
path.write_text(text, encoding="utf-8")


# Add reusable collection actions for Inspector and multi-selection UX.
path = Path("src/ui/collections.rs")
text = path.read_text(encoding="utf-8")
anchor = """    pub(super) fn clear_collection_filter(&mut self) {\n        self.collections.active_filter = None;\n    }\n\n"""
insert = anchor + """    pub(super) fn collection_count(&self) -> usize {\n        self.collections.items.len()\n    }\n\n    pub(super) fn show_add_to_collection_menu(\n        &mut self,\n        ui: &mut egui::Ui,\n        label: &str,\n        paths: &[PathBuf],\n    ) {\n        let items = self\n            .collections\n            .items\n            .iter()\n            .map(|item| (item.id, item.name.clone(), self.collections.count(item.id)))\n            .collect::<Vec<_>>();\n        let mut target = None;\n        ui.add_enabled_ui(!self.busy && !items.is_empty() && !paths.is_empty(), |ui| {\n            ui.menu_button(label, |ui| {\n                for (id, name, count) in &items {\n                    if ui.button(format!(\"{name} ({count})\")).clicked() {\n                        target = Some(*id);\n                        ui.close();\n                    }\n                }\n            });\n        });\n        if let Some(id) = target {\n            self.apply_collection_action(CollectionAction::Drop(id, paths.to_vec()));\n        }\n    }\n\n"""
text = once(text, anchor, insert, "collection quick actions")
path.write_text(text, encoding="utf-8")


# Add Collection action to Inspector and respect the explicit visibility toggle.
path = Path("src/ui/inspector.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    """        if self.selected_paths.is_empty() {\n            return;\n        }\n""",
    """        if !self.inspector_open || self.selected_paths.is_empty() {\n            return;\n        }\n""",
    "inspector visibility",
)
text = once(
    text,
    """                    if ui.button(\"Copy path\").clicked() {\n                        ui.ctx().copy_text(record.path.display().to_string());\n                    }\n""",
    """                    self.show_add_to_collection_menu(\n                        ui,\n                        \"Add to Collection\",\n                        &[record.path.clone()],\n                    );\n                    if ui.button(\"Copy path\").clicked() {\n                        ui.ctx().copy_text(record.path.display().to_string());\n                    }\n""",
    "inspector collection action",
)
path.write_text(text, encoding="utf-8")


# Add the same Collection action to the contextual selection bar.
path = Path("src/ui/ux.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    """        let all_paths = self\n            .selected_paths\n            .iter()\n            .map(|path| path.display().to_string())\n            .collect::<Vec<_>>();\n""",
    """        let selected_paths = self.selected_paths.iter().cloned().collect::<Vec<_>>();\n        let all_paths = selected_paths\n            .iter()\n            .map(|path| path.display().to_string())\n            .collect::<Vec<_>>();\n""",
    "selection paths vector",
)
text = once(
    text,
    """                if ui.button(\"Copy path(s)\").clicked() {\n                    ui.ctx().copy_text(all_paths.join(\"\\n\"));\n                }\n                if ui.button(\"Clear selection\").clicked() {\n""",
    """                if ui.button(\"Copy path(s)\").clicked() {\n                    ui.ctx().copy_text(all_paths.join(\"\\n\"));\n                }\n                self.show_add_to_collection_menu(ui, \"Add to Collection\", &selected_paths);\n                if ui.button(\"Clear selection\").clicked() {\n""",
    "selection collection action",
)
path.write_text(text, encoding="utf-8")


# Provide an explicit top-level route into the Collections preferences page.
path = Path("src/ui/settings_window.rs")
text = path.read_text(encoding="utf-8")
anchor = """#[derive(Default)]\nstruct Effects {\n"""
helper = """pub(super) fn open_collections(app: &mut ImageSearchApp, ctx: &egui::Context) {\n    app.settings_open = true;\n    let category_id = egui::Id::new(\"preferences-category\");\n    ctx.data_mut(|data| data.insert_temp(category_id, SettingsCategory::Collections.index()));\n}\n\n#[derive(Default)]\nstruct Effects {\n"""
text = once(text, anchor, helper, "open collections preferences helper")
path.write_text(text, encoding="utf-8")


# Add first-class Library/Collections navigation and Inspector toggle.
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    """    pub(super) sort_mode: SortMode,\n    pub(super) textures: HashMap<PathBuf, TextureHandle>,\n""",
    """    pub(super) sort_mode: SortMode,\n    pub(super) inspector_open: bool,\n    pub(super) textures: HashMap<PathBuf, TextureHandle>,\n""",
    "inspector field",
)
text = once(
    text,
    """            sort_mode: SortMode::Relevance,\n            textures: HashMap::new(),\n""",
    """            sort_mode: SortMode::Relevance,\n            inspector_open: true,\n            textures: HashMap::new(),\n""",
    "inspector initial state",
)
old_top = """            ui.horizontal(|ui| {\n                ui.strong(\"Windows Image Search\");\n                if ui.button(\"Settings\").clicked() {\n                    self.settings_open = true;\n                }\n                if ui\n                    .add_enabled(!self.busy, egui::Button::new(\"People\"))\n                    .clicked()\n                {\n                    self.open_people_manager();\n                }\n                if ui\n                    .add_enabled(\n                        !self.busy && !self.roots.is_empty(),\n                        egui::Button::new(\"Rescan\"),\n                    )\n                    .clicked()\n                {\n                    self.start_rescan();\n                }\n                ui.separator();\n                ui.small(format!(\"{} indexed images\", self.images.len()));\n                if self.indexing {\n                    ui.small(\"Indexing… committed results appear live\");\n                }\n            });\n"""
new_top = """            ui.horizontal(|ui| {\n                ui.strong(\"Windows Image Search\");\n                ui.separator();\n                if ui\n                    .selectable_label(self.collection_filter_chip().is_none(), \"Library\")\n                    .clicked()\n                {\n                    self.clear_collection_filter();\n                }\n                if ui\n                    .button(format!(\"Collections ({})\", self.collection_count()))\n                    .clicked()\n                {\n                    settings_window::open_collections(self, ctx);\n                }\n                if ui\n                    .add_enabled(!self.busy, egui::Button::new(\"People\"))\n                    .clicked()\n                {\n                    self.open_people_manager();\n                }\n                ui.separator();\n                if ui\n                    .add_enabled(\n                        !self.busy && !self.roots.is_empty(),\n                        egui::Button::new(\"Rescan\"),\n                    )\n                    .clicked()\n                {\n                    self.start_rescan();\n                }\n                if ui.button(\"Settings\").clicked() {\n                    self.settings_open = true;\n                }\n                ui.separator();\n                ui.small(format!(\"{} indexed images\", self.images.len()));\n                if self.indexing {\n                    ui.small(\"Indexing… committed results appear live\");\n                }\n            });\n"""
text = once(text, old_top, new_top, "top level navigation")
text = once(
    text,
    """                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {\n                    egui::ComboBox::from_id_salt(\"result-sort\")\n""",
    """                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {\n                    ui.toggle_value(&mut self.inspector_open, \"Inspector\");\n                    ui.separator();\n                    egui::ComboBox::from_id_salt(\"result-sort\")\n""",
    "results inspector toggle",
)
path.write_text(text, encoding="utf-8")
