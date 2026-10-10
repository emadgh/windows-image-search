use super::{ImageSearchApp, SearchMode};
use crate::db::{Collection, CollectionMembership};
use crate::navigation::{folder_matches, path_key, within, NavigationSettings};
use eframe::egui;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(super) struct VisualInput {
    pub path: PathBuf,
    pub description: Option<String>,
    pub settings: crate::indexer::SimilaritySettings,
    pub region: Option<crate::visual_query::QueryRegion>,
}

#[derive(Clone)]
pub(super) enum LibraryQuery {
    Visual(VisualInput),
    Face(
        crate::face_search::IndexedFaceSuggestion,
        crate::face_search::IndexedFaceSearchOptions,
    ),
    ExternalFace(
        crate::external_face_query::ExternalFaceChoice,
        crate::face_search::IndexedFaceSearchOptions,
    ),
}

#[derive(Clone)]
enum Kind {
    All,
    Collection(i64),
    Folder(Option<i64>, PathBuf),
    Group,
}

#[derive(Clone)]
struct Node {
    key: String,
    label: String,
    kind: Kind,
    count: usize,
    unavailable: bool,
    parent: Option<usize>,
    children: Vec<usize>,
}

#[derive(Default)]
struct Model {
    nodes: Vec<Node>,
    roots: Vec<usize>,
    folders: HashMap<String, Vec<usize>>,
}

impl Model {
    fn add(
        &mut self,
        key: String,
        label: String,
        kind: Kind,
        count: usize,
        unavailable: bool,
        parent: Option<usize>,
    ) -> usize {
        let index = self.nodes.len();
        if let Kind::Folder(_, path) = &kind {
            self.folders.entry(path_key(path)).or_default().push(index);
        }
        self.nodes.push(Node {
            key,
            label,
            kind,
            count,
            unavailable,
            parent,
            children: vec![],
        });
        if let Some(parent) = parent {
            self.nodes[parent].children.push(index);
        } else {
            self.roots.push(index);
        }
        index
    }
}

struct Snapshot {
    revision: (u64, u64, Vec<PathBuf>, Vec<PathBuf>),
    collections: Vec<(Collection, CollectionMembership)>,
    paths: Vec<PathBuf>,
}

#[derive(Default)]
struct FolderEntry {
    path: PathBuf,
    parent: Option<String>,
    count: usize,
}

fn minimal_roots(mut roots: Vec<PathBuf>) -> Vec<PathBuf> {
    roots.sort_by_key(|p| (path_key(p).len(), path_key(p)));
    let mut result: Vec<PathBuf> = vec![];
    for root in roots {
        if !result.iter().any(|p| within(&root, p)) {
            result.push(root);
        }
    }
    result
}

fn add_chain(
    entries: &mut BTreeMap<String, FolderEntry>,
    folder: &Path,
    root: &Path,
    count: usize,
) {
    let mut current = folder.to_path_buf();
    loop {
        let at_root = path_key(&current) == path_key(root);
        let parent = (!at_root)
            .then(|| current.parent().map(Path::to_path_buf))
            .flatten();
        let entry = entries
            .entry(path_key(&current))
            .or_insert_with(|| FolderEntry {
                path: current.clone(),
                parent: parent.as_ref().map(|p| path_key(p)),
                count: 0,
            });
        entry.count += count;
        let Some(next) = parent else {
            break;
        };
        if !within(&next, root) {
            break;
        }
        current = next;
    }
}

fn append_folders(
    model: &mut Model,
    parent: usize,
    prefix: &str,
    collection: Option<i64>,
    roots: Vec<PathBuf>,
    paths: &[PathBuf],
    available: &[PathBuf],
) {
    let roots = minimal_roots(roots);
    let mut entries = BTreeMap::new();
    for root in &roots {
        add_chain(&mut entries, root, root, 0);
    }
    for path in paths {
        if let Some(root) = roots.iter().find(|root| within(path, root)) {
            if let Some(folder) = path.parent() {
                add_chain(&mut entries, folder, root, 1);
            }
        }
    }
    let mut children: HashMap<Option<String>, Vec<String>> = HashMap::new();
    for (key, entry) in &entries {
        children
            .entry(entry.parent.clone())
            .or_default()
            .push(key.clone());
    }
    fn append(
        model: &mut Model,
        parent: usize,
        key: &str,
        prefix: &str,
        collection: Option<i64>,
        entries: &BTreeMap<String, FolderEntry>,
        children: &HashMap<Option<String>, Vec<String>>,
        available: &[PathBuf],
    ) {
        let entry = &entries[key];
        let label = if entry.parent.is_none() {
            entry.path.display().to_string()
        } else {
            entry
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        let unavailable = !available
            .iter()
            .any(|root| within(&entry.path, root) || within(root, &entry.path));
        let index = model.add(
            format!("{prefix}/{key}"),
            label,
            Kind::Folder(collection, entry.path.clone()),
            entry.count,
            unavailable,
            Some(parent),
        );
        if let Some(keys) = children.get(&Some(key.to_owned())) {
            for child in keys {
                append(
                    model, index, child, prefix, collection, entries, children, available,
                );
            }
        }
    }
    if let Some(keys) = children.get(&None) {
        for key in keys {
            append(
                model, parent, key, prefix, collection, &entries, &children, available,
            );
        }
    }
}

fn build_model(snapshot: &Snapshot) -> Model {
    let mut model = Model::default();
    let available = &snapshot.revision.2;
    let mut unique = BTreeMap::new();
    for path in &snapshot.paths {
        unique.entry(path_key(path)).or_insert(path.clone());
    }
    let paths = unique.into_values().collect::<Vec<_>>();
    model.add(
        "all".into(),
        "All Photos".into(),
        Kind::All,
        paths.len(),
        false,
        None,
    );
    let mut collections = snapshot.collections.clone();
    collections.sort_by_key(|(c, _)| (c.name.to_lowercase(), c.id));
    let mut registered = vec![];
    for (collection, membership) in collections {
        registered.extend(membership.folders.clone());
        let manual: HashSet<_> = membership.files.iter().map(|p| path_key(p)).collect();
        let eligible = paths
            .iter()
            .filter(|p| {
                manual.contains(&path_key(p)) || membership.folders.iter().any(|f| within(p, f))
            })
            .cloned()
            .collect::<Vec<_>>();
        let prefix = format!("collection:{}", collection.id);
        let parent = model.add(
            prefix.clone(),
            collection.name,
            Kind::Collection(collection.id),
            eligible.len(),
            false,
            None,
        );
        append_folders(
            &mut model,
            parent,
            &prefix,
            Some(collection.id),
            membership.folders.clone(),
            &eligible,
            available,
        );
        let individual = membership
            .files
            .iter()
            .filter(|p| !membership.folders.iter().any(|f| within(p, f)))
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect::<Vec<_>>();
        if !individual.is_empty() {
            let manual_paths = eligible
                .iter()
                .filter(|p| {
                    manual.contains(&path_key(p))
                        && !membership.folders.iter().any(|f| within(p, f))
                })
                .cloned()
                .collect::<Vec<_>>();
            let prefix = format!("{prefix}/individual");
            let group = model.add(
                prefix.clone(),
                "Individual files".into(),
                Kind::Group,
                manual_paths.len(),
                false,
                Some(parent),
            );
            append_folders(
                &mut model,
                group,
                &prefix,
                Some(collection.id),
                individual,
                &manual_paths,
                available,
            );
        }
    }
    let roots = available
        .iter()
        .chain(&snapshot.revision.3)
        .filter(|root| !registered.iter().any(|folder| within(root, folder)))
        .cloned()
        .collect::<Vec<_>>();
    if !roots.is_empty() {
        let paths = paths
            .into_iter()
            .filter(|p| roots.iter().any(|root| within(p, root)))
            .collect::<Vec<_>>();
        let parent = model.add(
            "unassigned".into(),
            "Other folders".into(),
            Kind::Group,
            paths.len(),
            false,
            None,
        );
        append_folders(
            &mut model,
            parent,
            "unassigned",
            None,
            roots,
            &paths,
            available,
        );
    }
    model
}

pub(super) struct NavigationState {
    pub settings: NavigationSettings,
    settings_path: PathBuf,
    pub folder: Option<PathBuf>,
    pub scope_revision: u64,
    pub query: Option<LibraryQuery>,
    pub rerun_due: Option<Instant>,
    pub tree_focus: bool,
    pub observed_file: Option<PathBuf>,
    observed_scope: Option<(Option<i64>, Option<String>, bool)>,
    requested: Option<(u64, u64, Vec<PathBuf>, Vec<PathBuf>)>,
    built: Option<(u64, u64, Vec<PathBuf>, Vec<PathBuf>)>,
    tx: Sender<Snapshot>,
    rx: Receiver<(Snapshot, Model)>,
    model: Model,
    expanded: HashSet<String>,
    visible: Arc<[(usize, usize)]>,
    focused: Option<String>,
    highlight: Option<(String, Instant)>,
    scroll_to: Option<String>,
}

impl NavigationState {
    pub fn new(path: PathBuf) -> Self {
        let (tx, requests) = mpsc::channel::<Snapshot>();
        let (results, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(mut snapshot) = requests.recv() {
                while let Ok(newer) = requests.try_recv() {
                    snapshot = newer;
                }
                let model = build_model(&snapshot);
                if results.send((snapshot, model)).is_err() {
                    break;
                }
            }
        });
        Self {
            settings: NavigationSettings::load(&path),
            settings_path: path,
            folder: None,
            scope_revision: 0,
            query: None,
            rerun_due: None,
            tree_focus: false,
            observed_file: None,
            observed_scope: None,
            requested: None,
            built: None,
            tx,
            rx,
            model: Model::default(),
            expanded: HashSet::new(),
            visible: Vec::new().into(),
            focused: None,
            highlight: None,
            scroll_to: None,
        }
    }

    fn reveal_folder(&mut self, folder: &Path, active: Option<i64>) {
        let candidate = self
            .model
            .folders
            .get(&path_key(folder))
            .into_iter()
            .flatten()
            .filter_map(|&index| match &self.model.nodes[index].kind {
                Kind::Folder(collection, _) => {
                    Some((index, active.is_some() && *collection == active))
                }
                _ => None,
            })
            .min_by_key(|(index, active)| (!*active, *index))
            .map(|(index, _)| index);
        let Some(index) = candidate else {
            return;
        };
        let node = self.model.nodes[index].clone();
        if self.settings.highlight_folder {
            self.highlight = Some((node.key.clone(), Instant::now()));
        }
        if self.settings.auto_follow {
            let mut parent = node.parent;
            while let Some(index) = parent {
                self.expanded.insert(self.model.nodes[index].key.clone());
                parent = self.model.nodes[index].parent;
            }
            self.rebuild_visible();
            self.scroll_to = Some(node.key);
        }
    }

    fn rebuild_visible(&mut self) {
        fn visit(
            model: &Model,
            expanded: &HashSet<String>,
            output: &mut Vec<(usize, usize)>,
            index: usize,
            depth: usize,
        ) {
            output.push((index, depth));
            if expanded.contains(&model.nodes[index].key) {
                for &child in &model.nodes[index].children {
                    visit(model, expanded, output, child, depth + 1);
                }
            }
        }
        let mut visible = Vec::new();
        for &root in &self.model.roots {
            visit(&self.model, &self.expanded, &mut visible, root, 0);
        }
        self.visible = visible.into();
    }
}

impl ImageSearchApp {
    pub(super) fn folder_filter_matches(&self, path: &Path) -> bool {
        folder_matches(
            path,
            self.navigation.folder.as_ref(),
            self.navigation.settings.include_subfolders,
        )
    }

    pub(super) fn save_navigation_settings(&mut self) {
        if let Err(error) = self
            .navigation
            .settings
            .save(&self.navigation.settings_path)
        {
            self.last_error = Some(format!("Cannot save navigation preferences: {error:#}"));
        }
    }

    pub(super) fn navigation_scope_changed(&mut self) {
        self.navigation.scope_revision = self.navigation.scope_revision.wrapping_add(1);
        self.navigation.observed_scope = Some((
            self.collection_filter_cache_token().0,
            self.navigation.folder.as_ref().map(|p| path_key(p)),
            self.navigation.settings.include_subfolders,
        ));
        self.text_search_generation = self.text_search_generation.wrapping_add(1);
        if !self.search_text.trim().is_empty() {
            self.text_search_due = Some(Instant::now() + Duration::from_millis(200));
            self.text_search_pending = true;
        }
        self.cancel_visual_search();
        self.cancel_face_scope_search();
        self.navigation.rerun_due = (self.navigation.settings.auto_rerun
            && self.navigation.query.is_some())
        .then(|| Instant::now() + Duration::from_millis(200));
    }

    pub(super) fn set_library_scope(&mut self, collection: Option<i64>, folder: Option<PathBuf>) {
        if self.collections.active_filter == collection && self.navigation.folder == folder {
            return;
        }
        self.collections.active_filter = collection;
        self.navigation.folder = folder;
        self.navigation_scope_changed();
    }

    pub(super) fn process_navigation(&mut self, ctx: &egui::Context) {
        if self
            .navigation
            .observed_scope
            .as_ref()
            .is_some_and(|old| old.0 != self.collection_filter_cache_token().0)
        {
            self.navigation.folder = None;
        }
        let scope = (
            self.collection_filter_cache_token().0,
            self.navigation.folder.as_ref().map(|p| path_key(p)),
            self.navigation.settings.include_subfolders,
        );
        if self
            .navigation
            .observed_scope
            .as_ref()
            .is_some_and(|old| old != &scope)
        {
            self.navigation_scope_changed();
        }
        self.navigation.observed_scope = Some(scope);
        let revision = (
            self.image_catalog_revision,
            self.collection_filter_cache_token().1,
            self.roots.clone(),
            self.unavailable_roots.clone(),
        );
        if self.navigation.requested.as_ref() != Some(&revision) {
            self.navigation.requested = Some(revision.clone());
            let snapshot = Snapshot {
                revision,
                collections: self.navigation_collections(),
                paths: self.images.iter().map(|i| i.path.clone()).collect(),
            };
            let _ = self.navigation.tx.send(snapshot);
        }
        while let Ok((snapshot, model)) = self.navigation.rx.try_recv() {
            if self.navigation.requested.as_ref() != Some(&snapshot.revision) {
                continue;
            }
            let old: HashSet<_> = self
                .navigation
                .model
                .nodes
                .iter()
                .map(|n| n.key.clone())
                .collect();
            self.navigation.built = Some(snapshot.revision);
            self.navigation.model = model;
            self.navigation.expanded.extend(
                self.navigation
                    .model
                    .roots
                    .iter()
                    .map(|&i| self.navigation.model.nodes[i].key.clone())
                    .filter(|key| !old.contains(key)),
            );
            self.navigation.rebuild_visible();
            self.navigation.observed_file = None;
        }
        if self.navigation.model.nodes.is_empty() || self.navigation.rerun_due.is_some() {
            ctx.request_repaint_after(Duration::from_millis(40));
        } else if self.navigation.requested != self.navigation.built {
            // Poll background model completions without blocking the UI thread.
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }

    pub(super) fn dispatch_navigation_search(&mut self) {
        let Some(due) = self.navigation.rerun_due else {
            return;
        };
        if Instant::now() < due
            || self.text_search_pending
            || self.people_filter_work_pending()
            || self.search_text != self.text_search_observed
            || (self.busy && !self.indexing)
            || self.face_search_running()
        {
            return;
        }
        let query = self.navigation.query.clone();
        self.navigation.rerun_due = None;
        if self.images.is_empty() {
            self.similarity_results = Some(vec![]);
            return;
        }
        match query {
            Some(LibraryQuery::Visual(input))
                if matches!(
                    self.search_mode,
                    SearchMode::SimilarImage | SearchMode::Semantic
                ) =>
            {
                self.run_visual_input(input);
            }
            Some(LibraryQuery::Face(query, options)) if self.search_mode == SearchMode::Face => {
                self.rerun_indexed_face_search(query, options)
            }
            Some(LibraryQuery::ExternalFace(query, options))
                if self.search_mode == SearchMode::Face =>
            {
                self.rerun_external_face_search(query, options)
            }
            _ => {}
        }
    }

    pub(super) fn navigation_preferences(&mut self, ui: &mut egui::Ui) {
        ui.heading("Folders & Collections");
        let before = self.navigation.settings.clone();
        let settings = &mut self.navigation.settings;
        ui.checkbox(&mut settings.show_tree, "Show folder and collection tree");
        ui.checkbox(
            &mut settings.filter_on_click,
            "Filter results when clicking a folder",
        );
        ui.checkbox(
            &mut settings.include_subfolders,
            "Include subfolders in the selected folder scope",
        );
        ui.checkbox(
            &mut settings.auto_follow,
            "Reveal and scroll to the selected file's folder",
        );
        ui.checkbox(
            &mut settings.highlight_folder,
            "Briefly highlight the selected file's folder",
        );
        ui.checkbox(
            &mut settings.auto_rerun,
            "Re-run the active search when its folder or collection scope changes",
        );
        if before != self.navigation.settings {
            if !self.navigation.settings.auto_follow {
                self.navigation.scroll_to = None;
            }
            if !self.navigation.settings.highlight_folder {
                self.navigation.highlight = None;
            }
            if before.include_subfolders != self.navigation.settings.include_subfolders {
                self.navigation_scope_changed();
            }
            if !self.navigation.settings.auto_rerun {
                self.navigation.rerun_due = None;
            }
            self.navigation.observed_file = None;
            self.save_navigation_settings();
        }
    }

    fn select_navigation_node(&mut self, index: usize) {
        let node = self.navigation.model.nodes[index].clone();
        self.navigation.focused = Some(node.key);
        match node.kind {
            Kind::All => self.set_library_scope(None, None),
            Kind::Collection(id) => self.set_library_scope(Some(id), None),
            Kind::Folder(collection, folder) if self.navigation.settings.filter_on_click => {
                self.set_library_scope(collection, Some(folder))
            }
            _ => {}
        }
    }

    fn reveal_navigation_file(&mut self) {
        if self.navigation.observed_file == self.focused_result {
            return;
        }
        self.navigation.observed_file = self.focused_result.clone();
        let Some(path) = self
            .focused_result
            .as_ref()
            .filter(|p| self.selected_paths.contains(*p))
        else {
            return;
        };
        let Some(folder) = path.parent() else {
            return;
        };
        let active = self.collection_filter_cache_token().0;
        self.navigation.reveal_folder(folder, active);
    }

    pub(super) fn show_navigation_tree(&mut self, ui: &mut egui::Ui) {
        self.reveal_navigation_file();
        let region = ui.available_rect_before_wrap();
        ui.horizontal(|ui| {
            ui.strong("Folders & Collections");
            if ui.small_button("Settings").clicked() {
                self.navigation.tree_focus = false;
                self.settings_open = true;
                ui.ctx()
                    .data_mut(|data| data.insert_temp(egui::Id::new("preferences-category"), 6_u8));
            }
        });
        let keyboard_event = ui.input(|input| {
            [
                egui::Key::ArrowUp,
                egui::Key::ArrowDown,
                egui::Key::ArrowLeft,
                egui::Key::ArrowRight,
                egui::Key::Enter,
            ]
            .into_iter()
            .any(|key| input.key_pressed(key))
        });
        let focused = keyboard_event
            .then(|| {
                self.navigation.visible.iter().position(|(index, _)| {
                    Some(&self.navigation.model.nodes[*index].key)
                        == self.navigation.focused.as_ref()
                })
            })
            .flatten();
        if self.navigation.tree_focus
            && !self.settings_open
            && !self.collections_open
            && !self.preview_open()
            && !self.task_center_open
            && !self.people_manager_ui.open
            && !self.close_confirmation_open
        {
            if let Some(row) = focused {
                let (up, down, left, right, enter) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::ArrowUp),
                        i.key_pressed(egui::Key::ArrowDown),
                        i.key_pressed(egui::Key::ArrowLeft),
                        i.key_pressed(egui::Key::ArrowRight),
                        i.key_pressed(egui::Key::Enter),
                    )
                });
                let target = if up {
                    row.saturating_sub(1)
                } else if down {
                    (row + 1).min(self.navigation.visible.len() - 1)
                } else {
                    row
                };
                let index = self.navigation.visible[target].0;
                let node = self.navigation.model.nodes[index].clone();
                if up || down || enter {
                    self.select_navigation_node(index);
                    self.navigation.scroll_to = Some(node.key.clone());
                }
                if right {
                    self.navigation.expanded.insert(node.key.clone());
                    self.navigation.rebuild_visible();
                }
                if left {
                    if !self.navigation.expanded.remove(&node.key) {
                        if let Some(parent) = node.parent {
                            self.navigation.focused =
                                Some(self.navigation.model.nodes[parent].key.clone());
                        }
                    }
                    self.navigation.rebuild_visible();
                }
            }
        }
        // Keep virtualization geometry identical to the compact widget geometry.
        const ROW: f32 = 20.0;
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            ui.spacing_mut().button_padding.y = 0.0;
            ui.spacing_mut().interact_size.y = ROW;
            let stride = ROW;
            let mut area = egui::ScrollArea::vertical()
                .id_salt("library-navigation-tree")
                .auto_shrink([false, false]);
            if let Some(key) = self.navigation.scroll_to.take() {
                if let Some(row) = self
                    .navigation
                    .visible
                    .iter()
                    .position(|(index, _)| self.navigation.model.nodes[*index].key == key)
                {
                    area = area.vertical_scroll_offset(
                        (row as f32 * stride - ui.available_height() * 0.5).max(0.0),
                    );
                }
            }
            let visible = self.navigation.visible.clone();
            let mut toggle = None;
            let mut selected = None;
            area.show_rows(ui, ROW, visible.len(), |ui, range| {
                for row in range {
                    let (index, depth) = visible[row];
                    let node = &self.navigation.model.nodes[index];
                    ui.push_id(&node.key, |ui| {
                        ui.horizontal(|ui| {
                            ui.set_height(ROW);
                            ui.add_space(depth as f32 * 14.0);
                            if node.children.is_empty() {
                                ui.add_space(20.0);
                            } else if super::icons::button(
                                ui,
                                egui::vec2(20.0, ROW),
                                if self.navigation.expanded.contains(&node.key) {
                                    super::icons::Icon::Collapse
                                } else {
                                    super::icons::Icon::Expand
                                },
                                false,
                            )
                            .clicked()
                            {
                                toggle = Some(node.key.clone());
                                self.navigation.tree_focus = true;
                            }
                            let active = match &node.kind {
                                Kind::All => {
                                    self.collections.active_filter.is_none()
                                        && self.navigation.folder.is_none()
                                }
                                Kind::Collection(id) => {
                                    self.collections.active_filter == Some(*id)
                                        && self.navigation.folder.is_none()
                                }
                                Kind::Folder(collection, folder) => {
                                    self.collections.active_filter == *collection
                                        && self
                                            .navigation
                                            .folder
                                            .as_ref()
                                            .is_some_and(|p| path_key(p) == path_key(folder))
                                }
                                Kind::Group => false,
                            };
                            let text = format!(
                                "{} ({}){}",
                                node.label,
                                node.count,
                                if node.unavailable {
                                    " · Unavailable"
                                } else {
                                    ""
                                }
                            );
                            let mut button = egui::Button::selectable(
                                active,
                                (egui::RichText::new(text), egui::Atom::grow()),
                            )
                            .truncate()
                            .corner_radius(0);
                            if self.navigation.focused.as_deref() == Some(node.key.as_str())
                                && !active
                            {
                                button = button.stroke(ui.visuals().selection.stroke);
                            }
                            if let Some((key, started)) = &self.navigation.highlight {
                                let elapsed = started.elapsed().as_secs_f32();
                                if key == &node.key && elapsed < 1.5 {
                                    button = button.fill(
                                        ui.visuals()
                                            .selection
                                            .bg_fill
                                            .gamma_multiply(0.5 * (1.0 - elapsed / 1.5)),
                                    );
                                    ui.ctx().request_repaint_after(Duration::from_millis(30));
                                }
                            }
                            let response = ui
                                .add_sized([ui.available_width(), ROW], button)
                                .on_hover_text(match &node.kind {
                                    Kind::Folder(_, path) => path.display().to_string(),
                                    _ => node.label.clone(),
                                });
                            if response.clicked() {
                                selected = Some(index);
                                self.navigation.tree_focus = true;
                            }
                        });
                    });
                }
            });
            if let Some(key) = toggle {
                self.navigation.focused = Some(key.clone());
                if !self.navigation.expanded.remove(&key) {
                    self.navigation.expanded.insert(key);
                }
                self.navigation.rebuild_visible();
            }
            if let Some(index) = selected {
                self.select_navigation_node(index);
            }
        });
        if ui.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|pos| !region.contains(pos))
        }) {
            self.navigation.tree_focus = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ui_fixture() -> (ImageSearchApp, PathBuf) {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("wis-navigation-ui-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::update::save_settings(
            &dir.join("update-settings.ini"),
            crate::update::UpdateSettings {
                auto_check: false,
                auto_download: false,
            },
        )
        .unwrap();
        let mut app = ImageSearchApp::new(dir.join("index.sqlite3"), dir.join("models"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.busy && Instant::now() < deadline {
            app.process_startup_messages();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.busy, "test startup failed: {:?}", app.last_error);
        app.navigation.model = build_model(&collections_snapshot());
        app.navigation.rebuild_visible();
        (app, dir)
    }

    fn render(
        app: &mut ImageSearchApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.show_navigation_tree(ui));
            },
        )
    }

    fn text_position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("tree row not rendered: {label}"))
    }

    fn click(app: &mut ImageSearchApp, ctx: &egui::Context, pos: egui::Pos2) -> egui::FullOutput {
        render(
            app,
            ctx,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        render(
            app,
            ctx,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
    }

    fn cleanup_ui(app: ImageSearchApp, dir: PathBuf) {
        drop(app);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => break,
                Err(error) if Instant::now() >= deadline => panic!("test cleanup failed: {error}"),
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    #[test]
    fn compact_rows_touch_even_with_the_application_spacing_defaults() {
        let (mut app, dir) = ui_fixture();
        let ctx = egui::Context::default();
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.interact_size.y = 28.0;
        ctx.set_style(style);
        let output = render(&mut app, &ctx, vec![]);
        let positions = [
            "All Photos (4)",
            "Alpha (3)",
            "Beta (2)",
            "Other folders (2)",
        ]
        .map(|label| text_position(&output, label));
        for pair in positions.windows(2) {
            assert!(
                (pair[1].y - pair[0].y - 20.0).abs() < 0.5,
                "tree rows should have a 20px pitch without a gap: {:?}",
                pair
            );
        }
        assert_eq!(
            ctx.style().spacing.item_spacing.y,
            6.0,
            "compact styling must remain local to the tree"
        );
        cleanup_ui(app, dir);
    }

    #[test]
    fn tree_labels_align_at_the_left_edge() {
        let (mut app, dir) = ui_fixture();
        let ctx = egui::Context::default();
        let output = render(&mut app, &ctx, vec![]);
        let labels = [
            "All Photos (4)",
            "Alpha (3)",
            "Beta (2)",
            "Other folders (2)",
        ];
        let positions = labels.map(|label| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => Some(text.pos.x),
                    _ => None,
                })
                .unwrap()
        });
        assert!(
            positions[0] < 60.0,
            "tree labels should start near the left edge"
        );
        assert!(positions.iter().all(|x| (x - positions[0]).abs() < 0.5));
        cleanup_ui(app, dir);
    }

    #[test]
    fn real_tree_widgets_support_mouse_and_keyboard_without_arrow_filtering() {
        let (mut app, dir) = ui_fixture();
        let ctx = egui::Context::default();
        let output = render(&mut app, &ctx, vec![]);
        let alpha = text_position(&output, "Alpha (3)");
        let arrow = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::LineSegment { points, .. }
                    if (points[0].y - points[1].y).abs() < 0.5
                        && (points[0].x - points[1].x).abs() <= 10.0
                        && (points[0].y - alpha.y).abs() < 3.0 =>
                {
                    Some(points[0] + (points[1] - points[0]) * 0.5)
                }
                _ => None,
            })
            .unwrap();
        click(&mut app, &ctx, arrow);
        assert!(app.navigation.expanded.contains("collection:1"));
        assert!(app.collections.active_filter.is_none());
        assert!(app.navigation.folder.is_none());
        let output = render(&mut app, &ctx, vec![]);
        click(&mut app, &ctx, text_position(&output, "Alpha (3)"));
        assert_eq!(app.collections.active_filter, Some(1));
        app.navigation
            .expanded
            .insert(r"collection:1/c:\photos".into());
        app.navigation.rebuild_visible();
        let output = render(&mut app, &ctx, vec![]);
        click(&mut app, &ctx, text_position(&output, "child (1)"));
        assert_eq!(
            app.navigation.folder.as_deref(),
            Some(Path::new(r"C:\Photos\child"))
        );
        render(
            &mut app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::ArrowUp,
                physical_key: Some(egui::Key::ArrowUp),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(
            app.navigation.folder.as_deref(),
            Some(Path::new(r"C:\Photos"))
        );
        cleanup_ui(app, dir);
    }

    #[test]
    fn folder_scope_is_shared_by_visible_results_and_search_candidates() {
        let (mut app, dir) = ui_fixture();
        let root = dir.join("photos");
        let folder = root.join("selected");
        std::fs::create_dir_all(folder.join("child")).unwrap();
        let paths = [
            folder.join("direct.jpg"),
            folder.join("child").join("nested.jpg"),
            root.join("outside.jpg"),
        ];
        for path in &paths {
            std::fs::write(path, b"test").unwrap();
        }
        app.roots = vec![root.clone()];
        app.images = paths
            .iter()
            .map(|path| crate::db::ImageSummary {
                path: path.clone(),
                root: root.clone(),
                file_name: path.file_name().unwrap().to_string_lossy().into_owned(),
                extension: "jpg".into(),
                size: 4,
                modified: 0,
                width: 1,
                height: 1,
                description: String::new(),
                keywords: String::new(),
                dominant: [0, 0, 0],
                score: None,
            })
            .collect();
        let collection = crate::db::create_collection(&app.db_path, "Test collection").unwrap();
        crate::db::add_collection_folders(
            &app.db_path,
            collection.id,
            std::slice::from_ref(&folder),
        )
        .unwrap();
        app.collections =
            super::super::collections::CollectionsState::load(&app.db_path, &app.images).unwrap();
        app.set_library_scope(Some(collection.id), Some(folder));
        assert_eq!(app.visible_indices().len(), 2);
        assert_eq!(
            app.search_eligibility().unwrap(),
            HashSet::from([paths[0].clone(), paths[1].clone()])
        );
        app.navigation.settings.include_subfolders = false;
        app.navigation_scope_changed();
        assert_eq!(app.visible_indices().len(), 1);
        assert_eq!(
            app.search_eligibility().unwrap(),
            HashSet::from([paths[0].clone()])
        );
        app.clear_collection_filter();
        assert_eq!(app.visible_indices().len(), 3);
        assert!(app.search_eligibility().is_none());
        cleanup_ui(app, dir);
    }

    #[test]
    fn rapid_scope_changes_cancel_old_results_and_wait_for_text_filters() {
        let (mut app, dir) = ui_fixture();
        let request = app.visual_search_session.start();
        app.searching = true;
        app.busy = true;
        app.navigation.query = Some(LibraryQuery::Visual(VisualInput {
            path: PathBuf::from("query.jpg"),
            description: None,
            settings: crate::indexer::SimilaritySettings::default(),
            region: None,
        }));
        app.search_text = "beach".into();
        app.text_search_observed = app.search_text.clone();
        app.set_library_scope(Some(1), Some(PathBuf::from(r"C:\Photos")));
        let due = app.navigation.rerun_due.unwrap();
        app.set_library_scope(Some(1), Some(PathBuf::from(r"C:\Photos\child")));
        assert_eq!(app.navigation.scope_revision, 2);
        assert!(app.navigation.rerun_due.unwrap() >= due);
        assert!(request.check().is_err());
        app.tx
            .send(crate::indexer::WorkerMessage::VisualSearch {
                id: request.id,
                event: crate::indexer::VisualSearchEvent::Finished(Err("stale result".into())),
            })
            .unwrap();
        app.process_worker_messages();
        assert_ne!(app.last_error.as_deref(), Some("stale result"));
        app.navigation.rerun_due = Some(Instant::now());
        app.dispatch_navigation_search();
        assert!(
            app.navigation.rerun_due.is_some(),
            "must wait for pending text filter"
        );
        app.text_search_pending = false;
        app.dispatch_navigation_search();
        assert!(app.navigation.rerun_due.is_none());
        assert!(app.similarity_results.as_ref().unwrap().is_empty());
        app.navigation.settings.auto_rerun = false;
        app.set_library_scope(None, None);
        assert!(app.navigation.rerun_due.is_none());
        cleanup_ui(app, dir);
    }

    #[test]
    fn sidebar_split_keeps_tree_header_fixed_when_search_controls_scroll() {
        let (mut app, dir) = ui_fixture();
        app.navigation.settings.tree_fraction = 0.8;
        let ctx = egui::Context::default();
        let draw = |app: &mut ImageSearchApp, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 700.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| app.show_search_sidebar(ctx),
            )
        };
        let output = draw(&mut app, vec![]);
        let before = text_position(&output, "Folders & Collections");
        assert!(before.y > 120.0 && before.y < 250.0);
        let output = draw(
            &mut app,
            vec![
                egui::Event::PointerMoved(egui::pos2(100.0, 80.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -300.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let after = text_position(&output, "Folders & Collections");
        assert!((before.y - after.y).abs() < 1.0);
        cleanup_ui(app, dir);
    }

    #[test]
    fn virtual_tree_reveals_a_folder_far_outside_the_viewport() {
        let (mut app, dir) = ui_fixture();
        let snapshot = Snapshot {
            revision: (1, 1, vec![PathBuf::from(r"C:\Photos")], vec![]),
            collections: vec![],
            paths: (0..10_000)
                .map(|i| PathBuf::from(format!(r"C:\Photos\folder{i:05}\a.jpg")))
                .collect(),
        };
        app.navigation.model = build_model(&snapshot);
        app.navigation.expanded.clear();
        app.navigation.rebuild_visible();
        let ctx = egui::Context::default();
        render(&mut app, &ctx, vec![]);
        app.navigation
            .reveal_folder(Path::new(r"C:\Photos\folder09999"), None);
        let output = render(&mut app, &ctx, vec![]);
        text_position(&output, "folder09999 (1)");
        assert!(
            output.shapes.len() < 600,
            "offscreen rows should not be drawn"
        );
        assert!(app.navigation.folder.is_none());
        cleanup_ui(app, dir);
    }

    fn collections_snapshot() -> Snapshot {
        Snapshot {
            revision: (
                1,
                1,
                vec![PathBuf::from(r"C:\Photos"), PathBuf::from(r"D:\Other")],
                vec![],
            ),
            collections: vec![
                (
                    Collection {
                        id: 2,
                        name: "Beta".into(),
                    },
                    CollectionMembership {
                        folders: vec![PathBuf::from(r"C:\Photos")],
                        files: vec![],
                    },
                ),
                (
                    Collection {
                        id: 1,
                        name: "Alpha".into(),
                    },
                    CollectionMembership {
                        folders: vec![
                            PathBuf::from(r"C:\Photos"),
                            PathBuf::from(r"C:\Photos\child"),
                        ],
                        files: vec![PathBuf::from(r"D:\Other\loose\manual.jpg")],
                    },
                ),
            ],
            paths: vec![
                PathBuf::from(r"C:\Photos\a.jpg"),
                PathBuf::from(r"C:\Photos\child\b.jpg"),
                PathBuf::from(r"D:\Other\loose\manual.jpg"),
                PathBuf::from(r"D:\Other\loose\unrelated.jpg"),
            ],
        }
    }

    #[test]
    fn shared_folders_and_individual_files_keep_collection_membership() {
        let model = build_model(&collections_snapshot());
        let alpha = model
            .nodes
            .iter()
            .find(|n| matches!(n.kind, Kind::Collection(1)))
            .unwrap();
        let beta = model
            .nodes
            .iter()
            .find(|n| matches!(n.kind, Kind::Collection(2)))
            .unwrap();
        assert_eq!(alpha.count, 3);
        assert_eq!(beta.count, 2);
        assert_eq!(
            model
                .nodes
                .iter()
                .filter(|n| n.key == r"collection:1/c:\photos\child")
                .count(),
            1
        );
        assert_eq!(
            model
                .nodes
                .iter()
                .find(|n| n.key == r"collection:1/individual/d:\other\loose")
                .unwrap()
                .count,
            1
        );
        assert_eq!(
            model
                .nodes
                .iter()
                .find(|n| n.key == r"unassigned/d:\other\loose")
                .unwrap()
                .count,
            2
        );
    }

    #[test]
    fn revealing_file_prefers_active_collection_without_changing_scope() {
        let mut state =
            NavigationState::new(std::env::temp_dir().join("wis-no-navigation-preferences.json"));
        state.model = build_model(&collections_snapshot());
        state.folder = Some(PathBuf::from(r"D:\Other"));
        state.rebuild_visible();
        let before = state.visible.len();
        state.reveal_folder(Path::new(r"C:\Photos\child"), Some(2));
        assert_eq!(state.folder.as_deref(), Some(Path::new(r"D:\Other")));
        assert_eq!(state.scope_revision, 0);
        assert_eq!(
            state.scroll_to.as_deref(),
            Some(r"collection:2/c:\photos\child")
        );
        assert!(state.visible.len() > before);
        assert_eq!(
            state.highlight.as_ref().unwrap().0,
            r"collection:2/c:\photos\child"
        );
        state.reveal_folder(Path::new(r"C:\Photos\child"), None);
        assert_eq!(
            state.scroll_to.as_deref(),
            Some(r"collection:1/c:\photos\child")
        );
    }

    #[test]
    fn follow_and_highlight_can_be_disabled_independently() {
        let mut state =
            NavigationState::new(std::env::temp_dir().join("wis-no-navigation-preferences.json"));
        state.model = build_model(&collections_snapshot());
        state.settings.auto_follow = false;
        state.settings.highlight_folder = true;
        state.reveal_folder(Path::new(r"C:\Photos\child"), Some(1));
        assert!(state.expanded.is_empty());
        assert!(state.scroll_to.is_none());
        assert!(state.highlight.is_some());
        state.highlight = None;
        state.settings.highlight_folder = false;
        state.settings.auto_follow = true;
        state.reveal_folder(Path::new(r"C:\Photos\child"), Some(1));
        assert!(state.highlight.is_none());
        assert!(state.scroll_to.is_some());
    }

    #[test]
    fn large_tree_keeps_closed_subtrees_out_of_visible_rows() {
        let paths = (0..10_000)
            .map(|i| PathBuf::from(format!(r"C:\Photos\folder{i:05}\a.jpg")))
            .collect();
        let snapshot = Snapshot {
            revision: (1, 1, vec![PathBuf::from(r"C:\Photos")], vec![]),
            collections: vec![],
            paths,
        };
        let mut state =
            NavigationState::new(std::env::temp_dir().join("wis-no-navigation-preferences.json"));
        state.model = build_model(&snapshot);
        state.rebuild_visible();
        assert_eq!(state.visible.len(), 2);
        assert_eq!(state.model.nodes[0].count, 10_000);
        state.expanded.insert("unassigned".into());
        state.rebuild_visible();
        assert_eq!(state.visible.len(), 3);
        state.expanded.insert(r"unassigned/c:\photos".into());
        state.rebuild_visible();
        assert_eq!(state.visible.len(), 10_003);
    }

    #[test]
    fn overlapping_roots_have_unique_counts_and_offline_folders_remain() {
        let snapshot = Snapshot {
            revision: (
                1,
                1,
                vec![PathBuf::from(r"C:\Photos")],
                vec![PathBuf::from(r"R:\Offline")],
            ),
            collections: vec![],
            paths: vec![
                PathBuf::from(r"C:\Photos\child\a.jpg"),
                PathBuf::from(r"c:\photos\CHILD\A.jpg"),
            ],
        };
        let model = build_model(&snapshot);
        assert_eq!(model.nodes[0].count, 1);
        assert!(model
            .nodes
            .iter()
            .any(|n| n.unavailable && n.label.contains("Offline")));
        assert_eq!(
            model
                .nodes
                .iter()
                .find(|n| n.label == "child")
                .unwrap()
                .count,
            1
        );
    }
}
