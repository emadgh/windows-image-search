use crate::{people_effective, people_overrides};
use anyhow::{bail, Context, Result};
use rusqlite::Connection;

pub fn materialize_effective_person(
    conn: &Connection,
    effective_person_id: &str,
    display_name: Option<&str>,
) -> Result<String> {
    let catalog = people_effective::load(conn)?;
    let person = catalog
        .people
        .iter()
        .find(|person| person.person_id == effective_person_id)
        .with_context(|| format!("effective Person does not exist: {effective_person_id}"))?;

    if person.source == people_effective::EffectivePersonSource::Manual {
        if let Some(name) = display_name {
            people_overrides::rename_person(conn, &person.person_id, name)?;
        }
        return Ok(person.person_id.clone());
    }

    let mut members = catalog
        .members
        .iter()
        .filter(|member| {
            member.person_id.as_deref() == Some(effective_person_id)
                && member.source == Some(people_effective::EffectivePersonSource::Automatic)
        })
        .map(|member| (member.library_id.clone(), member.face_id.clone()))
        .collect::<Vec<_>>();
    members.sort();
    members.dedup();
    if members.is_empty() {
        bail!("automatic Person has no effective members: {effective_person_id}");
    }

    let representative = person
        .representative_library_id
        .clone()
        .zip(person.representative_face_id.clone())
        .filter(|candidate| members.contains(candidate))
        .unwrap_or_else(|| members[0].clone());
    let manual = people_overrides::create_person(
        conn,
        display_name.unwrap_or_default(),
        &representative.0,
        &representative.1,
    )?;
    for (library_id, face_id) in &members {
        people_overrides::anchor_face(conn, library_id, face_id, &manual.manual_person_id)?;
    }
    people_overrides::set_representative(
        conn,
        &manual.manual_person_id,
        &representative.0,
        &representative.1,
    )?;
    Ok(manual.manual_person_id)
}

pub fn rename_effective_person(
    conn: &Connection,
    effective_person_id: &str,
    display_name: &str,
) -> Result<String> {
    materialize_effective_person(conn, effective_person_id, Some(display_name))
}

pub fn merge_effective_people(
    conn: &mut Connection,
    effective_person_ids: &[String],
    display_name: Option<&str>,
) -> Result<String> {
    let mut ids = effective_person_ids
        .iter()
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    if ids.len() < 2 {
        bail!("People merge requires at least two distinct groups");
    }

    let mut manual_ids = Vec::with_capacity(ids.len());
    for id in &ids {
        manual_ids.push(materialize_effective_person(conn, id, None)?);
    }
    manual_ids.sort();
    manual_ids.dedup();
    let keep = manual_ids
        .first()
        .cloned()
        .context("People merge produced no manual Person")?;
    let merge = manual_ids.iter().skip(1).cloned().collect::<Vec<_>>();
    people_overrides::merge_people(conn, &keep, &merge)?;
    if let Some(name) = display_name {
        people_overrides::rename_person(conn, &keep, name)?;
    }
    Ok(keep)
}

pub fn move_face_to_person(
    conn: &Connection,
    library_id: &str,
    face_id: &str,
    manual_person_id: &str,
) -> Result<()> {
    people_overrides::assign_face(conn, library_id, face_id, manual_person_id)
}

pub fn split_face_to_new_person(
    conn: &Connection,
    library_id: &str,
    face_id: &str,
    display_name: &str,
) -> Result<String> {
    let person = people_overrides::create_person(conn, display_name, library_id, face_id)?;
    people_overrides::assign_face(conn, library_id, face_id, &person.manual_person_id)?;
    people_overrides::set_representative(conn, &person.manual_person_id, library_id, face_id)?;
    Ok(person.manual_person_id)
}

pub fn detach_face(conn: &Connection, library_id: &str, face_id: &str) -> Result<()> {
    people_overrides::detach_face(conn, library_id, face_id)
}

pub fn ignore_face(conn: &Connection, library_id: &str, face_id: &str) -> Result<()> {
    people_overrides::ignore_face(conn, library_id, face_id)
}

pub fn restore_automatic_face(conn: &Connection, library_id: &str, face_id: &str) -> Result<bool> {
    people_overrides::clear_face_override(conn, library_id, face_id)
}

pub fn set_person_representative(
    conn: &Connection,
    manual_person_id: &str,
    library_id: &str,
    face_id: &str,
) -> Result<()> {
    // Do not downgrade an existing cluster anchor merely because it became the representative.
    let already_assigned_to_person = people_overrides::load_face_overrides(conn)?
        .into_iter()
        .any(|item| {
            item.library_id == library_id
                && item.face_id == face_id
                && item.disposition == people_overrides::FaceOverrideDisposition::Assigned
                && item.manual_person_id.as_deref() == Some(manual_person_id)
        });
    if !already_assigned_to_person {
        people_overrides::assign_face(conn, library_id, face_id, manual_person_id)?;
    }
    people_overrides::set_representative(conn, manual_person_id, library_id, face_id)
}

pub fn delete_manual_person(conn: &Connection, manual_person_id: &str) -> Result<bool> {
    people_overrides::delete_person(conn, manual_person_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::people_store;

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

    fn cluster(id: &str, rep: &str, count: usize) -> people_store::PersonCluster {
        people_store::PersonCluster {
            person_id: id.to_owned(),
            representative_library_id: "library-a".to_owned(),
            representative_face_id: rep.to_owned(),
            member_count: count,
        }
    }

    fn member(face: &str, person: Option<&str>) -> people_store::PersonClusterMember {
        people_store::PersonClusterMember {
            library_id: "library-a".to_owned(),
            face_id: face.to_owned(),
            person_id: person.map(str::to_owned),
            assignment_similarity: person.map(|_| 0.93),
            is_outlier: person.is_none(),
        }
    }

    fn write_auto(
        conn: &mut Connection,
        clusters: Vec<people_store::PersonCluster>,
        members: Vec<people_store::PersonClusterMember>,
    ) {
        people_store::replace_automatic_snapshot(conn, &state(), &clusters, &members).unwrap();
    }

    #[test]
    fn renaming_automatic_person_materializes_cluster_anchors_and_survives_recluster() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        write_auto(
            &mut conn,
            vec![cluster("auto-alice", "alice-1", 2)],
            vec![
                member("alice-1", Some("auto-alice")),
                member("alice-2", Some("auto-alice")),
            ],
        );

        let manual_id = rename_effective_person(&conn, "auto-alice", "Alice").unwrap();
        let overrides = people_overrides::load_face_overrides(&conn).unwrap();
        assert_eq!(overrides.len(), 2);
        assert!(overrides.iter().all(|item| item.propagates_cluster));

        write_auto(
            &mut conn,
            vec![cluster("auto-alice", "alice-1", 3)],
            vec![
                member("alice-1", Some("auto-alice")),
                member("alice-2", Some("auto-alice")),
                member("alice-3", Some("auto-alice")),
            ],
        );
        let catalog = people_effective::load(&conn).unwrap();
        let alice = catalog
            .people
            .iter()
            .find(|person| person.person_id == manual_id)
            .unwrap();
        assert_eq!(alice.display_name.as_deref(), Some("Alice"));
        assert_eq!(alice.member_count, 3);
    }

    #[test]
    fn split_face_is_explicit_and_does_not_claim_the_source_cluster() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        write_auto(
            &mut conn,
            vec![cluster("auto-alice", "face-1", 3)],
            vec![
                member("face-1", Some("auto-alice")),
                member("face-2", Some("auto-alice")),
                member("face-3", Some("auto-alice")),
            ],
        );
        let alice = rename_effective_person(&conn, "auto-alice", "Alice").unwrap();
        let bob = split_face_to_new_person(&conn, "library-a", "face-3", "Bob").unwrap();

        let catalog = people_effective::load(&conn).unwrap();
        let alice_count = catalog
            .people
            .iter()
            .find(|person| person.person_id == alice)
            .unwrap()
            .member_count;
        let bob_count = catalog
            .people
            .iter()
            .find(|person| person.person_id == bob)
            .unwrap()
            .member_count;
        assert_eq!(alice_count, 2);
        assert_eq!(bob_count, 1);
        let bob_override = people_overrides::load_face_overrides(&conn)
            .unwrap()
            .into_iter()
            .find(|item| item.face_id == "face-3")
            .unwrap();
        assert!(!bob_override.propagates_cluster);
    }

    #[test]
    fn merging_two_automatic_people_creates_one_durable_manual_identity() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        write_auto(
            &mut conn,
            vec![cluster("auto-a", "a-1", 2), cluster("auto-b", "b-1", 2)],
            vec![
                member("a-1", Some("auto-a")),
                member("a-2", Some("auto-a")),
                member("b-1", Some("auto-b")),
                member("b-2", Some("auto-b")),
            ],
        );
        let merged = merge_effective_people(
            &mut conn,
            &["auto-a".to_owned(), "auto-b".to_owned()],
            Some("Merged person"),
        )
        .unwrap();
        let catalog = people_effective::load(&conn).unwrap();
        let person = catalog
            .people
            .iter()
            .find(|person| person.person_id == merged)
            .unwrap();
        assert_eq!(person.display_name.as_deref(), Some("Merged person"));
        assert_eq!(person.member_count, 4);
        assert_eq!(
            person.source,
            people_effective::EffectivePersonSource::Manual
        );
    }
}
