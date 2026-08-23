from pathlib import Path


def replace_once(path: str, old: str, new: str):
    p = Path(path)
    text = p.read_text(encoding='utf-8')
    if old not in text:
        raise SystemExit(f'anchor missing in {path}: {old[:180]!r}')
    p.write_text(text.replace(old, new, 1), encoding='utf-8')

# Compile the manager.
replace_once(
    'src/main.rs',
    'mod face_embedding_store;\nmod face_pipeline;',
    'mod face_embedding_store;\nmod face_model_manager;\nmod face_pipeline;',
)

# Adopt a previously verified cached SFace automatically at startup.
replace_once(
    'src/ui/mod.rs',
    '''        let face_settings_path = app_data_dir.join("face-embedding-settings.ini");
        let face_embedding_settings = face_settings::load(&face_settings_path);
        let face_runtime = face_runtime::FaceRuntimeState::new(app_data_dir);''',
    '''        let face_settings_path = app_data_dir.join("face-embedding-settings.ini");
        let mut face_embedding_settings = face_settings::load(&face_settings_path);
        let face_runtime = face_runtime::FaceRuntimeState::new(app_data_dir);
        if !face_embedding_settings.configured() {
            let cache = crate::face_model_manager::cache_dir(app_data_dir);
            if matches!(
                crate::face_model_manager::inspect(&cache, crate::face_model_manager::SFACE),
                crate::face_model_manager::ManagedModelState::Ready
            ) {
                face_embedding_settings.model_path =
                    crate::face_model_manager::model_path(&cache, crate::face_model_manager::SFACE);
                let _ = face_settings::save(&face_settings_path, &face_embedding_settings);
            }
        }''',
)
replace_once(
    'src/ui/mod.rs',
    '''        if self.busy {
            ctx.request_repaint_after(Duration::from_millis(50));
        }''',
    '''        if self.busy || self.face_model_download_running() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }''',
)
replace_once(
    'src/ui/mod.rs',
    '''                if self.busy {
                    ui.spinner();
                }''',
    '''                if self.busy || self.face_model_download_running() {
                    ui.spinner();
                }''',
)

# Face runtime imports and state.
replace_once(
    'src/ui/face_runtime.rs',
    'use crate::face_pipeline::{FacePipelineEvent, FacePipelineOptions, FacePipelineSummary};\nuse crate::face_sface_production;',
    'use crate::face_pipeline::{FacePipelineEvent, FacePipelineOptions, FacePipelineSummary};\nuse crate::face_model_manager::{self, FaceModelKind, ManagedModelState, SFACE, YUNET};\nuse crate::face_sface_production;',
)
replace_once(
    'src/ui/face_runtime.rs',
    'use std::sync::mpsc::{Receiver, Sender};',
    'use std::sync::atomic::{AtomicBool, Ordering};\nuse std::sync::mpsc::{Receiver, Sender};\nuse std::sync::Arc;',
)
replace_once(
    'src/ui/face_runtime.rs',
    '''enum FaceRuntimeMessage {
    DetectionEvent(FacePipelineEvent),
    EmbeddingEvent(FaceEmbeddingPipelineEvent),
    PeopleFinished(Result<PeopleClusteringSummary, String>),
    Finished(Result<(FacePipelineSummary, Option<FaceEmbeddingPipelineSummary>), String>),
}''',
    '''enum FaceRuntimeMessage {
    DetectionEvent(FacePipelineEvent),
    EmbeddingEvent(FaceEmbeddingPipelineEvent),
    PeopleFinished(Result<PeopleClusteringSummary, String>),
    ModelDownloadProgress {
        kind: FaceModelKind,
        downloaded: u64,
        total: u64,
    },
    ModelDownloadFinished(Result<DownloadedDefaultModels, String>),
    Finished(Result<(FacePipelineSummary, Option<FaceEmbeddingPipelineSummary>), String>),
}

#[derive(Debug, Default)]
struct DownloadedDefaultModels {
    yunet: Option<PathBuf>,
    sface: Option<PathBuf>,
}''',
)
replace_once(
    'src/ui/face_runtime.rs',
    '''    running: bool,
    run_after_base_index: bool,
    run_people_after_embedding: bool,
}''',
    '''    running: bool,
    run_after_base_index: bool,
    run_people_after_embedding: bool,
    model_cache_dir: PathBuf,
    managed_yunet_state: ManagedModelState,
    managed_sface_state: ManagedModelState,
    model_download_running: bool,
    model_download_kind: Option<FaceModelKind>,
    model_downloaded: u64,
    model_download_total: u64,
    model_download_cancel: Arc<AtomicBool>,
    run_face_after_model_download: bool,
}''',
)
replace_once(
    'src/ui/face_runtime.rs',
    '''        let settings_path = app_data_dir.join("face-detector-settings.ini");
        let settings = yunet_settings::load(&settings_path);
        let people_settings_path = app_data_dir.join("people-settings.ini");''',
    '''        let settings_path = app_data_dir.join("face-detector-settings.ini");
        let mut settings = yunet_settings::load(&settings_path);
        let model_cache_dir = face_model_manager::cache_dir(app_data_dir);
        let managed_yunet_state = face_model_manager::inspect(&model_cache_dir, YUNET);
        let managed_sface_state = face_model_manager::inspect(&model_cache_dir, SFACE);
        if !settings.configured() && matches!(managed_yunet_state, ManagedModelState::Ready) {
            settings.model_path = face_model_manager::model_path(&model_cache_dir, YUNET);
            let _ = yunet_settings::save(&settings_path, &settings);
        }
        let people_settings_path = app_data_dir.join("people-settings.ini");''',
)
replace_once(
    'src/ui/face_runtime.rs',
    '''            running: false,
            run_after_base_index: false,
            run_people_after_embedding: false,
        }''',
    '''            running: false,
            run_after_base_index: false,
            run_people_after_embedding: false,
            model_cache_dir,
            managed_yunet_state,
            managed_sface_state,
            model_download_running: false,
            model_download_kind: None,
            model_downloaded: 0,
            model_download_total: 0,
            model_download_cancel: Arc::new(AtomicBool::new(false)),
            run_face_after_model_download: false,
        }''',
)

# Process model download events before regular pipeline completion.
replace_once(
    'src/ui/face_runtime.rs',
    '''                FaceRuntimeMessage::PeopleFinished(result) => {
                    self.face_runtime.running = false;''',
    '''                FaceRuntimeMessage::ModelDownloadProgress {
                    kind,
                    downloaded,
                    total,
                } => {
                    self.face_runtime.model_download_kind = Some(kind);
                    self.face_runtime.model_downloaded = downloaded;
                    self.face_runtime.model_download_total = total;
                    self.status = format!(
                        "Downloading {}: {:.1}%",
                        kind.label(),
                        if total == 0 { 0.0 } else { downloaded as f64 * 100.0 / total as f64 }
                    );
                }
                FaceRuntimeMessage::ModelDownloadFinished(result) => {
                    self.face_runtime.model_download_running = false;
                    self.face_runtime.model_download_kind = None;
                    self.face_runtime.managed_yunet_state =
                        face_model_manager::inspect(&self.face_runtime.model_cache_dir, YUNET);
                    self.face_runtime.managed_sface_state =
                        face_model_manager::inspect(&self.face_runtime.model_cache_dir, SFACE);
                    match result {
                        Ok(downloaded) => {
                            if let Some(path) = downloaded.yunet {
                                self.face_runtime.settings.model_path = path;
                                let _ = yunet_settings::save(
                                    &self.face_runtime.settings_path,
                                    &self.face_runtime.settings,
                                );
                            }
                            if let Some(path) = downloaded.sface {
                                self.face_embedding_settings.model_path = path;
                                let _ = crate::face_settings::save(
                                    &self.face_settings_path,
                                    &self.face_embedding_settings,
                                );
                            }
                            self.status = "Default face models are verified and ready".to_owned();
                        }
                        Err(error) if error.to_lowercase().contains("cancelled") => {
                            self.status = "Face model download cancelled".to_owned();
                        }
                        Err(error) => {
                            self.status = "Face model download failed".to_owned();
                            self.last_error = Some(error);
                        }
                    }
                    let run_after = self.face_runtime.run_face_after_model_download;
                    self.face_runtime.run_face_after_model_download = false;
                    if run_after
                        && !self.face_runtime.model_download_running
                        && !self.busy
                        && self.face_runtime.settings.configured()
                        && self.face_runtime.settings.model_path.is_file()
                    {
                        self.start_face_pipeline();
                    }
                }
                FaceRuntimeMessage::PeopleFinished(result) => {
                    self.face_runtime.running = false;''',
)

# Insert model management methods before start_face_pipeline.
replace_once(
    'src/ui/face_runtime.rs',
    '    fn start_face_pipeline(&mut self) {',
    '''    pub(super) fn face_model_download_running(&self) -> bool {
        self.face_runtime.model_download_running
    }

    fn model_path_is_custom(&self, kind: FaceModelKind) -> bool {
        let path = match kind {
            FaceModelKind::YuNet => &self.face_runtime.settings.model_path,
            FaceModelKind::SFace => &self.face_embedding_settings.model_path,
        };
        if path.as_os_str().is_empty() {
            return false;
        }
        let manifest = match kind {
            FaceModelKind::YuNet => YUNET,
            FaceModelKind::SFace => SFACE,
        };
        !face_model_manager::is_managed_path(path, &self.face_runtime.model_cache_dir, manifest)
    }

    fn start_default_face_model_download(&mut self, force: bool, run_face_after: bool) {
        if self.face_runtime.model_download_running {
            self.face_runtime.run_face_after_model_download |= run_face_after;
            return;
        }
        let include_yunet = !self.model_path_is_custom(FaceModelKind::YuNet);
        let include_sface = !self.model_path_is_custom(FaceModelKind::SFace);
        if !include_yunet && !include_sface {
            self.status = "Custom YuNet and SFace models are in use; managed defaults were not changed".to_owned();
            return;
        }

        self.face_runtime.model_download_cancel.store(false, Ordering::Relaxed);
        self.face_runtime.model_download_running = true;
        self.face_runtime.model_downloaded = 0;
        self.face_runtime.model_download_total = 0;
        self.face_runtime.run_face_after_model_download = run_face_after;
        self.last_error = None;
        self.status = "Preparing default face model download…".to_owned();

        let cache_dir = self.face_runtime.model_cache_dir.clone();
        let cancel = self.face_runtime.model_download_cancel.clone();
        let tx = self.face_runtime.tx.clone();
        std::thread::spawn(move || {
            let mut outcome = DownloadedDefaultModels::default();
            let result = (|| -> anyhow::Result<()> {
                if include_yunet {
                    let path = face_model_manager::download_model(
                        &cache_dir,
                        YUNET,
                        force,
                        &cancel,
                        |downloaded, total| {
                            let _ = tx.send(FaceRuntimeMessage::ModelDownloadProgress {
                                kind: FaceModelKind::YuNet,
                                downloaded,
                                total,
                            });
                        },
                    )?;
                    outcome.yunet = Some(path);
                }
                if include_sface {
                    let path = face_model_manager::download_model(
                        &cache_dir,
                        SFACE,
                        force,
                        &cancel,
                        |downloaded, total| {
                            let _ = tx.send(FaceRuntimeMessage::ModelDownloadProgress {
                                kind: FaceModelKind::SFace,
                                downloaded,
                                total,
                            });
                        },
                    )?;
                    outcome.sface = Some(path);
                }
                Ok(())
            })();
            let message = match result {
                Ok(()) => FaceRuntimeMessage::ModelDownloadFinished(Ok(outcome)),
                Err(err) => FaceRuntimeMessage::ModelDownloadFinished(Err(format!("{err:#}"))),
            };
            let _ = tx.send(message);
        });
    }

    fn cancel_face_model_download(&mut self) {
        if self.face_runtime.model_download_running {
            self.face_runtime.model_download_cancel.store(true, Ordering::Relaxed);
            self.status = "Cancelling face model download…".to_owned();
        }
    }

    fn show_managed_face_models(&mut self, ui: &mut egui::Ui) {
        ui.strong("Default face models");
        ui.small(format!(
            "Managed cache: {}",
            self.face_runtime.model_cache_dir.display()
        ));

        for (kind, manifest, state) in [
            (FaceModelKind::YuNet, YUNET, &self.face_runtime.managed_yunet_state),
            (FaceModelKind::SFace, SFACE, &self.face_runtime.managed_sface_state),
        ] {
            let custom = self.model_path_is_custom(kind);
            ui.horizontal_wrapped(|ui| {
                ui.label(kind.label());
                if custom {
                    ui.strong("Custom");
                } else {
                    match state {
                        ManagedModelState::Missing => { ui.label("Missing"); }
                        ManagedModelState::Ready => { ui.strong("Ready"); }
                        ManagedModelState::Invalid(_) => {
                            ui.colored_label(egui::Color32::LIGHT_RED, "Invalid");
                        }
                    }
                }
                ui.small(format!("{} · {}", manifest.file_name, manifest.license));
            });
            if !custom {
                if let ManagedModelState::Invalid(error) = state {
                    ui.small(error);
                }
            }
        }

        if self.face_runtime.model_download_running {
            let fraction = if self.face_runtime.model_download_total == 0 {
                0.0
            } else {
                self.face_runtime.model_downloaded as f32
                    / self.face_runtime.model_download_total as f32
            };
            let label = self
                .face_runtime
                .model_download_kind
                .map(|kind| kind.label())
                .unwrap_or("Face model");
            ui.add(
                egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                    .desired_width(ui.available_width().min(520.0))
                    .text(format!("Downloading {label}")),
            );
            if ui.button("Cancel download").clicked() {
                self.cancel_face_model_download();
            }
        } else {
            ui.horizontal(|ui| {
                if ui.button("Download default models").clicked() {
                    self.start_default_face_model_download(false, false);
                }
                if ui.button("Repair / re-download defaults").clicked() {
                    self.start_default_face_model_download(true, false);
                }
            });
        }
        ui.small("Defaults are downloaded from OpenCV Zoo, verified by exact size + SHA-256, validated as ONNX, then atomically installed. Browse paths below remain advanced custom overrides.");
    }

    fn start_face_pipeline(&mut self) {''',
)

# Auto-download missing defaults on first actual face pipeline use.
replace_once(
    'src/ui/face_runtime.rs',
    '''    fn start_face_pipeline(&mut self) {
        if self.face_runtime.running || self.busy {
            return;
        }
        if !self.face_runtime.settings.configured() {''',
    '''    fn start_face_pipeline(&mut self) {
        if self.face_runtime.running || self.busy {
            return;
        }
        if !self.face_runtime.settings.configured() || !self.face_embedding_settings.configured() {
            self.start_default_face_model_download(false, true);
            return;
        }
        if !self.face_runtime.settings.configured() {''',
)

# Put managed models at the top of Faces/People settings and lock custom edits while downloading.
replace_once(
    'src/ui/face_runtime.rs',
    '''    pub(super) fn show_face_detector_settings(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.separator();
        ui.heading("Face detection (YuNet)");
        ui.label(
            "Use an external OpenCV YuNet-compatible ONNX model. Face detection runs only for collections with Detect faces enabled.",
        );''',
    '''    pub(super) fn show_face_detector_settings(&mut self, ui: &mut egui::Ui) {
        self.show_managed_face_models(ui);
        ui.add_space(12.0);
        ui.separator();
        ui.heading("Face detection (YuNet)");
        ui.label(
            "The verified managed YuNet is the default. Browse an ONNX file below only to use a custom detector. Face detection runs only for collections with Detect faces enabled.",
        );''',
)
replace_once(
    'src/ui/face_runtime.rs',
    '        ui.add_enabled_ui(!self.busy && !self.face_runtime.running, |ui| {',
    '        ui.add_enabled_ui(\n            !self.busy && !self.face_runtime.running && !self.face_runtime.model_download_running,\n            |ui| {',
)
# The closure ending remains valid after rustfmt.

# SFace copy and custom editing lock in Preferences.
replace_once(
    'src/ui/settings_window.rs',
    '''    ui.label(
        "Use an external SFace-compatible ONNX model for portable face embeddings. Model weights are never downloaded or stored by the application.",
    );
    ui.add_enabled_ui(!app.busy, |ui| {''',
    '''    ui.label(
        "The verified managed SFace model is the default. Browse an ONNX file below only to use a custom identity model.",
    );
    ui.add_enabled_ui(!app.busy && !app.face_model_download_running(), |ui| {''',
)

# Explain model sources in Storage/About-ish area without adding another permanent window.
replace_once(
    'src/ui/settings_window.rs',
    '''    ui.strong("Session database");
    ui.label(app.db_path.display().to_string());''',
    '''    ui.strong("Face model attribution");
    ui.small("YuNet 2026may — OpenCV Zoo — MIT. SFace 2021dec — OpenCV Zoo — Apache-2.0. Managed downloads are checksum-pinned; custom model paths remain user-owned.");
    ui.add_space(12.0);
    ui.strong("Session database");
    ui.label(app.db_path.display().to_string());''',
)
