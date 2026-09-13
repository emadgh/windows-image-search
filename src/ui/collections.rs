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
    filter_revision: u64,
    new_name: String,
    rename_name: String,
    statistics: super::collection_statistics::StatisticsState,
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
        self.filter_revision = self.filter_revision.wrapping_add(1);
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

    pub(super) fn collection_filter_cache_token(&self) -> (Option<i64>, u64) {
        (
            self.collections.active_filter,
            self.collections.filter_revision,
        )
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

    pub(super) fn collection_index_status(&mut self, ui: &mut egui::Ui) {
        // Index controls apply to the whole library, independently of selection.
        ui.horizontal_wrapped(|ui| {
            ui.strong("Library indexing");
            ui.label(if self.index_paused {
                "Paused"
            } else if self.indexing {
                "Running"
            } else {
                "Idle"
            });
            if self.indexing
                && self.index_control.is_some()
                && ui
                    .button(if self.index_paused { "Resume" } else { "Pause" })
                    .clicked()
            {
                self.toggle_index_pause();
            }
            if ui
                .add_enabled(
                    !self.busy && !self.roots.is_empty(),
                    egui::Button::new("Rescan changed"),
                )
                .clicked()
            {
                self.start_rescan();
            }
            ui.menu_button("More", |ui| {
                if ui
                    .add_enabled(
                        !self.busy && !self.roots.is_empty(),
                        egui::Button::new("Rebuild all descriptors"),
                    )
                    .on_hover_text("Force a rescan of every folder in the library.")
                    .clicked()
                {
                    self.start_force_rescan();
                    ui.close();
                }
            });
        });
        if self.indexing || self.searching {
            if let Some((done, total)) = self.progress.filter(|(_, total)| *total > 0) {
                ui.add(
                    egui::ProgressBar::new(done as f32 / total as f32)
                        .desired_width(ui.available_width())
                        .text(format!("{done} / {total}")),
                );
            }
            ui.add(egui::Label::new(&self.status).truncate())
                .on_hover_text(format!(
                    "{}\n{}",
                    self.status,
                    self.current_file.as_deref().unwrap_or("")
                ));
        }
        if self.busy {
            ui.small("Editing is unavailable while background work is active.");
        }
    }

    pub(super) fn show_collections_settings(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
        let height = (ui.available_height() - 18.0).max(120.0);
        let width = ui.available_width().max(880.0);
        egui::ScrollArea::horizontal()
            .id_salt("collections-three-columns")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(190.0, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(190.0);
                            self.collection_navigation(ui, height, &mut action);
                        },
                    );
                    ui.separator();
                    ui.vertical(|ui| self.collection_details(ui, &mut action));
                });
            });
        if let Some(action) = action {
            self.apply_collection_action(action);
        }
    }

    fn collection_create_row(&mut self, ui: &mut egui::Ui, action: &mut Option<CollectionAction>) {
        ui.horizontal(|ui| {
            ui.add_enabled(
                !self.busy,
                egui::TextEdit::singleline(&mut self.collections.new_name)
                    .hint_text("New collection")
                    .desired_width((ui.available_width() - 48.0).max(40.0)),
            );
            if ui
                .add_enabled(
                    !self.busy && !self.collections.new_name.trim().is_empty(),
                    egui::Button::new("Add"),
                )
                .clicked()
            {
                *action = Some(CollectionAction::Create(
                    self.collections.new_name.trim().to_owned(),
                ));
            }
        });
    }

    fn collection_navigation(
        &mut self,
        ui: &mut egui::Ui,
        height: f32,
        action: &mut Option<CollectionAction>,
    ) {
        ui.strong(format!("Collections ({})", self.collections.items.len()));
        self.collection_create_row(ui, action);
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("collections-manage-list")
            .max_height((height - 64.0).max(60.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.collections.items.is_empty() {
                    ui.weak("Create a collection, then add a folder.");
                }
                for item in self.collections.items.clone() {
                    let text = format!(
                        "{}\n{} / {} indexed",
                        item.name,
                        self.collections.count(item.id),
                        self.collections.total_count(item.id)
                    );
                    let response = ui.add_sized(
                        [ui.available_width(), 50.0],
                        collection_selection_button(
                            self.collections.selected_manage == Some(item.id),
                            text,
                        ),
                    );
                    if response.clicked() {
                        self.collections.selected_manage = Some(item.id);
                        self.collections.rename_name = item.name;
                        self.collections.statistics.folder = None;
                        self.collections.statistics.reset();
                    }
                    if !self.busy {
                        paint_drop_feedback(ui, &response);
                        if let Some(paths) = released_drop_paths(ui, &response) {
                            *action = Some(CollectionAction::Drop(item.id, paths));
                        }
                    }
                }
            });
    }

    fn collection_details(&mut self, ui: &mut egui::Ui, action: &mut Option<CollectionAction>) {
        let Some(id) = self.collections.selected_manage else {
            ui.heading("Build your library");
            ui.label("Create a collection to group folders and indexed images.");
            return;
        };
        let name = self
            .collections
            .items
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.name.clone())
            .unwrap_or_default();
        let membership = self
            .collections
            .memberships
            .get(&id)
            .cloned()
            .unwrap_or_default();
        ui.add(egui::Label::new(egui::RichText::new(&name).heading()).wrap());
        ui.weak(format!(
            "{} / {} indexed  ·  {} folders  ·  {} individual files",
            self.collections.count(id),
            self.collections.total_count(id),
            membership.folders.len(),
            membership.files.len()
        ));
        ui.add_space(10.0);
        ui.columns(2, |columns| {
            columns[0].strong("Contents");
            columns[0].separator();
            egui::ScrollArea::vertical().id_salt("collection-content-column")
                .auto_shrink([false, false]).show(&mut columns[0], |ui| {
                    if ui.selectable_label(self.collections.statistics.folder.is_none(), "All collection images").clicked() {
                        self.collections.statistics.folder = None;
                        self.collections.statistics.reset();
                    }

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.busy, egui::Button::new("Add folder"))
                .clicked()
            {
                *action = Some(CollectionAction::AddFolderDialog(id));
            }
            if ui
                .add_enabled(!self.busy, egui::Button::new("Add indexed files"))
                .on_hover_text("New images must be added by folder.")
                .clicked()
            {
                *action = Some(CollectionAction::AddFilesDialog(id));
            }
        });
        let drop_response = egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.weak("Drop folders or indexed images to add them to this collection");
        }).response;
        if !self.busy {
            paint_drop_feedback(ui, &drop_response);
            if let Some(paths) = released_drop_paths(ui, &drop_response) {
                *action = Some(CollectionAction::Drop(id, paths));
            }
        }
        ui.add_space(10.0);
        ui.strong(format!("Folders ({})", membership.folders.len()));
        if membership.folders.is_empty() {
            ui.weak("Add a folder to start indexing this collection.");
        }
        for folder in &membership.folders {
            self.collection_path_row(ui, id, folder, true, action);
        }
        ui.add_space(8.0);
        egui::CollapsingHeader::new(format!(
            "Individual indexed files ({})",
            membership.files.len()
        ))
        .id_salt(("collection-files", id))
        .show(ui, |ui| {
            if membership.files.is_empty() {
                ui.weak("No individually assigned images.");
            }
            for file in &membership.files {
                self.collection_path_row(ui, id, file, false, action);
            }
        });
        ui.add_space(10.0);


                });
            columns[1].strong("Settings");
            columns[1].separator();
            egui::ScrollArea::vertical().id_salt("collection-settings-column")
                .auto_shrink([false, false]).show(&mut columns[1], |ui| {
                ui.label("Name");
                ui.horizontal(|ui| {
                    ui.add_enabled(
                        !self.busy,
                        egui::TextEdit::singleline(&mut self.collections.rename_name)
                            .desired_width((ui.available_width() - 76.0).max(40.0)),
                    );
                    if ui
                        .add_enabled(
                            !self.busy
                                && !self.collections.rename_name.trim().is_empty()
                                && self.collections.rename_name.trim() != name,
                            egui::Button::new("Rename"),
                        )
                        .clicked()
                    {
                        *action = Some(CollectionAction::Rename(
                            id,
                            self.collections.rename_name.trim().to_owned(),
                        ));
                    }
                });

        ui.add_space(16.0);
        ui.strong("Face indexing");
        let mut detect_faces = self
            .collections
            .face_detection
            .get(&id)
            .copied()
            .unwrap_or(false);
        if ui.add_enabled(!self.busy, egui::Checkbox::new(&mut detect_faces, "Detect faces"))
            .on_hover_text("Enable face indexing for images in this collection. Existing face data is kept when disabled.").changed() {
            *action = Some(CollectionAction::SetFaceDetection(id, detect_faces));
        }

        ui.separator();
                ui.add_space(8.0);
                ui.small("Removing a collection keeps source images and portable indexes on disk.");
                ui.menu_button("Delete collection…", |ui| {
                    ui.label(format!("Remove ‘{name}’ and its memberships?"));
                    if ui
                        .add_enabled(!self.busy, egui::Button::new("Delete collection"))
                        .clicked()
                    {
                        *action = Some(CollectionAction::Delete(id));
                        ui.close();
                    }
                });


                    ui.add_space(16.0);
                    self.collection_statistics_panel(ui, id, &membership);
                });
        });
    }

    fn collection_statistics_panel(
        &mut self,
        ui: &mut egui::Ui,
        id: i64,
        membership: &CollectionMembership,
    ) {
        ui.separator();
        ui.heading("Statistics");
        if self
            .collections
            .statistics
            .folder
            .as_ref()
            .is_some_and(|f| !membership.folders.contains(f))
        {
            self.collections.statistics.folder = None;
            self.collections.statistics.reset();
        }

        let label = self
            .collections
            .statistics
            .folder
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Entire collection".to_owned());
        ui.add(egui::Label::new(label).wrap());
        let mut refresh = false;
        ui.horizontal(|ui| {
            refresh = ui
                .add_enabled(
                    !self.collections.statistics.loading(),
                    egui::Button::new("Refresh statistics"),
                )
                .clicked();
            if self.collections.statistics.loading() {
                ui.spinner();
            }
        });
        // Refresh while indexing, but never run a query or cache scan per UI frame.
        refresh |= !self.collections.statistics.loading()
            && self.indexing
            && self
                .collections
                .statistics
                .updated
                .is_some_and(|t| t.elapsed().as_secs() >= 15);
        self.collections
            .statistics
            .poll(id, &self.db_path, membership, ui.ctx(), refresh);
        if self.indexing {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
        }
        let Some(result) = &self.collections.statistics.result else {
            ui.weak("Reading saved indexes and thumbnail files…");
            return;
        };
        let stats = match result {
            Ok(stats) => stats,
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
                return;
            }
        };
        if let Some(updated) = self.collections.statistics.updated {
            ui.small(format!("Snapshot {}s ago", updated.elapsed().as_secs()));
        }
        let mut values = vec![
            ("Images discovered", stats.discovered.to_string()),
            ("Images indexed", stats.indexed.to_string()),
            (
                "Not yet indexed",
                stats.discovered.saturating_sub(stats.indexed).to_string(),
            ),
            ("Recorded decode failures", stats.errors.to_string()),
            (
                "CLIP embeddings stored",
                format!("{} / {}", stats.clip, stats.indexed),
            ),
            (
                "CLIP embeddings missing",
                stats.indexed.saturating_sub(stats.clip).to_string(),
            ),
            (
                "Current thumbnail files",
                format!("{} / {}", stats.thumbnails, stats.indexed),
            ),
            (
                "No current thumbnail / unavailable",
                stats.indexed.saturating_sub(stats.thumbnails).to_string(),
            ),
            ("Source files unavailable", stats.unavailable.to_string()),
            (
                "Thumbnail storage",
                format!("{:.2} MiB", stats.thumbnail_bytes as f64 / 1048576.0),
            ),
            (
                "Indexed source size",
                format!("{:.2} GiB", stats.source_bytes as f64 / 1073741824.0),
            ),
            (
                "Visual hashes",
                format!("{} / {}", stats.hashes, stats.indexed),
            ),
            (
                "Color descriptors",
                format!("{} / {}", stats.colors, stats.indexed),
            ),
            (
                "Current material descriptors",
                format!("{} / {}", stats.material, stats.indexed),
            ),
        ];
        let enabled = self
            .collections
            .face_detection
            .get(&id)
            .copied()
            .unwrap_or(false);
        values.push((
            "Face indexing",
            if enabled {
                "Enabled"
            } else {
                "Disabled (saved data kept)"
            }
            .to_owned(),
        ));
        if enabled || stats.faces > 0 {
            let face_value = |value: usize| {
                if stats.warnings.is_empty() {
                    value.to_string()
                } else {
                    format!("{value} (partial)")
                }
            };
            values.extend([
                ("Images with detection saved", face_value(stats.face_images)),
                (
                    "Images without detection records",
                    face_value(stats.indexed.saturating_sub(stats.face_images)),
                ),
                ("Images with no faces", face_value(stats.no_faces)),
                ("Faces detected", face_value(stats.faces)),
                ("Face embeddings stored", face_value(stats.face_vectors)),
                (
                    "Face embeddings missing",
                    face_value(stats.faces.saturating_sub(stats.face_vectors)),
                ),
            ]);
        }
        egui::Grid::new("collection-statistics-values")
            .striped(true)
            .num_columns(2)
            .max_col_width(190.0)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for (label, value) in values {
                    ui.label(label);
                    ui.strong(value);
                    ui.end_row();
                }
            });
        ui.add_space(6.0);
        ui.small("Thumbnails count existing non-empty cache files matching current source metadata; cache contents are not decoded. Face counts describe saved records, not model freshness. Each database is a separate snapshot.");
        for warning in &stats.warnings {
            ui.colored_label(ui.visuals().warn_fg_color, warning);
        }
        egui::CollapsingHeader::new("Image formats").show(ui, |ui| {
            for (format, count) in &stats.formats {
                ui.label(format!("{format}: {count}"));
            }
        });
        if !stats.error_reasons.is_empty() {
            egui::CollapsingHeader::new("Decode failure details").show(ui, |ui| {
                for (reason, count) in &stats.error_reasons {
                    ui.label(format!("{count}: {reason}"));
                }
            });
        }
    }

    fn collection_path_row(
        &mut self,
        ui: &mut egui::Ui,
        id: i64,
        path: &Path,
        folder: bool,
        action: &mut Option<CollectionAction>,
    ) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Remove").small())
                    .on_hover_text("Remove membership; keep the source on disk.")
                    .clicked()
                {
                    *action = Some(if folder {
                        CollectionAction::RemoveFolder(id, path.to_owned())
                    } else {
                        CollectionAction::RemoveFile(id, path.to_owned())
                    });
                }
                let available = if folder {
                    self.folder_assignment_available(path)
                } else {
                    self.image_positions.contains_key(path)
                };
                let text = format!("{}{}", if available { "" } else { "⚠ " }, path.display());
                let response = ui
                    .add_sized(
                        [ui.available_width(), ui.spacing().interact_size.y],
                        egui::Button::selectable(
                            folder && self.collections.statistics.folder.as_deref() == Some(path),
                            (text, egui::Atom::grow()),
                        )
                        .truncate(),
                    )
                    .on_hover_text(format!(
                        "{}{}",
                        path.display(),
                        if available {
                            ""
                        } else {
                            "\nUnavailable or not currently indexed"
                        }
                    ));
                if folder && response.clicked() {
                    self.collections.statistics.folder = Some(path.to_owned());
                    self.collections.statistics.reset();
                }
            });
        });
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
            self.collections.statistics.reset();
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

fn collection_selection_button(selected: bool, text: String) -> egui::Button<'static> {
    egui::Button::selectable(selected, (text, egui::Atom::grow()))
}

#[cfg(test)]
mod collection_layout_tests {
    use super::*;
    #[test]
    fn sidebar_text_stays_at_left_of_full_width_button() {
        let ctx = egui::Context::default();
        let mut rect = egui::Rect::NOTHING;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 200.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    rect = ui
                        .add_sized(
                            [400.0, 50.0],
                            collection_selection_button(true, "people\n118 indexed".into()),
                        )
                        .rect;
                });
            },
        );
        let positions: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text().contains("people") => {
                    Some(text.pos.x)
                }
                _ => None,
            })
            .collect();
        assert!(!positions.is_empty(), "button text must be painted");
        assert!(
            positions
                .iter()
                .all(|x| *x >= rect.left() && *x < rect.left() + 20.0),
            "text is not left aligned: {positions:?}, {rect:?}"
        );
    }
}
