from pathlib import Path
import re


def replace_once(path: str, old: str, new: str):
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if old not in text:
        raise SystemExit(f"expected anchor missing in {path}: {old[:160]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


def replace_span(path: str, start: str, end: str, replacement: str):
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    start_at = text.find(start)
    if start_at < 0:
        raise SystemExit(f"start anchor missing in {path}: {start[:160]!r}")
    end_at = text.find(end, start_at)
    if end_at < 0:
        raise SystemExit(f"end anchor missing in {path}: {end[:160]!r}")
    p.write_text(text[:start_at] + replacement + text[end_at:], encoding="utf-8")


def replace_face_crop_calls(path: str):
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    pattern = re.compile(
        r"face_crop_widget\(\s*ui,\s*&texture,\s*([A-Za-z0-9_\.]+),\s*(egui::vec2\([^\n]+?\)),\s*([A-Za-z0-9_]+),\s*\)",
        re.MULTILINE,
    )
    text, count = pattern.subn(
        r"photo_grid::photo_tile(ui, &texture, \2, PhotoTileMode::Face(\1), \3, egui::Sense::click())",
        text,
    )
    if count == 0:
        raise SystemExit(f"no face_crop_widget calls replaced in {path}")
    p.write_text(text, encoding="utf-8")


replace_once(
    "src/ui/mod.rs",
    "mod people_manager;\n",
    "mod people_manager;\nmod photo_grid;\n",
)

replace_once(
    "src/ui/views.rs",
    "use super::{ImageSearchApp, ThumbnailFit};\n",
    "use super::photo_grid::{self, PhotoGridSpec, PhotoTileMode};\nuse super::{ImageSearchApp, ThumbnailFit};\n",
)

replace_span(
    "src/ui/views.rs",
    "    pub(super) fn show_grid(&mut self, ui: &mut egui::Ui, visible: &[usize]) {\n",
    "    pub(super) fn show_details(&mut self, ui: &mut egui::Ui, visible: &[usize]) {\n",
    '''    pub(super) fn show_grid(&mut self, ui: &mut egui::Ui, visible: &[usize]) {
        let cell_width = self.thumb_size + 24.0;
        let row_height = self.thumb_size + 62.0;
        let fit = self.thumb_fit;
        let spec = PhotoGridSpec::new("main-result-photo-grid", cell_width, row_height);

        photo_grid::show(ui, visible.len(), spec, |ui, pos| {
            let record = self.record_view(visible[pos]);
            let selected = self.selected_paths.contains(&record.path);

            let response = if let Some(texture) = self.thumbnail(&record.path) {
                let response = photo_grid::photo_tile(
                    ui,
                    &texture,
                    egui::vec2(self.thumb_size, self.thumb_size),
                    PhotoTileMode::Full(fit),
                    selected,
                    egui::Sense::click_and_drag(),
                );
                if let Some(bbox) = self.face_match_box(&record.path) {
                    draw_face_match_box(ui, &texture, response.rect, fit, bbox);
                }
                response
            } else {
                let response = ui.add_sized(
                    [self.thumb_size, self.thumb_size],
                    egui::Button::new("Loading thumbnail…")
                        .sense(egui::Sense::click_and_drag()),
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

            self.handle_result_response(&response, &record.path);
            self.attach_collection_drag_source(&response, &record.path);

            let label = ui.add(
                egui::Label::new(truncate_middle(&record.file_name, 30))
                    .sense(egui::Sense::click_and_drag()),
            );
            self.handle_result_response(&label, &record.path);
            self.attach_collection_drag_source(&label, &record.path);
            label.on_hover_text(record.path.display().to_string());

            ui.horizontal(|ui| {
                swatch(ui, record.dominant);
                ui.small(format!("{}×{}", record.width, record.height));
                if let Some(score) = record.score {
                    ui.small(format!("{:.1}%", score * 100.0));
                }
            });
        });
    }

''',
)

replace_once(
    "src/ui/face_search_panel.rs",
    "use super::ImageSearchApp;\n",
    "use super::photo_grid::{self, PhotoGridSpec, PhotoTileMode};\nuse super::ImageSearchApp;\n",
)
replace_face_crop_calls("src/ui/face_search_panel.rs")

replace_span(
    "src/ui/face_search_panel.rs",
    "                let available = ui.available_width().max(300.0);\n",
    "            });\n        self.face_search_ui.open = open;\n",
    '''                let spec = PhotoGridSpec::new("face-search-database-photo-grid", 108.0, 142.0);
                photo_grid::show(ui, suggestions.len(), spec, |ui, index| {
                    let face = &suggestions[index];
                    let is_selected = self
                        .face_search_ui
                        .selected_face_id
                        .as_ref()
                        .is_some_and(|id| id == &face.face_id);
                    let response = if let Some(texture) = self.thumbnail(&face.image_path) {
                        photo_grid::photo_tile(
                            ui,
                            &texture,
                            egui::vec2(96.0, 96.0),
                            PhotoTileMode::Face(face.bbox),
                            is_selected,
                            egui::Sense::click(),
                        )
                    } else {
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
                    if response.clicked() {
                        self.face_search_ui.selected_face_id = Some(face.face_id.clone());
                    }
                    if response.double_clicked() && !self.busy {
                        self.start_indexed_face_search(face.clone());
                    }
                    if let Some(name) = self.face_search_ui.suggestion_names.get(&face.face_id) {
                        ui.strong(truncate(name, 16));
                    }
                    if let Some(group_size) = face.group_size {
                        ui.small(format!(
                            "Person · {group_size} face{}",
                            if group_size == 1 { "" } else { "s" }
                        ));
                    } else {
                        ui.small(format!("{:.0}%", face.confidence * 100.0));
                    }
                    ui.small(
                        face.image_path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .map(|name| truncate(name, 16))
                            .unwrap_or_else(|| "image".to_owned()),
                    )
                    .on_hover_text(face.image_path.display().to_string());
                });
''',
)

# Remove Face Search's duplicate crop renderer; the shared component owns it now.
p = Path("src/ui/face_search_panel.rs")
text = p.read_text(encoding="utf-8")
start = text.find("\nfn face_crop_widget(\n")
end = text.find("\nfn truncate(", start)
if start < 0 or end < 0:
    raise SystemExit("face_search_panel.rs duplicate crop renderer not found")
p.write_text(text[:start] + "\n" + text[end:], encoding="utf-8")

replace_once(
    "src/ui/people_manager.rs",
    "use super::ImageSearchApp;\n",
    "use super::photo_grid::{self, PhotoGridSpec, PhotoTileMode};\nuse super::ImageSearchApp;\n",
)
replace_face_crop_calls("src/ui/people_manager.rs")

replace_span(
    "src/ui/people_manager.rs",
    "                        egui::ScrollArea::vertical()\n                            .id_salt(\"people-manager-members\")\n",
    "\n\n                        if let Some((library_id, face_id)) =\n",
    '''                        let member_grid = PhotoGridSpec::new(
                            "people-manager-members",
                            94.0,
                            108.0,
                        )
                        .max_height(250.0);
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
''',
)

# Remove People's duplicate crop renderer; shared Face crop uses the Face Search framing.
p = Path("src/ui/people_manager.rs")
text = p.read_text(encoding="utf-8")
start = text.find("\nfn face_crop_widget(\n")
if start < 0:
    raise SystemExit("people_manager.rs duplicate crop renderer not found")
p.write_text(text[:start].rstrip() + "\n", encoding="utf-8")
