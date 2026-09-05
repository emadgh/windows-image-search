use super::ImageSearchApp;
use crate::face_detection::yunet_adapter::YuNetExecutionProvider;
use crate::face_detection::yunet_production;
use crate::face_detection::yunet_settings::{self, FaceDetectorSettings};
use crate::face_embedding_pipeline::{
    FaceEmbeddingPipelineEvent, FaceEmbeddingPipelineOptions, FaceEmbeddingPipelineSummary,
};
use crate::face_model_manager::{self, FaceModelKind, ManagedModelState, SFACE, YUNET};
use crate::face_pipeline::{FacePipelineEvent, FacePipelineOptions, FacePipelineSummary};
use crate::face_scope;
use crate::face_sface_production;
use crate::people_clustering::{self, PeopleClusteringOptions, PeopleClusteringSummary};
use crate::people_settings::{self, PeopleSettings};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

#[derive(Debug)]
enum FaceRuntimeMessage {
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
}

pub(super) struct FaceRuntimeState {
    settings: FaceDetectorSettings,
    settings_path: PathBuf,
    people_settings: PeopleSettings,
    people_settings_path: PathBuf,
    tx: Sender<FaceRuntimeMessage>,
    rx: Receiver<FaceRuntimeMessage>,
    running: bool,
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
}

impl FaceRuntimeState {
    pub(super) fn new(app_data_dir: &Path) -> Self {
        let settings_path = app_data_dir.join("face-detector-settings.ini");
        let mut settings = yunet_settings::load(&settings_path);
        let model_cache_dir = face_model_manager::cache_dir(app_data_dir);
        let managed_yunet_state = face_model_manager::inspect(&model_cache_dir, YUNET);
        let managed_sface_state = face_model_manager::inspect(&model_cache_dir, SFACE);
        if !settings.configured() && matches!(managed_yunet_state, ManagedModelState::Ready) {
            settings.model_path = face_model_manager::model_path(&model_cache_dir, YUNET);
            let _ = yunet_settings::save(&settings_path, &settings);
        }
        let people_settings_path = app_data_dir.join("people-settings.ini");
        let people_settings = people_settings::load(&people_settings_path);
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            settings,
            settings_path,
            people_settings,
            people_settings_path,
            tx,
            rx,
            running: false,
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
        }
    }

    fn configured_and_available(&self) -> bool {
        self.settings.configured() && self.settings.model_path.is_file()
    }
}

impl ImageSearchApp {
    pub(super) fn face_detector_settings_snapshot(&self) -> FaceDetectorSettings {
        self.face_runtime.settings.clone().sanitized()
    }

    pub(super) fn schedule_face_pipeline_after_base_index(&mut self) {
        let face_needed = self
            .roots
            .iter()
            .any(|root| face_scope::count_eligible_paths(&self.db_path, root).unwrap_or(0) > 0);
        if face_needed {
            self.face_runtime.run_after_base_index = true;
        }
    }

    pub(super) fn process_face_runtime_messages(&mut self) {
        while let Ok(message) = self.face_runtime.rx.try_recv() {
            match message {
                FaceRuntimeMessage::DetectionEvent(event) => self.apply_face_pipeline_event(event),
                FaceRuntimeMessage::EmbeddingEvent(event) => {
                    self.apply_face_embedding_pipeline_event(event)
                }
                FaceRuntimeMessage::ModelDownloadProgress {
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
                        if total == 0 {
                            0.0
                        } else {
                            downloaded as f64 * 100.0 / total as f64
                        }
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
                    self.face_runtime.running = false;
                    self.progress = None;
                    self.busy = self.indexing || self.searching;
                    match result {
                        Ok(summary) => {
                            self.status = format!(
                                "People clustering complete: {} group{}, {} clustered face{}, {} outlier{}, {} reused Person ID{}",
                                summary.people_created,
                                if summary.people_created == 1 { "" } else { "s" },
                                summary.faces_clustered,
                                if summary.faces_clustered == 1 { "" } else { "s" },
                                summary.outliers,
                                if summary.outliers == 1 { "" } else { "s" },
                                summary.reused_person_ids,
                                if summary.reused_person_ids == 1 { "" } else { "s" },
                            );
                            self.refresh_face_suggestions();
                            self.refresh_people_filter_catalog();
                        }
                        Err(error) => {
                            self.status = "People clustering failed".to_owned();
                            self.last_error = Some(error);
                        }
                    }
                }
                FaceRuntimeMessage::Finished(result) => {
                    self.face_runtime.running = false;
                    self.progress = None;
                    self.busy = self.indexing || self.searching;
                    match result {
                        Ok((detection, embedding)) => {
                            let built_identity_embeddings = embedding.is_some();
                            self.face_runtime.run_people_after_embedding =
                                built_identity_embeddings;
                            if let Some(embedding) = embedding {
                                self.status = format!(
                                    "Face pipeline complete: {} image{} processed, {} face{} found, {} embedding{} updated, {} total failure{}",
                                    detection.images_processed,
                                    if detection.images_processed == 1 { "" } else { "s" },
                                    detection.faces_detected,
                                    if detection.faces_detected == 1 { "" } else { "s" },
                                    embedding.faces_embedded,
                                    if embedding.faces_embedded == 1 { "" } else { "s" },
                                    detection.failures + embedding.failures,
                                    if detection.failures + embedding.failures == 1 {
                                        ""
                                    } else {
                                        "s"
                                    }
                                );
                            } else {
                                self.status = format!(
                                    "Face detection complete: {} image{} processed, {} face{} found, {} failure{}. Configure SFace to build searchable identity embeddings.",
                                    detection.images_processed,
                                    if detection.images_processed == 1 { "" } else { "s" },
                                    detection.faces_detected,
                                    if detection.faces_detected == 1 { "" } else { "s" },
                                    detection.failures,
                                    if detection.failures == 1 { "" } else { "s" }
                                );
                            }
                        }
                        Err(error) => {
                            self.status = "Face pipeline failed".to_owned();
                            self.last_error = Some(error);
                        }
                    }
                }
            }
        }

        if self.face_runtime.run_after_base_index && !self.busy && !self.face_runtime.running {
            self.face_runtime.run_after_base_index = false;
            self.start_face_pipeline();
        }

        if self.face_runtime.run_people_after_embedding && !self.busy && !self.face_runtime.running
        {
            self.face_runtime.run_people_after_embedding = false;
            self.start_people_incremental_update();
        }
    }

    pub(super) fn face_model_download_running(&self) -> bool {
        self.face_runtime.model_download_running
    }

    pub(super) fn face_model_download_progress(&self) -> Option<(&'static str, u64, u64)> {
        if !self.face_runtime.model_download_running {
            return None;
        }
        Some((
            self.face_runtime
                .model_download_kind
                .map(|kind| kind.label())
                .unwrap_or("Face models"),
            self.face_runtime.model_downloaded,
            self.face_runtime.model_download_total,
        ))
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

    fn managed_default_models_needed(&self) -> bool {
        let yunet_needed = !self.model_path_is_custom(FaceModelKind::YuNet)
            && !matches!(
                face_model_manager::inspect(&self.face_runtime.model_cache_dir, YUNET),
                ManagedModelState::Ready
            );
        let sface_needed = !self.model_path_is_custom(FaceModelKind::SFace)
            && !matches!(
                face_model_manager::inspect(&self.face_runtime.model_cache_dir, SFACE),
                ManagedModelState::Ready
            );
        yunet_needed || sface_needed
    }

    fn start_default_face_model_download(&mut self, force: bool, run_face_after: bool) {
        if self.face_runtime.model_download_running {
            self.face_runtime.run_face_after_model_download |= run_face_after;
            return;
        }
        let include_yunet = !self.model_path_is_custom(FaceModelKind::YuNet);
        let include_sface = !self.model_path_is_custom(FaceModelKind::SFace);
        if !include_yunet && !include_sface {
            self.status =
                "Custom YuNet and SFace models are in use; managed defaults were not changed"
                    .to_owned();
            return;
        }

        self.face_runtime
            .model_download_cancel
            .store(false, Ordering::Relaxed);
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
            self.face_runtime
                .model_download_cancel
                .store(true, Ordering::Relaxed);
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
            (
                FaceModelKind::YuNet,
                YUNET,
                &self.face_runtime.managed_yunet_state,
            ),
            (
                FaceModelKind::SFace,
                SFACE,
                &self.face_runtime.managed_sface_state,
            ),
        ] {
            let custom = self.model_path_is_custom(kind);
            ui.horizontal_wrapped(|ui| {
                ui.label(kind.label());
                if custom {
                    ui.strong("Custom");
                } else {
                    match state {
                        ManagedModelState::Missing => {
                            ui.label("Missing");
                        }
                        ManagedModelState::Ready => {
                            ui.strong("Ready");
                        }
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
        ui.small("Defaults are downloaded from this project's GitHub repository, verified by exact size + SHA-256, validated as ONNX, then atomically installed. Browse paths below remain advanced custom overrides.");
    }

    fn start_face_pipeline(&mut self) {
        if self.face_runtime.running || self.busy {
            return;
        }
        if self.managed_default_models_needed()
            || !self.face_runtime.settings.configured()
            || !self.face_embedding_settings.configured()
        {
            self.start_default_face_model_download(false, true);
            return;
        }
        if !self.face_runtime.settings.configured() {
            self.status = "YuNet model is not configured".to_owned();
            return;
        }
        if !self.face_runtime.settings.model_path.is_file() {
            self.last_error = Some(format!(
                "YuNet model path is unavailable: {}",
                self.face_runtime.settings.model_path.display()
            ));
            return;
        }

        let session_db_path = self.db_path.clone();
        let roots = self.roots.clone();
        let detector_settings = self.face_runtime.settings.clone().sanitized();
        let embedding_settings = self.face_embedding_settings.clone();
        let run_embeddings =
            embedding_settings.configured() && embedding_settings.model_path.is_file();
        let tx = self.face_runtime.tx.clone();
        self.face_runtime.running = true;
        self.busy = true;
        self.progress = None;
        self.status = format!(
            "Face detection: loading YuNet on {}…",
            detector_settings.provider_label()
        );

        std::thread::spawn(move || {
            let detection = yunet_production::run_available_roots(
                &session_db_path,
                &roots,
                &detector_settings,
                FacePipelineOptions::default(),
                |event| {
                    let _ = tx.send(FaceRuntimeMessage::DetectionEvent(event));
                },
            );
            let result = match detection {
                Ok(detection) => {
                    let embedding = if run_embeddings {
                        match face_sface_production::run_available_roots(
                            &roots,
                            &embedding_settings,
                            FaceEmbeddingPipelineOptions::default(),
                            |event| {
                                let _ = tx.send(FaceRuntimeMessage::EmbeddingEvent(event));
                            },
                        ) {
                            Ok(summary) => Some(summary),
                            Err(err) => {
                                let _ = tx.send(FaceRuntimeMessage::Finished(Err(format!(
                                    "SFace embedding backfill failed after detection: {err:#}"
                                ))));
                                return;
                            }
                        }
                    } else {
                        None
                    };
                    Ok((detection, embedding))
                }
                Err(err) => Err(format!("{err:#}")),
            };
            let _ = tx.send(FaceRuntimeMessage::Finished(result));
        });
    }

    fn start_people_incremental_update(&mut self) {
        self.start_people_maintenance(true);
    }

    fn start_people_rebuild(&mut self) {
        self.start_people_maintenance(false);
    }

    fn start_people_maintenance(&mut self, incremental: bool) {
        if self.face_runtime.running || self.busy {
            return;
        }
        if !self.face_embedding_settings.configured() {
            self.status = "SFace model is not configured".to_owned();
            return;
        }
        if !self.face_embedding_settings.model_path.is_file() {
            self.last_error = Some(format!(
                "SFace model path is unavailable: {}",
                self.face_embedding_settings.model_path.display()
            ));
            return;
        }
        if self.roots.is_empty() {
            self.status = "No indexed roots available for People clustering".to_owned();
            return;
        }

        let session_db_path = self.db_path.clone();
        let roots = self.roots.clone();
        let embedding_settings = self.face_embedding_settings.clone();
        let people_settings = self.face_runtime.people_settings.sanitized();
        let tx = self.face_runtime.tx.clone();
        self.face_runtime.running = true;
        self.busy = true;
        self.progress = None;
        self.last_error = None;
        self.status = if incremental {
            "People: incrementally updating current SFace embeddings…".to_owned()
        } else {
            "People: rebuilding all groups from current SFace embeddings…".to_owned()
        };

        std::thread::spawn(move || {
            let result = face_sface_production::embedding_revision(&embedding_settings)
                .and_then(|revision| {
                    let options = PeopleClusteringOptions {
                        similarity_threshold: people_settings.similarity_threshold,
                        min_cluster_size: people_settings.min_cluster_size,
                    };
                    if incremental {
                        people_clustering::run_incremental(
                            &session_db_path,
                            &roots,
                            &revision,
                            options,
                        )
                    } else {
                        people_clustering::run(&session_db_path, &roots, &revision, options)
                    }
                })
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(FaceRuntimeMessage::PeopleFinished(result));
        });
    }

    fn apply_face_pipeline_event(&mut self, event: FacePipelineEvent) {
        match event {
            FacePipelineEvent::RootStarted { root, eligible } => {
                self.status = format!(
                    "Face detection: {} — {eligible} eligible image{}",
                    root.display(),
                    if eligible == 1 { "" } else { "s" }
                );
                self.progress = Some((0, eligible));
            }
            FacePipelineEvent::Progress {
                root,
                visited,
                eligible,
                processed,
                faces,
                failures,
            } => {
                self.status = format!(
                    "Face detection: {} — {processed} processed, {faces} faces, {failures} failures",
                    root.display()
                );
                self.progress = Some((visited.min(eligible), eligible));
            }
            FacePipelineEvent::ImageFailed { image, error, .. } => {
                self.status = format!("Face detection skipped {}", image.display());
                self.last_error = Some(format!("{}: {error}", image.display()));
            }
            FacePipelineEvent::RootUnavailable { root } => {
                self.status = format!("Face detection root unavailable: {}", root.display());
            }
            FacePipelineEvent::RootFinished {
                root,
                visited,
                processed,
                faces,
                failures,
            } => {
                self.status = format!(
                    "Face detection finished {} — {processed}/{visited} processed, {faces} faces, {failures} failures",
                    root.display()
                );
            }
        }
    }

    fn apply_face_embedding_pipeline_event(&mut self, event: FaceEmbeddingPipelineEvent) {
        match event {
            FaceEmbeddingPipelineEvent::RootStarted { root, pending } => {
                self.status = format!(
                    "Face identity: {} — {pending} embedding{} pending",
                    root.display(),
                    if pending == 1 { "" } else { "s" }
                );
                self.progress = Some((0, pending));
            }
            FaceEmbeddingPipelineEvent::Progress {
                root,
                visited,
                pending,
                embedded,
                failures,
            } => {
                self.status = format!(
                    "Face identity: {} — {embedded} embedded, {failures} failures",
                    root.display()
                );
                self.progress = Some((visited.min(pending), pending));
            }
            FaceEmbeddingPipelineEvent::FaceFailed {
                image,
                face_id,
                error,
                ..
            } => {
                self.status = format!("Face identity skipped {face_id} in {}", image.display());
                self.last_error = Some(format!("{} / {face_id}: {error}", image.display()));
            }
            FaceEmbeddingPipelineEvent::RootUnavailable { root } => {
                self.status = format!("Face identity root unavailable: {}", root.display());
            }
            FaceEmbeddingPipelineEvent::RootFinished {
                root,
                visited,
                embedded,
                failures,
            } => {
                self.status = format!(
                    "Face identity finished {} — {embedded}/{visited} embedded, {failures} failures",
                    root.display()
                );
            }
        }
    }

    pub(super) fn show_face_detector_settings(&mut self, ui: &mut egui::Ui) {
        self.show_managed_face_models(ui);
        ui.add_space(12.0);
        ui.separator();
        ui.heading("Face detection (YuNet)");
        ui.label(
            "The verified managed YuNet is the default. Browse an ONNX file below only to use a custom detector. Face detection runs only for collections with Detect faces enabled.",
        );

        let mut changed = false;
        let mut people_changed = false;
        ui.add_enabled_ui(
            !self.busy && !self.face_runtime.running && !self.face_runtime.model_download_running,
            |ui| {
                ui.horizontal(|ui| {
                    let mut model_path = self
                        .face_runtime
                        .settings
                        .model_path
                        .to_string_lossy()
                        .into_owned();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut model_path)
                                .hint_text("Path to external YuNet .onnx")
                                .desired_width(560.0),
                        )
                        .changed()
                    {
                        self.face_runtime.settings.model_path = PathBuf::from(model_path.trim());
                        changed = true;
                    }
                    if ui.button("Browse…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("ONNX model", &["onnx"])
                            .pick_file()
                        {
                            self.face_runtime.settings.model_path = path;
                            changed = true;
                        }
                    }
                });

                let provider_before = self.face_runtime.settings.provider;
                egui::ComboBox::from_label("YuNet execution provider")
                    .selected_text(self.face_runtime.settings.provider_label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.face_runtime.settings.provider,
                            YuNetExecutionProvider::Cpu,
                            "CPU",
                        );
                        ui.selectable_value(
                            &mut self.face_runtime.settings.provider,
                            YuNetExecutionProvider::DirectMl,
                            "DirectML (Windows GPU)",
                        );
                    });
                changed |= provider_before != self.face_runtime.settings.provider;

                ui.horizontal(|ui| {
                    ui.label("Score threshold");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut self.face_runtime.settings.score_threshold)
                                .range(0.0..=1.0)
                                .speed(0.01),
                        )
                        .changed();
                    ui.label("NMS threshold");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut self.face_runtime.settings.nms_threshold)
                                .range(0.0..=1.0)
                                .speed(0.01),
                        )
                        .changed();
                    ui.label("Top-K");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut self.face_runtime.settings.top_k)
                                .range(1..=100_000)
                                .speed(10.0),
                        )
                        .changed();
                });

                ui.add_space(8.0);
                ui.separator();
                ui.strong("People clustering");
                ui.small(
                "These thresholds are separate from the one-shot Face Search similarity threshold.",
            );
                ui.horizontal(|ui| {
                    ui.label("Identity threshold");
                    people_changed |= ui
                        .add(
                            egui::Slider::new(
                                &mut self.face_runtime.people_settings.similarity_threshold,
                                0.0..=1.0,
                            )
                            .fixed_decimals(2),
                        )
                        .changed();
                    ui.label("Minimum faces per Person");
                    people_changed |= ui
                        .add(
                            egui::DragValue::new(
                                &mut self.face_runtime.people_settings.min_cluster_size,
                            )
                            .range(2..=1_000_000)
                            .speed(1.0),
                        )
                        .changed();
                });

                let can_run =
                    self.face_runtime.configured_and_available() && !self.roots.is_empty();
                let label = if self.face_embedding_settings.configured()
                    && self.face_embedding_settings.model_path.is_file()
                {
                    "Run face detection + identity backfill now"
                } else {
                    "Run face detection now"
                };
                if ui.add_enabled(can_run, egui::Button::new(label)).clicked() {
                    self.start_face_pipeline();
                }

                let can_rebuild_people = self.face_embedding_settings.configured()
                    && self.face_embedding_settings.model_path.is_file()
                    && !self.roots.is_empty();
                if ui
                    .add_enabled(
                        can_rebuild_people,
                        egui::Button::new("Rebuild People groups from current embeddings"),
                    )
                    .clicked()
                {
                    self.start_people_rebuild();
                }
            },
        );

        self.face_runtime.settings = self.face_runtime.settings.clone().sanitized();
        self.face_runtime.people_settings = self.face_runtime.people_settings.sanitized();
        if people_changed {
            if let Err(err) = people_settings::save(
                &self.face_runtime.people_settings_path,
                &self.face_runtime.people_settings,
            ) {
                self.last_error = Some(format!("Cannot save People settings: {err:#}"));
            }
        }
        if changed {
            if let Err(err) = yunet_settings::save(
                &self.face_runtime.settings_path,
                &self.face_runtime.settings,
            ) {
                self.last_error = Some(format!("Cannot save YuNet settings: {err:#}"));
            }
        }

        if self.face_runtime.running {
            ui.small("Face/People maintenance is running in the background.");
        } else if !self.face_runtime.settings.configured() {
            ui.small("No YuNet model configured. Face detection remains disabled.");
        } else if self.face_runtime.settings.model_path.is_file() {
            ui.small(format!(
                "Configured: {} on {}",
                self.face_runtime.settings.model_path.display(),
                self.face_runtime.settings.provider_label()
            ));
        } else {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                "Configured YuNet model path is not currently available.",
            );
        }
    }
}
