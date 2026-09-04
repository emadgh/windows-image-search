from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


# Wire new UI modules and replace the legacy all-in-one search sidebar.
p = Path("src/ui/mod.rs")
s = p.read_text()
s = replace_once(s, "mod face_search_panel;\n", "mod face_search_panel;\nmod inspector;\n", "inspector module")
s = replace_once(s, "mod people_manager;\n", "mod people_manager;\nmod search_panel;\n", "search panel module")
s = replace_once(s, "mod thumbnails;\nmod views;\n", "mod thumbnails;\nmod ux;\nmod views;\n", "ux module")
start = s.find("    fn show_search_sidebar(&mut self, ctx: &egui::Context) {")
end = s.find("    fn show_close_confirmation", start)
if start < 0 or end < 0:
    raise SystemExit("legacy show_search_sidebar block not found")
s = s[:start] + s[end:]
s = replace_once(
    s,
    "        self.show_close_confirmation(ctx);\n\n        if self.busy || self.face_model_download_running() {",
    "        self.show_close_confirmation(ctx);\n        self.show_error_banner(ctx);\n\n        if self.busy || self.face_model_download_running() {",
    "global error banner",
)
s = replace_once(
    s,
    "        self.show_search_sidebar(ctx);\n        self.show_settings_window(ctx);",
    "        self.show_search_sidebar(ctx);\n        self.show_inspector(ctx);\n        self.show_settings_window(ctx);",
    "inspector call",
)
old_empty = '''            ui.separator();
            if visible.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(if self.images.is_empty() {
                        "No indexed images yet. Open Settings to add a folder, then Rescan."
                    } else {
                        "No images match the current filters."
                    });
                });
            } else if self.view_mode == ViewMode::Grid {
'''
new_empty = '''            ui.separator();
            self.show_active_filter_chips(ui);
            self.show_selection_bar(ui);
            if visible.is_empty() {
                self.show_empty_state(ui);
            } else if self.view_mode == ViewMode::Grid {
'''
s = replace_once(s, old_empty, new_empty, "results empty state")
p.write_text(s)


# Add Collection-facing helpers required by global chips and first-run onboarding.
p = Path("src/ui/collections.rs")
s = p.read_text()
needle = '''    pub(super) fn collection_filter_matches(&self, path: &Path) -> bool {
        self.collections.filter_matches(path)
    }
'''
insert = needle + '''
    pub(super) fn collection_filter_chip(&self) -> Option<String> {
        let id = self.collections.active_filter?;
        self.collections
            .items
            .iter()
            .find(|item| item.id == id)
            .map(|item| format!("Collection: {}", item.name))
    }

    pub(super) fn clear_collection_filter(&mut self) {
        self.collections.active_filter = None;
    }

    pub(super) fn prompt_add_library_folder(&mut self) {
        if self.busy {
            return;
        }
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };

        let collection_id = if let Some(id) = self.collections.selected_manage {
            id
        } else if let Some(item) = self.collections.items.first() {
            item.id
        } else {
            match db::create_collection(&self.db_path, "Library") {
                Ok(created) => {
                    self.collections.selected_manage = Some(created.id);
                    created.id
                }
                Err(err) => {
                    self.last_error = Some(format!("Cannot create the default Library collection: {err:#}"));
                    return;
                }
            }
        };

        self.apply_collection_action(CollectionAction::Drop(collection_id, vec![folder]));
    }
'''
s = replace_once(s, needle, insert, "collection UX helpers")
p.write_text(s)


# Add People-filter helpers required by the global filter chip row.
p = Path("src/ui/people_filter.rs")
s = p.read_text()
needle = '''    pub(super) fn people_filter_matches(&self, path: &Path) -> bool {
        self.people_filter_ui.resolved.matches(path)
    }
'''
insert = needle + '''
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
'''
s = replace_once(s, needle, insert, "people filter UX helpers")
p.write_text(s)


# Share byte formatting with the Inspector.
p = Path("src/ui/views.rs")
s = p.read_text()
s = replace_once(s, "fn format_bytes(bytes: u64) -> String {", "pub(super) fn format_bytes(bytes: u64) -> String {", "format_bytes visibility")
p.write_text(s)
