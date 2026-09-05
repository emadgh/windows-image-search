use super::ImageSearchApp;
use crate::db::{self, Collection, CollectionMembership, ImageSummary};
use crate::face_scope;
use crate::portable;
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
        migrate_legacy_roots_into_collections(db_path)?;
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

    pub(super) fn collection_filter_chip(&self) -> Option<String> {
        let id = self.collections.active_filter?;
        self.collections
            .items
            .iter()
            .find(|item| item.id == id)
            .map(|item| format!("Collection: {}", item.name))
    }

    pub(super) fn clear_collection_filter(&mut self) {
        self.collections.active_filter = None;
    }

    pub(super) fn collection_count(&self) -> usize {
        self.collections.items.len()
    }

    pub(super) fn show_add_to_collection_menu(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        paths: &[PathBuf],
    ) {
        let items = self
            .collections
            .items
            .iter()
            .map(|item| (item.id, item.name.clone(), self.collections.count(item.id)))
            .collect::<Vec<_>>();
        let mut target = None;
        ui.add_enabled_ui(!self.busy && !items.is_empty() && !paths.is_empty(), |ui| {
            ui.menu_button(label, |ui| {
                for (id, name, count) in &items {
                    if ui.button(format!("{name} ({count})")).clicked() {
                        target = Some(*id);
                        ui.close();
                    }
                }
            });
        });
        if let Some(id) = target {
            self.apply_collection_action(CollectionAction::Drop(id, paths.to_vec()));
        }
    }

    pub(super) fn prompt_add_library_folder(&mut self) {
        if self.busy {
            return;
        }
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };

        let collection_id = if let Some(id) = self.collections.selected_manage {
            id
        } else if let Some(item) = self.collections.items.first() {
            item.id
        } else {
            match db::create_collection(&self.db_path, "Library") {
                Ok(created) => {
                    self.collections.selected_manage = Some(created.id);
                    created.id
                }
                Err(err) => {
                    self.last_error = Some(format!(
                        "Cannot create the default Library collection: {err:#}"
                    ));
                    return;
                }
            }
        };

        self.apply_collection_action(CollectionAction::Drop(collection_id, vec![folder]));
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
            "Collections are now the library/indexing entry point. Adding a folder attaches its portable .imagesearch index automatically; removing a collection never deletes source files or the on-disk portable index.",
        );

        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.busy && !self.roots.is_empty(),
                    egui::Button::new("Rescan changed"),
                )
                .clicked()
            {
                self.start_rescan();
            }
            if ui
                .add_enabled(
                    !self.busy && !self.roots.is_empty(),
                    egui::Button::new("Force rescan all"),
                )
                .on_hover_text("Rebuild all descriptors for folders referenced by Collections.")
                .clicked()
            {
                self.start_force_rescan();
            }
            if self.indexing && self.index_control.is_some() {
                let label = if self.index_paused { "Resume" } else { "Pause" };
                if ui
                    .add_enabled(!self.searching, egui::Button::new(label))
                    .clicked()
                {
                    self.toggle_index_pause();
                }
            }
        });
        if self.indexing || self.searching || self.progress.is_some() {
            ui.add_space(5.0);
            ui.group(|ui| {
                if let Some((done, total)) = self.progress.filter(|(_, total)| *total > 0) {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total as f32)
                            .desired_width(ui.available_width().min(520.0))
                            .text(format!("{done}/{total}")),
                    );
                }
                if let Some(file_name) = &self.current_file {
                    ui.small(format!("Current: {file_name}"));
                }
                ui.small(super::views::truncate_middle(&self.status, 92))
                    .on_hover_text(&self.status);
            });
        }
        if self.busy && !self.indexing {
            ui.small("Collection editing is locked while search/background work is active.");
        }

        let items = self.collections.items.clone();
        let selected_id = self.collections.selected_manage;
        let selected_membership = selected_id
            .and_then(|id| self.collections.memberships.get(&id).cloned())
            .unwrap_or_default();
        let mut action: Option<CollectionAction> = None;
        let mut select_collection: Option<(i64, String)> = None;

        ui.add_space(10.0);
        ui.strong("Collections");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.collections.new_name)
                    .hint_text("New collection")
                    .desired_width((ui.available_width() - 72.0).clamp(140.0, 320.0)),
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

        egui::ScrollArea::vertical()
            .id_salt("collections-manage-list")
            .max_height(150.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if items.is_empty() {
                    ui.label("No collections yet. Create one, then add a folder.");
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

        ui.separator();
        let Some(id) = selected_id else {
            ui.label("Select or create a Collection to manage its indexed folders and files.");
            if let Some((id, name)) = select_collection {
                self.collections.selected_manage = Some(id);
                self.collections.rename_name = name;
            }
            if let Some(action) = action {
                self.apply_collection_action(action);
            }
            return;
        };

        let selected_name = items
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.name.clone())
            .unwrap_or_else(|| "Collection".to_owned());
        ui.heading(selected_name);
        ui.small(format!(
            "{} indexed / {} discovered image{}",
            self.collections.count(id),
            self.collections.total_count(id),
            if self.collections.total_count(id) == 1 {
                ""
            } else {
                "s"
            }
        ));

        ui.horizontal_wrapped(|ui| {
            ui.add_enabled(
                !self.busy,
                egui::TextEdit::singleline(&mut self.collections.rename_name)
                    .desired_width((ui.available_width() * 0.55).clamp(160.0, 340.0)),
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
        ui.small("Deleting a Collection removes only its membership and unused session root registrations. Source images and .imagesearch folders stay untouched.");

        let mut detect_faces = self
            .collections
            .face_detection
            .get(&id)
            .copied()
            .unwrap_or(false);
        if ui
            .add_enabled(
                !self.busy,
                egui::Checkbox::new(&mut detect_faces, "Detect faces in this collection"),
            )
            .on_hover_text(
                "Only effective members of face-enabled Collections are sent to the face detector.",
            )
            .changed()
        {
            action = Some(CollectionAction::SetFaceDetection(id, detect_faces));
        }

        ui.add_space(8.0);
        let drop_response = ui.add_sized(
            [ui.available_width(), 46.0],
            egui::Label::new(
                "Drop folders here to attach/index them, or drag already-indexed Grid/Details images here",
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
            ui.strong("Indexed folders");
            if ui
                .add_enabled(!self.busy, egui::Button::new("Add folder").small())
                .clicked()
            {
                action = Some(CollectionAction::AddFolderDialog(id));
            }
        });
        if selected_membership.folders.is_empty() {
            ui.small(
                "No folders yet. Add a folder to make it part of this Collection and index it.",
            );
        } else {
            egui::ScrollArea::vertical()
                .max_height(150.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for folder in &selected_membership.folders {
                        ui.horizontal(|ui| {
                            let available = self.folder_assignment_available(folder);
                            let label = if available {
                                folder.display().to_string()
                            } else {
                                format!("⚠ {} (unavailable)", folder.display())
                            };
                            ui.label(super::views::truncate_middle(&label, 76))
                                .on_hover_text(folder.display().to_string());
                            if ui
                                .add_enabled(!self.busy, egui::Button::new("Remove").small())
                                .clicked()
                            {
                                action = Some(CollectionAction::RemoveFolder(id, folder.clone()));
                            }
                        });
                    }
                });
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.strong("Manually added indexed files");
            if ui
                .add_enabled(!self.busy, egui::Button::new("Add files").small())
                .clicked()
            {
                action = Some(CollectionAction::AddFilesDialog(id));
            }
        });
        if selected_membership.files.is_empty() {
            ui.small(
                "No individual file assignments. New/unindexed content should be added by folder.",
            );
        } else {
            egui::ScrollArea::vertical()
                .max_height(150.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for file in &selected_membership.files {
                        ui.horizontal(|ui| {
                            let available = self.image_positions.contains_key(file);
                            let label = if available {
                                file.display().to_string()
                            } else {
                                format!("⚠ {} (not currently indexed)", file.display())
                            };
                            ui.label(super::views::truncate_middle(&label, 76))
                                .on_hover_text(file.display().to_string());
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

        if let Some((id, name)) = select_collection {
            self.collections.selected_manage = Some(id);
            self.collections.rename_name = name;
        }
        if let Some(action) = action {
            self.apply_collection_action(action);
        }
    }

    fn apply_collection_action(&mut self, action: CollectionAction) {
        let mut rescan_after = false;
        let mut sync_roots_after = false;
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
                    sync_roots_after = true;
                    "Deleted collection; source files and portable indexes were not changed"
                        .to_owned()
                }
                CollectionAction::AddFolderDialog(id) => {
                    let Some(folder) = rfd::FileDialog::new().pick_folder() else {
                        return Ok(String::new());
                    };
                    let (added, skipped, attached) =
                        self.assign_paths_to_collection(id, vec![folder])?;
                    rescan_after |= attached;
                    format_collection_assignment_status(added, skipped)
                }
                CollectionAction::AddFilesDialog(id) => {
                    let Some(files) = rfd::FileDialog::new()
                        .add_filter("Images", &["jpg", "jpeg", "png", "tif", "tiff"])
                        .pick_files()
                    else {
                        return Ok(String::new());
                    };
                    let (added, skipped, attached) = self.assign_paths_to_collection(id, files)?;
                    rescan_after |= attached;
                    format_collection_assignment_status(added, skipped)
                }
                CollectionAction::RemoveFolder(id, folder) => {
                    db::remove_collection_folder(&self.db_path, id, &folder)?;
                    sync_roots_after = true;
                    "Removed folder from Collection".to_owned()
                }
                CollectionAction::RemoveFile(id, file) => {
                    db::remove_collection_file(&self.db_path, id, &file)?;
                    sync_roots_after = true;
                    "Removed manual file membership".to_owned()
                }
                CollectionAction::Drop(id, paths) => {
                    let (added, skipped, attached) = self.assign_paths_to_collection(id, paths)?;
                    rescan_after |= attached;
                    format_collection_assignment_status(added, skipped)
                }
            };
            self.collections.reload(&self.db_path, &self.images)?;
            if sync_roots_after {
                self.sync_roots_to_collection_memberships()?;
                self.collections.reload(&self.db_path, &self.images)?;
            }
            Ok(message)
        })();

        match result {
            Ok(message) if !message.is_empty() => self.status = message,
            Ok(_) => {}
            Err(err) => self.last_error = Some(format!("Collection update failed: {err:#}")),
        }
        if rescan_after && !self.busy && !self.roots.is_empty() {
            self.start_rescan();
        }
    }

    fn assign_paths_to_collection(
        &mut self,
        collection_id: i64,
        paths: Vec<PathBuf>,
    ) -> anyhow::Result<(usize, usize, bool)> {
        let mut folders = Vec::new();
        let mut files = Vec::new();
        let mut skipped = 0usize;
        let mut attached_new_root = false;
        let mut unique = HashSet::new();

        for path in paths {
            if !unique.insert(path.clone()) {
                continue;
            }
            if path.is_dir() {
                if !self.folder_is_indexed(&path) {
                    self.attach_collection_root(&path)?;
                    attached_new_root = true;
                }
                folders.push(path);
            } else if is_supported_image(&path) && self.image_positions.contains_key(&path) {
                files.push(path);
            } else {
                skipped += 1;
            }
        }

        let added_folders = db::add_collection_folders(&self.db_path, collection_id, &folders)?;
        let added_files = db::add_collection_files(&self.db_path, collection_id, &files)?;
        Ok((added_folders + added_files, skipped, attached_new_root))
    }

    fn attach_collection_root(&mut self, folder: &Path) -> anyhow::Result<()> {
        if self.folder_is_indexed(folder) {
            return Ok(());
        }
        portable::attach_root(&self.db_path, folder)?;
        self.reload_after_root_registry_change();
        Ok(())
    }

    fn sync_roots_to_collection_memberships(&mut self) -> anyhow::Result<()> {
        let collections = db::load_collections(&self.db_path)?;
        let mut folders = Vec::new();
        let mut files = Vec::new();
        for collection in collections {
            let membership = db::load_collection_membership(&self.db_path, collection.id)?;
            folders.extend(membership.folders);
            files.extend(membership.files);
        }

        let mut removed_any = false;
        for root in self.roots.clone() {
            let referenced = folders
                .iter()
                .any(|folder| folder.starts_with(&root) || root.starts_with(folder))
                || files.iter().any(|file| file.starts_with(&root));
            if !referenced {
                db::remove_root(&self.db_path, &root)?;
                removed_any = true;
            }
        }
        if removed_any {
            self.reload_after_root_registry_change();
        }
        Ok(())
    }

    fn reload_after_root_registry_change(&mut self) {
        self.roots = db::load_roots(&self.db_path).unwrap_or_default();
        self.root_counts = db::load_root_counts(&self.db_path).unwrap_or_default();
        self.thumb_pool.set_roots(self.roots.clone());
        self.fs_watch_service.set_roots(self.roots.clone());
        self.images = db::load_image_summaries(&self.db_path).unwrap_or_default();
        self.rebuild_image_positions();
        self.refresh_collection_effective_membership();
        self.refresh_text_search_after_data_change();
        self.similarity_results = None;
        self.selected_paths.clear();
    }

    fn folder_is_indexed(&self, folder: &Path) -> bool {
        self.roots.iter().any(|root| folder.starts_with(root))
    }

    fn folder_assignment_available(&self, folder: &Path) -> bool {
        folder.exists() && self.folder_is_indexed(folder)
    }
}

fn migrate_legacy_roots_into_collections(db_path: &Path) -> anyhow::Result<()> {
    let roots = db::load_roots(db_path)?;
    if roots.is_empty() {
        return Ok(());
    }
    let collections = db::load_collections(db_path)?;
    let mut assigned_folders = Vec::new();
    for collection in &collections {
        assigned_folders.extend(db::load_collection_membership(db_path, collection.id)?.folders);
    }
    let uncovered = roots
        .into_iter()
        .filter(|root| {
            !assigned_folders
                .iter()
                .any(|folder| root.starts_with(folder))
        })
        .collect::<Vec<_>>();
    if uncovered.is_empty() {
        return Ok(());
    }

    let imported = collections
        .iter()
        .find(|collection| collection.name == "Imported Library")
        .cloned()
        .unwrap_or(db::create_collection(db_path, "Imported Library")?);
    db::add_collection_folders(db_path, imported.id, &uncovered)?;
    Ok(())
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
            "Added {added} collection assignment{}; skipped {skipped} path{} because individual files must already be indexed; add new content by folder",
            if added == 1 { "" } else { "s" },
            if skipped == 1 { "" } else { "s" }
        )
    }
}
