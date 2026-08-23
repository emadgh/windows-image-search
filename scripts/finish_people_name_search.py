from pathlib import Path


def replace_once(path: str, old: str, new: str):
    p = Path(path)
    text = p.read_text(encoding='utf-8')
    if old not in text:
        raise SystemExit(f'anchor missing in {path}: {old[:180]!r}')
    p.write_text(text.replace(old, new, 1), encoding='utf-8')

# face_search.rs: effective People representatives carry display names.
replace_once(
    'src/face_search.rs',
    'use crate::{db, people_store, portable};',
    'use crate::{db, people_effective, portable};',
)
replace_once(
    'src/face_search.rs',
    '    pub bbox: FaceBox,\n    pub group_size: Option<usize>,\n}',
    '    pub bbox: FaceBox,\n    pub group_size: Option<usize>,\n    pub display_name: Option<String>,\n}',
)
start = '''pub fn list_people_representatives(
    session_db_path: &Path,
    roots: &[PathBuf],
    limit: usize,
) -> Result<Vec<IndexedFaceSuggestion>> {'''
end = '''    Ok(suggestions)
}

pub fn resolve_searchable_face('''
p = Path('src/face_search.rs')
text = p.read_text(encoding='utf-8')
si = text.find(start)
ei = text.find(end, si)
if si < 0 or ei < 0:
    raise SystemExit('list_people_representatives function anchors missing')
new_fn = '''pub fn list_people_representatives(
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
    let catalog = people_effective::load(&conn)?;
    if catalog.people.is_empty() {
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
    for person in catalog.people {
        if suggestions.len() >= limit {
            break;
        }
        let Some((library_id, face_id)) = person
            .representative_library_id
            .as_deref()
            .zip(person.representative_face_id.as_deref())
        else {
            continue;
        };
        let Some(root) = roots_by_library.get(library_id) else {
            continue;
        };
        let Ok(root_conn) = open_read_only(root) else {
            continue;
        };
        let Some(mut suggestion) = load_searchable_face_by_id(&root_conn, root, face_id)? else {
            continue;
        };
        suggestion.group_size = Some(person.member_count);
        suggestion.display_name = person.display_name;
        suggestions.push(suggestion);
    }
    Ok(suggestions)
}

pub fn resolve_searchable_face('''
text = text[:si] + new_fn + text[ei + len(end):]
p.write_text(text, encoding='utf-8')

# All non-People face suggestions are unnamed.
replace_once(
    'src/face_search.rs',
    '                bbox,\n                group_size: None,\n            });',
    '                bbox,\n                group_size: None,\n                display_name: None,\n            });',
)
replace_once(
    'src/face_search.rs',
    '        bbox,\n        group_size: None,\n    }))',
    '        bbox,\n        group_size: None,\n        display_name: None,\n    }))',
)

# People-name lookup for the main text-search worker.
p = Path('src/people_filter.rs')
text = p.read_text(encoding='utf-8')
anchor = 'pub fn resolve_filter(\n'
idx = text.find(anchor)
if idx < 0:
    raise SystemExit('people_filter resolve_filter anchor missing')
helper = '''pub fn search_named_people_paths(
    session_db_path: &Path,
    roots: &[PathBuf],
    query: &str,
) -> Result<HashSet<PathBuf>> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() || roots.is_empty() {
        return Ok(HashSet::new());
    }
    let conn = db::open(session_db_path)
        .with_context(|| format!("opening People catalog {}", session_db_path.display()))?;
    let catalog = people_effective::load(&conn)?;
    let matching_ids = catalog
        .people
        .iter()
        .filter_map(|person| {
            let name = person.display_name.as_deref()?.trim();
            (!name.is_empty() && name.to_lowercase().contains(&needle))
                .then(|| person.person_id.clone())
        })
        .collect::<HashSet<_>>();
    if matching_ids.is_empty() {
        return Ok(HashSet::new());
    }

    let face_keys = catalog
        .members
        .iter()
        .filter(|member| {
            !member.ignored
                && !member.detached
                && member
                    .person_id
                    .as_ref()
                    .is_some_and(|id| matching_ids.contains(id))
        })
        .map(|member| (member.library_id.clone(), member.face_id.clone()))
        .collect::<Vec<_>>();
    let resolved = resolve_current_face_paths(roots, face_keys.iter())?;
    Ok(resolved.into_values().collect())
}

'''
text = text[:idx] + helper + text[idx:]
p.write_text(text, encoding='utf-8')

# Text search worker unions normal metadata matches with matching named People paths.
p = Path('src/text_search.rs')
text = p.read_text(encoding='utf-8')
text = text.replace('use crate::db;', 'use crate::{db, people_filter};', 1)
text = text.replace(
    'struct SearchRequest {\n    generation: u64,\n    query: String,\n}',
    'struct SearchRequest {\n    generation: u64,\n    query: String,\n    roots: Vec<PathBuf>,\n}',
    1,
)
old = '''                    let paths = match &connection {
                        Ok(conn) => db::search_text(conn, &request.query)
                            .map(|paths| paths.into_iter().collect())
                            .map_err(|err| format!("{err:#}")),
                        Err(err) => Err(err.clone()),
                    };'''
new = '''                    let paths = match &connection {
                        Ok(conn) => db::search_text(conn, &request.query)
                            .map(|paths| {
                                let mut paths = paths.into_iter().collect::<HashSet<_>>();
                                if let Ok(people_paths) = people_filter::search_named_people_paths(
                                    &db_path,
                                    &request.roots,
                                    &request.query,
                                ) {
                                    paths.extend(people_paths);
                                }
                                paths
                            })
                            .map_err(|err| format!("{err:#}")),
                        Err(err) => Err(err.clone()),
                    };'''
if old not in text:
    raise SystemExit('text search body anchor missing')
text = text.replace(old, new, 1)
text = text.replace(
    '    pub fn request(&self, generation: u64, query: String) {\n        let _ = self.request_tx.send(SearchRequest { generation, query });\n    }',
    '    pub fn request(&self, generation: u64, query: String, roots: Vec<PathBuf>) {\n        let _ = self.request_tx.send(SearchRequest { generation, query, roots });\n    }',
    1,
)
p.write_text(text, encoding='utf-8')

# UI dispatch supplies current roots.
replace_once(
    'src/ui/mod.rs',
    '        self.text_search_service\n            .request(self.text_search_generation, self.search_text.clone());',
    '        self.text_search_service.request(\n            self.text_search_generation,\n            self.search_text.clone(),\n            self.roots.clone(),\n        );',
)

# Face Search gets a named-Person filter and displays names on representative cards.
replace_once(
    'src/ui/face_search_panel.rs',
    '    last_rows_considered: usize,\n}',
    '    last_rows_considered: usize,\n    name_query: String,\n}',
)
replace_once(
    'src/ui/face_search_panel.rs',
    '            last_rows_considered: 0,\n        }',
    '            last_rows_considered: 0,\n            name_query: String::new(),\n        }',
)
replace_once(
    'src/ui/face_search_panel.rs',
    '                ui.separator();\n                ui.strong("People / searchable faces in database");',
    '''                ui.separator();
                ui.strong("People / searchable faces in database");
                ui.add(
                    egui::TextEdit::singleline(&mut self.face_search_ui.name_query)
                        .hint_text("Filter named people…")
                        .desired_width(320.0),
                );''',
)
replace_once(
    'src/ui/face_search_panel.rs',
    '                let suggestions = self.face_search_ui.suggestions.clone();',
    '''                let name_query = self.face_search_ui.name_query.trim().to_lowercase();
                let suggestions = self
                    .face_search_ui
                    .suggestions
                    .iter()
                    .filter(|face| {
                        name_query.is_empty()
                            || face
                                .display_name
                                .as_deref()
                                .is_some_and(|name| name.to_lowercase().contains(&name_query))
                    })
                    .cloned()
                    .collect::<Vec<_>>();''',
)
replace_once(
    'src/ui/face_search_panel.rs',
    '''                                            if let Some(group_size) = face.group_size {
                                                ui.small(format!(
                                                    "Person · {group_size} face{}",
                                                    if group_size == 1 { "" } else { "s" }
                                                ));
                                            } else {''',
    '''                                            if let Some(name) = face.display_name.as_deref() {
                                                ui.strong(truncate(name, 16));
                                            }
                                            if let Some(group_size) = face.group_size {
                                                ui.small(format!(
                                                    "Person · {group_size} face{}",
                                                    if group_size == 1 { "" } else { "s" }
                                                ));
                                            } else {''',
)

# Keep docs honest.
p = Path('docs/people-search-integration.md')
text = p.read_text(encoding='utf-8')
if 'Main text search' not in text:
    text += '''\n## Main text search\n\nThe existing text field unions filename/path/metadata matches with effective named-People matches. Person-name resolution runs on the text-search worker with a snapshot of attached portable roots; the frame loop only checks the resulting in-memory path set.\n\nFace Search also filters representative cards by current manual/effective display name without recomputing embeddings.\n'''
p.write_text(text, encoding='utf-8')
