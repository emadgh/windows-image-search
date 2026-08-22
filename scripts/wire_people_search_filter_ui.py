from pathlib import Path

ui = Path('src/ui/mod.rs')
text = ui.read_text(encoding='utf-8')

replacements = [
    (
        'mod face_search_panel;\nmod people_manager;\n',
        'mod face_search_panel;\nmod people_filter;\nmod people_manager;\n',
    ),
    (
        '    face_search_ui: face_search_panel::FaceSearchUiState,\n    people_manager_ui: people_manager::PeopleManagerUiState,\n',
        '    face_search_ui: face_search_panel::FaceSearchUiState,\n    people_filter_ui: people_filter::PeopleFilterUiState,\n    people_manager_ui: people_manager::PeopleManagerUiState,\n',
    ),
    (
        '        let face_search_ui = face_search_panel::FaceSearchUiState::default();\n        let people_manager_ui = people_manager::PeopleManagerUiState::default();\n',
        '        let face_search_ui = face_search_panel::FaceSearchUiState::default();\n        let people_filter_ui = people_filter::PeopleFilterUiState::default();\n        let people_manager_ui = people_manager::PeopleManagerUiState::default();\n',
    ),
    (
        '            face_runtime,\n            face_search_ui,\n            people_manager_ui,\n',
        '            face_runtime,\n            face_search_ui,\n            people_filter_ui,\n            people_manager_ui,\n',
    ),
    (
        '                if !self.collection_filter_matches(&record.path) {\n                    return false;\n                }\n                if text_filter_active {\n',
        '                if !self.collection_filter_matches(&record.path) {\n                    return false;\n                }\n                if !self.people_filter_matches(&record.path) {\n                    return false;\n                }\n                if text_filter_active {\n',
    ),
    (
        '                    ui.heading("Search");\n                    self.show_collection_filter(ui);\n                    ui.add(\n',
        '                    ui.heading("Search");\n                    self.show_collection_filter(ui);\n                    self.show_people_filter(ui);\n                    ui.add_space(6.0);\n                    ui.add(\n',
    ),
    (
        '        self.process_face_runtime_messages();\n        self.process_face_search_messages();\n        self.process_fs_watch_messages();\n',
        '        self.process_face_runtime_messages();\n        self.process_face_search_messages();\n        self.process_people_filter_messages();\n        self.process_fs_watch_messages();\n',
    ),
    (
        '        if self.text_search_pending || self.text_search_due.is_some() {\n            ctx.request_repaint_after(Duration::from_millis(50));\n        }\n',
        '        if self.text_search_pending\n            || self.text_search_due.is_some()\n            || self.people_filter_work_pending()\n        {\n            ctx.request_repaint_after(Duration::from_millis(50));\n        }\n',
    ),
]

for old, new in replacements:
    if old not in text:
        raise SystemExit(f'ui/mod.rs anchor missing:\n{old[:160]}')
    text = text.replace(old, new, 1)
ui.write_text(text, encoding='utf-8')

face_runtime = Path('src/ui/face_runtime.rs')
text = face_runtime.read_text(encoding='utf-8')
old = '                            self.refresh_face_suggestions();\n'
new = '                            self.refresh_face_suggestions();\n                            self.refresh_people_filter_catalog();\n'
if old not in text:
    raise SystemExit('face_runtime refresh anchor missing')
text = text.replace(old, new, 1)
face_runtime.write_text(text, encoding='utf-8')

manager = Path('src/ui/people_manager.rs')
text = manager.read_text(encoding='utf-8')
old = '        self.refresh_people_manager();\n        self.refresh_face_suggestions();\n'
new = '        self.refresh_people_manager();\n        self.refresh_face_suggestions();\n        self.refresh_people_filter_catalog();\n'
if old not in text:
    raise SystemExit('people_manager refresh anchor missing')
text = text.replace(old, new, 1)
manager.write_text(text, encoding='utf-8')
