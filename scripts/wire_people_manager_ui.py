from pathlib import Path

# Public face preview resolver used by People management.
p = Path('src/face_search.rs')
s = p.read_text(encoding='utf-8')
needle = 'pub fn list_searchable_faces(\n'
if 'pub fn resolve_searchable_face(' not in s:
    helper = '''pub fn resolve_searchable_face(\n    roots: &[PathBuf],\n    library_id: &str,\n    face_id: &str,\n) -> Result<Option<IndexedFaceSuggestion>> {\n    if library_id.trim().is_empty() || face_id.trim().is_empty() {\n        return Ok(None);\n    }\n    for root in roots {\n        let Ok(conn) = open_read_only(root) else {\n            continue;\n        };\n        let Ok(candidate_library_id) = portable_library_id(&conn) else {\n            continue;\n        };\n        if candidate_library_id != library_id {\n            continue;\n        }\n        return load_searchable_face_by_id(&conn, root, face_id);\n    }\n    Ok(None)\n}\n\n'''
    if needle not in s:
        raise SystemExit('face_search insertion point missing')
    s = s.replace(needle, helper + needle, 1)
p.write_text(s, encoding='utf-8')

# Let the sibling People Manager request a Face Search suggestion refresh after corrections.
p = Path('src/ui/face_search_panel.rs')
s = p.read_text(encoding='utf-8')
s = s.replace('    fn refresh_face_suggestions(&mut self) {', '    pub(super) fn refresh_face_suggestions(&mut self) {', 1)
p.write_text(s, encoding='utf-8')

p = Path('src/ui/mod.rs')
s = p.read_text(encoding='utf-8')
if 'mod people_manager;' not in s:
    s = s.replace('mod face_search_panel;\n', 'mod face_search_panel;\nmod people_manager;\n', 1)
if 'people_manager_ui: people_manager::PeopleManagerUiState,' not in s:
    s = s.replace(
        '    face_search_ui: face_search_panel::FaceSearchUiState,\n',
        '    face_search_ui: face_search_panel::FaceSearchUiState,\n    people_manager_ui: people_manager::PeopleManagerUiState,\n',
        1,
    )
if 'let people_manager_ui = people_manager::PeopleManagerUiState::default();' not in s:
    s = s.replace(
        '        let face_search_ui = face_search_panel::FaceSearchUiState::default();\n',
        '        let face_search_ui = face_search_panel::FaceSearchUiState::default();\n        let people_manager_ui = people_manager::PeopleManagerUiState::default();\n',
        1,
    )
if '            people_manager_ui,\n' not in s:
    s = s.replace('            face_search_ui,\n', '            face_search_ui,\n            people_manager_ui,\n', 1)

settings_button = '''                if ui.button("⚙ Settings").clicked() {\n                    self.settings_open = true;\n                }\n'''
if 'egui::Button::new("👥 People")' not in s:
    replacement = settings_button + '''                if ui\n                    .add_enabled(!self.busy, egui::Button::new("👥 People"))\n                    .clicked()\n                {\n                    self.open_people_manager();\n                }\n'''
    if settings_button not in s:
        raise SystemExit('top toolbar Settings button insertion point missing')
    s = s.replace(settings_button, replacement, 1)

if '        self.show_people_manager_window(ctx);\n' not in s:
    needle = '        self.show_face_search_window(ctx);\n'
    if needle not in s:
        raise SystemExit('window render insertion point missing')
    s = s.replace(needle, needle + '        self.show_people_manager_window(ctx);\n', 1)

p.write_text(s, encoding='utf-8')
