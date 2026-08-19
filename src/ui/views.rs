use super::ImageSearchApp;
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

    pub(super) fn show_grid(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, visible: &[usize]) {
        let cell_width = self.thumb_size + 24.0;
        let columns = ((ui.available_width() / cell_width).floor() as usize).max(1);
        let rows = visible.len().div_ceil(columns);
        let row_height = self.thumb_size + 58.0;
        egui::ScrollArea::vertical().show_rows(ui, row_height, rows, |ui, row_range| {
            for row in row_range {
                ui.horizontal(|ui| {
                    for column in 0..columns {
                        let pos = row * columns + column;
                        if pos >= visible.len() {
                            break;
                        }
                        let record = self.record_view(visible[pos]);
                        ui.allocate_ui_with_layout(
                            egui::vec2(cell_width, row_height),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| {
                                let response =
                                    if let Some(texture) = self.thumbnail(ctx, &record.path) {
                                        ui.add(
                                            egui::Image::new((
                                                texture.id(),
                                                egui::vec2(self.thumb_size, self.thumb_size),
                                            ))
                                            .fit_to_exact_size(egui::vec2(
                                                self.thumb_size,
                                                self.thumb_size,
                                            ))
                                            .sense(egui::Sense::click()),
                                        )
                                    } else {
                                        ui.add_sized(
                                            [self.thumb_size, self.thumb_size],
                                            egui::Button::new("No preview"),
                                        )
                                    };
                                handle_file_response(&response, &record.path);
                                response.context_menu(|ui| file_context_menu(ui, &record.path));
                                ui.label(truncate_middle(&record.file_name, 30))
                                    .on_hover_text(record.path.display().to_string());
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

    pub(super) fn show_details(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        visible: &[usize],
    ) {
        ui.horizontal(|ui| {
            ui.strong("Name");
            ui.add_space(245.0);
            ui.strong("Info");
            ui.add_space(140.0);
            ui.strong("Metadata / score");
        });
        ui.separator();
        egui::ScrollArea::vertical().show_rows(ui, 62.0, visible.len(), |ui, range| {
            for row in range {
                let record = self.record_view(visible[row]);
                ui.horizontal(|ui| {
                    if let Some(texture) = self.thumbnail(ctx, &record.path) {
                        let response = ui.add(
                            egui::Image::new((texture.id(), egui::vec2(50.0, 50.0)))
                                .fit_to_exact_size(egui::vec2(50.0, 50.0))
                                .sense(egui::Sense::click()),
                        );
                        handle_file_response(&response, &record.path);
                        response.context_menu(|ui| file_context_menu(ui, &record.path));
                    } else {
                        ui.add_sized([50.0, 50.0], egui::Label::new("—"));
                    }
                    ui.vertical(|ui| {
                        let response = ui.add_sized(
                            [235.0, 20.0],
                            egui::Label::new(truncate_middle(&record.file_name, 36))
                                .sense(egui::Sense::click()),
                        );
                        handle_file_response(&response, &record.path);
                        response.context_menu(|ui| file_context_menu(ui, &record.path));
                        ui.add_sized(
                            [235.0, 18.0],
                            egui::Label::new(truncate_middle(
                                &record.root.display().to_string(),
                                38,
                            )),
                        );
                    });
                    ui.vertical(|ui| {
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
                    });
                    swatch(ui, record.dominant);
                    ui.vertical(|ui| {
                        let meta = if !record.keywords.is_empty() {
                            &record.keywords
                        } else {
                            &record.description
                        };
                        ui.add_sized([320.0, 20.0], egui::Label::new(truncate_middle(meta, 52)));
                        if let Some(score) = record.score {
                            ui.small(format!("Similarity: {:.2}%", score * 100.0));
                        }
                    });
                });
                ui.separator();
            }
        });
    }
}

fn handle_file_response(response: &egui::Response, path: &Path) {
    if response.double_clicked() {
        let _ = open::that(path);
    }
}

fn file_context_menu(ui: &mut egui::Ui, path: &Path) {
    if ui.button("Open").clicked() {
        let _ = open::that(path);
        ui.close();
    }
    if ui.button("Open containing folder").clicked() {
        open_containing_folder(path);
        ui.close();
    }
    if ui.button("Copy path").clicked() {
        ui.ctx().copy_text(path.display().to_string());
        ui.close();
    }
}

fn open_containing_folder(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer.exe")
            .arg(format!("/select,{}", path.display()))
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
        egui::Stroke::new(1.0, egui::Color32::GRAY),
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
