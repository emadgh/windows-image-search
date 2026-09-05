use super::ImageSearchApp;
use eframe::egui;

impl ImageSearchApp {
    fn has_background_activity(&self) -> bool {
        self.busy
            || self.indexing
            || self.searching
            || self.face_model_download_running()
            || !self.pending_fs_paths.is_empty()
    }

    pub(super) fn show_task_status_button(&mut self, ui: &mut egui::Ui) {
        let active = self.has_background_activity();
        let label = if active { "Tasks · Active" } else { "Tasks" };
        if ui.selectable_label(self.task_center_open, label).clicked() {
            self.task_center_open = !self.task_center_open;
        }
    }

    pub(super) fn show_task_center(&mut self, ctx: &egui::Context) {
        if !self.task_center_open {
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.task_center_open = false;
            return;
        }

        let mut open = self.task_center_open;
        egui::Window::new("Task Center")
            .open(&mut open)
            .resizable(true)
            .default_size([520.0, 330.0])
            .min_width(400.0)
            .show(ctx, |ui| {
                ui.heading("Background activity");
                ui.small("Indexing, search, model downloads and filesystem work are consolidated here.");
                ui.add_space(8.0);

                if self.has_background_activity() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            let title = if self.indexing {
                                if self.index_paused { "Indexing paused" } else { "Indexing library" }
                            } else if self.searching {
                                "Searching"
                            } else if self.face_model_download_running() {
                                "Downloading face model"
                            } else {
                                "Background work"
                            };
                            ui.strong(title);
                        });
                        ui.label(super::views::truncate_middle(&self.status, 120))
                            .on_hover_text(&self.status);

                        if let Some((label, downloaded, total)) = self.face_model_download_progress() {
                            let fraction = if total == 0 {
                                0.0
                            } else {
                                downloaded as f32 / total as f32
                            };
                            let detail = if total == 0 {
                                format!("Preparing {label} download…")
                            } else {
                                format!(
                                    "{label}: {:.1}% · {:.1}/{:.1} MB",
                                    fraction * 100.0,
                                    downloaded as f64 / 1_048_576.0,
                                    total as f64 / 1_048_576.0
                                )
                            };
                            ui.add(
                                egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                                    .desired_width(ui.available_width().min(440.0))
                                    .text(detail),
                            );
                        } else if let Some((done, total)) =
                            self.progress.filter(|(_, total)| *total > 0)
                        {
                            ui.add(
                                egui::ProgressBar::new(done as f32 / total as f32)
                                    .desired_width(ui.available_width().min(440.0))
                                    .text(format!("{done}/{total}")),
                            );
                        }
                        if let Some(file_name) = &self.current_file {
                            ui.small(format!("Current: {file_name}"));
                        }
                        if self.indexing && self.index_control.is_some() {
                            let label = if self.index_paused { "Resume indexing" } else { "Pause indexing" };
                            if ui
                                .add_enabled(!self.searching, egui::Button::new(label))
                                .clicked()
                            {
                                self.toggle_index_pause();
                            }
                        }
                    });
                } else {
                    ui.group(|ui| {
                        ui.strong("No active tasks");
                        ui.small("Background indexing and search activity will appear here when it starts.");
                    });
                }

                if !self.pending_fs_paths.is_empty() {
                    ui.add_space(8.0);
                    ui.group(|ui| {
                        ui.strong("Filesystem queue");
                        ui.label(format!(
                            "{} changed path{} waiting for the current operation to finish.",
                            self.pending_fs_paths.len(),
                            if self.pending_fs_paths.len() == 1 { " is" } else { "s are" }
                        ));
                    });
                }

                if let Some(reason) = &self.watcher_reconcile_required {
                    ui.add_space(8.0);
                    ui.group(|ui| {
                        ui.strong("Reconciliation recommended");
                        ui.label(reason);
                    });
                }

                if let Some(error) = &self.last_error {
                    ui.add_space(8.0);
                    ui.group(|ui| {
                        ui.strong("Needs attention");
                        ui.label(error);
                    });
                }
            });
        self.task_center_open = open;
    }
}
