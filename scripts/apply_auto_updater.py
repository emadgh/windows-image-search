from pathlib import Path


def replace(path: str, old: str, new: str, label: str) -> None:
    target = Path(path)
    text = target.read_text()
    if old not in text:
        raise SystemExit(f"{label}: anchor not found in {path}")
    target.write_text(text.replace(old, new, 1))


replace(
    "Cargo.toml",
    'sha2 = "0.10"\nureq = "2.12"',
    'sha2 = "0.10"\nupdate-via-github = { git = "https://github.com/emadgh/update-via-github.git", rev = "122a89ba8b9b00baf002453a10d829095b59b8a3" }\nureq = "2.12"',
    "Cargo dependency",
)
replace(
    "src/main.rs",
    "mod ui;\nmod windows_shell;",
    "mod ui;\nmod update;\nmod windows_shell;",
    "main update module",
)

Path("src/update.rs").write_text(r'''use std::path::Path;

pub use update_via_github::UpdateStatus;
use update_via_github::UpdateConfig;

const REPOSITORY: &str = "emadgh/windows-image-search";
const ASSET_NAME: &str = "windows-image-search.exe";
const CHECKSUM_ASSET_NAME: &str = "windows-image-search.exe.sha256";
const MAX_DOWNLOAD_SIZE: usize = 256 * 1024 * 1024;
const MIN_EXECUTABLE_SIZE: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdateSettings {
    pub auto_check: bool,
    pub auto_download: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_check: true,
            auto_download: true,
        }
    }
}

pub fn load_settings(path: &Path) -> UpdateSettings {
    let mut settings = UpdateSettings::default();
    let Ok(content) = std::fs::read_to_string(path) else {
        return settings;
    };
    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let parsed = matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );
        match key.trim() {
            "auto_check" => settings.auto_check = parsed,
            "auto_download" => settings.auto_download = parsed,
            _ => {}
        }
    }
    settings
}

pub fn save_settings(path: &Path, settings: UpdateSettings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        format!(
            "auto_check={}\nauto_download={}\n",
            settings.auto_check, settings.auto_download
        ),
    )
}

#[derive(Clone)]
pub struct UpdateManager {
    inner: update_via_github::UpdateManager,
}

impl Default for UpdateManager {
    fn default() -> Self {
        let config = UpdateConfig::new(REPOSITORY, ASSET_NAME, env!("CARGO_PKG_VERSION"))
            .with_app_name("WindowsImageSearch")
            .with_checksum_asset(CHECKSUM_ASSET_NAME)
            .with_required_checksum(true)
            .with_max_download_size(MAX_DOWNLOAD_SIZE)
            .with_min_executable_size(MIN_EXECUTABLE_SIZE);
        Self {
            inner: update_via_github::UpdateManager::new(config),
        }
    }
}

impl UpdateManager {
    pub fn status(&self) -> UpdateStatus {
        self.inner.status()
    }

    pub fn start_check(&self, auto_download: bool) -> bool {
        self.inner.start_check(auto_download)
    }

    pub fn start_download(&self) -> bool {
        self.inner.start_download()
    }

    pub fn apply_ready(&self) -> Result<bool, String> {
        self.inner.apply_ready()
    }
}
''')

Path("src/ui/update_ui.rs").write_text(r'''use super::ImageSearchApp;
use crate::update::UpdateStatus;
use eframe::egui;
use std::time::Duration;

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
    if app.busy || app.face_model_download_running() {
        app.last_error = Some(
            "Finish active indexing/search/model work before installing the update.".to_owned(),
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
                    ui.strong(format!("Windows Image Search {} is available", info.version));
                    if ui.button("Download update").clicked() {
                        app.update_manager.start_download();
                    }
                    if ui.button("Open update settings").clicked() {
                        app.settings_open = true;
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
                    let can_install = !app.busy && !app.face_model_download_running();
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
                        app.settings_open = true;
                    }
                });
            });
        }
        _ => {}
    }
}
''')

replace(
    "src/ui/mod.rs",
    "mod thumbnails;\nmod views;",
    "mod thumbnails;\nmod update_ui;\nmod views;",
    "ui module declaration",
)
replace(
    "src/ui/mod.rs",
    "use crate::settings::{self, ClipExecutionProvider, IndexingSettings};\nuse crate::text_search::TextSearchService;",
    "use crate::settings::{self, ClipExecutionProvider, IndexingSettings};\nuse crate::text_search::TextSearchService;\nuse crate::update::{UpdateManager, UpdateSettings};",
    "ui update imports",
)
replace(
    "src/ui/mod.rs",
    "    settings_path: PathBuf,\n    face_embedding_settings: FaceEmbeddingSettings,",
    "    settings_path: PathBuf,\n    pub(super) update_settings: UpdateSettings,\n    pub(super) update_settings_path: PathBuf,\n    pub(super) update_manager: UpdateManager,\n    pub(super) update_install_requested: bool,\n    face_embedding_settings: FaceEmbeddingSettings,",
    "ui update fields",
)
replace(
    "src/ui/mod.rs",
    '        let indexing_settings = settings::load(&settings_path);\n        let face_settings_path = app_data_dir.join("face-embedding-settings.ini");',
    '        let indexing_settings = settings::load(&settings_path);\n        let update_settings_path = app_data_dir.join("update-settings.ini");\n        let update_settings = crate::update::load_settings(&update_settings_path);\n        let update_manager = UpdateManager::default();\n        if update_settings.auto_check {\n            update_manager.start_check(update_settings.auto_download);\n        }\n        let face_settings_path = app_data_dir.join("face-embedding-settings.ini");',
    "ui update init",
)
replace(
    "src/ui/mod.rs",
    "            indexing_settings,\n            settings_path,\n            face_embedding_settings,",
    "            indexing_settings,\n            settings_path,\n            update_settings,\n            update_settings_path,\n            update_manager,\n            update_install_requested: false,\n            face_embedding_settings,",
    "ui update assignments",
)
replace(
    "src/ui/mod.rs",
    "        self.process_text_search_results();\n\n        if self.text_search_pending",
    "        self.process_text_search_results();\n        update_ui::process(self, ctx);\n\n        if self.text_search_pending",
    "ui update processing",
)
replace(
    "src/ui/mod.rs",
    "        });\n\n        self.show_search_sidebar(ctx);",
    "        });\n        update_ui::show_banner(self, ctx);\n\n        self.show_search_sidebar(ctx);",
    "ui update banner",
)

replace(
    "src/ui/settings_window.rs",
    "use crate::settings::{self, ClipExecutionProvider, IndexingSettings};",
    "use crate::settings::{self, ClipExecutionProvider, IndexingSettings};\nuse crate::update::{self, UpdateStatus};",
    "settings update imports",
)
replace(
    "src/ui/settings_window.rs",
    "    Performance,\n    Storage,",
    "    Performance,\n    Storage,\n    Updates,",
    "settings category variant",
)
replace(
    "src/ui/settings_window.rs",
    "    const ALL: [Self; 5] = [\n        Self::Collections,\n        Self::SearchClip,\n        Self::FacesPeople,\n        Self::Performance,\n        Self::Storage,\n    ];",
    "    const ALL: [Self; 6] = [\n        Self::Collections,\n        Self::SearchClip,\n        Self::FacesPeople,\n        Self::Performance,\n        Self::Storage,\n        Self::Updates,\n    ];",
    "settings category all",
)
replace(
    "src/ui/settings_window.rs",
    '            Self::Storage => "Storage",\n        }',
    '            Self::Storage => "Storage",\n            Self::Updates => "Updates",\n        }',
    "settings category label",
)
replace(
    "src/ui/settings_window.rs",
    "            Self::Storage => 4,\n        }",
    "            Self::Storage => 4,\n            Self::Updates => 5,\n        }",
    "settings category index",
)
replace(
    "src/ui/settings_window.rs",
    "            4 => Self::Storage,\n            _ => Self::Collections,",
    "            4 => Self::Storage,\n            5 => Self::Updates,\n            _ => Self::Collections,",
    "settings category from index",
)
replace(
    "src/ui/settings_window.rs",
    "    save_face_settings: bool,\n}",
    "    save_face_settings: bool,\n    save_update_settings: bool,\n    check_updates: bool,\n    download_update: bool,\n}",
    "settings effects",
)
replace(
    "src/ui/settings_window.rs",
    "                                    SettingsCategory::Storage => {\n                                        settings_storage(app, ui, &mut effects)\n                                    }\n                                });",
    "                                    SettingsCategory::Storage => {\n                                        settings_storage(app, ui, &mut effects)\n                                    }\n                                    SettingsCategory::Updates => {\n                                        settings_updates(app, ui, &mut effects)\n                                    }\n                                });",
    "settings category renderer",
)

settings_updates = r'''fn settings_updates(app: &mut ImageSearchApp, ui: &mut egui::Ui, effects: &mut Effects) {
    section_title(
        ui,
        "Updates",
        "Windows Image Search checks GitHub Releases in the background. Update installation always requires an explicit restart confirmation.",
    );

    ui.strong(format!("Current version: {}", env!("CARGO_PKG_VERSION")));
    ui.small("Channel: stable GitHub Releases. Prerelease builds are never installed automatically.");
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
        UpdateStatus::Idle => ui.label("Not checked in this session."),
        UpdateStatus::Checking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking GitHub Releases…");
            });
        }
        UpdateStatus::UpToDate => ui.label("✓ You are running the newest stable release."),
        UpdateStatus::Available(info) => ui.label(format!("Version {} is available.", info.version)),
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
                "✓ Version {} is downloaded, verified, and ready.",
                info.version
            ));
        }
        UpdateStatus::Failed(error) => {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let checking = matches!(status, UpdateStatus::Checking | UpdateStatus::Downloading { .. });
        if ui
            .add_enabled(!checking, egui::Button::new("Check now"))
            .clicked()
        {
            effects.check_updates = true;
        }
        if matches!(status, UpdateStatus::Available(_)) && ui.button("Download now").clicked() {
            effects.download_update = true;
        }
        if matches!(status, UpdateStatus::Ready(_, _)) {
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
settings_path = Path("src/ui/settings_window.rs")
settings_text = settings_path.read_text()
storage_anchor = "fn settings_storage(app: &mut ImageSearchApp, ui: &mut egui::Ui, effects: &mut Effects) {"
if storage_anchor not in settings_text:
    raise SystemExit("settings updates insertion anchor not found")
settings_path.write_text(settings_text.replace(storage_anchor, settings_updates + storage_anchor, 1))

replace(
    "src/ui/settings_window.rs",
    "    if effects.save_face_settings {",
    '''    if effects.save_update_settings {
        if let Err(err) = update::save_settings(&app.update_settings_path, app.update_settings) {
            app.last_error = Some(format!("Cannot save update settings: {err}"));
        }
    }
    if effects.check_updates {
        app.update_manager.start_check(app.update_settings.auto_download);
    }
    if effects.download_update {
        app.update_manager.start_download();
    }

    if effects.save_face_settings {''',
    "settings update effects",
)

Path(".github/workflows/release.yml").write_text(r'''name: Windows release

on:
  push:
    tags:
      - "v*"

permissions:
  contents: write

jobs:
  release:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Test
        run: cargo test --all-targets
      - name: Build release
        run: cargo build --release
      - name: Prepare updater assets
        shell: pwsh
        run: |
          New-Item -ItemType Directory -Force dist | Out-Null
          Copy-Item target/release/windows-image-search.exe dist/windows-image-search.exe
          $hash = (Get-FileHash -Algorithm SHA256 dist/windows-image-search.exe).Hash.ToLowerInvariant()
          "$hash  windows-image-search.exe" | Set-Content -Encoding ascii -NoNewline dist/windows-image-search.exe.sha256
      - name: Publish GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          generate_release_notes: true
          prerelease: ${{ contains(github.ref_name, '-') }}
          files: |
            dist/windows-image-search.exe
            dist/windows-image-search.exe.sha256
''')
