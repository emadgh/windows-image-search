from pathlib import Path
import re


def replace_once(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected 1 match, got {count}")
    return text.replace(old, new, 1)


def replace_between(text, start, end, new, label):
    i = text.find(start)
    if i < 0:
        raise SystemExit(f"{label}: start missing")
    j = text.find(end, i)
    if j < 0:
        raise SystemExit(f"{label}: end missing")
    return text[:i] + new + text[j:]


# Parent UI: delegate old Settings window to grouped Preferences.
mod_path = Path("src/ui/mod.rs")
mod = mod_path.read_text(encoding="utf-8")
if "mod settings_window;" not in mod:
    mod = replace_once(mod, "mod people_manager;\n", "mod people_manager;\nmod settings_window;\n", "settings module")
mod = replace_between(
    mod,
    "    fn show_settings_window(&mut self, ctx: &egui::Context) {",
    "    fn show_search_sidebar(&mut self, ctx: &egui::Context) {",
    "    fn show_settings_window(&mut self, ctx: &egui::Context) {\n        settings_window::show(self, ctx);\n    }\n\n",
    "settings delegate",
)
mod_path.write_text(mod, encoding="utf-8")


# Grouped Preferences copied from the validated #144 branch by the workflow.
settings_path = Path("src/ui/settings_window.rs")
settings = settings_path.read_text(encoding="utf-8")
settings = settings.replace("    LibraryIndexing,\n", "")
settings = settings.replace("    const ALL: [Self; 6] = [\n        Self::LibraryIndexing,\n", "    const ALL: [Self; 5] = [\n")
settings = settings.replace('            Self::LibraryIndexing => "Library / Indexing",\n', "")
settings = settings.replace(
    "            Self::LibraryIndexing => 0,\n            Self::Collections => 1,\n            Self::SearchClip => 2,\n            Self::FacesPeople => 3,\n            Self::Performance => 4,\n            Self::Storage => 5,\n",
    "            Self::Collections => 0,\n            Self::SearchClip => 1,\n            Self::FacesPeople => 2,\n            Self::Performance => 3,\n            Self::Storage => 4,\n",
)
settings = settings.replace(
    "            1 => Self::Collections,\n            2 => Self::SearchClip,\n            3 => Self::FacesPeople,\n            4 => Self::Performance,\n            5 => Self::Storage,\n            _ => Self::LibraryIndexing,\n",
    "            1 => Self::SearchClip,\n            2 => Self::FacesPeople,\n            3 => Self::Performance,\n            4 => Self::Storage,\n            _ => Self::Collections,\n",
)
settings = re.sub(
    r"\s*SettingsCategory::LibraryIndexing => \{\s*settings_library_indexing\(app, ui, &mut effects\)\s*\}\s*",
    "\n",
    settings,
    count=1,
)
settings = settings.replace('.default_size([920.0, 640.0])\n        .min_width(780.0)\n        .min_height(500.0)', '.default_size([860.0, 620.0])\n        .min_width(580.0)\n        .min_height(460.0)')
settings = settings.replace("            let sidebar_width = 190.0;", "            let sidebar_width = 168.0;")
settings = settings.replace(".max(420.0);", ".max(300.0);")
settings = settings.replace(
    '        "Collections",\n        "Create virtual groups without moving or deleting source files.",',
    '        "Collections / Indexing",\n        "Collections are the library. Add folders here to attach/index them; source files and portable .imagesearch data are never deleted by collection edits.",',
)
settings_path.write_text(settings, encoding="utf-8")


# Collections become the sole user-facing library/indexing surface.
collections_path = Path("src/ui/collections.rs")
collections = collections_path.read_text(encoding="utf-8")
collections = replace_once(
    collections,
    "use crate::face_scope;\n",
    "use crate::face_scope;\nuse crate::portable;\n",
    "portable import",
)
collections = replace_once(
    collections,
    "    pub(super) fn load(db_path: &Path, images: &[ImageSummary]) -> anyhow::Result<Self> {\n        let mut state = Self::default();",
    "    pub(super) fn load(db_path: &Path, images: &[ImageSummary]) -> anyhow::Result<Self> {\n        migrate_legacy_roots_into_collections(db_path)?;\n        let mut state = Self::default();",
    "legacy migration",
)

show_start = "    pub(super) fn show_collections_settings(&mut self, ui: &mut egui::Ui) {"
show_end = "    fn apply_collection_action(&mut self, action: CollectionAction) {"
new_show = r'''    pub(super) fn show_collections_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(
            "Collections are now the library/indexing entry point. Adding a folder attaches its portable .imagesearch index automatically; removing a collection never deletes source files or the on-disk portable index.",
        );

        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.busy && !self.roots.is_empty(),
                    egui::Button::new("⟳ Rescan changed"),
                )
                .clicked()
            {
                self.start_rescan();
            }
            if ui
                .add_enabled(
                    !self.busy && !self.roots.is_empty(),
                    egui::Button::new("⟳ Force rescan all"),
                )
                .on_hover_text("Rebuild all descriptors for folders referenced by Collections.")
                .clicked()
            {
                self.start_force_rescan();
            }
            if self.indexing && self.index_control.is_some() {
                let label = if self.index_paused { "▶ Resume" } else { "⏸ Pause" };
                if ui.add_enabled(!self.searching, egui::Button::new(label)).clicked() {
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
            if self.collections.total_count(id) == 1 { "" } else { "s" }
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
            .on_hover_text("Only effective members of face-enabled Collections are sent to the face detector.")
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
                .add_enabled(!self.busy, egui::Button::new("＋ Add folder").small())
                .clicked()
            {
                action = Some(CollectionAction::AddFolderDialog(id));
            }
        });
        if selected_membership.folders.is_empty() {
            ui.small("No folders yet. Add a folder to make it part of this Collection and index it.");
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
                .add_enabled(!self.busy, egui::Button::new("＋ Add files").small())
                .clicked()
            {
                action = Some(CollectionAction::AddFilesDialog(id));
            }
        });
        if selected_membership.files.is_empty() {
            ui.small("No individual file assignments. New/unindexed content should be added by folder.");
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

'''
collections = replace_between(collections, show_start, show_end, new_show, "collections UI")

apply_start = "    fn apply_collection_action(&mut self, action: CollectionAction) {"
apply_end = "    fn folder_is_indexed(&self, folder: &Path) -> bool {"
new_apply = r'''    fn apply_collection_action(&mut self, action: CollectionAction) {
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
                    "Deleted collection; source files and portable indexes were not changed".to_owned()
                }
                CollectionAction::AddFolderDialog(id) => {
                    let Some(folder) = rfd::FileDialog::new().pick_folder() else {
                        return Ok(String::new());
                    };
                    let (added, skipped, attached) = self.assign_paths_to_collection(id, vec![folder])?;
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

'''
collections = replace_between(collections, apply_start, apply_end, new_apply, "collection actions")
collections = collections.replace(
    "skipped {skipped} path{} because they are not indexed images/folders",
    "skipped {skipped} path{} because individual files must already be indexed; add new content by folder",
)
collections_path.write_text(collections, encoding="utf-8")


# Add one safe migration Collection for legacy user-facing roots that are not covered yet.
collections = collections_path.read_text(encoding="utf-8")
migration = r'''
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
        .filter(|root| !assigned_folders.iter().any(|folder| root.starts_with(folder)))
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

'''
insert_at = collections.find("fn paint_drop_feedback")
if insert_at < 0:
    raise SystemExit("migration insertion marker missing")
collections = collections[:insert_at] + migration + collections[insert_at:]
collections_path.write_text(collections, encoding="utf-8")
