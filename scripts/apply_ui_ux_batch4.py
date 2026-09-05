from pathlib import Path


def once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


# Selection anchor/focus state and range-selection helpers.
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    """    pub(super) selected_paths: HashSet<PathBuf>,\n    thumb_pool: ThumbnailPool,\n""",
    """    pub(super) selected_paths: HashSet<PathBuf>,\n    pub(super) selection_anchor: Option<PathBuf>,\n    pub(super) focused_result: Option<PathBuf>,\n    pub(super) result_grid_columns: usize,\n    thumb_pool: ThumbnailPool,\n""",
    "selection navigation fields",
)
text = once(
    text,
    """            selected_paths: HashSet::new(),\n            thumb_pool,\n""",
    """            selected_paths: HashSet::new(),\n            selection_anchor: None,\n            focused_result: None,\n            result_grid_columns: 1,\n            thumb_pool,\n""",
    "selection navigation defaults",
)
old = """    pub(super) fn select_path(&mut self, path: &Path, additive: bool) {\n        if additive {\n            if !self.selected_paths.insert(path.to_path_buf()) {\n                self.selected_paths.remove(path);\n            }\n        } else {\n            self.selected_paths.clear();\n            self.selected_paths.insert(path.to_path_buf());\n        }\n    }\n"""
new = """    pub(super) fn visible_result_paths(&self) -> Vec<PathBuf> {\n        let indices = self.visible_indices();\n        let source = self.source();\n        indices\n            .into_iter()\n            .filter_map(|index| source.get(index).map(|record| record.path.clone()))\n            .collect()\n    }\n\n    pub(super) fn select_path(&mut self, path: &Path, additive: bool) {\n        if additive {\n            if !self.selected_paths.insert(path.to_path_buf()) {\n                self.selected_paths.remove(path);\n            }\n        } else {\n            self.selected_paths.clear();\n            self.selected_paths.insert(path.to_path_buf());\n        }\n        self.selection_anchor = Some(path.to_path_buf());\n        self.focused_result = Some(path.to_path_buf());\n    }\n\n    pub(super) fn select_path_with_modifiers(\n        &mut self,\n        path: &Path,\n        additive: bool,\n        range: bool,\n    ) {\n        if !range {\n            self.select_path(path, additive);\n            return;\n        }\n\n        let visible = self.visible_result_paths();\n        let anchor = self\n            .selection_anchor\n            .as_ref()\n            .or(self.focused_result.as_ref())\n            .and_then(|anchor| visible.iter().position(|candidate| candidate == anchor));\n        let target = visible.iter().position(|candidate| candidate == path);\n        let (Some(anchor), Some(target)) = (anchor, target) else {\n            self.select_path(path, additive);\n            return;\n        };\n\n        if !additive {\n            self.selected_paths.clear();\n        }\n        let start = anchor.min(target);\n        let end = anchor.max(target);\n        self.selected_paths\n            .extend(visible[start..=end].iter().cloned());\n        self.focused_result = Some(path.to_path_buf());\n    }\n"""
text = once(text, old, new, "selection helper")
path.write_text(text, encoding="utf-8")


# Make result clicks support Shift ranges and remember grid geometry for arrows.
path = Path("src/ui/views.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    """        let fit = self.thumb_fit;\n        let spec = PhotoGridSpec::new(\"main-result-photo-grid\", cell_width, row_height);\n\n        photo_grid::show(ui, visible.len(), spec, |ui, pos| {\n""",
    """        let fit = self.thumb_fit;\n        let spec = PhotoGridSpec::new(\"main-result-photo-grid\", cell_width, row_height);\n        self.result_grid_columns = photo_grid::columns_for_width(\n            ui.available_width(),\n            cell_width,\n            ui.spacing().item_spacing.x,\n        );\n\n        photo_grid::show(ui, visible.len(), spec, |ui, pos| {\n""",
    "grid column tracking",
)
text = once(
    text,
    """    pub(super) fn show_details(&mut self, ui: &mut egui::Ui, visible: &[usize]) {\n        // Reserve the vertical scrollbar gutter once, then reuse one geometry for the\n""",
    """    pub(super) fn show_details(&mut self, ui: &mut egui::Ui, visible: &[usize]) {\n        self.result_grid_columns = 1;\n        // Reserve the vertical scrollbar gutter once, then reuse one geometry for the\n""",
    "details navigation geometry",
)
text = once(
    text,
    """        if response.clicked() {\n            let additive = response\n                .ctx\n                .input(|input| input.modifiers.ctrl || input.modifiers.command);\n            self.select_path(path, additive);\n        }\n""",
    """        if response.clicked() {\n            let modifiers = response.ctx.input(|input| input.modifiers);\n            let additive = modifiers.ctrl || modifiers.command;\n            self.select_path_with_modifiers(path, additive, modifiers.shift);\n        }\n""",
    "shift click selection",
)
path.write_text(text, encoding="utf-8")


# Arrow navigation, Shift+arrows, Space Inspector toggle, and predictable Esc.
path = Path("src/ui/ux.rs")
text = path.read_text(encoding="utf-8")
old = """    pub(super) fn handle_result_shortcuts(&mut self, ctx: &egui::Context) {\n        if ctx.wants_keyboard_input() || self.settings_open || self.close_confirmation_open {\n            return;\n        }\n\n        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {\n            self.selected_paths.clear();\n        }\n\n        if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::A)) {\n            let source = self.source();\n            let paths = self\n                .visible_indices()\n                .into_iter()\n                .filter_map(|index| source.get(index).map(|record| record.path.clone()))\n                .collect::<Vec<_>>();\n            self.selected_paths.clear();\n            self.selected_paths.extend(paths);\n        }\n\n        if ctx.input(|input| input.key_pressed(egui::Key::Enter)) {\n            if let Some(path) = self.selected_path() {\n                let _ = open::that(path);\n            }\n        }\n    }\n"""
new = """    pub(super) fn handle_result_shortcuts(&mut self, ctx: &egui::Context) {\n        if ctx.wants_keyboard_input() || self.settings_open || self.close_confirmation_open {\n            return;\n        }\n\n        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {\n            self.selected_paths.clear();\n            self.selection_anchor = None;\n            self.focused_result = None;\n            return;\n        }\n\n        if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::A)) {\n            let paths = self.visible_result_paths();\n            self.selected_paths.clear();\n            self.selected_paths.extend(paths.iter().cloned());\n            self.selection_anchor = paths.first().cloned();\n            self.focused_result = paths.last().cloned();\n            return;\n        }\n\n        if ctx.input(|input| input.key_pressed(egui::Key::Enter)) {\n            if let Some(path) = self.selected_path() {\n                let _ = open::that(path);\n            }\n            return;\n        }\n\n        if ctx.input(|input| input.key_pressed(egui::Key::Space)) {\n            if !self.selected_paths.is_empty() {\n                self.inspector_open = !self.inspector_open;\n            }\n            return;\n        }\n\n        let navigation = ctx.input(|input| {\n            let delta = if input.key_pressed(egui::Key::ArrowLeft) {\n                Some(-1_isize)\n            } else if input.key_pressed(egui::Key::ArrowRight) {\n                Some(1_isize)\n            } else if input.key_pressed(egui::Key::ArrowUp) {\n                Some(-(self.result_grid_columns.max(1) as isize))\n            } else if input.key_pressed(egui::Key::ArrowDown) {\n                Some(self.result_grid_columns.max(1) as isize)\n            } else {\n                None\n            };\n            delta.map(|delta| (delta, input.modifiers.shift))\n        });\n        let Some((delta, extend_range)) = navigation else {\n            return;\n        };\n\n        let visible = self.visible_result_paths();\n        if visible.is_empty() {\n            return;\n        }\n        let current = self\n            .focused_result\n            .as_ref()\n            .and_then(|path| visible.iter().position(|candidate| candidate == path))\n            .or_else(|| {\n                (self.selected_paths.len() == 1).then(|| {\n                    self.selected_paths\n                        .iter()\n                        .next()\n                        .and_then(|path| visible.iter().position(|candidate| candidate == path))\n                })\n                .flatten()\n            })\n            .unwrap_or(0);\n        let target = (current as isize + delta).clamp(0, visible.len() as isize - 1) as usize;\n        let target_path = visible[target].clone();\n        self.select_path_with_modifiers(&target_path, false, extend_range);\n    }\n"""
text = once(text, old, new, "keyboard navigation")
path.write_text(text, encoding="utf-8")
