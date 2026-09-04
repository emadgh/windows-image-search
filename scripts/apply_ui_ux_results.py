from pathlib import Path


def once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


# Sorting state, keyboard hook, and a clearer results toolbar.
p = Path("src/ui/mod.rs")
s = p.read_text()
needle = '''#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ThumbnailFit {
    Contain,
    Cover,
}
'''
insert = needle + '''
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SortMode {
    Relevance,
    Name,
    Modified,
    Size,
    Resolution,
}

impl SortMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Relevance => "Relevance",
            Self::Name => "Name",
            Self::Modified => "Modified",
            Self::Size => "File size",
            Self::Resolution => "Resolution",
        }
    }
}
'''
s = once(s, needle, insert, "SortMode")
s = once(
    s,
    "    pub(super) thumb_fit: ThumbnailFit,\n",
    "    pub(super) thumb_fit: ThumbnailFit,\n    pub(super) sort_mode: SortMode,\n",
    "sort field",
)
s = once(
    s,
    "            thumb_fit: ThumbnailFit::Contain,\n",
    "            thumb_fit: ThumbnailFit::Contain,\n            sort_mode: SortMode::Relevance,\n",
    "sort default",
)
start = s.find("    pub(super) fn visible_indices(&self) -> Vec<usize> {")
end = s.find("    fn observe_text_search_input", start)
if start < 0 or end < 0:
    raise SystemExit("visible_indices block not found")
new_visible = '''    pub(super) fn visible_indices(&self) -> Vec<usize> {
        let text_filter_active = !self.search_text.trim().is_empty();
        let source = self.source();
        let mut visible = source
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                if !self.collection_filter_matches(&record.path) {
                    return false;
                }
                if !self.people_filter_matches(&record.path) {
                    return false;
                }
                if text_filter_active {
                    let Some(matches) = &self.text_search_matches else {
                        return false;
                    };
                    if !matches.contains(&record.path) {
                        return false;
                    }
                }
                !self.color_enabled
                    || views::color_distance(record.dominant, self.target_color)
                        <= self.color_tolerance
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        match self.sort_mode {
            SortMode::Relevance => {}
            SortMode::Name => visible.sort_by(|a, b| {
                source[*a]
                    .file_name
                    .to_lowercase()
                    .cmp(&source[*b].file_name.to_lowercase())
            }),
            SortMode::Modified => {
                visible.sort_by(|a, b| source[*b].modified.cmp(&source[*a].modified))
            }
            SortMode::Size => visible.sort_by(|a, b| source[*b].size.cmp(&source[*a].size)),
            SortMode::Resolution => visible.sort_by(|a, b| {
                let a_pixels = source[*a].width as u64 * source[*a].height as u64;
                let b_pixels = source[*b].width as u64 * source[*b].height as u64;
                b_pixels.cmp(&a_pixels)
            }),
        }
        visible
    }

'''
s = s[:start] + new_visible + s[end:]
s = once(
    s,
    "        self.process_text_search_results();\n\n        if self.text_search_pending",
    "        self.process_text_search_results();\n        self.handle_result_shortcuts(ctx);\n\n        if self.text_search_pending",
    "shortcut hook",
)
s = s.replace('ui.button("⚙ Settings")', 'ui.button("Settings")')
s = s.replace('egui::Button::new("👥 People")', 'egui::Button::new("People")')
s = s.replace('egui::Button::new("⟳ Rescan")', 'egui::Button::new("Rescan")')
old_toolbar = '''                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let view_label = match self.view_mode {
                        ViewMode::Grid => "▦ Grid",
                        ViewMode::Details => "☷ Details",
                    };
                    if ui.button(view_label).clicked() {
                        self.view_mode = match self.view_mode {
                            ViewMode::Grid => ViewMode::Details,
                            ViewMode::Details => ViewMode::Grid,
                        };
                    }

                    let fit_label = match self.thumb_fit {
                        ThumbnailFit::Contain => "Contain",
                        ThumbnailFit::Cover => "Cover",
                    };
                    if ui.button(fit_label).clicked() {
                        self.thumb_fit = match self.thumb_fit {
                            ThumbnailFit::Contain => ThumbnailFit::Cover,
                            ThumbnailFit::Cover => ThumbnailFit::Contain,
                        };
                    }

                    ui.add(
                        egui::Slider::new(&mut self.thumb_size, 96.0..=512.0)
                            .text("Thumbnail")
                            .suffix(" px"),
                    );
                });
'''
new_toolbar = '''                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("result-sort")
                        .selected_text(self.sort_mode.label())
                        .width(112.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.sort_mode, SortMode::Relevance, "Relevance");
                            ui.selectable_value(&mut self.sort_mode, SortMode::Name, "Name");
                            ui.selectable_value(&mut self.sort_mode, SortMode::Modified, "Modified");
                            ui.selectable_value(&mut self.sort_mode, SortMode::Size, "File size");
                            ui.selectable_value(&mut self.sort_mode, SortMode::Resolution, "Resolution");
                        });
                    ui.small("Sort");
                    ui.separator();
                    ui.selectable_value(&mut self.view_mode, ViewMode::Details, "Details");
                    ui.selectable_value(&mut self.view_mode, ViewMode::Grid, "Grid");
                    ui.separator();

                    let fit_label = match self.thumb_fit {
                        ThumbnailFit::Contain => "Fit: Contain",
                        ThumbnailFit::Cover => "Fit: Cover",
                    };
                    if ui.button(fit_label).clicked() {
                        self.thumb_fit = match self.thumb_fit {
                            ThumbnailFit::Contain => ThumbnailFit::Cover,
                            ThumbnailFit::Cover => ThumbnailFit::Contain,
                        };
                    }

                    ui.add(
                        egui::Slider::new(&mut self.thumb_size, 96.0..=512.0)
                            .text("Size")
                            .suffix(" px"),
                    );
                });
'''
s = once(s, old_toolbar, new_toolbar, "results toolbar")
p.write_text(s)


# Keyboard-first result actions.
p = Path("src/ui/ux.rs")
s = p.read_text()
marker = '''    pub(super) fn show_error_banner(&mut self, ctx: &egui::Context) {
'''
method = '''    pub(super) fn handle_result_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() || self.settings_open || self.close_confirmation_open {
            return;
        }

        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.selected_paths.clear();
        }

        if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::A)) {
            let source = self.source();
            let paths = self
                .visible_indices()
                .into_iter()
                .filter_map(|index| source.get(index).map(|record| record.path.clone()))
                .collect::<Vec<_>>();
            self.selected_paths.clear();
            self.selected_paths.extend(paths);
        }

        if ctx.input(|input| input.key_pressed(egui::Key::Enter)) {
            if let Some(path) = self.selected_path() {
                let _ = open::that(path);
            }
        }
    }

'''
s = once(s, marker, method + marker, "keyboard shortcuts")
p.write_text(s)


# Shared neutral thumbnail-loading tile.
p = Path("src/ui/photo_grid.rs")
s = p.read_text()
marker = '''pub(super) fn photo_tile(
'''
loading = '''pub(super) fn loading_tile(
    ui: &mut egui::Ui,
    desired: egui::Vec2,
    selected: bool,
    sense: egui::Sense,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(desired, sense);
    ui.painter()
        .rect_filled(rect, 5.0, ui.visuals().extreme_bg_color);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Loading…",
        egui::FontId::proportional(12.0),
        ui.visuals().weak_text_color(),
    );
    if selected {
        ui.painter().rect_stroke(
            rect,
            5.0,
            egui::Stroke::new(3.0, ui.visuals().selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    }
    response
}

'''
s = once(s, marker, loading + marker, "loading tile helper")
p.write_text(s)


# Use neutral loading tile in the main Grid and Details views.
p = Path("src/ui/views.rs")
s = p.read_text()
old_grid = '''            } else {
                let response = ui.add_sized(
                    [self.thumb_size, self.thumb_size],
                    egui::Button::new("Loading thumbnail…").sense(egui::Sense::click_and_drag()),
                );
                if selected {
                    ui.painter().rect_stroke(
                        response.rect,
                        5.0,
                        egui::Stroke::new(3.0, ui.visuals().selection.stroke.color),
                        egui::StrokeKind::Inside,
                    );
                }
                response
            };
'''
new_grid = '''            } else {
                photo_grid::loading_tile(
                    ui,
                    egui::vec2(self.thumb_size, self.thumb_size),
                    selected,
                    egui::Sense::click_and_drag(),
                )
            };
'''
s = once(s, old_grid, new_grid, "main grid loading tile")
old_details = '''                        } else {
                            ui.add_sized(
                                [widths.thumb, 54.0],
                                egui::Button::new("…").sense(egui::Sense::click_and_drag()),
                            )
                        };
'''
new_details = '''                        } else {
                            photo_grid::loading_tile(
                                ui,
                                egui::vec2(widths.thumb, 54.0),
                                selected,
                                egui::Sense::click_and_drag(),
                            )
                        };
'''
s = once(s, old_details, new_details, "details loading tile")
p.write_text(s)


# Apply the same loading affordance in Face Search grids.
p = Path("src/ui/face_search_panel.rs")
s = p.read_text()
s = s.replace(
    'ui.add_sized([104.0, 104.0], egui::Button::new("Loading…"))',
    'photo_grid::loading_tile(ui, egui::vec2(104.0, 104.0), is_selected, egui::Sense::click())',
)
old_db = '''                    } else {
                        let response = ui.add_sized([96.0, 96.0], egui::Button::new("Loading…"));
                        if is_selected {
                            ui.painter().rect_stroke(
                                response.rect,
                                5.0,
                                egui::Stroke::new(3.0, ui.visuals().selection.stroke.color),
                                egui::StrokeKind::Inside,
                            );
                        }
                        response
                    };
'''
new_db = '''                    } else {
                        photo_grid::loading_tile(
                            ui,
                            egui::vec2(96.0, 96.0),
                            is_selected,
                            egui::Sense::click(),
                        )
                    };
'''
s = once(s, old_db, new_db, "face database loading tile")
p.write_text(s)
