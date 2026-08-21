use super::ImageSearchApp;
use crate::face_detection::FaceBox;
use crate::face_search::{
    self, IndexedFaceSearchOptions, IndexedFaceSearchReport, IndexedFaceSuggestion,
};
use eframe::egui;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

const DEFAULT_SUGGESTION_LIMIT: usize = 240;

#[derive(Debug)]
enum FaceSearchUiMessage {
    Suggestions(Result<Vec<IndexedFaceSuggestion>, String>),
    SearchFinished {
        query: IndexedFaceSuggestion,
        report: Result<IndexedFaceSearchReport, String>,
    },
}

pub(super) struct FaceSearchUiState {
    open: bool,
    suggestions: Vec<IndexedFaceSuggestion>,
    selected_face_id: Option<String>,
    options: IndexedFaceSearchOptions,
    tx: Sender<FaceSearchUiMessage>,
    rx: Receiver<FaceSearchUiMessage>,
    loading: bool,
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
            selected_face_id: None,
            options: IndexedFaceSearchOptions::default(),
            tx,
            rx,
            loading: false,
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
                        Ok(suggestions) => {
                            self.face_search_ui.suggestions = suggestions;
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
                        }
                        Err(error) => {
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
            }
        }
    }

    fn refresh_face_suggestions(&mut self) {
        if self.face_search_ui.loading {
            return;
        }
        let roots = self.roots.clone();
        let tx = self.face_search_ui.tx.clone();
        self.face_search_ui.loading = true;
        self.status = "Loading searchable faces from portable indexes…".to_owned();
        std::thread::spawn(move || {
            let result = face_search::list_searchable_faces(&roots, DEFAULT_SUGGESTION_LIMIT)
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(FaceSearchUiMessage::Suggestions(result));
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

    fn apply_face_search_report(
        &mut self,
        query: IndexedFaceSuggestion,
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
        self.face_search_ui.active_query = Some(query.clone());
        self.face_search_ui.match_boxes = match_boxes;
        self.face_search_ui.last_rows_considered = report.rows_considered;
        self.similarity_results = Some(results);
        self.query_image = Some(query.image_path.clone());
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
                            egui::Button::new("⟳ Refresh faces"),
                        )
                        .clicked()
                    {
                        self.refresh_face_suggestions();
                    }
                    if self.face_search_ui.loading {
                        ui.spinner();
                        ui.small("Reading face index…");
                    }
                });

                ui.label(
                    "Choose any detected face already stored in the database. Search returns parent images ranked by identity similarity.",
                );
                ui.small(
                    "These are detected face instances, not unique people yet. People clustering will group repeated appearances in the next stage.",
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

                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            selected.is_some() && !self.busy,
                            egui::Button::new("Search selected face"),
                        )
                        .clicked()
                    {
                        if let Some(query) = selected.clone() {
                            self.start_indexed_face_search(query);
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

                ui.separator();
                if self.face_search_ui.suggestions.is_empty() && !self.face_search_ui.loading {
                    ui.vertical_centered(|ui| {
                        ui.add_space(40.0);
                        ui.heading("No searchable faces yet");
                        ui.label(
                            "Enable Detect faces for a collection, configure YuNet + SFace in Settings, then run the face pipeline.",
                        );
                    });
                    return;
                }

                let suggestions = self.face_search_ui.suggestions.clone();
                let available = ui.available_width().max(300.0);
                let cell = 116.0;
                let columns = ((available / cell).floor() as usize).max(1);
                let rows = suggestions.len().div_ceil(columns);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show_rows(ui, 142.0, rows, |ui, row_range| {
                        for row in row_range {
                            ui.horizontal(|ui| {
                                for column in 0..columns {
                                    let index = row * columns + column;
                                    if index >= suggestions.len() {
                                        break;
                                    }
                                    let face = &suggestions[index];
                                    let is_selected = self
                                        .face_search_ui
                                        .selected_face_id
                                        .as_ref()
                                        .is_some_and(|id| id == &face.face_id);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(108.0, 136.0),
                                        egui::Layout::top_down(egui::Align::Center),
                                        |ui| {
                                            let response = if let Some(texture) = self.thumbnail(&face.image_path) {
                                                face_crop_widget(
                                                    ui,
                                                    &texture,
                                                    face.bbox,
                                                    egui::vec2(96.0, 96.0),
                                                    is_selected,
                                                )
                                            } else {
                                                let response = ui.add_sized(
                                                    [96.0, 96.0],
                                                    egui::Button::new("Loading…"),
                                                );
                                                if is_selected {
                                                    ui.painter().rect_stroke(
                                                        response.rect,
                                                        5.0,
                                                        egui::Stroke::new(
                                                            3.0,
                                                            ui.visuals().selection.stroke.color,
                                                        ),
                                                        egui::StrokeKind::Inside,
                                                    );
                                                }
                                                response
                                            };
                                            if response.clicked() {
                                                self.face_search_ui.selected_face_id =
                                                    Some(face.face_id.clone());
                                            }
                                            if response.double_clicked() && !self.busy {
                                                self.start_indexed_face_search(face.clone());
                                            }
                                            ui.small(format!("{:.0}%", face.confidence * 100.0));
                                            ui.small(
                                                face.image_path
                                                    .file_name()
                                                    .and_then(|name| name.to_str())
                                                    .map(|name| truncate(name, 16))
                                                    .unwrap_or_else(|| "image".to_owned()),
                                            )
                                            .on_hover_text(face.image_path.display().to_string());
                                        },
                                    );
                                }
                            });
                        }
                    });
            });
        self.face_search_ui.open = open;
    }
}

fn face_crop_widget(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    bbox: FaceBox,
    desired: egui::Vec2,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
    ui.painter()
        .rect_filled(rect, 5.0, ui.visuals().extreme_bg_color);

    let center_x = bbox.x + bbox.width * 0.5;
    let center_y = bbox.y + bbox.height * 0.5;
    let square = (bbox.width.max(bbox.height) * 1.45).clamp(0.06, 1.0);
    let half = square * 0.5;
    let mut min_x = (center_x - half).clamp(0.0, 1.0);
    let mut min_y = (center_y - half).clamp(0.0, 1.0);
    let mut max_x = (center_x + half).clamp(0.0, 1.0);
    let mut max_y = (center_y + half).clamp(0.0, 1.0);
    if max_x - min_x < square {
        if min_x <= 0.0 {
            max_x = square.min(1.0);
        } else if max_x >= 1.0 {
            min_x = (1.0 - square).max(0.0);
        }
    }
    if max_y - min_y < square {
        if min_y <= 0.0 {
            max_y = square.min(1.0);
        } else if max_y >= 1.0 {
            min_y = (1.0 - square).max(0.0);
        }
    }
    ui.painter().image(
        texture.id(),
        rect,
        egui::Rect::from_min_max(egui::pos2(min_x, min_y), egui::pos2(max_x, max_y)),
        egui::Color32::WHITE,
    );
    if selected {
        ui.painter().rect_stroke(
            rect,
            5.0,
            egui::Stroke::new(3.0, ui.visuals().selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    }
    response
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut output = value.chars().take(max_chars.saturating_sub(1)).collect::<String>();
    output.push('…');
    output
}
