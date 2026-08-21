use super::ImageSearchApp;
use crate::face_detection::yunet_adapter::YuNetExecutionProvider;
use crate::face_detection::yunet_production;
use crate::face_detection::yunet_settings::{self, FaceDetectorSettings};
use crate::face_pipeline::{FacePipelineEvent, FacePipelineOptions, FacePipelineSummary};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

#[derive(Debug)]
enum FaceRuntimeMessage {
    Event(FacePipelineEvent),
    Finished(Result<FacePipelineSummary, String>),
}

pub(super) struct FaceRuntimeState {
    settings: FaceDetectorSettings,
    settings_path: PathBuf,
    tx: Sender<FaceRuntimeMessage>,
    rx: Receiver<FaceRuntimeMessage>,
    running: bool,
    run_after_base_index: bool,
}

impl FaceRuntimeState {
    pub(super) fn new(app_data_dir: &Path) -> Self {
        let settings_path = app_data_dir.join("face-detector-settings.ini");
        let settings = yunet_settings::load(&settings_path);
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            settings,
            settings_path,
            tx,
            rx,
            running: false,
            run_after_base_index: false,
        }
    }

    fn configured_and_available(&self) -> bool {
        self.settings.configured() && self.settings.model_path.is_file()
    }
}

impl ImageSearchApp {
    pub(super) fn schedule_face_pipeline_after_base_index(&mut self) {
        if self.face_runtime.configured_and_available() {
            self.face_runtime.run_after_base_index = true;
        }
    }

    pub(super) fn process_face_runtime_messages(&mut self) {
        while let Ok(message) = self.face_runtime.rx.try_recv() {
            match message {
                FaceRuntimeMessage::Event(event) => self.apply_face_pipeline_event(event),
                FaceRuntimeMessage::Finished(result) => {
                    self.face_runtime.running = false;
                    self.progress = None;
                    self.busy = self.indexing || self.searching;
                    match result {
                        Ok(summary) => {
                            self.status = format!(
                                "Face detection complete: {} image{} processed, {} face{} found, {} failure{}",
                                summary.images_processed,
                                if summary.images_processed == 1 { "" } else { "s" },
                                summary.faces_detected,
                                if summary.faces_detected == 1 { "" } else { "s" },
                                summary.failures,
                                if summary.failures == 1 { "" } else { "s" }
                            );
                        }
                        Err(error) => {
                            self.status = "Face detection failed".to_owned();
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
    }

    fn start_face_pipeline(&mut self) {
        if self.face_runtime.running || self.busy {
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
        let settings = self.face_runtime.settings.clone().sanitized();
        let tx = self.face_runtime.tx.clone();
        self.face_runtime.running = true;
        self.busy = true;
        self.progress = None;
        self.status = format!(
            "Face detection: loading YuNet on {}…",
            settings.provider_label()
        );

        std::thread::spawn(move || {
            let result = yunet_production::run_available_roots(
                &session_db_path,
                &roots,
                &settings,
                FacePipelineOptions::default(),
                |event| {
                    let _ = tx.send(FaceRuntimeMessage::Event(event));
                },
            )
            .map_err(|err| format!("{err:#}"));
            let _ = tx.send(FaceRuntimeMessage::Finished(result));
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

    pub(super) fn show_face_detector_settings(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.separator();
        ui.heading("Face detection (YuNet)");
        ui.label(
            "Use an external OpenCV YuNet-compatible ONNX model. Face detection runs only for collections with Detect faces enabled.",
        );

        let mut changed = false;
        ui.add_enabled_ui(!self.busy && !self.face_runtime.running, |ui| {
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

            let can_run = self.face_runtime.configured_and_available() && !self.roots.is_empty();
            if ui
                .add_enabled(can_run, egui::Button::new("Run face detection now"))
                .clicked()
            {
                self.start_face_pipeline();
            }
        });

        self.face_runtime.settings = self.face_runtime.settings.clone().sanitized();
        if changed {
            if let Err(err) = yunet_settings::save(
                &self.face_runtime.settings_path,
                &self.face_runtime.settings,
            ) {
                self.last_error = Some(format!("Cannot save YuNet settings: {err:#}"));
            }
        }

        if self.face_runtime.running {
            ui.small("YuNet face detection is running in the background.");
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
