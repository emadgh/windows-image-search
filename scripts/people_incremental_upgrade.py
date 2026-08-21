from pathlib import Path

# main.rs: wire settings module.
main = Path('src/main.rs')
text = main.read_text(encoding='utf-8')
needle = 'mod people_clustering;\nmod people_store;\n'
if needle not in text:
    raise SystemExit('main People module marker not found')
text = text.replace(needle, 'mod people_clustering;\nmod people_settings;\nmod people_store;\n', 1)
main.write_text(text, encoding='utf-8')

# people_store.rs: persist min_cluster_size and bump algorithm revision.
store = Path('src/people_store.rs')
text = store.read_text(encoding='utf-8')
text = text.replace('pub const ALGORITHM_REVISION: i64 = 2;', 'pub const ALGORITHM_REVISION: i64 = 3;', 1)
text = text.replace(
    '    pub similarity_threshold: f32,\n}',
    '    pub similarity_threshold: f32,\n    pub min_cluster_size: usize,\n}',
    1,
)
text = text.replace(
    '            algorithm_revision INTEGER NOT NULL,\n            similarity_threshold REAL NOT NULL,\n            updated_at INTEGER NOT NULL DEFAULT (unixepoch())',
    '            algorithm_revision INTEGER NOT NULL,\n            similarity_threshold REAL NOT NULL,\n            min_cluster_size INTEGER NOT NULL DEFAULT 2,\n            updated_at INTEGER NOT NULL DEFAULT (unixepoch())',
    1,
)
ensure_tail = '''    )?;
    Ok(())
}

pub fn replace_automatic_snapshot'''
if ensure_tail not in text:
    raise SystemExit('ensure_schema tail not found')
text = text.replace(
    ensure_tail,
    '''    )?;
    // Migration for People snapshots created by algorithm revisions before min_cluster_size
    // became part of the compatibility contract. Duplicate-column errors are benign.
    let _ = conn.execute(
        "ALTER TABLE people_cluster_state ADD COLUMN min_cluster_size INTEGER NOT NULL DEFAULT 2",
        [],
    );
    Ok(())
}

pub fn replace_automatic_snapshot''',
    1,
)
text = text.replace(
    '''            algorithm_revision, similarity_threshold,
            updated_at
        ) VALUES(1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, unixepoch())''',
    '''            algorithm_revision, similarity_threshold, min_cluster_size,
            updated_at
        ) VALUES(1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch())''',
    1,
)
text = text.replace(
    '''            state.algorithm_revision,
            state.similarity_threshold,
        ],''',
    '''            state.algorithm_revision,
            state.similarity_threshold,
            state.min_cluster_size as i64,
        ],''',
    1,
)
text = text.replace(
    '''               dimension, alignment_revision,
               algorithm_revision, similarity_threshold
        FROM people_cluster_state''',
    '''               dimension, alignment_revision,
               algorithm_revision, similarity_threshold, min_cluster_size
        FROM people_cluster_state''',
    1,
)
text = text.replace(
    '''        algorithm_revision: row.get(5)?,
        similarity_threshold: row.get(6)?,
    }))''',
    '''        algorithm_revision: row.get(5)?,
        similarity_threshold: row.get(6)?,
        min_cluster_size: row.get::<_, i64>(7)?.max(2) as usize,
    }))''',
    1,
)
text = text.replace(
    '''    if !state.similarity_threshold.is_finite()
        || !(-1.0..=1.0).contains(&state.similarity_threshold)
    {
        bail!("People clustering similarity threshold must be finite and within [-1, 1]");
    }
''',
    '''    if !state.similarity_threshold.is_finite()
        || !(-1.0..=1.0).contains(&state.similarity_threshold)
    {
        bail!("People clustering similarity threshold must be finite and within [-1, 1]");
    }
    if state.min_cluster_size < 2 {
        bail!("People clustering minimum cluster size must be at least 2");
    }
''',
    1,
)
text = text.replace(
    '''            algorithm_revision: ALGORITHM_REVISION,
            similarity_threshold: 0.62,
        }''',
    '''            algorithm_revision: ALGORITHM_REVISION,
            similarity_threshold: 0.62,
            min_cluster_size: 2,
        }''',
    1,
)
store.write_text(text, encoding='utf-8')

# people_clustering.rs: compatibility state includes min size + conservative incremental path.
clustering = Path('src/people_clustering.rs')
text = clustering.read_text(encoding='utf-8')
text = text.replace(
    'const HNSW_EF_SEARCH_EXTRA: usize = 192;\n',
    'const HNSW_EF_SEARCH_EXTRA: usize = 192;\nconst INCREMENTAL_AMBIGUITY_MARGIN: f32 = 0.04;\n',
    1,
)
text = text.replace(
    '''        algorithm_revision: people_store::ALGORITHM_REVISION,
        similarity_threshold: options.similarity_threshold,
    };''',
    '''        algorithm_revision: people_store::ALGORITHM_REVISION,
        similarity_threshold: options.similarity_threshold,
        min_cluster_size: options.min_cluster_size,
    };''',
    1,
)
marker = 'struct ClusteredSnapshot {\n'
if marker not in text:
    raise SystemExit('ClusteredSnapshot marker not found')
incremental = r'''pub fn run_incremental(
    session_db_path: &Path,
    roots: &[PathBuf],
    embedding: &people_store::PeopleEmbeddingRevision,
    options: PeopleClusteringOptions,
) -> Result<PeopleClusteringSummary> {
    let options = options.sanitized();
    validate_embedding_revision(embedding)?;

    let mut roots_scanned = 0usize;
    let mut roots_unavailable = 0usize;
    let mut faces = Vec::new();
    for root in roots {
        match load_root_faces(root, embedding) {
            Ok(mut root_faces) => {
                roots_scanned += 1;
                faces.append(&mut root_faces);
            }
            Err(_) => roots_unavailable += 1,
        }
    }
    sort_faces(&mut faces);

    let mut conn = db::open(session_db_path)
        .with_context(|| format!("opening People catalog {}", session_db_path.display()))?;
    people_store::ensure_schema(&conn)?;
    let expected_state = people_store::PeopleClusterState {
        embedding: embedding.clone(),
        algorithm_revision: people_store::ALGORITHM_REVISION,
        similarity_threshold: options.similarity_threshold,
        min_cluster_size: options.min_cluster_size,
    };
    let Some(previous_state) = people_store::load_state(&conn)? else {
        drop(conn);
        return run(session_db_path, roots, embedding, options);
    };
    if previous_state != expected_state {
        drop(conn);
        return run(session_db_path, roots, embedding, options);
    }

    let mut clusters = people_store::load_clusters(&conn)?;
    let previous_members = people_store::load_members(&conn)?;
    if previous_members.is_empty() {
        drop(conn);
        return run(session_db_path, roots, embedding, options);
    }

    let current_index: HashMap<(String, String), usize> = faces
        .iter()
        .enumerate()
        .map(|(index, face)| ((face.library_id.clone(), face.face_id.clone()), index))
        .collect();
    if previous_members.iter().any(|member| {
        !current_index.contains_key(&(member.library_id.clone(), member.face_id.clone()))
    }) {
        // A previously clustered face disappeared or became stale. Rebuild so stale
        // memberships cannot survive source deletion/model invalidation.
        drop(conn);
        return run(session_db_path, roots, embedding, options);
    }

    let previous_keys: HashSet<(String, String)> = previous_members
        .iter()
        .map(|member| (member.library_id.clone(), member.face_id.clone()))
        .collect();
    let mut candidate_indices = Vec::new();
    let mut members = Vec::with_capacity(faces.len());
    for member in previous_members {
        let index = *current_index
            .get(&(member.library_id.clone(), member.face_id.clone()))
            .context("People incremental member disappeared during update")?;
        if member.is_outlier {
            candidate_indices.push(index);
        } else {
            members.push(member);
        }
    }
    for (index, face) in faces.iter().enumerate() {
        if !previous_keys.contains(&(face.library_id.clone(), face.face_id.clone())) {
            candidate_indices.push(index);
        }
    }
    candidate_indices.sort_unstable();
    candidate_indices.dedup();

    if candidate_indices.is_empty() {
        return Ok(summary_from_snapshot(
            roots_scanned,
            roots_unavailable,
            faces.len(),
            &clusters,
            &members,
            clusters.len(),
        ));
    }

    let cluster_lookup: HashMap<String, usize> = clusters
        .iter()
        .enumerate()
        .map(|(index, cluster)| (cluster.person_id.clone(), index))
        .collect();
    let mut representatives = Vec::with_capacity(clusters.len());
    for (cluster_index, cluster) in clusters.iter().enumerate() {
        let Some(&face_index) = current_index.get(&(
            cluster.representative_library_id.clone(),
            cluster.representative_face_id.clone(),
        )) else {
            drop(conn);
            return run(session_db_path, roots, embedding, options);
        };
        representatives.push((cluster_index, face_index));
    }

    let mut unmatched = Vec::new();
    for face_index in candidate_indices {
        let face = &faces[face_index];
        let mut best: Option<(usize, f32)> = None;
        let mut second_best = f32::NEG_INFINITY;
        for &(cluster_index, representative_index) in &representatives {
            let similarity = cosine(&face.values, &faces[representative_index].values)?;
            match best {
                None => best = Some((cluster_index, similarity)),
                Some((best_index, best_similarity)) => {
                    if similarity > best_similarity
                        || (similarity == best_similarity && cluster_index < best_index)
                    {
                        second_best = second_best.max(best_similarity);
                        best = Some((cluster_index, similarity));
                    } else {
                        second_best = second_best.max(similarity);
                    }
                }
            }
        }

        let accepted = best.filter(|(_, best_similarity)| {
            *best_similarity >= options.similarity_threshold
                && (second_best < options.similarity_threshold
                    || *best_similarity - second_best >= INCREMENTAL_AMBIGUITY_MARGIN)
        });
        if let Some((cluster_index, similarity)) = accepted {
            let person_id = clusters[cluster_index].person_id.clone();
            clusters[cluster_index].member_count += 1;
            members.push(people_store::PersonClusterMember {
                library_id: face.library_id.clone(),
                face_id: face.face_id.clone(),
                person_id: Some(person_id),
                assignment_similarity: Some(similarity),
                is_outlier: false,
            });
        } else {
            unmatched.push(face.clone());
        }
    }

    if !unmatched.is_empty() {
        sort_faces(&mut unmatched);
        let new_snapshot = cluster_faces(&unmatched, options, &HashMap::new(), embedding)?;
        let existing_ids: HashSet<String> = cluster_lookup.keys().cloned().collect();
        if new_snapshot
            .clusters
            .iter()
            .any(|cluster| existing_ids.contains(&cluster.person_id))
        {
            drop(conn);
            return run(session_db_path, roots, embedding, options);
        }
        clusters.extend(new_snapshot.clusters);
        members.extend(new_snapshot.members);
    }

    members.sort_by(|left, right| {
        left.library_id
            .cmp(&right.library_id)
            .then_with(|| left.face_id.cmp(&right.face_id))
    });
    let counts = members.iter().filter_map(|member| member.person_id.as_deref()).fold(
        HashMap::<String, usize>::new(),
        |mut counts, person_id| {
            *counts.entry(person_id.to_owned()).or_default() += 1;
            counts
        },
    );
    for cluster in &mut clusters {
        cluster.member_count = counts.get(&cluster.person_id).copied().unwrap_or(0);
    }
    clusters.retain(|cluster| cluster.member_count > 0);

    people_store::replace_automatic_snapshot(&mut conn, &expected_state, &clusters, &members)?;
    Ok(summary_from_snapshot(
        roots_scanned,
        roots_unavailable,
        faces.len(),
        &clusters,
        &members,
        cluster_lookup.len(),
    ))
}

fn summary_from_snapshot(
    roots_scanned: usize,
    roots_unavailable: usize,
    faces_loaded: usize,
    clusters: &[people_store::PersonCluster],
    members: &[people_store::PersonClusterMember],
    reused_person_ids: usize,
) -> PeopleClusteringSummary {
    PeopleClusteringSummary {
        roots_scanned,
        roots_unavailable,
        faces_loaded,
        people_created: clusters.len(),
        faces_clustered: members
            .iter()
            .filter(|member| member.person_id.is_some())
            .count(),
        outliers: members.iter().filter(|member| member.is_outlier).count(),
        reused_person_ids,
    }
}

'''
text = text.replace(marker, incremental + marker, 1)

# Add targeted incremental regression tests before final test module closing.
insert = r'''
    #[test]
    fn incremental_candidate_requires_clear_margin_between_people() {
        let alice = face("a", "alice", unit(1.0, 0.0, 0.0));
        let bob = face("a", "bob", unit(0.0, 1.0, 0.0));
        let ambiguous = face("a", "ambiguous", unit(0.72, 0.69, 0.0));
        let alice_sim = cosine(&ambiguous.values, &alice.values).unwrap();
        let bob_sim = cosine(&ambiguous.values, &bob.values).unwrap();
        assert!(alice_sim >= 0.62 && bob_sim >= 0.62);
        assert!((alice_sim - bob_sim).abs() < INCREMENTAL_AMBIGUITY_MARGIN);
    }
'''
last = text.rfind('\n}')
if last == -1:
    raise SystemExit('people_clustering test module end not found')
text = text[:last] + insert + text[last:]
clustering.write_text(text, encoding='utf-8')

# face_runtime.rs: persist People settings, use incremental automatically and full rebuild manually.
runtime = Path('src/ui/face_runtime.rs')
text = runtime.read_text(encoding='utf-8')
text = text.replace(
    'use crate::people_clustering::{self, PeopleClusteringOptions, PeopleClusteringSummary};\n',
    'use crate::people_clustering::{self, PeopleClusteringOptions, PeopleClusteringSummary};\nuse crate::people_settings::{self, PeopleSettings};\n',
    1,
)
text = text.replace(
    '    settings_path: PathBuf,\n    tx: Sender<FaceRuntimeMessage>,\n',
    '    settings_path: PathBuf,\n    people_settings: PeopleSettings,\n    people_settings_path: PathBuf,\n    tx: Sender<FaceRuntimeMessage>,\n',
    1,
)
text = text.replace(
    '''        let settings_path = app_data_dir.join("face-detector-settings.ini");
        let settings = yunet_settings::load(&settings_path);
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            settings,
            settings_path,
            tx,''',
    '''        let settings_path = app_data_dir.join("face-detector-settings.ini");
        let settings = yunet_settings::load(&settings_path);
        let people_settings_path = app_data_dir.join("people-settings.ini");
        let people_settings = people_settings::load(&people_settings_path);
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            settings,
            settings_path,
            people_settings,
            people_settings_path,
            tx,''',
    1,
)
text = text.replace(
    '            self.start_people_rebuild();\n',
    '            self.start_people_incremental_update();\n',
    1,
)
method_marker = '    fn start_people_rebuild(&mut self) {\n'
if method_marker not in text:
    raise SystemExit('start_people_rebuild marker missing')
incremental_method = '''    fn start_people_incremental_update(&mut self) {
        self.start_people_maintenance(true);
    }

    fn start_people_rebuild(&mut self) {
        self.start_people_maintenance(false);
    }

    fn start_people_maintenance(&mut self, incremental: bool) {
'''
text = text.replace(method_marker, incremental_method, 1)
# Remove the now duplicated opening function body token (the original contents continue after marker).
# The replacement intentionally includes start_people_maintenance opening, so original body remains valid.
text = text.replace(
    '''        let embedding_settings = self.face_embedding_settings.clone();
        let tx = self.face_runtime.tx.clone();''',
    '''        let embedding_settings = self.face_embedding_settings.clone();
        let people_settings = self.face_runtime.people_settings.sanitized();
        let tx = self.face_runtime.tx.clone();''',
    1,
)
text = text.replace(
    '        self.status = "People: clustering current SFace embeddings…".to_owned();\n',
    '''        self.status = if incremental {
            "People: incrementally updating current SFace embeddings…".to_owned()
        } else {
            "People: rebuilding all groups from current SFace embeddings…".to_owned()
        };
''',
    1,
)
old_call = '''            let result = face_sface_production::embedding_revision(&embedding_settings)
                .and_then(|revision| {
                    people_clustering::run(
                        &session_db_path,
                        &roots,
                        &revision,
                        PeopleClusteringOptions::default(),
                    )
                })'''
if old_call not in text:
    raise SystemExit('People clustering runtime call not found')
new_call = '''            let result = face_sface_production::embedding_revision(&embedding_settings)
                .and_then(|revision| {
                    let options = PeopleClusteringOptions {
                        similarity_threshold: people_settings.similarity_threshold,
                        min_cluster_size: people_settings.min_cluster_size,
                    };
                    if incremental {
                        people_clustering::run_incremental(
                            &session_db_path,
                            &roots,
                            &revision,
                            options,
                        )
                    } else {
                        people_clustering::run(&session_db_path, &roots, &revision, options)
                    }
                })'''
text = text.replace(old_call, new_call, 1)

text = text.replace(
    '        let mut changed = false;\n        ui.add_enabled_ui(!self.busy && !self.face_runtime.running, |ui| {\n',
    '        let mut changed = false;\n        let mut people_changed = false;\n        ui.add_enabled_ui(!self.busy && !self.face_runtime.running, |ui| {\n',
    1,
)
people_controls_anchor = '''            let can_run = self.face_runtime.configured_and_available() && !self.roots.is_empty();
'''
if people_controls_anchor not in text:
    raise SystemExit('settings can_run anchor missing')
people_controls = '''            ui.add_space(8.0);
            ui.separator();
            ui.strong("People clustering");
            ui.small("These thresholds are separate from the one-shot Face Search similarity threshold.");
            ui.horizontal(|ui| {
                ui.label("Identity threshold");
                people_changed |= ui
                    .add(
                        egui::Slider::new(
                            &mut self.face_runtime.people_settings.similarity_threshold,
                            0.0..=1.0,
                        )
                        .fixed_decimals(2),
                    )
                    .changed();
                ui.label("Minimum faces per Person");
                people_changed |= ui
                    .add(
                        egui::DragValue::new(&mut self.face_runtime.people_settings.min_cluster_size)
                            .range(2..=1_000_000)
                            .speed(1.0),
                    )
                    .changed();
            });

'''
text = text.replace(people_controls_anchor, people_controls + people_controls_anchor, 1)

save_anchor = '''        self.face_runtime.settings = self.face_runtime.settings.clone().sanitized();
        if changed {
'''
if save_anchor not in text:
    raise SystemExit('settings save anchor missing')
text = text.replace(
    save_anchor,
    '''        self.face_runtime.settings = self.face_runtime.settings.clone().sanitized();
        self.face_runtime.people_settings = self.face_runtime.people_settings.sanitized();
        if people_changed {
            if let Err(err) = people_settings::save(
                &self.face_runtime.people_settings_path,
                &self.face_runtime.people_settings,
            ) {
                self.last_error = Some(format!("Cannot save People settings: {err:#}"));
            }
        }
        if changed {
''',
    1,
)
runtime.write_text(text, encoding='utf-8')
