use super::ImageSearchApp;
use chrono::{Local, TimeZone};
use eframe::egui;

impl ImageSearchApp {
    pub(super) fn show_inspector(&mut self, ctx: &egui::Context) {
        if self.selected_paths.is_empty() {
            return;
        }

        egui::SidePanel::right("image_inspector")
            .resizable(true)
            .default_width(320.0)
            .min_width(260.0)
            .max_width(430.0)
            .show(ctx, |ui| {
                ui.heading("Inspector");
                ui.separator();

                if self.selected_paths.len() > 1 {
                    ui.strong(format!("{} images selected", self.selected_paths.len()));
                    ui.add_space(6.0);
                    ui.label("Use the selection bar to copy paths or clear the current selection.");
                    return;
                }

                let Some(path) = self.selected_path() else {
                    return;
                };
                let record = self
                    .source()
                    .iter()
                    .find(|record| record.path == path)
                    .cloned()
                    .or_else(|| {
                        self.images
                            .iter()
                            .find(|record| record.path == path)
                            .cloned()
                    });
                let Some(record) = record else {
                    ui.label("The selected image is no longer available in the active catalog.");
                    return;
                };

                if let Some(texture) = self.thumbnail(&record.path) {
                    super::views::show_query_preview(ui, &texture, ui.available_width().min(300.0));
                } else {
                    ui.vertical_centered(|ui| {
                        ui.spinner();
                        ui.small("Loading preview…");
                    });
                }

                ui.add_space(8.0);
                ui.strong(&record.file_name);
                ui.small(super::views::truncate_middle(
                    &record.path.display().to_string(),
                    52,
                ))
                .on_hover_text(record.path.display().to_string());

                ui.add_space(10.0);
                ui.separator();
                ui.label(format!("{} × {} px", record.width, record.height));
                ui.label(format!(
                    "{} · {}",
                    record.extension.to_ascii_uppercase(),
                    super::views::format_bytes(record.size)
                ));
                if let Some(modified) = Local.timestamp_opt(record.modified, 0).single() {
                    ui.label(format!("Modified {}", modified.format("%Y-%m-%d %H:%M")));
                }
                if let Some(score) = record.score {
                    ui.label(format!("Similarity {:.1}%", score * 100.0));
                }

                ui.horizontal(|ui| {
                    let [r, g, b] = record.dominant;
                    let color = egui::Color32::from_rgb(r, g, b);
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 4.0, color);
                    ui.label(format!("Dominant #{r:02X}{g:02X}{b:02X}"));
                });

                if !record.keywords.trim().is_empty() || !record.description.trim().is_empty() {
                    ui.add_space(10.0);
                    ui.separator();
                    if !record.keywords.trim().is_empty() {
                        ui.strong("Keywords");
                        ui.label(&record.keywords);
                    }
                    if !record.description.trim().is_empty() {
                        ui.add_space(6.0);
                        ui.strong("Description");
                        ui.label(&record.description);
                    }
                }

                ui.add_space(12.0);
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Open").clicked() {
                        let _ = open::that(&record.path);
                    }
                    if ui.button("Open folder").clicked() {
                        if let Some(parent) = record.path.parent() {
                            let _ = open::that(parent);
                        }
                    }
                    if ui.button("Copy path").clicked() {
                        ui.ctx().copy_text(record.path.display().to_string());
                    }
                });
            });
    }
}
