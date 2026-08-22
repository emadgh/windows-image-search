use super::ImageSearchApp;
use crate::db::{self, Collection, CollectionMembership, ImageSummary};
use crate::face_scope;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub(super) struct CollectionDragPayload {
    paths: Vec<PathBuf>,
}

#[derive(Default)]
pub(super) struct CollectionsState {
    items: Vec<Collection>,
    selected_manage: Option<i64>,
    active_filter: Option<i64>,
    memberships: HashMap<i64, CollectionMembership>,
    effective: HashMap<i64, HashSet<PathBuf>>,
    discovered_counts: HashMap<i64, usize>,
    face_detection: HashMap<i64, bool>,
    new_name: String,
    rename_name: String,
}

impl CollectionsState {
    pub(super) fn load(db_path: &Path, images: &[ImageSummary]) -> anyhow::Result<Self> {
        let mut state = Self::default();
        state.reload(db_path, images)?;
        Ok(state)
    }

    fn reload(&mut self, db_path: &Path, images: &[ImageSummary]) -> anyhow::Result<()> {
        self.items = db::load_collections(db_path)?;
        self.face_detection = face_scope::load_collection_flags(db_path)?;
        if self
            .selected_manage
            .is_some_and(|id| !self.items.iter().any(|item| item.id == id))
        {
            self.selected_manage = None;
        }
        if self
            .active_filter
            .is_some_and(|id| !self.items.iter().any(|item| item.id == id))
        {
            self.active_filter = None;
        }
        if self.selected_manage.is_none() {
            self.selected_manage = self.items.first().map(|item| item.id);
        }

        self.memberships.clear();
        for item in &self.items {
            self.memberships
                .insert(item.id, db::load_collection_membership(db_path, item.id)?);
        }
        if let Some(id) = self.selected_manage {
            self.rename_name = self
                .items
                .iter()
                .find(|item| item.id == id)
                .map(|item| item.name.clone())
                .unwrap_or_default();
        } else {
            self.rename_name.clear();
        }
        self.rebuild_effective(images);
        let discovered = db::load_discovered_paths(db_path)?;
        self.rebuild_discovered_counts(&discovered);
        Ok(())
    }

    fn rebuild_discovered_counts(&mut self, discovered: &[PathBuf]) {
        self.discovered_counts.clear();
        for item in &self.items {
            let Some(membership) = self.memberships.get(&item.id) else {
                self.discovered_counts.insert(item.id, 0);
                continue;
            };
            let manual: HashSet<&Path> = membership.files.iter().map(PathBuf::as_path).collect();
            let count = discovered
                .iter()
                .filter(|path| {
                    manual.contains(path.as_path())
                        || membership
                            .folders
                            .iter()
                            .any(|folder| path.starts_with(folder))
                })
                .count();
            self.discovered_counts.insert(item.id, count);
        }
    }

    pub(super) fn refresh_discovered_counts(&mut self, db_path: &Path) -> anyhow::Result<()> {
        let discovered = db::load_discovered_paths(db_path)?;
        self.rebuild_discovered_counts(&discovered);
        Ok(())
    }

    pub(super) fn rebuild_effective(&mut self, images: &[ImageSummary]) {
        self.effective.clear();
        for item in &self.items {
            let Some(membership) = self.memberships.get(&item.id) else {
                self.effective.insert(item.id, HashSet::new());
                continue;
            };
            let manual: HashSet<&Path> = membership.files.iter().map(PathBuf::as_path).collect();
            let mut paths = HashSet::new();
            for image in images {
                if manual.contains(image.path.as_path())
                    || membership
                        .folders
                        .iter()
                        .any(|folder| image.path.starts_with(folder))
                {
                    paths.insert(image.path.clone());
                }
            }
            self.effective.insert(item.id, paths);
        }
    }

    fn count(&self, id: i64) -> usize {
        self.effective.get(&id).map_or(0, HashSet::len)
    }

    fn total_count(&self, id: i64) -> usize {
        self.discovered_counts
            .get(&id)
            .copied()
            .unwrap_or(0)
            .max(self.count(id))
    }

    fn filter_matches(&self, path: &Path) -> bool {
        self.active_filter
            .map(|id| {
                self.effective
                    .get(&id)
                    .is_some_and(|paths| paths.contains(path))
            })
            .unwrap_or(true)
    }

    fn filter_label(&self) -> String {
        self.active_filter
            .and_then(|id| self.items.iter().find(|item| item.id == id))
            .map(|item| format!("{} ({})", item.name, self.count(item.id)))
            .unwrap_or_else(|| "All images".to_owned())
    }
}

#[derive(Clone, Debug)]
enum CollectionAction {
    Create(String),
    Rename(i64, String),
    SetFaceDetection(i64, bool),
    Delete(i64),
    AddFolderDialog(i64),
    AddFilesDialog(i64),
    RemoveFolder(i64, PathBuf),
    RemoveFile(i64, PathBuf),
    Drop(i64, Vec<PathBuf>),
}

impl ImageSearchApp {
    pub(super) fn refresh_collection_effective_membership(&mut self) {
        self.collections.rebuild_effective(&self.images);
    }

    pub(super) fn collection_filter_matches(&self, path: &Path) -> bool {
        self.collections.filter_matches(path)
    }

    pub(super) fn attach_collection_drag_source(&self, response: &egui::Response, path: &Path) {
        let paths = if self.selected_paths.contains(path) && self.selected_paths.len() > 1 {
            self.selected_paths.iter().cloned().collect()
        } else {
            vec![path.to_path_buf()]
        };
        response.dnd_set_drag_payload(CollectionDragPayload { paths });
    }

    pub(super) fn show_collection_filter(&mut self, ui: &mut egui::Ui) {
        ui.strong("Collection");
        let items = self.collections.items.clone();
        let counts: HashMap<i64, usize> = items
            .iter()
            .map(|item| (item.id, self.collections.count(item.id)))
            .collect();
        egui::ComboBox::from_id_salt("collection_filter")
            .selected_text(self.collections.filter_label())
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.collections.active_filter, None, "All images");
                for item in items {
                    let count = counts.get(&item.id).copied().unwrap_or(0);
                    ui.selectable_value(
                        &mut self.collections.active_filter,
                        Some(item.id),
                        format!("{} ({count})", item.name),
                    );
                }
            });
        ui.add_space(8.0);
    }

    pub(super) fn show_collections_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(
            "Collections are virtual groups. Assign indexed folders recursively, add individual indexed files, or drag items here. Source files are never moved or deleted.",
        );
        if self.busy {
            ui.small("Collection editing is locked while indexing/search work is active.");
        }

        let items = self.collections.items.clone();
        let selected_id = self.collections.selected_manage;
        let selected_membership = selected_id
            .and_then(|id| self.collections.memberships.get(&id).cloned())
            .unwrap_or_default();
        let mut action: Option<CollectionAction> = None;
        let mut select_collection: Option<(i64, String)> = None;

        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_min_width(250.0);
                ui.strong("Collections");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.collections.new_name)
                            .hint_text("New collection")
                            .desired_width(155.0),
                    );
                    if ui
                        .add_enabled(
                            !self.busy && !self.collections.new_name.trim().is_empty(),
                            egui::Button::new("Add"),
                        )
                        .clicked()
                    {
                        action = Some(CollectionAction::Create(
                            self.collections.new_name.trim().to_owned(),
                        ));
                    }
                });
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        if items.is_empty() {
                            ui.label("No collections yet.");
                        }
                        for item in &items {
                            let count = self.collections.count(item.id);
                            let response = ui.selectable_label(
                                selected_id == Some(item.id),
                                format!(
                                    "{}  ·  {count}/{} indexed",
                                    item.name,
                                    self.collections.total_count(item.id)
                                ),
                            );
                            if response.clicked() {
                                select_collection = Some((item.id, item.name.clone()));
                            }
                            if !self.busy {
                                paint_drop_feedback(ui, &response);
                                if let Some(paths) = released_drop_paths(ui, &response) {
                                    action = Some(CollectionAction::Drop(item.id, paths));
                                }
                            }
                        }
                    });
            });

            ui.separator();

            ui.vertical(|ui| {
                ui.set_min_width(470.0);
                let Some(id) = selected_id else {
                    ui.label("Select or create a collection to manage assignments.");
                    return;
                };
                let selected_name = items
                    .iter()
                    .find(|item| item.id == id)
                    .map(|item| item.name.clone())
                    .unwrap_or_else(|| "Collection".to_owned());
                ui.strong(selected_name);
                ui.small(format!(
                    "{} indexed / {} discovered image{}",
                    self.collections.count(id),
                    self.collections.total_count(id),
                    if self.collections.total_count(id) == 1 { "" } else { "s" }
                ));

                ui.horizontal(|ui| {
                    ui.add_enabled(
                        !self.busy,
                        egui::TextEdit::singleline(&mut self.collections.rename_name)
                            .desired_width(240.0),
                    );
                    if ui
                        .add_enabled(
                            !self.busy && !self.collections.rename_name.trim().is_empty(),
                            egui::Button::new("Rename"),
                        )
                        .clicked()
                    {
                        action = Some(CollectionAction::Rename(
                            id,
                            self.collections.rename_name.trim().to_owned(),
                        ));
                    }
                    if ui
                        .add_enabled(!self.busy, egui::Button::new("Delete collection"))
                        .clicked()
                    {
                        action = Some(CollectionAction::Delete(id));
                    }
                });
                ui.small("Deleting a collection only removes its membership records; image files stay untouched.");

                let mut detect_faces = self
                    .collections
                    .face_detection
                    .get(&id)
                    .copied()
                    .unwrap_or(false);
                if ui
                    .add_enabled(
                        !self.busy,
                        egui::Checkbox::new(
                            &mut detect_faces,
                            "Detect faces in this collection",
                        ),
                    )
                    .on_hover_text(
                        "Only effective members of face-enabled collections are sent to the face detector.",
                    )
                    .changed()
                {
                    action = Some(CollectionAction::SetFaceDetection(id, detect_faces));
                }
                ui.small(
                    "Off by default. Texture-only collections are skipped completely by face detection. Existing face data is kept when this is turned off.",
                );

                ui.add_space(8.0);
                let drop_response = ui.add_sized(
                    [ui.available_width(), 48.0],
                    egui::Label::new(
                        "Drop indexed files/folders here from Explorer or drag Grid/Details items here",
                    )
                    .sense(egui::Sense::hover()),
                );
                if !self.busy {
                    paint_drop_feedback(ui, &drop_response);
                    if let Some(paths) = released_drop_paths(ui, &drop_response) {
                        action = Some(CollectionAction::Drop(id, paths));
                    }
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.strong("Assigned folders");
                    if ui
                        .add_enabled(!self.busy, egui::Button::new("＋ Add folder").small())
                        .clicked()
                    {
                        action = Some(CollectionAction::AddFolderDialog(id));
                    }
                });
                if selected_membership.folders.is_empty() {
                    ui.small("No folder assignments.");
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(130.0)
                        .show(ui, |ui| {
                            for folder in &selected_membership.folders {
                                ui.horizontal(|ui| {
                                    let available = self.folder_assignment_available(folder);
                                    let label = if available {
                                        folder.display().to_string()
                                    } else {
                                        format!("⚠ {} (unavailable / not indexed)", folder.display())
                                    };
                                    ui.label(label).on_hover_text(folder.display().to_string());
                                    if ui
                                        .add_enabled(!self.busy, egui::Button::new("Remove").small())
                                        .clicked()
                                    {
                                        action = Some(CollectionAction::RemoveFolder(
                                            id,
                                            folder.clone(),
                                        ));
                                    }
                                });
                            }
                        });
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.strong("Manually added files");
                    if ui
                        .add_enabled(!self.busy, egui::Button::new("＋ Add files").small())
                        .clicked()
                    {
                        action = Some(CollectionAction::AddFilesDialog(id));
                    }
                });
                if selected_membership.files.is_empty() {
                    ui.small("No manually added files.");
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(150.0)
                        .show(ui, |ui| {
                            for file in &selected_membership.files {
                                ui.horizontal(|ui| {
                                    let available = self.image_positions.contains_key(file);
                                    let label = if available {
                                        file.display().to_string()
                                    } else {
                                        format!("⚠ {} (not currently indexed)", file.display())
                                    };
                                    ui.label(label).on_hover_text(file.display().to_string());
                                    if ui
                                        .add_enabled(!self.busy, egui::Button::new("Remove").small())
                                        .clicked()
                                    {
                                        action = Some(CollectionAction::RemoveFile(id, file.clone()));
                                    }
                                });
                            }
                        });
                }
            });
        });

        if let Some((id, name)) = select_collection {
            self.collections.selected_manage = Some(id);
            self.collections.rename_name = name;
        }
        if let Some(action) = action {
            self.apply_collection_action(action);
        }
    }

    fn apply_collection_action(&mut self, action: CollectionAction) {
        let result = (|| -> anyhow::Result<String> {
            let message = match action {
                CollectionAction::Create(name) => {
                    let created = db::create_collection(&self.db_path, &name)?;
                    self.collections.selected_manage = Some(created.id);
                    self.collections.new_name.clear();
                    format!("Created collection ‘{}’", created.name)
                }
                CollectionAction::Rename(id, name) => {
                    db::rename_collection(&self.db_path, id, &name)?;
                    format!("Renamed collection to ‘{name}’")
                }
                CollectionAction::SetFaceDetection(id, enabled) => {
                    face_scope::set_collection_enabled(&self.db_path, id, enabled)?;
                    if enabled {
                        "Face detection enabled for this collection".to_owned()
                    } else {
                        "Face detection disabled for this collection; existing face data was kept"
                            .to_owned()
                    }
                }
                CollectionAction::Delete(id) => {
                    db::delete_collection(&self.db_path, id)?;
                    format!("Deleted collection; source images were not changed")
                }
                CollectionAction::AddFolderDialog(id) => {
                    let Some(folder) = rfd::FileDialog::new().pick_folder() else {
                        return Ok(String::new());
                    };
                    let (added, skipped) = self.assign_paths_to_collection(id, vec![folder])?;
                    format_collection_assignment_status(added, skipped)
                }
                CollectionAction::AddFilesDialog(id) => {
                    let Some(files) = rfd::FileDialog::new()
                        .add_filter("Images", &["jpg", "jpeg", "png", "tif", "tiff"])
                        .pick_files()
                    else {
                        return Ok(String::new());
                    };
                    let (added, skipped) = self.assign_paths_to_collection(id, files)?;
                    format_collection_assignment_status(added, skipped)
                }
                CollectionAction::RemoveFolder(id, folder) => {
                    db::remove_collection_folder(&self.db_path, id, &folder)?;
                    "Removed folder assignment".to_owned()
                }
                CollectionAction::RemoveFile(id, file) => {
                    db::remove_collection_file(&self.db_path, id, &file)?;
                    "Removed manual file membership".to_owned()
                }
                CollectionAction::Drop(id, paths) => {
                    let (added, skipped) = self.assign_paths_to_collection(id, paths)?;
                    format_collection_assignment_status(added, skipped)
                }
            };
            self.collections.reload(&self.db_path, &self.images)?;
            Ok(message)
        })();

        match result {
            Ok(message) if !message.is_empty() => self.status = message,
            Ok(_) => {}
            Err(err) => self.last_error = Some(format!("Collection update failed: {err:#}")),
        }
    }

    fn assign_paths_to_collection(
        &self,
        collection_id: i64,
        paths: Vec<PathBuf>,
    ) -> anyhow::Result<(usize, usize)> {
        let mut folders = Vec::new();
        let mut files = Vec::new();
        let mut skipped = 0usize;
        let mut unique = HashSet::new();

        for path in paths {
            if !unique.insert(path.clone()) {
                continue;
            }
            if path.is_dir() {
                if self.folder_is_indexed(&path) {
                    folders.push(path);
                } else {
                    skipped += 1;
                }
            } else if is_supported_image(&path) && self.image_positions.contains_key(&path) {
                files.push(path);
            } else {
                skipped += 1;
            }
        }

        let added_folders = db::add_collection_folders(&self.db_path, collection_id, &folders)?;
        let added_files = db::add_collection_files(&self.db_path, collection_id, &files)?;
        Ok((added_folders + added_files, skipped))
    }

    fn folder_is_indexed(&self, folder: &Path) -> bool {
        self.roots.iter().any(|root| folder.starts_with(root))
    }

    fn folder_assignment_available(&self, folder: &Path) -> bool {
        folder.exists() && self.folder_is_indexed(folder)
    }
}

fn paint_drop_feedback(ui: &egui::Ui, response: &egui::Response) {
    let in_app = response
        .dnd_hover_payload::<CollectionDragPayload>()
        .is_some();
    let explorer =
        response.contains_pointer() && ui.input(|input| !input.raw.hovered_files.is_empty());
    if in_app || explorer {
        ui.painter().rect_stroke(
            response.rect,
            4.0,
            egui::Stroke::new(2.0_f32, ui.visuals().selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    }
}

fn released_drop_paths(ui: &egui::Ui, response: &egui::Response) -> Option<Vec<PathBuf>> {
    if let Some(payload) = response.dnd_release_payload::<CollectionDragPayload>() {
        return Some(payload.paths.clone());
    }
    if !response.contains_pointer() {
        return None;
    }
    let dropped = ui.input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .filter_map(|file| file.path.clone())
            .collect::<Vec<_>>()
    });
    (!dropped.is_empty()).then_some(dropped)
}

fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "tif" | "tiff"
            )
        })
        .unwrap_or(false)
}

fn format_collection_assignment_status(added: usize, skipped: usize) -> String {
    if skipped == 0 {
        format!(
            "Added {added} collection assignment{}",
            if added == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "Added {added} collection assignment{}; skipped {skipped} path{} because they are not indexed images/folders",
            if added == 1 { "" } else { "s" },
            if skipped == 1 { "" } else { "s" }
        )
    }
}
