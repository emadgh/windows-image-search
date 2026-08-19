from pathlib import Path

path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")

old_sidebar = '''                    ui.add_space(10.0);
                    ui.separator();
                    ui.strong("View");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.view_mode, ViewMode::Grid, "▦ Grid");
                        ui.selectable_value(&mut self.view_mode, ViewMode::Details, "☷ Details");
                    });
                    if self.view_mode == ViewMode::Grid {
                        ui.add(
                            egui::Slider::new(&mut self.thumb_size, 96.0..=512.0)
                                .text("Thumbnail")
                                .suffix(" px"),
                        );
                    }
                    ui.horizontal(|ui| {
                        ui.label("Image fit:");
                        ui.selectable_value(&mut self.thumb_fit, ThumbnailFit::Contain, "Contain");
                        ui.selectable_value(&mut self.thumb_fit, ThumbnailFit::Cover, "Cover");
                    });

'''
if old_sidebar not in text:
    raise SystemExit("sidebar view block not found")
text = text.replace(old_sidebar, "", 1)

old_header = '''            ui.horizontal(|ui| {
                ui.strong(format!(
                    "{} result{}",
                    visible.len(),
                    if visible.len() == 1 { "" } else { "s" }
                ));
                if self.similarity_results.is_some() {
                    ui.small("Hybrid similarity order using current weights");
                }
            });
'''

new_header = '''            ui.horizontal(|ui| {
                ui.strong(format!(
                    "{} result{}",
                    visible.len(),
                    if visible.len() == 1 { "" } else { "s" }
                ));
                if self.similarity_results.is_some() {
                    ui.small("Hybrid similarity order using current weights");
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
            });
'''
if old_header not in text:
    raise SystemExit("results header block not found")
text = text.replace(old_header, new_header, 1)

path.write_text(text, encoding="utf-8")
