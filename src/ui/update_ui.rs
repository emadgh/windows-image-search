use super::ImageSearchApp;
use crate::update::UpdateStatus;
use eframe::egui;
use std::time::Duration;

pub(super) fn install_blocked(app: &ImageSearchApp) -> bool {
    app.busy
        || app.face_model_download_running()
        || app.people_filter_work_pending()
        || app.text_search_pending
        || app.text_search_due.is_some()
}

pub(super) fn process(app: &mut ImageSearchApp, ctx: &egui::Context) {
    if matches!(
        app.update_manager.status(),
        UpdateStatus::Checking | UpdateStatus::Downloading { .. }
    ) {
        ctx.request_repaint_after(Duration::from_millis(120));
    }

    if !app.update_install_requested {
        return;
    }
    app.update_install_requested = false;
    if install_blocked(app) {
        app.last_error = Some(
            "Finish active indexing/search/model/People work before installing the update."
                .to_owned(),
        );
        return;
    }

    match app.update_manager.apply_ready() {
        Ok(true) => {
            app.allow_close = true;
            app.status = "Applying update and restarting…".to_owned();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        Ok(false) => {
            app.last_error = Some("No downloaded update is ready to install.".to_owned());
        }
        Err(error) => {
            app.last_error = Some(format!("Cannot install update: {error}"));
        }
    }
}

pub(super) fn show_banner(app: &mut ImageSearchApp, ctx: &egui::Context) {
    match app.update_manager.status() {
        UpdateStatus::Available(info) => {
            egui::TopBottomPanel::top("update-available-banner").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(format!(
                        "Windows Image Search {} is available",
                        info.version
                    ));
                    if ui.button("Download update").clicked() {
                        app.update_manager.start_download();
                    }
                    if ui.button("Open update settings").clicked() {
                        super::settings_window::open_updates(app, ctx);
                    }
                });
            });
        }
        UpdateStatus::Downloading {
            info,
            downloaded,
            total,
        } => {
            egui::TopBottomPanel::top("update-download-banner").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.small(format!("Downloading update {}…", info.version));
                    let fraction = total
                        .filter(|total| *total > 0)
                        .map(|total| downloaded as f32 / total as f32)
                        .unwrap_or(0.0);
                    let text = match total {
                        Some(total) if total > 0 => format!(
                            "{:.1} / {:.1} MiB",
                            downloaded as f64 / 1_048_576.0,
                            total as f64 / 1_048_576.0
                        ),
                        _ => format!("{:.1} MiB", downloaded as f64 / 1_048_576.0),
                    };
                    ui.add(
                        egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                            .desired_width(220.0)
                            .text(text),
                    );
                });
            });
        }
        UpdateStatus::Ready(info, _) => {
            egui::TopBottomPanel::top("update-ready-banner").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(format!("Update {} is ready to install", info.version));
                    let can_install = !install_blocked(app);
                    if ui
                        .add_enabled(can_install, egui::Button::new("Restart & install"))
                        .on_hover_text(if can_install {
                            "Close Windows Image Search, replace the executable, and restart"
                        } else {
                            "Finish active background work before installing"
                        })
                        .clicked()
                    {
                        app.update_install_requested = true;
                    }
                    if ui.button("Update settings").clicked() {
                        super::settings_window::open_updates(app, ctx);
                    }
                });
            });
        }
        _ => {}
    }
}
