from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


# -----------------------------------------------------------------------------
# db.rs — normalized collection schema + persistence API + regression tests.
# -----------------------------------------------------------------------------
path = Path("src/db.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "use anyhow::{Context, Result};\n",
    "use anyhow::{bail, Context, Result};\n",
    "db anyhow bail import",
)
text = replace_once(
    text,
    '    conn.pragma_update(None, "journal_mode", "WAL")?;\n    conn.pragma_update(None, "synchronous", "NORMAL")?;\n',
    '    conn.pragma_update(None, "journal_mode", "WAL")?;\n    conn.pragma_update(None, "synchronous", "NORMAL")?;\n    conn.pragma_update(None, "foreign_keys", "ON")?;\n',
    "enable foreign keys",
)
text = replace_once(
    text,
    '''        CREATE INDEX IF NOT EXISTS idx_images_root ON images(root);\n        CREATE INDEX IF NOT EXISTS idx_images_file_name ON images(file_name);\n        "#,\n''',
    '''        CREATE INDEX IF NOT EXISTS idx_images_root ON images(root);\n        CREATE INDEX IF NOT EXISTS idx_images_file_name ON images(file_name);\n\n        CREATE TABLE IF NOT EXISTS collections (\n            id INTEGER PRIMARY KEY AUTOINCREMENT,\n            name TEXT NOT NULL COLLATE NOCASE UNIQUE\n        );\n\n        CREATE TABLE IF NOT EXISTS collection_folders (\n            collection_id INTEGER NOT NULL,\n            folder_path TEXT NOT NULL COLLATE NOCASE,\n            PRIMARY KEY(collection_id, folder_path),\n            FOREIGN KEY(collection_id) REFERENCES collections(id) ON DELETE CASCADE\n        );\n\n        CREATE TABLE IF NOT EXISTS collection_files (\n            collection_id INTEGER NOT NULL,\n            file_path TEXT NOT NULL COLLATE NOCASE,\n            PRIMARY KEY(collection_id, file_path),\n            FOREIGN KEY(collection_id) REFERENCES collections(id) ON DELETE CASCADE\n        );\n\n        CREATE INDEX IF NOT EXISTS idx_collection_folders_collection\n            ON collection_folders(collection_id);\n        CREATE INDEX IF NOT EXISTS idx_collection_files_collection\n            ON collection_files(collection_id);\n        "#,\n''',
    "collection schema",
)

collection_api = r'''
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collection {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CollectionMembership {
    pub folders: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
}

pub fn load_collections(db_path: &Path) -> Result<Vec<Collection>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare("SELECT id, name FROM collections ORDER BY name COLLATE NOCASE")?;
    let rows = stmt.query_map([], |row| {
        Ok(Collection {
            id: row.get(0)?,
            name: row.get(1)?,
        })
    })?;
    Ok(rows.filter_map(|row| row.ok()).collect())
}

pub fn create_collection(db_path: &Path, name: &str) -> Result<Collection> {
    let name = name.trim();
    if name.is_empty() {
        bail!("collection name cannot be empty");
    }
    let conn = open(db_path)?;
    conn.execute("INSERT INTO collections(name) VALUES(?1)", params![name])?;
    Ok(Collection {
        id: conn.last_insert_rowid(),
        name: name.to_owned(),
    })
}

pub fn rename_collection(db_path: &Path, collection_id: i64, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("collection name cannot be empty");
    }
    let conn = open(db_path)?;
    let changed = conn.execute(
        "UPDATE collections SET name = ?2 WHERE id = ?1",
        params![collection_id, name],
    )?;
    if changed == 0 {
        bail!("collection no longer exists");
    }
    Ok(())
}

pub fn delete_collection(db_path: &Path, collection_id: i64) -> Result<()> {
    let conn = open(db_path)?;
    conn.execute("DELETE FROM collections WHERE id = ?1", params![collection_id])?;
    Ok(())
}

pub fn add_collection_folders(
    db_path: &Path,
    collection_id: i64,
    folders: &[PathBuf],
) -> Result<usize> {
    let mut conn = open(db_path)?;
    let tx = conn.transaction()?;
    let mut inserted = 0usize;
    {
        let mut stmt = tx.prepare(
            "INSERT OR IGNORE INTO collection_folders(collection_id, folder_path) VALUES(?1, ?2)",
        )?;
        for folder in folders {
            inserted += stmt.execute(params![
                collection_id,
                folder.to_string_lossy().to_string()
            ])?;
        }
    }
    tx.commit()?;
    Ok(inserted)
}

pub fn add_collection_files(
    db_path: &Path,
    collection_id: i64,
    files: &[PathBuf],
) -> Result<usize> {
    let mut conn = open(db_path)?;
    let tx = conn.transaction()?;
    let mut inserted = 0usize;
    {
        let mut stmt = tx.prepare(
            "INSERT OR IGNORE INTO collection_files(collection_id, file_path) VALUES(?1, ?2)",
        )?;
        for file in files {
            inserted += stmt.execute(params![collection_id, file.to_string_lossy().to_string()])?;
        }
    }
    tx.commit()?;
    Ok(inserted)
}

pub fn remove_collection_folder(
    db_path: &Path,
    collection_id: i64,
    folder: &Path,
) -> Result<()> {
    let conn = open(db_path)?;
    conn.execute(
        "DELETE FROM collection_folders WHERE collection_id = ?1 AND folder_path = ?2",
        params![collection_id, folder.to_string_lossy().to_string()],
    )?;
    Ok(())
}

pub fn remove_collection_file(
    db_path: &Path,
    collection_id: i64,
    file: &Path,
) -> Result<()> {
    let conn = open(db_path)?;
    conn.execute(
        "DELETE FROM collection_files WHERE collection_id = ?1 AND file_path = ?2",
        params![collection_id, file.to_string_lossy().to_string()],
    )?;
    Ok(())
}

pub fn load_collection_membership(
    db_path: &Path,
    collection_id: i64,
) -> Result<CollectionMembership> {
    let conn = open(db_path)?;
    let folders = {
        let mut stmt = conn.prepare(
            "SELECT folder_path FROM collection_folders WHERE collection_id = ?1 ORDER BY folder_path COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![collection_id], |row| row.get::<_, String>(0))?;
        rows.filter_map(|row| row.ok()).map(PathBuf::from).collect()
    };
    let files = {
        let mut stmt = conn.prepare(
            "SELECT file_path FROM collection_files WHERE collection_id = ?1 ORDER BY file_path COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![collection_id], |row| row.get::<_, String>(0))?;
        rows.filter_map(|row| row.ok()).map(PathBuf::from).collect()
    };
    Ok(CollectionMembership { folders, files })
}

pub fn load_collection_effective_paths(
    db_path: &Path,
    collection_id: i64,
) -> Result<std::collections::HashSet<PathBuf>> {
    let membership = load_collection_membership(db_path, collection_id)?;
    let manual: std::collections::HashSet<&Path> =
        membership.files.iter().map(PathBuf::as_path).collect();
    let conn = open(db_path)?;
    let mut stmt = conn.prepare("SELECT path FROM images")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut effective = std::collections::HashSet::new();
    for path in rows.filter_map(|row| row.ok()).map(PathBuf::from) {
        if manual.contains(path.as_path())
            || membership
                .folders
                .iter()
                .any(|folder| path.starts_with(folder))
        {
            effective.insert(path);
        }
    }
    Ok(effective)
}

'''
text = replace_once(
    text,
    "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub struct FileState {\n",
    collection_api + "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub struct FileState {\n",
    "collection persistence api",
)

test_marker = '''    #[test]\n    fn delete_path_tree_removes_only_the_requested_subtree() {\n'''
collection_test = r'''    #[test]
    fn collections_persist_deduplicate_recursive_membership_and_delete_safely() {
        let db_path = temp_db_path("collections");
        let root = std::env::temp_dir().join("windows-image-search-collection-root");
        let assigned = root.join("assigned");
        let first = assigned.join("first.jpg");
        let nested = assigned.join("nested").join("second.jpg");
        let manual = root.join("manual.jpg");

        {
            let conn = open(&db_path).unwrap();
            for path in [&first, &nested, &manual] {
                let name = path.file_name().unwrap().to_string_lossy();
                upsert_image(
                    &conn,
                    path,
                    &root,
                    &name,
                    "jpg",
                    1,
                    1,
                    8,
                    8,
                    "",
                    "",
                    [1, 2, 3],
                    1,
                    &[1.0],
                )
                .unwrap();
            }
        }

        let collection = create_collection(&db_path, "Materials").unwrap();
        add_collection_folders(&db_path, collection.id, std::slice::from_ref(&assigned)).unwrap();
        add_collection_files(&db_path, collection.id, &[first.clone(), manual.clone()]).unwrap();

        let persisted = load_collections(&db_path).unwrap();
        assert_eq!(persisted, vec![collection.clone()]);
        let membership = load_collection_membership(&db_path, collection.id).unwrap();
        assert_eq!(membership.folders, vec![assigned.clone()]);
        assert_eq!(membership.files.len(), 2);

        // first.jpg belongs both through the folder and explicitly, but appears once.
        let effective = load_collection_effective_paths(&db_path, collection.id).unwrap();
        assert_eq!(effective.len(), 3);
        assert!(effective.contains(&first));
        assert!(effective.contains(&nested));
        assert!(effective.contains(&manual));

        rename_collection(&db_path, collection.id, "Stone Library").unwrap();
        assert_eq!(load_collections(&db_path).unwrap()[0].name, "Stone Library");

        delete_collection(&db_path, collection.id).unwrap();
        assert!(load_collections(&db_path).unwrap().is_empty());
        assert!(load_collection_membership(&db_path, collection.id)
            .unwrap()
            .folders
            .is_empty());
        // Deleting a collection must never delete indexed/source image records.
        assert_eq!(load_file_states(&open(&db_path).unwrap()).unwrap().len(), 3);

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }

    #[test]
    fn delete_path_tree_removes_only_the_requested_subtree() {
'''
text = replace_once(text, test_marker, collection_test, "collection regression test")
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# ui/collections.rs — Settings management, filter cache, Explorer + in-app DnD.
# -----------------------------------------------------------------------------
Path("src/ui/collections.rs").write_text(
    r'''use super::ImageSearchApp;
use crate::db::{self, Collection, CollectionMembership, ImageSummary};
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
        Ok(())
    }

    pub(super) fn rebuild_effective(&mut self, images: &[ImageSummary]) {
        self.effective.clear();
        for item in &self.items {
            let Some(membership) = self.memberships.get(&item.id) else {
                self.effective.insert(item.id, HashSet::new());
                continue;
            };
            let manual: HashSet<&Path> =
                membership.files.iter().map(PathBuf::as_path).collect();
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

    fn filter_matches(&self, path: &Path) -> bool {
        self.active_filter
            .map(|id| self.effective.get(&id).is_some_and(|paths| paths.contains(path)))
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

    pub(super) fn attach_collection_drag_source(
        &self,
        response: &egui::Response,
        path: &Path,
    ) {
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
                ui.selectable_value(
                    &mut self.collections.active_filter,
                    None,
                    "All images",
                );
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
        ui.add_space(12.0);
        ui.separator();
        ui.heading("Collections");
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
                                format!("{}  ·  {count}", item.name),
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
                    "{} effective indexed image{}",
                    self.collections.count(id),
                    if self.collections.count(id) == 1 { "" } else { "s" }
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
    let explorer = response.contains_pointer()
        && ui.input(|input| !input.raw.hovered_files.is_empty());
    if in_app || explorer {
        ui.painter().rect_stroke(
            response.rect,
            4.0,
            egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
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
        format!("Added {added} collection assignment{}", if added == 1 { "" } else { "s" })
    } else {
        format!(
            "Added {added} collection assignment{}; skipped {skipped} path{} because they are not indexed images/folders",
            if added == 1 { "" } else { "s" },
            if skipped == 1 { "" } else { "s" }
        )
    }
}
''',
    encoding="utf-8",
)


# -----------------------------------------------------------------------------
# ui/mod.rs — state wiring, settings section, Collection filter, live refresh.
# -----------------------------------------------------------------------------
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "mod thumbnails;\nmod views;\n",
    "mod collections;\nmod thumbnails;\nmod views;\n",
    "collections ui module",
)
text = replace_once(
    text,
    "    settings_path: PathBuf,\n    pub(super) search_text: String,\n",
    "    settings_path: PathBuf,\n    pub(super) collections: collections::CollectionsState,\n    pub(super) search_text: String,\n",
    "collections app state field",
)
text = replace_once(
    text,
    '''        let image_positions = images\n            .iter()\n            .enumerate()\n            .map(|(index, record)| (record.path.clone(), index))\n            .collect();\n        Self {\n''',
    '''        let image_positions = images\n            .iter()\n            .enumerate()\n            .map(|(index, record)| (record.path.clone(), index))\n            .collect();\n        let collections = collections::CollectionsState::load(&db_path, &images).unwrap_or_default();\n        Self {\n''',
    "initialize collections state",
)
text = replace_once(
    text,
    "            indexing_settings,\n            settings_path,\n            search_text: String::new(),\n",
    "            indexing_settings,\n            settings_path,\n            collections,\n            search_text: String::new(),\n",
    "store collections state",
)
text = replace_once(
    text,
    '''                WorkerMessage::IndexedBatch(records) => {\n                    self.merge_indexed_batch(records);\n                    self.refresh_text_search_after_data_change();\n                }\n''',
    '''                WorkerMessage::IndexedBatch(records) => {\n                    self.merge_indexed_batch(records);\n                    self.refresh_collection_effective_membership();\n                    self.refresh_text_search_after_data_change();\n                }\n''',
    "refresh collections on indexed batch",
)
text = replace_once(
    text,
    '''                WorkerMessage::RemovedPaths(paths) => {\n                    self.remove_indexed_paths(paths);\n                    self.refresh_text_search_after_data_change();\n                }\n''',
    '''                WorkerMessage::RemovedPaths(paths) => {\n                    self.remove_indexed_paths(paths);\n                    self.refresh_collection_effective_membership();\n                    self.refresh_text_search_after_data_change();\n                }\n''',
    "refresh collections on removed paths",
)
text = replace_once(
    text,
    '''                    self.progress = None;\n                    self.refresh_text_search_after_data_change();\n                }\n                WorkerMessage::SimilarityResults(results) => {\n''',
    '''                    self.progress = None;\n                    self.refresh_collection_effective_membership();\n                    self.refresh_text_search_after_data_change();\n                }\n                WorkerMessage::SimilarityResults(results) => {\n''',
    "refresh collections on reload",
)
text = replace_once(
    text,
    '''                self.similarity_results = None;\n                self.selected_paths.clear();\n                self.refresh_text_search_after_data_change();\n''',
    '''                self.similarity_results = None;\n                self.selected_paths.clear();\n                self.refresh_collection_effective_membership();\n                self.refresh_text_search_after_data_change();\n''',
    "refresh collections after root removal",
)
text = replace_once(
    text,
    '''            .filter(|(_, record)| {\n                if text_filter_active {\n''',
    '''            .filter(|(_, record)| {\n                if !self.collection_filter_matches(&record.path) {\n                    return false;\n                }\n                if text_filter_active {\n''',
    "collection result filter",
)
text = replace_once(
    text,
    '''                ui.add_space(12.0);\n                ui.separator();\n                ui.heading("Live indexing");\n''',
    '''                self.show_collections_settings(ui);\n\n                ui.add_space(12.0);\n                ui.separator();\n                ui.heading("Live indexing");\n''',
    "collections settings section",
)
text = replace_once(
    text,
    '''                    ui.heading("Search");\n                    ui.add(\n''',
    '''                    ui.heading("Search");\n                    self.show_collection_filter(ui);\n                    ui.add(\n''',
    "collection sidebar filter",
)
text = text.replace('.default_width(620.0)\n', '.default_width(920.0)\n', 1)
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# ui/views.rs — make result thumbnails/names native in-app drag sources.
# -----------------------------------------------------------------------------
path = Path("src/ui/views.rs")
text = path.read_text(encoding="utf-8")
text = text.replace("egui::Sense::click(),", "egui::Sense::click_and_drag(),")
text = text.replace(
    'egui::Button::new("Loading thumbnail…"),',
    'egui::Button::new("Loading thumbnail…").sense(egui::Sense::click_and_drag()),',
)
text = text.replace(
    'egui::Button::new("…"))',
    'egui::Button::new("…").sense(egui::Sense::click_and_drag()))',
)
text = text.replace(
    '.sense(egui::Sense::click()),',
    '.sense(egui::Sense::click_and_drag()),',
)
text = replace_once(
    text,
    '''                                self.handle_result_response(&response, &record.path);\n                                response.context_menu(|ui| file_context_menu(ui, &record.path));\n''',
    '''                                self.handle_result_response(&response, &record.path);\n                                self.attach_collection_drag_source(&response, &record.path);\n                                response.context_menu(|ui| file_context_menu(ui, &record.path));\n''',
    "grid thumbnail drag source",
)
text = replace_once(
    text,
    '''                                self.handle_result_response(&label, &record.path);\n                                label.context_menu(|ui| file_context_menu(ui, &record.path));\n''',
    '''                                self.handle_result_response(&label, &record.path);\n                                self.attach_collection_drag_source(&label, &record.path);\n                                label.context_menu(|ui| file_context_menu(ui, &record.path));\n''',
    "grid label drag source",
)
text = replace_once(
    text,
    '''                        self.handle_result_response(&response, &record.path);\n                        response.context_menu(|ui| file_context_menu(ui, &record.path));\n''',
    '''                        self.handle_result_response(&response, &record.path);\n                        self.attach_collection_drag_source(&response, &record.path);\n                        response.context_menu(|ui| file_context_menu(ui, &record.path));\n''',
    "details thumbnail drag source",
)
text = replace_once(
    text,
    '''                                self.handle_result_response(&name, &record.path);\n                                name.context_menu(|ui| file_context_menu(ui, &record.path));\n''',
    '''                                self.handle_result_response(&name, &record.path);\n                                self.attach_collection_drag_source(&name, &record.path);\n                                name.context_menu(|ui| file_context_menu(ui, &record.path));\n''',
    "details name drag source",
)
path.write_text(text, encoding="utf-8")
