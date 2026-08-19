use super::{ImageSearchApp, ThumbnailFit};
use chrono::{Local, TimeZone};
use eframe::egui;
use std::path::{Path, PathBuf};

#[derive(Clone)]
struct RecordView {
    path: PathBuf,
    root: PathBuf,
    file_name: String,
    extension: String,
    size: u64,
    modified: i64,
    width: u32,
    height: u32,
    description: String,
    keywords: String,
    dominant: [u8; 3],
    score: Option<f32>,
}

impl ImageSearchApp {
    fn record_view(&self, index: usize) -> RecordView {
        let record = &self.source()[index];
        RecordView {
            path: record.path.clone(),
            root: record.root.clone(),
            file_name: record.file_name.clone(),
            extension: record.extension.clone(),
            size: record.size,
            modified: record.modified,
            width: record.width,
            height: record.height,
            description: record.description.clone(),
            keywords: record.keywords.clone(),
            dominant: record.dominant,
            score: record.score,
        }
    }

    pub(super) fn show_grid(&mut self, ui: &mut egui::Ui, visible: &[usize]) {
        let cell_width = self.thumb_size + 24.0;
        let columns = ((ui.available_width() / cell_width).floor() as usize).max(1);
        let rows = visible.len().div_ceil(columns);
        let row_height = self.thumb_size + 62.0;

        egui::ScrollArea::vertical().show_rows(ui, row_height, rows, |ui, row_range| {
            for row in row_range {
                ui.horizontal(|ui| {
                    for column in 0..columns {
                        let pos = row * columns + column;
                        if pos >= visible.len() {
                            break;
                        }
                        let record = self.record_view(visible[pos]);
                        let selected = self.selected_paths.contains(&record.path);
                        let fit = self.thumb_fit;

                        ui.allocate_ui_with_layout(
                            egui::vec2(cell_width, row_height),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| {
                                let response = if let Some(texture) = self.thumbnail(&record.path) {
                                    thumbnail_widget(
                                        ui,
                                        &texture,
                                        egui::vec2(self.thumb_size, self.thumb_size),
                                        fit,
                                        selected,
                                        egui::Sense::click_and_drag(),
                                    )
                                } else {
                                    let response = ui.add_sized(
                                        [self.thumb_size, self.thumb_size],
                                        egui::Button::new("Loading thumbnail…")
                                            .sense(egui::Sense::click_and_drag()),
                                    );
                                    if selected {
                                        ui.painter().rect_stroke(
                                            response.rect,
                                            4.0,
                                            egui::Stroke::new(
                                                2.0,
                                                ui.visuals().selection.stroke.color,
                                            ),
                                            egui::StrokeKind::Inside,
                                        );
                                    }
                                    response
                                };

                                self.handle_result_response(&response, &record.path);
                                self.attach_collection_drag_source(&response, &record.path);
                                response.context_menu(|ui| file_context_menu(ui, &record.path));

                                let label = ui.add(
                                    egui::Label::new(truncate_middle(&record.file_name, 30))
                                        .sense(egui::Sense::click_and_drag()),
                                );
                                self.handle_result_response(&label, &record.path);
                                self.attach_collection_drag_source(&label, &record.path);
                                label.context_menu(|ui| file_context_menu(ui, &record.path));
                                label.on_hover_text(record.path.display().to_string());

                                ui.horizontal(|ui| {
                                    swatch(ui, record.dominant);
                                    ui.small(format!("{}×{}", record.width, record.height));
                                    if let Some(score) = record.score {
                                        ui.small(format!("{:.1}%", score * 100.0));
                                    }
                                });
                            },
                        );
                    }
                });
            }
        });
    }

    pub(super) fn show_details(&mut self, ui: &mut egui::Ui, visible: &[usize]) {
        let available = ui.available_width().max(720.0);
        let widths = DetailWidths::from_total(available);
        detail_header(ui, widths);
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, 66.0, visible.len(), |ui, range| {
                for row in range {
                    let record = self.record_view(visible[row]);
                    let selected = self.selected_paths.contains(&record.path);
                    let fit = self.thumb_fit;
                    let available = ui.available_width().max(720.0);
                    let widths = DetailWidths::from_total(available);

                    ui.horizontal(|ui| {
                        let response = if let Some(texture) = self.thumbnail(&record.path) {
                            thumbnail_widget(
                                ui,
                                &texture,
                                egui::vec2(widths.thumb, 54.0),
                                fit,
                                selected,
                                egui::Sense::click_and_drag(),
                            )
                        } else {
                            ui.add_sized(
                                [widths.thumb, 54.0],
                                egui::Button::new("…").sense(egui::Sense::click_and_drag()),
                            )
                        };
                        self.handle_result_response(&response, &record.path);
                        self.attach_collection_drag_source(&response, &record.path);
                        response.context_menu(|ui| file_context_menu(ui, &record.path));

                        ui.allocate_ui_with_layout(
                            egui::vec2(widths.name, 56.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                let name = ui.add(
                                    egui::Label::new(truncate_middle(&record.file_name, 44))
                                        .sense(egui::Sense::click_and_drag()),
                                );
                                self.handle_result_response(&name, &record.path);
                                self.attach_collection_drag_source(&name, &record.path);
                                name.context_menu(|ui| file_context_menu(ui, &record.path));
                                name.on_hover_text(record.path.display().to_string());
                                ui.small(truncate_middle(&record.root.display().to_string(), 48));
                            },
                        );

                        ui.allocate_ui_with_layout(
                            egui::vec2(widths.info, 56.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.label(format!(
                                    "{} × {}  {}",
                                    record.width,
                                    record.height,
                                    record.extension.to_ascii_uppercase()
                                ));
                                ui.small(format!(
                                    "{}  {}",
                                    format_bytes(record.size),
                                    format_modified(record.modified)
                                ));
                            },
                        );

                        ui.allocate_ui_with_layout(
                            egui::vec2(widths.color, 56.0),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| swatch(ui, record.dominant),
                        );

                        ui.allocate_ui_with_layout(
                            egui::vec2(widths.metadata, 56.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                let metadata = metadata_text(&record);
                                ui.add_sized(
                                    [widths.metadata, 42.0],
                                    egui::Label::new(truncate_middle(
                                        &metadata,
                                        metadata_chars(widths.metadata),
                                    ))
                                    .wrap(),
                                )
                                .on_hover_text(metadata);
                            },
                        );

                        ui.allocate_ui_with_layout(
                            egui::vec2(widths.score, 56.0),
                            egui::Layout::top_down(egui::Align::RIGHT),
                            |ui| {
                                if let Some(score) = record.score {
                                    ui.strong(format!("{:.2}%", score * 100.0));
                                } else {
                                    ui.label("—");
                                }
                            },
                        );
                    });
                    ui.separator();
                }
            });
    }

    fn handle_result_response(&mut self, response: &egui::Response, path: &Path) {
        if response.secondary_clicked() && !self.selected_paths.contains(path) {
            self.select_path(path, false);
        }
        if response.clicked() {
            let additive = response
                .ctx
                .input(|input| input.modifiers.ctrl || input.modifiers.command);
            self.select_path(path, additive);
        }
        if response.double_clicked() {
            let _ = open::that(path);
        }
    }
}

#[derive(Clone, Copy)]
struct DetailWidths {
    thumb: f32,
    name: f32,
    info: f32,
    color: f32,
    metadata: f32,
    score: f32,
}

impl DetailWidths {
    fn from_total(total: f32) -> Self {
        let thumb = 58.0;
        let color = 44.0;
        let score = 92.0;
        let gaps = 44.0;
        let usable = (total - thumb - color - score - gaps).max(480.0);
        let name = (usable * 0.28).clamp(170.0, 360.0);
        let info = (usable * 0.22).clamp(145.0, 280.0);
        let metadata = (usable - name - info).max(180.0);
        Self {
            thumb,
            name,
            info,
            color,
            metadata,
            score,
        }
    }
}

fn detail_header(ui: &mut egui::Ui, widths: DetailWidths) {
    ui.horizontal(|ui| {
        ui.add_sized([widths.thumb, 22.0], egui::Label::new(""));
        ui.add_sized(
            [widths.name, 22.0],
            egui::Label::new("Name").selectable(false),
        );
        ui.add_sized(
            [widths.info, 22.0],
            egui::Label::new("Info").selectable(false),
        );
        ui.add_sized(
            [widths.color, 22.0],
            egui::Label::new("Color").selectable(false),
        );
        ui.add_sized(
            [widths.metadata, 22.0],
            egui::Label::new("Metadata").selectable(false),
        );
        ui.add_sized(
            [widths.score, 22.0],
            egui::Label::new("Score").selectable(false),
        );
    });
}

fn metadata_text(record: &RecordView) -> String {
    match (record.keywords.is_empty(), record.description.is_empty()) {
        (false, false) => format!("{} | {}", record.keywords, record.description),
        (false, true) => record.keywords.clone(),
        (true, false) => record.description.clone(),
        (true, true) => "—".to_owned(),
    }
}

fn metadata_chars(width: f32) -> usize {
    ((width / 7.0) as usize).clamp(28, 120)
}

pub(super) fn show_query_preview(ui: &mut egui::Ui, texture: &egui::TextureHandle, width: f32) {
    let source = texture.size_vec2();
    let height = if source.x > 0.0 {
        (width * source.y / source.x).clamp(100.0, 240.0)
    } else {
        160.0
    };
    let _ = thumbnail_widget(
        ui,
        texture,
        egui::vec2(width, height),
        ThumbnailFit::Contain,
        false,
        egui::Sense::hover(),
    );
}

fn thumbnail_widget(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    desired: egui::Vec2,
    fit: ThumbnailFit,
    selected: bool,
    sense: egui::Sense,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(desired, sense);
    ui.painter()
        .rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);

    let source = texture.size_vec2();
    if source.x > 0.0 && source.y > 0.0 {
        match fit {
            ThumbnailFit::Contain => {
                let scale = (rect.width() / source.x).min(rect.height() / source.y);
                let size = source * scale;
                let paint_rect = egui::Rect::from_center_size(rect.center(), size);
                ui.painter().image(
                    texture.id(),
                    paint_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            ThumbnailFit::Cover => {
                let source_aspect = source.x / source.y;
                let target_aspect = rect.width() / rect.height().max(1.0);
                let uv = if source_aspect > target_aspect {
                    let visible = target_aspect / source_aspect;
                    let margin = (1.0 - visible) * 0.5;
                    egui::Rect::from_min_max(egui::pos2(margin, 0.0), egui::pos2(1.0 - margin, 1.0))
                } else {
                    let visible = source_aspect / target_aspect;
                    let margin = (1.0 - visible) * 0.5;
                    egui::Rect::from_min_max(egui::pos2(0.0, margin), egui::pos2(1.0, 1.0 - margin))
                };
                ui.painter()
                    .image(texture.id(), rect, uv, egui::Color32::WHITE);
            }
        }
    }

    if selected {
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(3.0, ui.visuals().selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    }
    response
}

fn file_context_menu(ui: &mut egui::Ui, path: &Path) {
    if ui.button("Open").clicked() {
        let _ = open::that(path);
        ui.close();
    }
    if ui.button("Show in Explorer").clicked() {
        show_in_explorer(path);
        ui.close();
    }
    if ui.button("Open containing folder").clicked() {
        if let Some(parent) = path.parent() {
            let _ = open::that(parent);
        }
        ui.close();
    }
    if ui.button("Copy path").clicked() {
        ui.ctx().copy_text(path.display().to_string());
        ui.close();
    }
}

fn show_in_explorer(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer.exe")
            .arg(format!("/select,\"{}\"", path.display()))
            .spawn();
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Some(parent) = path.parent() {
            let _ = open::that(parent);
        }
    }
}

fn swatch(ui: &mut egui::Ui, rgb: [u8; 3]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 3.0, egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
    ui.painter().rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0_f32, egui::Color32::GRAY),
        egui::StrokeKind::Inside,
    );
}

pub(super) fn color_distance(a: [u8; 3], b: [u8; 3]) -> f32 {
    let r_mean = (a[0] as f32 + b[0] as f32) * 0.5;
    let dr = a[0] as f32 - b[0] as f32;
    let dg = a[1] as f32 - b[1] as f32;
    let db = a[2] as f32 - b[2] as f32;
    let distance = ((2.0 + r_mean / 256.0) * dr * dr
        + 4.0 * dg * dg
        + (2.0 + (255.0 - r_mean) / 256.0) * db * db)
        .sqrt();
    (distance / 765.0).clamp(0.0, 1.0)
}

fn format_bytes(bytes: u64) -> String {
    let value = bytes as f64;
    if value >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", value / 1024.0 / 1024.0 / 1024.0)
    } else if value >= 1024.0 * 1024.0 {
        format!("{:.1} MB", value / 1024.0 / 1024.0)
    } else if value >= 1024.0 {
        format!("{:.0} KB", value / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn format_modified(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|v| v.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "—".into())
}

pub(super) fn truncate_middle(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_chars || max_chars < 5 {
        return value.to_owned();
    }
    let left = (max_chars - 1) / 2;
    let right = max_chars - 1 - left;
    format!(
        "{}…{}",
        chars[..left].iter().collect::<String>(),
        chars[chars.len() - right..].iter().collect::<String>()
    )
}
