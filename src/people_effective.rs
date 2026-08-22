use crate::{people_overrides, people_store};
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EffectivePersonSource {
    Automatic,
    Manual,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectivePerson {
    pub person_id: String,
    pub display_name: Option<String>,
    pub source: EffectivePersonSource,
    pub representative_library_id: Option<String>,
    pub representative_face_id: Option<String>,
    pub member_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveMember {
    pub library_id: String,
    pub face_id: String,
    pub person_id: Option<String>,
    pub source: Option<EffectivePersonSource>,
    pub ignored: bool,
    pub detached: bool,
    pub explicit_manual_assignment: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EffectivePeopleCatalog {
    pub people: Vec<EffectivePerson>,
    pub members: Vec<EffectiveMember>,
}

pub fn load(conn: &Connection) -> Result<EffectivePeopleCatalog> {
    people_store::ensure_schema(conn)?;
    people_overrides::ensure_schema(conn)?;

    let auto_clusters = people_store::load_clusters(conn)?;
    let auto_members = people_store::load_members(conn)?;
    let manual_people = people_overrides::load_people(conn)?;
    let overrides = people_overrides::load_face_overrides(conn)?;

    let override_by_face: HashMap<(String, String), people_overrides::FaceOverride> = overrides
        .into_iter()
        .map(|item| ((item.library_id.clone(), item.face_id.clone()), item))
        .collect();
    let manual_by_id: HashMap<String, people_overrides::ManualPerson> = manual_people
        .into_iter()
        .map(|person| (person.manual_person_id.clone(), person))
        .collect();
    let auto_cluster_by_id: HashMap<String, people_store::PersonCluster> = auto_clusters
        .into_iter()
        .map(|cluster| (cluster.person_id.clone(), cluster))
        .collect();

    let mut auto_group_members: BTreeMap<String, Vec<people_store::PersonClusterMember>> =
        BTreeMap::new();
    let mut automatic_outliers = Vec::new();
    for member in auto_members {
        if let Some(person_id) = member.person_id.clone() {
            auto_group_members
                .entry(person_id)
                .or_default()
                .push(member);
        } else {
            automatic_outliers.push(member);
        }
    }

    let mut effective_members = Vec::new();
    let mut people_members: BTreeMap<(EffectivePersonSource, String), Vec<(String, String)>> =
        BTreeMap::new();
    let mut seen_faces = HashSet::<(String, String)>::new();

    for (auto_person_id, members) in auto_group_members {
        let manual_anchors = members
            .iter()
            .filter_map(|member| {
                let key = (member.library_id.clone(), member.face_id.clone());
                override_by_face.get(&key).and_then(|item| {
                    (item.disposition == people_overrides::FaceOverrideDisposition::Assigned
                        && item.propagates_cluster)
                        .then(|| item.manual_person_id.clone())
                        .flatten()
                })
            })
            .collect::<HashSet<_>>();
        let inherited_manual = if manual_anchors.len() == 1 {
            manual_anchors.iter().next().cloned()
        } else {
            None
        };

        for member in members {
            let key = (member.library_id.clone(), member.face_id.clone());
            seen_faces.insert(key.clone());
            let override_item = override_by_face.get(&key);
            let resolved = resolve_face(
                &member.library_id,
                &member.face_id,
                Some(auto_person_id.as_str()),
                inherited_manual.as_deref(),
                override_item,
            );
            if let (Some(person_id), Some(source)) = (&resolved.person_id, resolved.source) {
                people_members
                    .entry((source, person_id.clone()))
                    .or_default()
                    .push(key);
            }
            effective_members.push(resolved);
        }
    }

    for member in automatic_outliers {
        let key = (member.library_id.clone(), member.face_id.clone());
        seen_faces.insert(key.clone());
        let resolved = resolve_face(
            &member.library_id,
            &member.face_id,
            None,
            None,
            override_by_face.get(&key),
        );
        if let (Some(person_id), Some(source)) = (&resolved.person_id, resolved.source) {
            people_members
                .entry((source, person_id.clone()))
                .or_default()
                .push(key);
        }
        effective_members.push(resolved);
    }

    // Keep explicit user work even when the current automatic snapshot no longer contains
    // the face. Portable availability is checked by the UI/search layer when rendering it.
    for (key, override_item) in &override_by_face {
        if seen_faces.contains(key) {
            continue;
        }
        let resolved = resolve_face(&key.0, &key.1, None, None, Some(override_item));
        if let (Some(person_id), Some(source)) = (&resolved.person_id, resolved.source) {
            people_members
                .entry((source, person_id.clone()))
                .or_default()
                .push(key.clone());
        }
        effective_members.push(resolved);
    }

    let mut people = Vec::new();
    for ((source, person_id), mut members) in people_members {
        members.sort();
        members.dedup();
        match source {
            EffectivePersonSource::Manual => {
                let manual = manual_by_id.get(&person_id).with_context(|| {
                    format!("effective People references unknown manual id {person_id}")
                })?;
                let configured_rep = manual
                    .representative_library_id
                    .clone()
                    .zip(manual.representative_face_id.clone())
                    .filter(|candidate| members.contains(candidate));
                let fallback_rep = choose_manual_fallback_representative(
                    &members,
                    &auto_cluster_by_id,
                    &effective_members,
                    &person_id,
                );
                let representative = configured_rep
                    .or(fallback_rep)
                    .or_else(|| members.first().cloned());
                people.push(EffectivePerson {
                    person_id,
                    display_name: (!manual.display_name.trim().is_empty())
                        .then(|| manual.display_name.clone()),
                    source,
                    representative_library_id: representative.as_ref().map(|item| item.0.clone()),
                    representative_face_id: representative.as_ref().map(|item| item.1.clone()),
                    member_count: members.len(),
                });
            }
            EffectivePersonSource::Automatic => {
                let auto = auto_cluster_by_id.get(&person_id).with_context(|| {
                    format!("effective People references unknown automatic id {person_id}")
                })?;
                let configured_rep = (
                    auto.representative_library_id.clone(),
                    auto.representative_face_id.clone(),
                );
                let representative = members
                    .contains(&configured_rep)
                    .then_some(configured_rep)
                    .or_else(|| members.first().cloned());
                people.push(EffectivePerson {
                    person_id,
                    display_name: None,
                    source,
                    representative_library_id: representative.as_ref().map(|item| item.0.clone()),
                    representative_face_id: representative.as_ref().map(|item| item.1.clone()),
                    member_count: members.len(),
                });
            }
        }
    }

    // Empty manual people still belong in the management surface so they can be renamed,
    // populated again, or deleted after a destructive correction.
    for manual in manual_by_id.values() {
        if people.iter().any(|person| {
            person.source == EffectivePersonSource::Manual
                && person.person_id == manual.manual_person_id
        }) {
            continue;
        }
        people.push(EffectivePerson {
            person_id: manual.manual_person_id.clone(),
            display_name: (!manual.display_name.trim().is_empty())
                .then(|| manual.display_name.clone()),
            source: EffectivePersonSource::Manual,
            representative_library_id: None,
            representative_face_id: None,
            member_count: 0,
        });
    }

    people.sort_by(|left, right| {
        let left_named = left.display_name.as_deref().unwrap_or("");
        let right_named = right.display_name.as_deref().unwrap_or("");
        right
            .member_count
            .cmp(&left.member_count)
            .then_with(|| {
                left_named
                    .to_ascii_lowercase()
                    .cmp(&right_named.to_ascii_lowercase())
            })
            .then_with(|| left.person_id.cmp(&right.person_id))
    });
    effective_members.sort_by(|left, right| {
        left.library_id
            .cmp(&right.library_id)
            .then_with(|| left.face_id.cmp(&right.face_id))
    });

    Ok(EffectivePeopleCatalog {
        people,
        members: effective_members,
    })
}

fn resolve_face(
    library_id: &str,
    face_id: &str,
    automatic_person_id: Option<&str>,
    inherited_manual_person_id: Option<&str>,
    override_item: Option<&people_overrides::FaceOverride>,
) -> EffectiveMember {
    if let Some(override_item) = override_item {
        match override_item.disposition {
            people_overrides::FaceOverrideDisposition::Assigned => {
                return EffectiveMember {
                    library_id: library_id.to_owned(),
                    face_id: face_id.to_owned(),
                    person_id: override_item.manual_person_id.clone(),
                    source: Some(EffectivePersonSource::Manual),
                    ignored: false,
                    detached: false,
                    explicit_manual_assignment: true,
                };
            }
            people_overrides::FaceOverrideDisposition::Detached => {
                return EffectiveMember {
                    library_id: library_id.to_owned(),
                    face_id: face_id.to_owned(),
                    person_id: None,
                    source: None,
                    ignored: false,
                    detached: true,
                    explicit_manual_assignment: false,
                };
            }
            people_overrides::FaceOverrideDisposition::Ignored => {
                return EffectiveMember {
                    library_id: library_id.to_owned(),
                    face_id: face_id.to_owned(),
                    person_id: None,
                    source: None,
                    ignored: true,
                    detached: false,
                    explicit_manual_assignment: false,
                };
            }
        }
    }

    if let Some(manual_person_id) = inherited_manual_person_id {
        return EffectiveMember {
            library_id: library_id.to_owned(),
            face_id: face_id.to_owned(),
            person_id: Some(manual_person_id.to_owned()),
            source: Some(EffectivePersonSource::Manual),
            ignored: false,
            detached: false,
            explicit_manual_assignment: false,
        };
    }

    EffectiveMember {
        library_id: library_id.to_owned(),
        face_id: face_id.to_owned(),
        person_id: automatic_person_id.map(str::to_owned),
        source: automatic_person_id.map(|_| EffectivePersonSource::Automatic),
        ignored: false,
        detached: false,
        explicit_manual_assignment: false,
    }
}

fn choose_manual_fallback_representative(
    members: &[(String, String)],
    automatic_clusters: &HashMap<String, people_store::PersonCluster>,
    effective_members: &[EffectiveMember],
    manual_person_id: &str,
) -> Option<(String, String)> {
    let member_set: HashSet<(&str, &str)> = members
        .iter()
        .map(|item| (item.0.as_str(), item.1.as_str()))
        .collect();
    automatic_clusters
        .values()
        .filter_map(|cluster| {
            let candidate = (
                cluster.representative_library_id.as_str(),
                cluster.representative_face_id.as_str(),
            );
            if !member_set.contains(&candidate) {
                return None;
            }
            let belongs = effective_members.iter().any(|member| {
                member.library_id == cluster.representative_library_id
                    && member.face_id == cluster.representative_face_id
                    && member.person_id.as_deref() == Some(manual_person_id)
                    && member.source == Some(EffectivePersonSource::Manual)
            });
            belongs.then(|| (candidate.0.to_owned(), candidate.1.to_owned()))
        })
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::people_overrides;

    fn state() -> people_store::PeopleClusterState {
        people_store::PeopleClusterState {
            embedding: people_store::PeopleEmbeddingRevision {
                model_id: "sface".to_owned(),
                model_version: "1".to_owned(),
                model_cache_revision: "cache-a".to_owned(),
                dimension: 128,
                alignment_revision: 2,
            },
            algorithm_revision: people_store::ALGORITHM_REVISION,
            similarity_threshold: 0.62,
            min_cluster_size: 2,
        }
    }

    fn write_auto(
        conn: &mut Connection,
        clusters: Vec<people_store::PersonCluster>,
        members: Vec<people_store::PersonClusterMember>,
    ) {
        people_store::replace_automatic_snapshot(conn, &state(), &clusters, &members).unwrap();
    }

    fn auto_cluster(id: &str, rep: &str, count: usize) -> people_store::PersonCluster {
        people_store::PersonCluster {
            person_id: id.to_owned(),
            representative_library_id: "library-a".to_owned(),
            representative_face_id: rep.to_owned(),
            member_count: count,
        }
    }

    fn auto_member(face: &str, person: Option<&str>) -> people_store::PersonClusterMember {
        people_store::PersonClusterMember {
            library_id: "library-a".to_owned(),
            face_id: face.to_owned(),
            person_id: person.map(str::to_owned),
            assignment_similarity: person.map(|_| 0.9),
            is_outlier: person.is_none(),
        }
    }

    #[test]
    fn untouched_automatic_group_remains_automatic() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        write_auto(
            &mut conn,
            vec![auto_cluster("person-auto", "face-1", 2)],
            vec![
                auto_member("face-1", Some("person-auto")),
                auto_member("face-2", Some("person-auto")),
            ],
        );
        let catalog = load(&conn).unwrap();
        assert_eq!(catalog.people.len(), 1);
        assert_eq!(catalog.people[0].source, EffectivePersonSource::Automatic);
        assert_eq!(catalog.people[0].member_count, 2);
    }

    #[test]
    fn one_manual_anchor_claims_new_unoverridden_members_from_same_auto_group() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        write_auto(
            &mut conn,
            vec![auto_cluster("person-auto", "face-1", 3)],
            vec![
                auto_member("face-1", Some("person-auto")),
                auto_member("face-2", Some("person-auto")),
                auto_member("face-3", Some("person-auto")),
            ],
        );
        let manual =
            people_overrides::create_person(&conn, "Alice", "library-a", "face-1").unwrap();
        people_overrides::anchor_face(&conn, "library-a", "face-1", &manual.manual_person_id)
            .unwrap();

        let catalog = load(&conn).unwrap();
        assert_eq!(catalog.people.len(), 1);
        assert_eq!(catalog.people[0].source, EffectivePersonSource::Manual);
        assert_eq!(catalog.people[0].display_name.as_deref(), Some("Alice"));
        assert_eq!(catalog.people[0].member_count, 3);
        assert_eq!(
            catalog
                .members
                .iter()
                .filter(
                    |member| member.person_id.as_deref() == Some(manual.manual_person_id.as_str())
                )
                .count(),
            3
        );
    }

    #[test]
    fn conflicting_manual_anchors_do_not_force_unoverridden_face_to_either_identity() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        write_auto(
            &mut conn,
            vec![auto_cluster("person-auto", "face-3", 3)],
            vec![
                auto_member("face-1", Some("person-auto")),
                auto_member("face-2", Some("person-auto")),
                auto_member("face-3", Some("person-auto")),
            ],
        );
        let alice = people_overrides::create_person(&conn, "Alice", "library-a", "face-1").unwrap();
        let bob = people_overrides::create_person(&conn, "Bob", "library-a", "face-2").unwrap();
        people_overrides::assign_face(&conn, "library-a", "face-1", &alice.manual_person_id)
            .unwrap();
        people_overrides::assign_face(&conn, "library-a", "face-2", &bob.manual_person_id).unwrap();

        let catalog = load(&conn).unwrap();
        let third = catalog
            .members
            .iter()
            .find(|member| member.face_id == "face-3")
            .unwrap();
        assert_eq!(third.person_id.as_deref(), Some("person-auto"));
        assert_eq!(third.source, Some(EffectivePersonSource::Automatic));
        assert_eq!(catalog.people.len(), 3);
    }

    #[test]
    fn ignored_and_detached_faces_are_removed_from_automatic_group() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        write_auto(
            &mut conn,
            vec![auto_cluster("person-auto", "face-1", 3)],
            vec![
                auto_member("face-1", Some("person-auto")),
                auto_member("face-2", Some("person-auto")),
                auto_member("face-3", Some("person-auto")),
            ],
        );
        people_overrides::ignore_face(&conn, "library-a", "face-2").unwrap();
        people_overrides::detach_face(&conn, "library-a", "face-3").unwrap();
        let catalog = load(&conn).unwrap();
        assert_eq!(catalog.people.len(), 1);
        assert_eq!(catalog.people[0].member_count, 1);
        assert!(
            catalog
                .members
                .iter()
                .find(|member| member.face_id == "face-2")
                .unwrap()
                .ignored
        );
        assert!(
            catalog
                .members
                .iter()
                .find(|member| member.face_id == "face-3")
                .unwrap()
                .detached
        );
    }

    #[test]
    fn automatic_outlier_can_be_explicitly_assigned_to_manual_person() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        write_auto(&mut conn, vec![], vec![auto_member("face-1", None)]);
        let manual =
            people_overrides::create_person(&conn, "Alice", "library-a", "face-1").unwrap();
        people_overrides::assign_face(&conn, "library-a", "face-1", &manual.manual_person_id)
            .unwrap();
        let catalog = load(&conn).unwrap();
        assert_eq!(catalog.people.len(), 1);
        assert_eq!(catalog.people[0].source, EffectivePersonSource::Manual);
        assert_eq!(catalog.people[0].member_count, 1);
    }

    #[test]
    fn explicit_assignment_does_not_claim_unoverridden_cluster_members() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        write_auto(
            &mut conn,
            vec![auto_cluster("person-auto", "face-1", 3)],
            vec![
                auto_member("face-1", Some("person-auto")),
                auto_member("face-2", Some("person-auto")),
                auto_member("face-3", Some("person-auto")),
            ],
        );
        let manual = people_overrides::create_person(&conn, "Bob", "library-a", "face-3").unwrap();
        people_overrides::assign_face(&conn, "library-a", "face-3", &manual.manual_person_id)
            .unwrap();

        let catalog = load(&conn).unwrap();
        let manual_members = catalog
            .members
            .iter()
            .filter(|member| member.person_id.as_deref() == Some(&manual.manual_person_id))
            .count();
        let auto_members = catalog
            .members
            .iter()
            .filter(|member| member.person_id.as_deref() == Some("person-auto"))
            .count();
        assert_eq!(manual_members, 1);
        assert_eq!(auto_members, 2);
    }
}
