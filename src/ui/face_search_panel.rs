use super::photo_grid::{self, PhotoGridSpec, PhotoTileMode};
use super::ImageSearchApp;
use crate::face_detection::{
    self, yunet_production::YuNetProductionDetector, yunet_settings::FaceDetectorSettings, FaceBox,
    FaceDetector,
};
use crate::face_embedding::{self, FaceEmbedder};
use crate::face_search::{
    self, IndexedFaceSearchOptions, IndexedFaceSearchReport, IndexedFaceSuggestion,
};
use crate::face_settings::FaceEmbeddingSettings;
use crate::face_sface_production::SFaceProductionEmbedder;
use crate::face_similarity::{FaceEmbeddingRevision, FaceSimilarityQuery};
use anyhow::{bail, Context, Result};
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

const DEFAULT_SUGGESTION_LIMIT: usize = 240;

#[derive(Clone, Debug)]
struct ExternalFaceChoice {
    image_path: PathBuf,
    ordinal: usize,
    confidence: f32,
    bbox: FaceBox,
    query: FaceSimilarityQuery,
}

#[derive(Debug)]
enum FaceSearchUiMessage {
    Suggestions(Result<(Vec<IndexedFaceSuggestion>, HashMap<String, String>), String>),
    ExternalPrepared {
        path: PathBuf,
        result: Result<Vec<ExternalFaceChoice>, String>,
    },
    SearchFinished {
        query: IndexedFaceSuggestion,
        report: Result<IndexedFaceSearchReport, String>,
    },
    ExternalSearchFinished {
        query: ExternalFaceChoice,
        report: Result<IndexedFaceSearchReport, String>,
    },
}

pub(super) struct FaceSearchUiState {
    open: bool,
    suggestions: Vec<IndexedFaceSuggestion>,
    suggestion_names: HashMap<String, String>,
    filter_text: String,
    selected_face_id: Option<String>,
    external_source: Option<PathBuf>,
    external_faces: Vec<ExternalFaceChoice>,
    selected_external_ordinal: Option<usize>,
    options: IndexedFaceSearchOptions,
    tx: Sender<FaceSearchUiMessage>,
    rx: Receiver<FaceSearchUiMessage>,
    loading: bool,
    external_loading: bool,
    searching: bool,
    active: bool,
    active_query: Option<IndexedFaceSuggestion>,
    match_boxes: HashMap<PathBuf, FaceBox>,
    last_rows_considered: usize,
}

impl Default for FaceSearchUiState {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            open: false,
            suggestions: Vec::new(),
            suggestion_names: HashMap::new(),
            filter_text: String::new(),
            selected_face_id: None,
            external_source: None,
            external_faces: Vec::new(),
            selected_external_ordinal: None,
            options: IndexedFaceSearchOptions::default(),
            tx,
            rx,
            loading: false,
            external_loading: false,
            searching: false,
            active: false,
            active_query: None,
            match_boxes: HashMap::new(),
            last_rows_considered: 0,
        }
    }
}

impl ImageSearchApp {
    pub(super) fn open_face_search(&mut self) {
        self.face_search_ui.open = true;
        if self.face_search_ui.suggestions.is_empty() && !self.face_search_ui.loading {
            self.refresh_face_suggestions();
        }
    }

    pub(super) fn face_search_active(&self) -> bool {
        self.face_search_ui.active
    }

    pub(super) fn face_match_box(&self, path: &PathBuf) -> Option<FaceBox> {
        self.face_search_ui.match_boxes.get(path).copied()
    }

    pub(super) fn clear_face_search_result_state(&mut self) {
        self.face_search_ui.active = false;
        self.face_search_ui.active_query = None;
        self.face_search_ui.match_boxes.clear();
        self.face_search_ui.last_rows_considered = 0;
    }

    pub(super) fn process_face_search_messages(&mut self) {
        while let Ok(message) = self.face_search_ui.rx.try_recv() {
            match message {
                FaceSearchUiMessage::Suggestions(result) => {
                    self.face_search_ui.loading = false;
                    match result {
                        Ok((suggestions, names)) => {
                            let suggestion_count = suggestions.len();
                            self.face_search_ui.suggestions = suggestions;
                            self.face_search_ui.suggestion_names = names;
                            let selected_exists = self
                                .face_search_ui
                                .selected_face_id
                                .as_ref()
                                .is_some_and(|id| {
                                    self.face_search_ui
                                        .suggestions
                                        .iter()
                                        .any(|face| &face.face_id == id)
                                });
                            if !selected_exists {
                                self.face_search_ui.selected_face_id = None;
                            }
                            self.status = if suggestion_count == 0 {
                                "People / searchable face suggestions loaded: no indexed faces available"
                                    .to_owned()
                            } else {
                                format!(
                                    "People / searchable face suggestions ready: {suggestion_count} suggestion{}",
                                    if suggestion_count == 1 { "" } else { "s" }
                                )
                            };
                        }
                        Err(error) => {
                            self.status =
                                "Failed to load People / searchable face suggestions".to_owned();
                            self.last_error = Some(error);
                        }
                    }
                }
                FaceSearchUiMessage::ExternalPrepared { path, result } => {
                    self.face_search_ui.external_loading = false;
                    self.face_search_ui.external_source = Some(path.clone());
                    self.face_search_ui.selected_external_ordinal = None;
                    match result {
                        Ok(faces) => {
                            self.face_search_ui.external_faces = faces;
                            self.status = if self.face_search_ui.external_faces.is_empty() {
                                format!("No faces detected in {}", path.display())
                            } else {
                                format!(
                                    "Detected {} face{} in external query image",
                                    self.face_search_ui.external_faces.len(),
                                    if self.face_search_ui.external_faces.len() == 1 {
                                        ""
                                    } else {
                                        "s"
                                    }
                                )
                            };
                        }
                        Err(error) => {
                            self.face_search_ui.external_faces.clear();
                            self.last_error = Some(error);
                        }
                    }
                }
                FaceSearchUiMessage::SearchFinished { query, report } => {
                    self.face_search_ui.searching = false;
                    self.busy = self.indexing || self.searching;
                    match report {
                        Ok(report) => {
                            self.apply_face_search_report(query, report);
                        }
                        Err(error) => {
                            self.status = "Face search failed".to_owned();
                            self.last_error = Some(error);
                        }
                    }
                }
                FaceSearchUiMessage::ExternalSearchFinished { query, report } => {
                    self.face_search_ui.searching = false;
                    self.busy = self.indexing || self.searching;
                    match report {
                        Ok(report) => {
                            self.apply_external_face_search_report(query, report);
                        }
                        Err(error) => {
                            self.status = "External face search failed".to_owned();
                            self.last_error = Some(error);
                        }
                    }
                }
            }
        }
    }

    pub(super) fn refresh_face_suggestions(&mut self) {
        if self.face_search_ui.loading {
            return;
        }
        let roots = self.roots.clone();
        let session_db_path = self.db_path.clone();
        let tx = self.face_search_ui.tx.clone();
        self.face_search_ui.loading = true;
        self.last_error = None;
        self.status = "Loading People / searchable face suggestions…".to_owned();
        std::thread::spawn(move || {
            let result = face_search::list_effective_people_representatives(
                &session_db_path,
                &roots,
                DEFAULT_SUGGESTION_LIMIT,
            )
            .and_then(|(people, names)| {
                if people.is_empty() {
                    face_search::list_searchable_faces(&roots, DEFAULT_SUGGESTION_LIMIT)
                        .map(|faces| (faces, HashMap::new()))
                } else {
                    Ok((people, names))
                }
            })
            .map_err(|err| format!("{err:#}"));
            let _ = tx.send(FaceSearchUiMessage::Suggestions(result));
        });
    }

    fn choose_external_face_file(&mut self) {
        if self.busy || self.face_search_ui.external_loading {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["jpg", "jpeg", "png", "tif", "tiff"])
            .pick_file()
        else {
            return;
        };

        let detector_settings = self.face_detector_settings_snapshot();
        let embedding_settings = self.face_embedding_settings.clone();
        let tx = self.face_search_ui.tx.clone();
        self.face_search_ui.external_loading = true;
        self.face_search_ui.external_source = Some(path.clone());
        self.face_search_ui.external_faces.clear();
        self.face_search_ui.selected_external_ordinal = None;
        self.last_error = None;
        self.status = format!("Detecting faces in external query {}…", path.display());

        std::thread::spawn(move || {
            let result = prepare_external_faces(&path, detector_settings, embedding_settings)
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(FaceSearchUiMessage::ExternalPrepared { path, result });
        });
    }

    fn start_indexed_face_search(&mut self, query: IndexedFaceSuggestion) {
        if self.busy || self.face_search_ui.searching {
            return;
        }
        let roots = self.roots.clone();
        let query_root = query.root.clone();
        let face_id = query.face_id.clone();
        let options = self.face_search_ui.options.sanitized();
        let tx = self.face_search_ui.tx.clone();

        self.face_search_ui.searching = true;
        self.busy = true;
        self.last_error = None;
        self.status = "Searching indexed face identities…".to_owned();

        std::thread::spawn(move || {
            let report = face_search::search_indexed_face(&roots, &query_root, &face_id, options)
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(FaceSearchUiMessage::SearchFinished { query, report });
        });
    }

    fn start_external_face_search(&mut self, query: ExternalFaceChoice) {
        if self.busy || self.face_search_ui.searching {
            return;
        }
        let roots = self.roots.clone();
        let similarity_query = query.query.clone();
        let options = self.face_search_ui.options.sanitized();
        let tx = self.face_search_ui.tx.clone();

        self.face_search_ui.searching = true;
        self.busy = true;
        self.last_error = None;
        self.status = format!(
            "Searching identity for face {} from external image…",
            query.ordinal + 1
        );

        std::thread::spawn(move || {
            let report = face_search::search_embedding_query(&roots, &similarity_query, options)
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(FaceSearchUiMessage::ExternalSearchFinished { query, report });
        });
    }

    fn apply_face_search_report(
        &mut self,
        query: IndexedFaceSuggestion,
        report: IndexedFaceSearchReport,
    ) {
        self.apply_face_search_results(query.image_path.clone(), Some(query), report);
    }

    fn apply_external_face_search_report(
        &mut self,
        query: ExternalFaceChoice,
        report: IndexedFaceSearchReport,
    ) {
        self.apply_face_search_results(query.image_path, None, report);
    }

    fn apply_face_search_results(
        &mut self,
        query_image: PathBuf,
        active_query: Option<IndexedFaceSuggestion>,
        report: IndexedFaceSearchReport,
    ) {
        let mut by_path = HashMap::with_capacity(self.images.len());
        for image in &self.images {
            by_path.insert(image.path.clone(), image.clone());
        }

        let mut results = Vec::with_capacity(report.matches.len());
        let mut match_boxes = HashMap::with_capacity(report.matches.len());
        for hit in &report.matches {
            let Some(mut image) = by_path.get(&hit.image_path).cloned() else {
                continue;
            };
            image.score = Some(hit.similarity);
            match_boxes.insert(hit.image_path.clone(), hit.bbox);
            results.push(image);
        }

        self.face_search_ui.active = true;
        self.face_search_ui.active_query = active_query;
        self.face_search_ui.match_boxes = match_boxes;
        self.face_search_ui.last_rows_considered = report.rows_considered;
        self.similarity_results = Some(results);
        self.query_image = Some(query_image);
        self.selected_paths.clear();
        self.status = format!(
            "Face search: {} match{} from {} compatible face embedding{}",
            report.matches.len(),
            if report.matches.len() == 1 { "" } else { "es" },
            report.rows_considered,
            if report.rows_considered == 1 { "" } else { "s" }
        );
    }

    pub(super) fn show_face_search_window(&mut self, ctx: &egui::Context) {
        if !self.face_search_ui.open {
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.face_search_ui.open = false;
            return;
        }

        let mut open = self.face_search_ui.open;
        egui::Window::new("Face Search")
            .open(&mut open)
            .resizable(true)
            .default_width(930.0)
            .default_height(720.0)
            .min_width(620.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Search by face");
                    if ui
                        .add_enabled(
                            !self.face_search_ui.loading && !self.busy,
                            egui::Button::new("⟳ Refresh people/faces"),
                        )
                        .clicked()
                    {
                        self.refresh_face_suggestions();
                    }
                    if ui
                        .add_enabled(
                            !self.face_search_ui.external_loading && !self.busy,
                            egui::Button::new("Face from file…"),
                        )
                        .clicked()
                    {
                        self.choose_external_face_file();
                    }
                    if self.face_search_ui.loading {
                        ui.spinner();
                        ui.small("Reading face index…");
                    }
                    if self.face_search_ui.external_loading {
                        ui.spinner();
                        ui.small("Detecting + embedding query faces…");
                    }
                });

                ui.label(
                    "Choose a detected face already stored in the database, or load any image and choose one of its detected faces.",
                );
                ui.small(
                    "Database suggestions prefer one representative per effective Person group. Before a People snapshot exists, they fall back to individual face instances.",
                );

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("Minimum similarity");
                    ui.add(
                        egui::Slider::new(
                            &mut self.face_search_ui.options.min_similarity,
                            0.0..=1.0,
                        )
                        .fixed_decimals(2),
                    );
                    ui.label("Top-K");
                    ui.add(
                        egui::DragValue::new(&mut self.face_search_ui.options.limit)
                            .range(1..=5_000)
                            .speed(5.0),
                    );
                });
                self.face_search_ui.options = self.face_search_ui.options.sanitized();

                let selected = self
                    .face_search_ui
                    .selected_face_id
                    .as_ref()
                    .and_then(|id| {
                        self.face_search_ui
                            .suggestions
                            .iter()
                            .find(|face| &face.face_id == id)
                    })
                    .cloned();
                let selected_external = self
                    .face_search_ui
                    .selected_external_ordinal
                    .and_then(|ordinal| {
                        self.face_search_ui
                            .external_faces
                            .iter()
                            .find(|face| face.ordinal == ordinal)
                    })
                    .cloned();

                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            selected.is_some() && !self.busy,
                            egui::Button::new("Search selected person/face"),
                        )
                        .clicked()
                    {
                        if let Some(query) = selected.clone() {
                            self.start_indexed_face_search(query);
                        }
                    }
                    if ui
                        .add_enabled(
                            selected_external.is_some() && !self.busy,
                            egui::Button::new("Search selected file face"),
                        )
                        .clicked()
                    {
                        if let Some(query) = selected_external.clone() {
                            self.start_external_face_search(query);
                        }
                    }
                    if self.face_search_ui.searching {
                        ui.spinner();
                        ui.small("Comparing face embeddings…");
                    }
                    if self.face_search_ui.active {
                        ui.separator();
                        ui.small(format!(
                            "{} compatible embeddings inspected in last search",
                            self.face_search_ui.last_rows_considered
                        ));
                    }
                });

                if !self.face_search_ui.external_faces.is_empty() {
                    ui.separator();
                    ui.strong("Faces from selected file");
                    if let Some(path) = &self.face_search_ui.external_source {
                        ui.small(path.display().to_string());
                    }
                    let faces = self.face_search_ui.external_faces.clone();
                    ui.horizontal_wrapped(|ui| {
                        for face in faces {
                            let is_selected = self
                                .face_search_ui
                                .selected_external_ordinal
                                .is_some_and(|ordinal| ordinal == face.ordinal);
                            ui.vertical(|ui| {
                                let response = if let Some(texture) = self.thumbnail(&face.image_path)
                                {
                                    photo_grid::photo_tile(ui, &texture, egui::vec2(104.0, 104.0), PhotoTileMode::Face(face.bbox), is_selected, egui::Sense::click())
                                } else {
                                    ui.add_sized([104.0, 104.0], egui::Button::new("Loading…"))
                                };
                                if response.clicked() {
                                    self.face_search_ui.selected_external_ordinal = Some(face.ordinal);
                                }
                                if response.double_clicked() && !self.busy {
                                    self.start_external_face_search(face.clone());
                                }
                                ui.small(format!(
                                    "Face {} · {:.0}%",
                                    face.ordinal + 1,
                                    face.confidence * 100.0
                                ));
                            });
                        }
                    });
                }

                ui.separator();
                ui.strong("People / searchable faces in database");
                ui.add(
                    egui::TextEdit::singleline(&mut self.face_search_ui.filter_text)
                        .hint_text("Filter named people…")
                        .desired_width(ui.available_width().min(360.0)),
                );
                if self.face_search_ui.suggestions.is_empty() && !self.face_search_ui.loading {
                    ui.vertical_centered(|ui| {
                        ui.add_space(28.0);
                        ui.heading("No searchable database faces yet");
                        ui.label(
                            "Enable Detect faces for a collection, configure YuNet + SFace in Settings, then run the face pipeline.",
                        );
                    });
                    return;
                }

                let filter = self.face_search_ui.filter_text.trim().to_lowercase();
                let suggestions = self
                    .face_search_ui
                    .suggestions
                    .iter()
                    .filter(|face| {
                        filter.is_empty()
                            || self
                                .face_search_ui
                                .suggestion_names
                                .get(&face.face_id)
                                .is_some_and(|name| name.to_lowercase().contains(&filter))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if !filter.is_empty() {
                    ui.small(format!("{} matching named People", suggestions.len()));
                }
                let spec = PhotoGridSpec::new("face-search-database-photo-grid", 108.0, 142.0);
                photo_grid::show(ui, suggestions.len(), spec, |ui, index| {
                    let face = &suggestions[index];
                    let is_selected = self
                        .face_search_ui
                        .selected_face_id
                        .as_ref()
                        .is_some_and(|id| id == &face.face_id);
                    let response = if let Some(texture) = self.thumbnail(&face.image_path) {
                        photo_grid::photo_tile(
                            ui,
                            &texture,
                            egui::vec2(96.0, 96.0),
                            PhotoTileMode::Face(face.bbox),
                            is_selected,
                            egui::Sense::click(),
                        )
                    } else {
                        let response = ui.add_sized([96.0, 96.0], egui::Button::new("Loading…"));
                        if is_selected {
                            ui.painter().rect_stroke(
                                response.rect,
                                5.0,
                                egui::Stroke::new(3.0, ui.visuals().selection.stroke.color),
                                egui::StrokeKind::Inside,
                            );
                        }
                        response
                    };
                    if response.clicked() {
                        self.face_search_ui.selected_face_id = Some(face.face_id.clone());
                    }
                    if response.double_clicked() && !self.busy {
                        self.start_indexed_face_search(face.clone());
                    }
                    if let Some(name) = self.face_search_ui.suggestion_names.get(&face.face_id) {
                        ui.strong(truncate(name, 16));
                    }
                    if let Some(group_size) = face.group_size {
                        ui.small(format!(
                            "Person · {group_size} face{}",
                            if group_size == 1 { "" } else { "s" }
                        ));
                    } else {
                        ui.small(format!("{:.0}%", face.confidence * 100.0));
                    }
                    ui.small(
                        face.image_path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .map(|name| truncate(name, 16))
                            .unwrap_or_else(|| "image".to_owned()),
                    )
                    .on_hover_text(face.image_path.display().to_string());
                });
            });
        self.face_search_ui.open = open;
    }
}

fn prepare_external_faces(
    path: &Path,
    detector_settings: FaceDetectorSettings,
    embedding_settings: FaceEmbeddingSettings,
) -> Result<Vec<ExternalFaceChoice>> {
    if !path.is_file() {
        bail!("external query image is unavailable: {}", path.display());
    }
    if !detector_settings.configured() || !detector_settings.model_path.is_file() {
        bail!("configure an available YuNet model in Settings before using Face from file");
    }
    if !embedding_settings.configured() || !embedding_settings.model_path.is_file() {
        bail!("configure an available SFace model in Settings before using Face from file");
    }

    let image = face_detection::decode_oriented(path)
        .with_context(|| format!("decoding external face query {}", path.display()))?;
    let mut detector = YuNetProductionDetector::load(&detector_settings)
        .context("loading YuNet for external face query")?;
    let detections = detector
        .detect(&image)
        .context("detecting faces in external query image")?;
    if detections.is_empty() {
        return Ok(Vec::new());
    }

    let mut embedder = SFaceProductionEmbedder::load(&embedding_settings)
        .context("loading SFace for external face query")?;
    let revision = FaceEmbeddingRevision {
        model_id: embedder.model_id().to_owned(),
        model_version: embedder.model_version().to_owned(),
        model_cache_revision: embedder.cache_revision(),
        schema_version: face_embedding::SCHEMA_VERSION,
        alignment_revision: embedder.alignment_revision(),
        dimension: embedder.embedding_dimension(),
    };

    let detected_count = detections.len();
    let mut output = Vec::with_capacity(detected_count);
    for (ordinal, face) in detections.into_iter().enumerate() {
        let aligned = match embedder.align_face(&image, face.bbox, &face.landmarks) {
            Ok(aligned) => aligned,
            Err(_) => continue,
        };
        let raw = match embedder.embed(&aligned) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        let values = match face_embedding::normalize_embedding(raw, revision.dimension) {
            Ok(values) => values,
            Err(_) => continue,
        };
        let query = FaceSimilarityQuery {
            root: PathBuf::new(),
            library_id: "external-query".to_owned(),
            face_id: format!("external-face-{ordinal}"),
            relative_image_path: path.to_path_buf(),
            revision: revision.clone(),
            values,
        };
        output.push(ExternalFaceChoice {
            image_path: path.to_path_buf(),
            ordinal,
            confidence: face.confidence,
            bbox: face.bbox,
            query,
        });
    }

    if output.is_empty() {
        bail!(
            "YuNet detected {detected_count} face(s), but SFace could not create a searchable embedding for any of them"
        );
    }
    Ok(output)
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut output = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
}
