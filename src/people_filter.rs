use crate::{db, people_effective, portable};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PeopleFilterMode {
    #[default]
    Any,
    All,
}

impl PeopleFilterMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Any => "ANY selected person",
            Self::All => "ALL selected people",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedPersonOption {
    pub person_id: String,
    pub display_name: String,
    pub member_count: usize,
    pub representative_image: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct ResolvedPeopleFilter {
    pub selected_person_ids: Vec<String>,
    pub mode: PeopleFilterMode,
    pub images_by_person: HashMap<String, HashSet<PathBuf>>,
    pub matching_images: HashSet<PathBuf>,
    pub unavailable_faces: usize,
}

impl ResolvedPeopleFilter {
    pub fn active(&self) -> bool {
        !self.selected_person_ids.is_empty()
    }

    pub fn matches(&self, image_path: &Path) -> bool {
        !self.active() || self.matching_images.contains(image_path)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageNamedPerson {
    pub person_id: String,
    pub display_name: String,
    pub face_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImagePeopleDetail {
    pub detected_face_count: usize,
    pub named_people: Vec<ImageNamedPerson>,
}

pub fn load_named_people(
    session_db_path: &Path,
    roots: &[PathBuf],
) -> Result<Vec<NamedPersonOption>> {
    let conn = db::open(session_db_path)
        .with_context(|| format!("opening People catalog {}", session_db_path.display()))?;
    let catalog = people_effective::load(&conn)?;
    let mut people = Vec::new();
    for person in catalog.people {
        let Some(display_name) = person
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
        else {
            continue;
        };
        let representative_image = person
            .representative_library_id
            .as_deref()
            .zip(person.representative_face_id.as_deref())
            .and_then(|(library_id, face_id)| {
                crate::face_search::resolve_searchable_face(roots, library_id, face_id)
                    .ok()
                    .flatten()
                    .map(|face| face.image_path)
            });
        people.push(NamedPersonOption {
            person_id: person.person_id,
            display_name,
            member_count: person.member_count,
            representative_image,
        });
    }
    people.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
            .then_with(|| left.person_id.cmp(&right.person_id))
    });
    Ok(people)
}

pub fn resolve_filter(
    session_db_path: &Path,
    roots: &[PathBuf],
    selected_person_ids: &[String],
    mode: PeopleFilterMode,
) -> Result<ResolvedPeopleFilter> {
    let requested = selected_person_ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if requested.is_empty() {
        return Ok(ResolvedPeopleFilter {
            mode,
            ..ResolvedPeopleFilter::default()
        });
    }

    let conn = db::open(session_db_path)
        .with_context(|| format!("opening People catalog {}", session_db_path.display()))?;
    let catalog = people_effective::load(&conn)?;
    let existing_ids = catalog
        .people
        .iter()
        .map(|person| person.person_id.as_str())
        .collect::<HashSet<_>>();
    let selected_person_ids = requested
        .into_iter()
        .filter(|id| existing_ids.contains(id.as_str()))
        .collect::<Vec<_>>();
    if selected_person_ids.is_empty() {
        return Ok(ResolvedPeopleFilter {
            mode,
            ..ResolvedPeopleFilter::default()
        });
    }
    let selected = selected_person_ids.iter().cloned().collect::<HashSet<_>>();

    let mut owners_by_face: HashMap<(String, String), Vec<String>> = HashMap::new();
    for member in &catalog.members {
        let Some(person_id) = member.person_id.as_ref() else {
            continue;
        };
        if !selected.contains(person_id) || member.ignored || member.detached {
            continue;
        }
        owners_by_face
            .entry((member.library_id.clone(), member.face_id.clone()))
            .or_default()
            .push(person_id.clone());
    }

    let resolved_paths = resolve_current_face_paths(roots, owners_by_face.keys())?;
    let mut images_by_person = selected_person_ids
        .iter()
        .cloned()
        .map(|id| (id, HashSet::new()))
        .collect::<HashMap<_, _>>();
    for (face_key, path) in &resolved_paths {
        if let Some(owners) = owners_by_face.get(face_key) {
            for owner in owners {
                images_by_person
                    .entry(owner.clone())
                    .or_default()
                    .insert(path.clone());
            }
        }
    }
    let matching_images = combine_match_sets(&selected_person_ids, &images_by_person, mode);

    Ok(ResolvedPeopleFilter {
        selected_person_ids,
        mode,
        images_by_person,
        matching_images,
        unavailable_faces: owners_by_face.len().saturating_sub(resolved_paths.len()),
    })
}

pub fn load_image_people_detail(
    session_db_path: &Path,
    roots: &[PathBuf],
    image_path: &Path,
) -> Result<ImagePeopleDetail> {
    let Some(root) = portable::indexed_root_for_path(image_path, roots) else {
        return Ok(ImagePeopleDetail::default());
    };
    let conn = open_portable_read_only(root)?;
    let library_id = portable_library_id(&conn)?;
    let relative = portable::relative_source_path(root, image_path)?;
    let relative_text = relative.to_string_lossy().to_string();

    let mut stmt = conn.prepare(
        r#"
        SELECT f.face_id
        FROM faces f
        JOIN face_detection_state s ON s.image_path = f.image_path
        JOIN images i ON i.path = f.image_path
        WHERE f.image_path = ?1
          AND s.detector_id = f.detector_id
          AND s.detector_version = f.detector_version
          AND s.detector_cache_revision = f.detector_cache_revision
          AND s.schema_version = f.schema_version
          AND s.source_size = f.source_size
          AND s.source_modified = f.source_modified
          AND i.size = f.source_size
          AND i.modified = f.source_modified
        ORDER BY f.face_ordinal ASC
        "#,
    )?;
    let face_ids = stmt
        .query_map([relative_text], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if face_ids.is_empty() {
        return Ok(ImagePeopleDetail::default());
    }

    let session = db::open(session_db_path)?;
    let catalog = people_effective::load(&session)?;
    let name_by_person = catalog
        .people
        .iter()
        .filter_map(|person| {
            let name = person.display_name.as_ref()?.trim();
            (!name.is_empty()).then(|| (person.person_id.as_str(), name.to_owned()))
        })
        .collect::<HashMap<_, _>>();
    let face_set = face_ids.iter().map(String::as_str).collect::<HashSet<_>>();
    let mut count_by_person = BTreeMap::<String, (String, usize)>::new();
    for member in &catalog.members {
        if member.library_id != library_id
            || member.ignored
            || member.detached
            || !face_set.contains(member.face_id.as_str())
        {
            continue;
        }
        let Some(person_id) = member.person_id.as_ref() else {
            continue;
        };
        let Some(display_name) = name_by_person.get(person_id.as_str()) else {
            continue;
        };
        let entry = count_by_person
            .entry(person_id.clone())
            .or_insert_with(|| (display_name.clone(), 0));
        entry.1 += 1;
    }

    let mut named_people = count_by_person
        .into_iter()
        .map(|(person_id, (display_name, face_count))| ImageNamedPerson {
            person_id,
            display_name,
            face_count,
        })
        .collect::<Vec<_>>();
    named_people.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
            .then_with(|| left.person_id.cmp(&right.person_id))
    });

    Ok(ImagePeopleDetail {
        detected_face_count: face_ids.len(),
        named_people,
    })
}

fn resolve_current_face_paths<'a>(
    roots: &[PathBuf],
    face_keys: impl Iterator<Item = &'a (String, String)>,
) -> Result<HashMap<(String, String), PathBuf>> {
    let mut faces_by_library = HashMap::<String, Vec<String>>::new();
    for (library_id, face_id) in face_keys {
        faces_by_library
            .entry(library_id.clone())
            .or_default()
            .push(face_id.clone());
    }
    for face_ids in faces_by_library.values_mut() {
        face_ids.sort();
        face_ids.dedup();
    }

    let mut resolved = HashMap::new();
    for root in roots {
        let Ok(conn) = open_portable_read_only(root) else {
            continue;
        };
        let Ok(library_id) = portable_library_id(&conn) else {
            continue;
        };
        let Some(face_ids) = faces_by_library.get(&library_id) else {
            continue;
        };
        let mut stmt = conn.prepare(
            r#"
            SELECT f.image_path
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
        )?;
        for face_id in face_ids {
            let relative = stmt
                .query_row([face_id], |row| row.get::<_, String>(0))
                .optional()?;
            let Some(relative) = relative else {
                continue;
            };
            let relative = PathBuf::from(relative);
            let Ok(absolute) = portable::absolute_source_path(root, &relative) else {
                continue;
            };
            resolved.insert((library_id.clone(), face_id.clone()), absolute);
        }
    }
    Ok(resolved)
}

fn combine_match_sets(
    selected_person_ids: &[String],
    images_by_person: &HashMap<String, HashSet<PathBuf>>,
    mode: PeopleFilterMode,
) -> HashSet<PathBuf> {
    let mut selected = selected_person_ids.iter();
    let Some(first_id) = selected.next() else {
        return HashSet::new();
    };
    let mut combined = images_by_person.get(first_id).cloned().unwrap_or_default();
    match mode {
        PeopleFilterMode::Any => {
            for person_id in selected {
                if let Some(paths) = images_by_person.get(person_id) {
                    combined.extend(paths.iter().cloned());
                }
            }
        }
        PeopleFilterMode::All => {
            for person_id in selected {
                let Some(paths) = images_by_person.get(person_id) else {
                    combined.clear();
                    break;
                };
                combined.retain(|path| paths.contains(path));
                if combined.is_empty() {
                    break;
                }
            }
        }
    }
    combined
}

fn open_portable_read_only(root: &Path) -> Result<Connection> {
    let path = portable::index_db_path(root);
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening portable People index for {}", root.display()))
}

fn portable_library_id(conn: &Connection) -> Result<String> {
    conn.query_row(
        "SELECT value FROM portable_meta WHERE key = 'library_id'",
        [],
        |row| row.get::<_, String>(0),
    )
    .context("portable index has no library_id")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(values: &[&str]) -> HashSet<PathBuf> {
        values.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn any_mode_unions_selected_people_without_duplicates() {
        let selected = vec!["alice".to_owned(), "bob".to_owned()];
        let by_person = HashMap::from([
            ("alice".to_owned(), paths(&["a.jpg", "shared.jpg"])),
            ("bob".to_owned(), paths(&["b.jpg", "shared.jpg"])),
        ]);
        assert_eq!(
            combine_match_sets(&selected, &by_person, PeopleFilterMode::Any),
            paths(&["a.jpg", "b.jpg", "shared.jpg"])
        );
    }

    #[test]
    fn all_mode_intersects_selected_people() {
        let selected = vec!["alice".to_owned(), "bob".to_owned()];
        let by_person = HashMap::from([
            ("alice".to_owned(), paths(&["a.jpg", "shared.jpg"])),
            ("bob".to_owned(), paths(&["b.jpg", "shared.jpg"])),
        ]);
        assert_eq!(
            combine_match_sets(&selected, &by_person, PeopleFilterMode::All),
            paths(&["shared.jpg"])
        );
    }

    #[test]
    fn all_mode_is_empty_when_one_selected_person_has_no_images() {
        let selected = vec!["alice".to_owned(), "bob".to_owned()];
        let by_person = HashMap::from([("alice".to_owned(), paths(&["a.jpg"]))]);
        assert!(combine_match_sets(&selected, &by_person, PeopleFilterMode::All).is_empty());
    }

    #[test]
    fn inactive_resolved_filter_accepts_every_image() {
        let filter = ResolvedPeopleFilter::default();
        assert!(filter.matches(Path::new("anything.jpg")));
    }
}
