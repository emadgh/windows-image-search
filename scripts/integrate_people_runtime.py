from pathlib import Path

# --- Face runtime: auto-cluster after SFace and provide explicit rebuild path.
runtime = Path('src/ui/face_runtime.rs')
text = runtime.read_text(encoding='utf-8')
text = text.replace(
    'use crate::face_sface_production;\n',
    'use crate::face_sface_production;\nuse crate::people_clustering::{self, PeopleClusteringOptions, PeopleClusteringSummary};\n',
    1,
)
text = text.replace(
    '    EmbeddingEvent(FaceEmbeddingPipelineEvent),\n    Finished(Result<(FacePipelineSummary, Option<FaceEmbeddingPipelineSummary>), String>),\n',
    '    EmbeddingEvent(FaceEmbeddingPipelineEvent),\n    PeopleFinished(Result<PeopleClusteringSummary, String>),\n    Finished(Result<(FacePipelineSummary, Option<FaceEmbeddingPipelineSummary>), String>),\n',
    1,
)
text = text.replace(
    '    running: bool,\n    run_after_base_index: bool,\n',
    '    running: bool,\n    run_after_base_index: bool,\n    run_people_after_embedding: bool,\n',
    1,
)
text = text.replace(
    '            running: false,\n            run_after_base_index: false,\n',
    '            running: false,\n            run_after_base_index: false,\n            run_people_after_embedding: false,\n',
    1,
)

finished_anchor = '                FaceRuntimeMessage::Finished(result) => {\n'
if finished_anchor not in text:
    raise SystemExit('FaceRuntime Finished arm not found')
people_arm = '''                FaceRuntimeMessage::PeopleFinished(result) => {
                    self.face_runtime.running = false;
                    self.progress = None;
                    self.busy = self.indexing || self.searching;
                    match result {
                        Ok(summary) => {
                            self.status = format!(
                                "People clustering complete: {} group{}, {} clustered face{}, {} outlier{}, {} reused Person ID{}",
                                summary.people_created,
                                if summary.people_created == 1 { "" } else { "s" },
                                summary.faces_clustered,
                                if summary.faces_clustered == 1 { "" } else { "s" },
                                summary.outliers,
                                if summary.outliers == 1 { "" } else { "s" },
                                summary.reused_person_ids,
                                if summary.reused_person_ids == 1 { "" } else { "s" },
                            );
                            self.refresh_face_suggestions();
                        }
                        Err(error) => {
                            self.status = "People clustering failed".to_owned();
                            self.last_error = Some(error);
                        }
                    }
                }
'''
text = text.replace(finished_anchor, people_arm + finished_anchor, 1)

ok_anchor = '                        Ok((detection, embedding)) => {\n'
if ok_anchor not in text:
    raise SystemExit('FaceRuntime successful completion arm not found')
text = text.replace(
    ok_anchor,
    ok_anchor
    + '                            let built_identity_embeddings = embedding.is_some();\n'
    + '                            self.face_runtime.run_people_after_embedding = built_identity_embeddings;\n',
    1,
)

after_face_schedule = '''        if self.face_runtime.run_after_base_index && !self.busy && !self.face_runtime.running {
            self.face_runtime.run_after_base_index = false;
            self.start_face_pipeline();
        }
'''
if after_face_schedule not in text:
    raise SystemExit('post-index face schedule block not found')
text = text.replace(
    after_face_schedule,
    after_face_schedule
    + '''
        if self.face_runtime.run_people_after_embedding
            && !self.busy
            && !self.face_runtime.running
        {
            self.face_runtime.run_people_after_embedding = false;
            self.start_people_rebuild();
        }
''',
    1,
)

method_marker = '    fn apply_face_pipeline_event(&mut self, event: FacePipelineEvent) {\n'
if method_marker not in text:
    raise SystemExit('face pipeline event method marker not found')
people_method = '''    fn start_people_rebuild(&mut self) {
        if self.face_runtime.running || self.busy {
            return;
        }
        if !self.face_embedding_settings.configured() {
            self.status = "SFace model is not configured".to_owned();
            return;
        }
        if !self.face_embedding_settings.model_path.is_file() {
            self.last_error = Some(format!(
                "SFace model path is unavailable: {}",
                self.face_embedding_settings.model_path.display()
            ));
            return;
        }
        if self.roots.is_empty() {
            self.status = "No indexed roots available for People clustering".to_owned();
            return;
        }

        let session_db_path = self.db_path.clone();
        let roots = self.roots.clone();
        let embedding_settings = self.face_embedding_settings.clone();
        let tx = self.face_runtime.tx.clone();
        self.face_runtime.running = true;
        self.busy = true;
        self.progress = None;
        self.last_error = None;
        self.status = "People: clustering current SFace embeddings…".to_owned();

        std::thread::spawn(move || {
            let result = face_sface_production::embedding_revision(&embedding_settings)
                .and_then(|revision| {
                    people_clustering::run(
                        &session_db_path,
                        &roots,
                        &revision,
                        PeopleClusteringOptions::default(),
                    )
                })
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(FaceRuntimeMessage::PeopleFinished(result));
        });
    }

'''
text = text.replace(method_marker, people_method + method_marker, 1)

button_anchor = '''            if ui.add_enabled(can_run, egui::Button::new(label)).clicked() {
                self.start_face_pipeline();
            }
'''
if button_anchor not in text:
    raise SystemExit('face run button block not found')
text = text.replace(
    button_anchor,
    button_anchor
    + '''
            let can_rebuild_people = self.face_embedding_settings.configured()
                && self.face_embedding_settings.model_path.is_file()
                && !self.roots.is_empty();
            if ui
                .add_enabled(
                    can_rebuild_people,
                    egui::Button::new("Rebuild People groups from current embeddings"),
                )
                .clicked()
            {
                self.start_people_rebuild();
            }
''',
    1,
)
text = text.replace(
    'ui.small("YuNet/SFace face pipeline is running in the background.");',
    'ui.small("Face/People maintenance is running in the background.");',
    1,
)
runtime.write_text(text, encoding='utf-8')

# --- Face search data source: prefer unique People representatives, fallback to raw face instances.
search = Path('src/face_search.rs')
text = search.read_text(encoding='utf-8')
text = text.replace(
    'use crate::portable;\n',
    'use crate::{db, people_store, portable};\n',
    1,
)
text = text.replace(
    'use rusqlite::{params, Connection, OpenFlags};\n',
    'use rusqlite::{params, Connection, OpenFlags, OptionalExtension};\nuse std::collections::HashMap;\n',
    1,
)
text = text.replace(
    '    pub confidence: f32,\n    pub bbox: FaceBox,\n}\n\n#[derive(Clone, Copy, Debug, PartialEq)]\npub struct IndexedFaceSearchOptions',
    '    pub confidence: f32,\n    pub bbox: FaceBox,\n    pub group_size: Option<usize>,\n}\n\n#[derive(Clone, Copy, Debug, PartialEq)]\npub struct IndexedFaceSearchOptions',
    1,
)

list_marker = 'pub fn list_searchable_faces(\n'
if list_marker not in text:
    raise SystemExit('list_searchable_faces marker not found')
people_list = r'''pub fn list_people_representatives(
    session_db_path: &Path,
    roots: &[PathBuf],
    limit: usize,
) -> Result<Vec<IndexedFaceSuggestion>> {
    let limit = limit.clamp(1, 2_000);
    if roots.is_empty() {
        return Ok(Vec::new());
    }

    let conn = db::open(session_db_path)
        .with_context(|| format!("opening People catalog {}", session_db_path.display()))?;
    people_store::ensure_schema(&conn)?;
    let clusters = people_store::load_clusters(&conn)?;
    if clusters.is_empty() {
        return Ok(Vec::new());
    }

    let mut roots_by_library: HashMap<String, PathBuf> = HashMap::new();
    for root in roots {
        let Ok(root_conn) = open_read_only(root) else {
            continue;
        };
        let Ok(library_id) = portable_library_id(&root_conn) else {
            continue;
        };
        roots_by_library.entry(library_id).or_insert_with(|| root.clone());
    }

    let mut suggestions = Vec::new();
    for cluster in clusters {
        if suggestions.len() >= limit {
            break;
        }
        let Some(root) = roots_by_library.get(&cluster.representative_library_id) else {
            continue;
        };
        let Ok(root_conn) = open_read_only(root) else {
            continue;
        };
        let Some(mut suggestion) = load_searchable_face_by_id(
            &root_conn,
            root,
            &cluster.representative_face_id,
        )? else {
            continue;
        };
        suggestion.group_size = Some(cluster.member_count);
        suggestions.push(suggestion);
    }
    Ok(suggestions)
}

'''
text = text.replace(list_marker, people_list + list_marker, 1)

constructor = '''            suggestions.push(IndexedFaceSuggestion {
                root: root.clone(),
                face_id,
                image_path,
                ordinal,
                confidence,
                bbox,
            });
'''
if constructor not in text:
    raise SystemExit('face suggestion constructor not found')
text = text.replace(
    constructor,
    '''            suggestions.push(IndexedFaceSuggestion {
                root: root.clone(),
                face_id,
                image_path,
                ordinal,
                confidence,
                bbox,
                group_size: None,
            });
''',
    1,
)

helper_marker = 'fn open_read_only(root: &Path) -> Result<Connection> {\n'
if helper_marker not in text:
    raise SystemExit('open_read_only helper marker not found')
helpers = r'''fn portable_library_id(conn: &Connection) -> Result<String> {
    let library_id = conn
        .query_row(
            "SELECT value FROM portable_meta WHERE key = 'library_id'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .context("portable index has no library_id")?;
    if library_id.trim().is_empty() {
        bail!("portable index library_id is empty");
    }
    Ok(library_id)
}

fn load_searchable_face_by_id(
    conn: &Connection,
    root: &Path,
    face_id: &str,
) -> Result<Option<IndexedFaceSuggestion>> {
    let row = conn
        .query_row(
            r#"
            SELECT f.face_id, f.image_path, f.face_ordinal, f.confidence,
                   f.bbox_x, f.bbox_y, f.bbox_width, f.bbox_height
            FROM faces f
            JOIN face_detection_state s ON s.image_path = f.image_path
            JOIN images i ON i.path = f.image_path
            JOIN face_embeddings e ON e.face_id = f.face_id
            WHERE f.face_id = ?1
              AND s.detector_id = f.detector_id
              AND s.detector_version = f.detector_version
              AND s.detector_cache_revision = f.detector_cache_revision
              AND s.schema_version = f.schema_version
              AND s.source_size = f.source_size
              AND s.source_modified = f.source_modified
              AND i.size = f.source_size
              AND i.modified = f.source_modified
              AND e.normalized = 1
              AND e.detector_id = f.detector_id
              AND e.detector_version = f.detector_version
              AND e.detector_cache_revision = f.detector_cache_revision
              AND e.detection_schema_version = f.schema_version
              AND e.source_size = f.source_size
              AND e.source_modified = f.source_modified
            LIMIT 1
            "#,
            params![face_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?.max(0) as usize,
                    row.get::<_, f32>(3)?,
                    FaceBox {
                        x: row.get(4)?,
                        y: row.get(5)?,
                        width: row.get(6)?,
                        height: row.get(7)?,
                    },
                ))
            },
        )
        .optional()?;
    let Some((face_id, relative, ordinal, confidence, bbox)) = row else {
        return Ok(None);
    };
    let image_path = portable::absolute_source_path(root, Path::new(&relative))?;
    Ok(Some(IndexedFaceSuggestion {
        root: root.to_path_buf(),
        face_id,
        image_path,
        ordinal,
        confidence,
        bbox,
        group_size: None,
    }))
}

'''
text = text.replace(helper_marker, helpers + helper_marker, 1)
search.write_text(text, encoding='utf-8')

# --- Face Search panel: prefer People cards and expose group size.
panel = Path('src/ui/face_search_panel.rs')
text = panel.read_text(encoding='utf-8')
text = text.replace('    fn refresh_face_suggestions(&mut self) {\n', '    pub(super) fn refresh_face_suggestions(&mut self) {\n', 1)
old_refresh = '''        let roots = self.roots.clone();
        let tx = self.face_search_ui.tx.clone();
        self.face_search_ui.loading = true;
        self.status = "Loading searchable faces from portable indexes…".to_owned();
        std::thread::spawn(move || {
            let result = face_search::list_searchable_faces(&roots, DEFAULT_SUGGESTION_LIMIT)
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(FaceSearchUiMessage::Suggestions(result));
        });
'''
if old_refresh not in text:
    raise SystemExit('face suggestion refresh block not found')
new_refresh = '''        let roots = self.roots.clone();
        let session_db_path = self.db_path.clone();
        let tx = self.face_search_ui.tx.clone();
        self.face_search_ui.loading = true;
        self.status = "Loading People / searchable face suggestions…".to_owned();
        std::thread::spawn(move || {
            let result = face_search::list_people_representatives(
                &session_db_path,
                &roots,
                DEFAULT_SUGGESTION_LIMIT,
            )
            .and_then(|people| {
                if people.is_empty() {
                    face_search::list_searchable_faces(&roots, DEFAULT_SUGGESTION_LIMIT)
                } else {
                    Ok(people)
                }
            })
            .map_err(|err| format!("{err:#}"));
            let _ = tx.send(FaceSearchUiMessage::Suggestions(result));
        });
'''
text = text.replace(old_refresh, new_refresh, 1)
text = text.replace('egui::Button::new("⟳ Refresh faces")', 'egui::Button::new("⟳ Refresh people/faces")', 1)
text = text.replace(
    '"Database suggestions are face instances, not unique people yet. People clustering will group repeated appearances in the next stage.",',
    '"Database suggestions prefer one representative per automatic Person group. Before a People snapshot exists, they fall back to individual face instances.",',
    1,
)
text = text.replace('ui.strong("Searchable faces in database");', 'ui.strong("People / searchable faces in database");', 1)
text = text.replace('egui::Button::new("Search selected database face")', 'egui::Button::new("Search selected person/face")', 1)
old_label = '                                            ui.small(format!("{:.0}%", face.confidence * 100.0));\n'
if old_label not in text:
    raise SystemExit('database card confidence label not found')
new_label = '''                                            if let Some(group_size) = face.group_size {
                                                ui.small(format!(
                                                    "Person · {group_size} face{}",
                                                    if group_size == 1 { "" } else { "s" }
                                                ));
                                            } else {
                                                ui.small(format!("{:.0}%", face.confidence * 100.0));
                                            }
'''
text = text.replace(old_label, new_label, 1)
panel.write_text(text, encoding='utf-8')
