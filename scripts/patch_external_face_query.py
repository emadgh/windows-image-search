from pathlib import Path


def replace(path: str, old: str, new: str, count: int = 1):
    p = Path(path)
    text = p.read_text(encoding='utf-8')
    actual = text.count(old)
    if actual < count:
        raise RuntimeError(f'{path}: expected {count}, found {actual}: {old[:120]!r}')
    text = text.replace(old, new, count)
    p.write_text(text, encoding='utf-8')

# Core: construct one normalized SFace query per YuNet face in an arbitrary image.
replace(
    'src/face_search.rs',
    '''use crate::face_detection::{FaceBox, FaceLandmark};
use crate::face_similarity::{self, FaceSimilarityOptions, FaceSimilarityQuery};
use crate::portable;
''',
    '''use crate::face_detection::yunet_production::YuNetProductionDetector;
use crate::face_detection::yunet_settings::FaceDetectorSettings;
use crate::face_detection::{self, FaceBox, FaceDetector, FaceLandmark};
use crate::face_embedding::{self, FaceEmbedder};
use crate::face_settings::FaceEmbeddingSettings;
use crate::face_sface_production::SFaceProductionEmbedder;
use crate::face_similarity::{
    self, FaceEmbeddingRevision, FaceSimilarityOptions, FaceSimilarityQuery,
};
use crate::portable;
''',
)
replace(
    'src/face_search.rs',
    '''#[derive(Clone, Debug, PartialEq)]
pub struct IndexedFaceSearchOptions {
''',
    '''#[derive(Clone, Debug)]
pub struct ExternalFaceQuery {
    pub image_path: PathBuf,
    pub ordinal: usize,
    pub confidence: f32,
    pub bbox: FaceBox,
    pub query: FaceSimilarityQuery,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexedFaceSearchOptions {
''',
)
replace(
    'src/face_search.rs',
    '''pub fn list_searchable_faces(
''',
    '''pub fn build_external_face_queries(
    image_path: &Path,
    detector_settings: &FaceDetectorSettings,
    embedding_settings: &FaceEmbeddingSettings,
) -> Result<Vec<ExternalFaceQuery>> {
    if !image_path.is_file() {
        bail!("external face query image is unavailable: {}", image_path.display());
    }
    if !detector_settings.configured() {
        bail!("YuNet must be configured before searching a face from file");
    }
    if !embedding_settings.configured() {
        bail!("SFace must be configured before searching a face from file");
    }

    let oriented = face_detection::decode_oriented(image_path)?;
    let mut detector = YuNetProductionDetector::load(detector_settings)?;
    let detections = detector.detect(&oriented)?;
    if detections.is_empty() {
        return Ok(Vec::new());
    }

    let mut embedder = SFaceProductionEmbedder::load(embedding_settings)?;
    let revision = FaceEmbeddingRevision {
        model_id: embedder.model_id().to_owned(),
        model_version: embedder.model_version().to_owned(),
        model_cache_revision: embedder.cache_revision(),
        schema_version: face_embedding::SCHEMA_VERSION,
        alignment_revision: embedder.alignment_revision(),
        dimension: embedder.embedding_dimension(),
    };

    let mut queries = Vec::with_capacity(detections.len());
    for (ordinal, detection) in detections.into_iter().enumerate() {
        let aligned = embedder.align_face(&oriented, detection.bbox, &detection.landmarks)?;
        let raw = embedder.embed(&aligned)?;
        let values = face_embedding::normalize_embedding(raw, revision.dimension)?;
        let query = FaceSimilarityQuery {
            root: PathBuf::new(),
            library_id: "external-query".to_owned(),
            face_id: format!("external-query-{ordinal}"),
            relative_image_path: image_path.to_path_buf(),
            revision: revision.clone(),
            values,
        };
        queries.push(ExternalFaceQuery {
            image_path: image_path.to_path_buf(),
            ordinal,
            confidence: detection.confidence,
            bbox: detection.bbox,
            query,
        });
    }
    Ok(queries)
}

pub fn list_searchable_faces(
''',
)
replace(
    'src/face_search.rs',
    '''        .filter(|item| item.similarity >= options.min_similarity)
        .map(|item| IndexedFaceSearchHit {
''',
    '''        .filter(|item| item.similarity >= options.min_similarity)
        .filter(|item| {
            query.library_id != "external-query" || item.image_path != query.relative_image_path
        })
        .map(|item| IndexedFaceSearchHit {
''',
)

# UI state/messages.
replace(
    'src/ui/face_search_panel.rs',
    '''use crate::face_search::{
    self, IndexedFaceSearchOptions, IndexedFaceSearchReport, IndexedFaceSuggestion,
};
''',
    '''use crate::face_search::{
    self, ExternalFaceQuery, IndexedFaceSearchOptions, IndexedFaceSearchReport,
    IndexedFaceSuggestion,
};
''',
)
replace(
    'src/ui/face_search_panel.rs',
    '''enum FaceSearchUiMessage {
    Suggestions(Result<Vec<IndexedFaceSuggestion>, String>),
    SearchFinished {
        query: IndexedFaceSuggestion,
        report: Result<IndexedFaceSearchReport, String>,
    },
}
''',
    '''enum FaceSearchUiMessage {
    Suggestions(Result<Vec<IndexedFaceSuggestion>, String>),
    ExternalQueries(Result<Vec<ExternalFaceQuery>, String>),
    SearchFinished {
        query_image: PathBuf,
        report: Result<IndexedFaceSearchReport, String>,
    },
}
''',
)
replace(
    'src/ui/face_search_panel.rs',
    '''    selected_face_id: Option<String>,
    options: IndexedFaceSearchOptions,
''',
    '''    selected_face_id: Option<String>,
    external_queries: Vec<ExternalFaceQuery>,
    selected_external: Option<usize>,
    options: IndexedFaceSearchOptions,
''',
)
replace(
    'src/ui/face_search_panel.rs',
    '''    loading: bool,
    searching: bool,
    active: bool,
    active_query: Option<IndexedFaceSuggestion>,
''',
    '''    loading: bool,
    loading_external: bool,
    searching: bool,
    active: bool,
    active_query: Option<PathBuf>,
''',
)
replace(
    'src/ui/face_search_panel.rs',
    '''            suggestions: Vec::new(),
            selected_face_id: None,
            options: IndexedFaceSearchOptions::default(),
''',
    '''            suggestions: Vec::new(),
            selected_face_id: None,
            external_queries: Vec::new(),
            selected_external: None,
            options: IndexedFaceSearchOptions::default(),
''',
)
replace(
    'src/ui/face_search_panel.rs',
    '''            loading: false,
            searching: false,
''',
    '''            loading: false,
            loading_external: false,
            searching: false,
''',
)

# Message processing.
replace(
    'src/ui/face_search_panel.rs',
    '''                FaceSearchUiMessage::SearchFinished { query, report } => {
                    self.face_search_ui.searching = false;
                    self.busy = self.indexing || self.searching;
                    match report {
                        Ok(report) => {
                            self.apply_face_search_report(query, report);
                        }
''',
    '''                FaceSearchUiMessage::ExternalQueries(result) => {
                    self.face_search_ui.loading_external = false;
                    match result {
                        Ok(queries) => {
                            self.face_search_ui.external_queries = queries;
                            self.face_search_ui.selected_external = None;
                            if self.face_search_ui.external_queries.len() == 1 {
                                self.face_search_ui.selected_external = Some(0);
                            }
                            if self.face_search_ui.external_queries.is_empty() {
                                self.status = "No face detected in selected query image".to_owned();
                            } else {
                                self.status = format!(
                                    "Detected {} face{} in query image",
                                    self.face_search_ui.external_queries.len(),
                                    if self.face_search_ui.external_queries.len() == 1 { "" } else { "s" }
                                );
                            }
                        }
                        Err(error) => self.last_error = Some(error),
                    }
                }
                FaceSearchUiMessage::SearchFinished { query_image, report } => {
                    self.face_search_ui.searching = false;
                    self.busy = self.indexing || self.searching;
                    match report {
                        Ok(report) => {
                            self.apply_face_search_report(query_image, report);
                        }
''',
)

# Indexed search finish payload and apply signature.
replace(
    'src/ui/face_search_panel.rs',
    '''            let _ = tx.send(FaceSearchUiMessage::SearchFinished { query, report });
''',
    '''            let _ = tx.send(FaceSearchUiMessage::SearchFinished {
                query_image: query.image_path,
                report,
            });
''',
)
replace(
    'src/ui/face_search_panel.rs',
    '''    fn apply_face_search_report(
        &mut self,
        query: IndexedFaceSuggestion,
        report: IndexedFaceSearchReport,
    ) {
''',
    '''    fn apply_face_search_report(
        &mut self,
        query_image: PathBuf,
        report: IndexedFaceSearchReport,
    ) {
''',
)
replace(
    'src/ui/face_search_panel.rs',
    '''        self.face_search_ui.active_query = Some(query.clone());
''',
    '''        self.face_search_ui.active_query = Some(query_image.clone());
''',
)
replace(
    'src/ui/face_search_panel.rs',
    '''        self.query_image = Some(query.image_path.clone());
''',
    '''        self.query_image = Some(query_image);
''',
)

# Add external loader/search methods before apply_face_search_report.
replace(
    'src/ui/face_search_panel.rs',
    '''    fn apply_face_search_report(
''',
    '''    fn choose_external_face_query(&mut self) {
        if self.busy || self.face_search_ui.loading_external {
            return;
        }
        let Some(image_path) = rfd::FileDialog::new()
            .add_filter("Images", &["jpg", "jpeg", "png", "tif", "tiff", "webp"])
            .pick_file()
        else {
            return;
        };
        let detector = self.face_detector_settings_snapshot();
        let embedding = self.face_embedding_settings.clone();
        let tx = self.face_search_ui.tx.clone();
        self.face_search_ui.loading_external = true;
        self.face_search_ui.external_queries.clear();
        self.face_search_ui.selected_external = None;
        self.status = "Detecting faces in query image…".to_owned();
        std::thread::spawn(move || {
            let result = face_search::build_external_face_queries(
                &image_path,
                &detector,
                &embedding,
            )
            .map_err(|err| format!("{err:#}"));
            let _ = tx.send(FaceSearchUiMessage::ExternalQueries(result));
        });
    }

    fn start_external_face_search(&mut self, external: ExternalFaceQuery) {
        if self.busy || self.face_search_ui.searching {
            return;
        }
        let roots = self.roots.clone();
        let options = self.face_search_ui.options.sanitized();
        let query = external.query;
        let query_image = external.image_path;
        let tx = self.face_search_ui.tx.clone();
        self.face_search_ui.searching = true;
        self.busy = true;
        self.last_error = None;
        self.status = "Searching external face identity…".to_owned();
        std::thread::spawn(move || {
            let report = face_search::search_embedding_query(&roots, &query, options)
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(FaceSearchUiMessage::SearchFinished { query_image, report });
        });
    }

    fn apply_face_search_report(
''',
)

# Top controls: add Face from file button and spinner.
replace(
    'src/ui/face_search_panel.rs',
    '''                    if self.face_search_ui.loading {
                        ui.spinner();
                        ui.small("Reading face index…");
                    }
''',
    '''                    if ui
                        .add_enabled(
                            !self.busy && !self.face_search_ui.loading_external,
                            egui::Button::new("Face from file…"),
                        )
                        .clicked()
                    {
                        self.choose_external_face_query();
                    }
                    if self.face_search_ui.loading {
                        ui.spinner();
                        ui.small("Reading face index…");
                    }
                    if self.face_search_ui.loading_external {
                        ui.spinner();
                        ui.small("Detecting query faces…");
                    }
''',
)

# Insert external multi-face chooser before database grid separator.
replace(
    'src/ui/face_search_panel.rs',
    '''                ui.separator();
                if self.face_search_ui.suggestions.is_empty() && !self.face_search_ui.loading {
''',
    '''                if !self.face_search_ui.external_queries.is_empty() {
                    ui.separator();
                    ui.strong("Faces detected in selected file");
                    ui.small("Choose one face below; if the file contains one face it is preselected.");
                    let external = self.face_search_ui.external_queries.clone();
                    ui.horizontal_wrapped(|ui| {
                        for (index, face) in external.iter().enumerate() {
                            let selected = self.face_search_ui.selected_external == Some(index);
                            ui.vertical(|ui| {
                                let response = if let Some(texture) = self.thumbnail(&face.image_path) {
                                    face_crop_widget(
                                        ui,
                                        &texture,
                                        face.bbox,
                                        egui::vec2(104.0, 104.0),
                                        selected,
                                    )
                                } else {
                                    ui.add_sized([104.0, 104.0], egui::Button::new("Loading…"))
                                };
                                if response.clicked() {
                                    self.face_search_ui.selected_external = Some(index);
                                }
                                ui.small(format!("Face {} · {:.0}%", index + 1, face.confidence * 100.0));
                            });
                        }
                    });
                    let selected_external = self
                        .face_search_ui
                        .selected_external
                        .and_then(|index| self.face_search_ui.external_queries.get(index))
                        .cloned();
                    if ui
                        .add_enabled(
                            selected_external.is_some() && !self.busy,
                            egui::Button::new("Search selected file face"),
                        )
                        .clicked()
                    {
                        if let Some(query) = selected_external {
                            self.start_external_face_search(query);
                        }
                    }
                }

                ui.separator();
                if self.face_search_ui.suggestions.is_empty() && !self.face_search_ui.loading {
''',
)

print('external face-query flow patched')
