from pathlib import Path


def replace(path: str, old: str, new: str, count: int = 1) -> None:
    p = Path(path)
    text = p.read_text(encoding='utf-8')
    actual = text.count(old)
    if actual < count:
        raise RuntimeError(f'{path}: expected at least {count} occurrence(s), found {actual}: {old[:120]!r}')
    text = text.replace(old, new, count)
    p.write_text(text, encoding='utf-8')

replace(
    'src/main.rs',
    'mod face_pipeline;\n#[cfg(test)]',
    'mod face_pipeline;\nmod face_search;\n#[cfg(test)]',
)

replace(
    'src/ui/mod.rs',
    'mod face_runtime;\nmod texture_lru;',
    'mod face_runtime;\nmod face_search_panel;\nmod texture_lru;',
)
replace(
    'src/ui/mod.rs',
    '    face_runtime: face_runtime::FaceRuntimeState,\n    collections: collections::CollectionsState,',
    '    face_runtime: face_runtime::FaceRuntimeState,\n    face_search_ui: face_search_panel::FaceSearchUiState,\n    collections: collections::CollectionsState,',
)
replace(
    'src/ui/mod.rs',
    '        let face_runtime = face_runtime::FaceRuntimeState::new(app_data_dir);\n        let embedding_service = EmbeddingService::new(model_cache);',
    '        let face_runtime = face_runtime::FaceRuntimeState::new(app_data_dir);\n        let face_search_ui = face_search_panel::FaceSearchUiState::default();\n        let embedding_service = EmbeddingService::new(model_cache);',
)
replace(
    'src/ui/mod.rs',
    '            face_settings_path,\n            face_runtime,\n            collections: collections::CollectionsState::default(),',
    '            face_settings_path,\n            face_runtime,\n            face_search_ui,\n            collections: collections::CollectionsState::default(),',
)
replace(
    'src/ui/mod.rs',
    '        self.searching = true;\n        self.busy = true;\n        self.last_error = None;',
    '        self.clear_face_search_result_state();\n        self.searching = true;\n        self.busy = true;\n        self.last_error = None;',
)
replace(
    'src/ui/mod.rs',
    '                        if self.similarity_results.is_some()\n                            && ui.button("Clear image search").clicked()\n                        {\n                            self.similarity_results = None;\n                            self.query_image = None;\n                            self.selected_paths.clear();\n                        }',
    '                        if self.similarity_results.is_some()\n                            && ui.button("Clear image search").clicked()\n                        {\n                            self.similarity_results = None;\n                            self.query_image = None;\n                            self.selected_paths.clear();\n                            self.clear_face_search_result_state();\n                        }\n                        if ui\n                            .add_enabled(!self.busy, egui::Button::new("👤 Face Search"))\n                            .clicked()\n                        {\n                            self.open_face_search();\n                        }',
)
replace(
    'src/ui/mod.rs',
    '        self.process_worker_messages();\n        self.process_face_runtime_messages();\n        self.process_fs_watch_messages();',
    '        self.process_worker_messages();\n        self.process_face_runtime_messages();\n        self.process_face_search_messages();\n        self.process_fs_watch_messages();',
)
replace(
    'src/ui/mod.rs',
    '        self.show_search_sidebar(ctx);\n        self.show_settings_window(ctx);',
    '        self.show_search_sidebar(ctx);\n        self.show_settings_window(ctx);\n        self.show_face_search_window(ctx);',
)
replace(
    'src/ui/mod.rs',
    '                if self.similarity_results.is_some() {\n                    ui.small("Hybrid similarity order using current weights");\n                }',
    '                if self.face_search_active() {\n                    ui.small("Face identity similarity order");\n                } else if self.similarity_results.is_some() {\n                    ui.small("Hybrid similarity order using current weights");\n                }',
)

print('face-search UI wiring patched')
