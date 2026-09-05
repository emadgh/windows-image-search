from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if new in text:
        return text
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


# main.rs
path = Path("src/main.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "mod ui;\nmod windows_shell;\n",
    "mod ui;\nmod update;\nmod windows_shell;\n",
    "main update module",
)
path.write_text(text, encoding="utf-8")

# ui/mod.rs
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
replacements = [
    (
        "mod thumbnails;\nmod ux;\n",
        "mod thumbnails;\nmod update_ui;\nmod ux;\n",
        "update ui module",
    ),
    (
        "use crate::thumbnail_cache;\nuse eframe::egui;\n",
        "use crate::thumbnail_cache;\nuse crate::update::{UpdateManager, UpdateSettings};\nuse eframe::egui;\n",
        "update imports",
    ),
    (
        "    settings_path: PathBuf,\n    face_embedding_settings: FaceEmbeddingSettings,\n",
        "    settings_path: PathBuf,\n    pub(super) update_settings: UpdateSettings,\n    pub(super) update_settings_path: PathBuf,\n    pub(super) update_manager: UpdateManager,\n    pub(super) update_install_requested: bool,\n    face_embedding_settings: FaceEmbeddingSettings,\n",
        "update app fields",
    ),
    (
        "        let settings_path = app_data_dir.join(\"performance-settings.ini\");\n        let indexing_settings = settings::load(&settings_path);\n        let face_settings_path = app_data_dir.join(\"face-embedding-settings.ini\");\n",
        "        let settings_path = app_data_dir.join(\"performance-settings.ini\");\n        let indexing_settings = settings::load(&settings_path);\n        let update_settings_path = app_data_dir.join(\"update-settings.ini\");\n        let update_settings = crate::update::load_settings(&update_settings_path);\n        let update_manager = UpdateManager::default();\n        if update_settings.auto_check {\n            update_manager.start_check(update_settings.auto_download);\n        }\n        let face_settings_path = app_data_dir.join(\"face-embedding-settings.ini\");\n",
        "update initialization",
    ),
    (
        "            indexing_settings,\n            settings_path,\n            face_embedding_settings,\n",
        "            indexing_settings,\n            settings_path,\n            update_settings,\n            update_settings_path,\n            update_manager,\n            update_install_requested: false,\n            face_embedding_settings,\n",
        "update self fields",
    ),
    (
        "        self.process_text_search_results();\n        self.handle_result_shortcuts(ctx);\n",
        "        self.process_text_search_results();\n        update_ui::process(self, ctx);\n        self.handle_result_shortcuts(ctx);\n",
        "update process call",
    ),
    (
        "\n        self.show_search_sidebar(ctx);\n        self.show_inspector(ctx);\n",
        "\n        update_ui::show_banner(self, ctx);\n        self.show_search_sidebar(ctx);\n        self.show_inspector(ctx);\n",
        "update banner call",
    ),
]
for old, new, label in replacements:
    text = replace_once(text, old, new, label)
path.write_text(text, encoding="utf-8")

# settings_window.rs
path = Path("src/ui/settings_window.rs")
text = path.read_text(encoding="utf-8")
replacements = [
    (
        "use crate::settings::{self, ClipExecutionProvider, IndexingSettings};\nuse eframe::egui;\n",
        "use crate::settings::{self, ClipExecutionProvider, IndexingSettings};\nuse crate::update::{self, UpdateStatus};\nuse eframe::egui;\n",
        "settings update import",
    ),
    (
        "    Performance,\n    Storage,\n}\n",
        "    Performance,\n    Storage,\n    Updates,\n}\n",
        "updates category",
    ),
    (
        "    const ALL: [Self; 5] = [\n        Self::Appearance,\n        Self::SearchClip,\n        Self::FacesPeople,\n        Self::Performance,\n        Self::Storage,\n    ];\n",
        "    const ALL: [Self; 6] = [\n        Self::Appearance,\n        Self::SearchClip,\n        Self::FacesPeople,\n        Self::Performance,\n        Self::Storage,\n        Self::Updates,\n    ];\n",
        "settings categories all",
    ),
    (
        "            Self::Performance => \"Performance\",\n            Self::Storage => \"Storage\",\n",
        "            Self::Performance => \"Performance\",\n            Self::Storage => \"Storage\",\n            Self::Updates => \"Updates\",\n",
        "updates category label",
    ),
    (
        "            Self::Performance => 3,\n            Self::Storage => 4,\n",
        "            Self::Performance => 3,\n            Self::Storage => 4,\n            Self::Updates => 5,\n",
        "updates category index",
    ),
    (
        "            3 => Self::Performance,\n            4 => Self::Storage,\n            _ => Self::Appearance,\n",
        "            3 => Self::Performance,\n            4 => Self::Storage,\n            5 => Self::Updates,\n            _ => Self::Appearance,\n",
        "updates category from index",
    ),
    (
        "    save_performance_settings: bool,\n    save_face_settings: bool,\n}\n",
        "    save_performance_settings: bool,\n    save_face_settings: bool,\n    save_update_settings: bool,\n    check_updates: bool,\n    download_update: bool,\n}\n",
        "update effects",
    ),
    (
        "                                    SettingsCategory::Storage => {\n                                        settings_storage(app, ui, &mut effects)\n                                    }\n",
        "                                    SettingsCategory::Storage => {\n                                        settings_storage(app, ui, &mut effects)\n                                    }\n                                    SettingsCategory::Updates => {\n                                        settings_updates(app, ui, &mut effects)\n                                    }\n",
        "updates settings route",
    ),
]
for old, new, label in replacements:
    text = replace_once(text, old, new, label)

settings_updates = r'''fn settings_updates(app: &mut ImageSearchApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(
        ui,
        "Updates",
        "Windows Image Search checks GitHub Releases in the background. Update installation always requires an explicit restart confirmation.",
    );

    ui.strong(format!("Current version: {}", env!("CARGO_PKG_VERSION")));
    ui.small(
        "Channel: stable GitHub Releases. Prerelease builds are never installed automatically.",
    );
    ui.add_space(10.0);

    if ui
        .checkbox(
            &mut app.update_settings.auto_check,
            "Check for updates automatically at startup",
        )
        .changed()
    {
        effects.save_update_settings = true;
        if app.update_settings.auto_check {
            effects.check_updates = true;
        }
    }
    if ui
        .checkbox(
            &mut app.update_settings.auto_download,
            "Download verified updates automatically",
        )
        .on_hover_text(
            "Downloaded updates are SHA-256 verified. Installation still waits for Restart & install.",
        )
        .changed()
    {
        effects.save_update_settings = true;
        if app.update_settings.auto_download
            && matches!(app.update_manager.status(), UpdateStatus::Available(_))
        {
            effects.download_update = true;
        }
    }

    ui.add_space(12.0);
    ui.separator();
    ui.strong("Update status");
    let status = app.update_manager.status();
    match &status {
        UpdateStatus::Idle => {
            ui.label("Not checked in this session.");
        }
        UpdateStatus::Checking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking GitHub Releases…");
            });
        }
        UpdateStatus::UpToDate => {
            ui.label("You are running the newest stable release.");
        }
        UpdateStatus::Available(info) => {
            ui.label(format!("Version {} is available.", info.version));
        }
        UpdateStatus::Downloading {
            info,
            downloaded,
            total,
        } => {
            ui.label(format!("Downloading version {}…", info.version));
            let fraction = total
                .filter(|total| *total > 0)
                .map(|total| *downloaded as f32 / total as f32)
                .unwrap_or(0.0);
            let text = total
                .map(|total| {
                    format!(
                        "{:.1} / {:.1} MiB",
                        *downloaded as f64 / 1_048_576.0,
                        total as f64 / 1_048_576.0
                    )
                })
                .unwrap_or_else(|| format!("{:.1} MiB", *downloaded as f64 / 1_048_576.0));
            ui.add(
                egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                    .desired_width(ui.available_width().min(540.0))
                    .text(text),
            );
        }
        UpdateStatus::Ready(info, _) => {
            ui.label(format!(
                "Version {} is downloaded, verified, and ready.",
                info.version
            ));
        }
        UpdateStatus::Failed(error) => {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let checking = matches!(
            &status,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        );
        if ui
            .add_enabled(!checking, egui::Button::new("Check now"))
            .clicked()
        {
            effects.check_updates = true;
        }
        if matches!(&status, UpdateStatus::Available(_)) && ui.button("Download now").clicked() {
            effects.download_update = true;
        }
        if matches!(&status, UpdateStatus::Ready(_, _)) {
            let can_install = !app.busy && !app.face_model_download_running();
            if ui
                .add_enabled(can_install, egui::Button::new("Restart & install"))
                .on_hover_text(if can_install {
                    "Close the application, replace the executable, then restart"
                } else {
                    "Finish active indexing/search/model work before installing"
                })
                .clicked()
            {
                app.update_install_requested = true;
            }
        }
    });

    ui.add_space(10.0);
    ui.small("Security: updater traffic is HTTPS-only; the downloaded file must be a plausible Windows executable and must pass SHA-256 verification before installation.");
}

'''
marker = "fn settings_storage(app: &mut ImageSearchApp, ui: &mut egui::Ui, effects: &mut Effects) {\n"
if settings_updates not in text:
    if marker not in text:
        raise SystemExit("missing patch target: settings storage marker")
    text = text.replace(marker, settings_updates + marker, 1)

update_effects = '''    if effects.save_update_settings {
        if let Err(err) = update::save_settings(&app.update_settings_path, app.update_settings) {
            app.last_error = Some(format!("Cannot save update settings: {err}"));
        }
    }
    if effects.check_updates {
        app.update_manager
            .start_check(app.update_settings.auto_download);
    }
    if effects.download_update {
        app.update_manager.start_download();
    }

'''
marker = "    if effects.save_face_settings {\n"
if update_effects not in text:
    if marker not in text:
        raise SystemExit("missing patch target: update effects marker")
    text = text.replace(marker, update_effects + marker, 1)

path.write_text(text, encoding="utf-8")
