use crate::{db, people_effective, people_overrides, portable};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Clone, Debug)]
struct ManualPersonRow {
    manual_person_id: String,
    display_name: String,
    representative_library_id: Option<String>,
    representative_face_id: Option<String>,
    updated_at: i64,
}

#[derive(Clone, Debug)]
struct ManualOverrideRow {
    library_id: String,
    face_id: String,
    disposition: String,
    manual_person_id: Option<String>,
    propagates_cluster: bool,
    updated_at: i64,
}

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
            sync_manual_cache_to_portable_roots(conn)?;
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
    sync_manual_cache_to_portable_roots(conn)?;
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
    sync_manual_cache_to_portable_roots(conn)?;
    Ok(keep)
}

pub fn move_face_to_person(
    conn: &Connection,
    library_id: &str,
    face_id: &str,
    manual_person_id: &str,
) -> Result<()> {
    people_overrides::assign_face(conn, library_id, face_id, manual_person_id)?;
    sync_manual_cache_to_portable_roots(conn)
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
    sync_manual_cache_to_portable_roots(conn)?;
    Ok(person.manual_person_id)
}

pub fn detach_face(conn: &Connection, library_id: &str, face_id: &str) -> Result<()> {
    people_overrides::detach_face(conn, library_id, face_id)?;
    sync_manual_cache_to_portable_roots(conn)
}

pub fn ignore_face(conn: &Connection, library_id: &str, face_id: &str) -> Result<()> {
    people_overrides::ignore_face(conn, library_id, face_id)?;
    sync_manual_cache_to_portable_roots(conn)
}

pub fn restore_automatic_face(conn: &Connection, library_id: &str, face_id: &str) -> Result<bool> {
    let changed = people_overrides::clear_face_override(conn, library_id, face_id)?;
    sync_manual_cache_to_portable_roots(conn)?;
    Ok(changed)
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
    people_overrides::set_representative(conn, manual_person_id, library_id, face_id)?;
    sync_manual_cache_to_portable_roots(conn)
}

pub fn delete_manual_person(conn: &Connection, manual_person_id: &str) -> Result<bool> {
    let changed = people_overrides::delete_person(conn, manual_person_id)?;
    sync_manual_cache_to_portable_roots(conn)?;
    Ok(changed)
}

/// Rebuild the disposable session copy of manual People edits from all currently
/// attached portable roots. Newest duplicate metadata wins by `updated_at`.
pub fn refresh_manual_cache_from_portable_roots(conn: &Connection) -> Result<()> {
    if !is_session_connection(conn)? {
        return Ok(());
    }
    people_overrides::ensure_schema(conn)?;

    let mut people = HashMap::<String, ManualPersonRow>::new();
    let mut overrides = HashMap::<(String, String), ManualOverrideRow>::new();

    for (library_id, root) in attached_portable_roots(conn)? {
        let db_path = portable::index_db_path(&root);
        if !db_path.is_file() {
            continue;
        }
        let root_conn = match db::open(&db_path) {
            Ok(conn) => conn,
            Err(_) => continue,
        };
        people_overrides::ensure_schema(&root_conn)?;

        let root_overrides = load_manual_override_rows(&root_conn)?
            .into_iter()
            .filter(|item| item.library_id == library_id)
            .collect::<Vec<_>>();
        let referenced_people = root_overrides
            .iter()
            .filter_map(|item| item.manual_person_id.clone())
            .collect::<HashSet<_>>();

        for item in load_manual_person_rows(&root_conn)? {
            if !referenced_people.contains(&item.manual_person_id) {
                continue;
            }
            match people.get(&item.manual_person_id) {
                Some(existing) if existing.updated_at > item.updated_at => {}
                _ => {
                    people.insert(item.manual_person_id.clone(), item);
                }
            }
        }
        for item in root_overrides {
            let key = (item.library_id.clone(), item.face_id.clone());
            match overrides.get(&key) {
                Some(existing) if existing.updated_at > item.updated_at => {}
                _ => {
                    overrides.insert(key, item);
                }
            }
        }
    }

    // Drop assigned overrides whose person metadata is unavailable/corrupt. Detached
    // and ignored overrides do not reference a manual Person and remain valid.
    overrides.retain(|_, item| {
        item.disposition != "assigned"
            || item
                .manual_person_id
                .as_ref()
                .is_some_and(|id| people.contains_key(id))
    });

    replace_manual_rows_local(
        conn,
        &people.into_values().collect::<Vec<_>>(),
        &overrides.into_values().collect::<Vec<_>>(),
    )
}

fn sync_manual_cache_to_portable_roots(conn: &Connection) -> Result<()> {
    if !is_session_connection(conn)? {
        return Ok(());
    }
    people_overrides::ensure_schema(conn)?;
    let people = load_manual_person_rows(conn)?;
    let overrides = load_manual_override_rows(conn)?;

    for (library_id, root) in attached_portable_roots(conn)? {
        let db_path = portable::index_db_path(&root);
        if !db_path.is_file() {
            continue;
        }
        let local_overrides = overrides
            .iter()
            .filter(|item| item.library_id == library_id)
            .cloned()
            .collect::<Vec<_>>();
        let person_ids = local_overrides
            .iter()
            .filter_map(|item| item.manual_person_id.clone())
            .collect::<HashSet<_>>();
        let local_people = people
            .iter()
            .filter(|item| person_ids.contains(&item.manual_person_id))
            .cloned()
            .collect::<Vec<_>>();

        let root_conn = db::open(&db_path).with_context(|| {
            format!("opening portable manual People shard {}", db_path.display())
        })?;
        people_overrides::ensure_schema(&root_conn)?;
        replace_manual_rows_local(&root_conn, &local_people, &local_overrides)?;
    }
    Ok(())
}

fn replace_manual_rows_local(
    conn: &Connection,
    people: &[ManualPersonRow],
    overrides: &[ManualOverrideRow],
) -> Result<()> {
    people_overrides::ensure_schema(conn)?;
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<()> {
        conn.execute("DELETE FROM people_manual_face_overrides", [])?;
        conn.execute("DELETE FROM people_manual_persons", [])?;

        for person in people {
            conn.execute(
                r#"
                INSERT INTO people_manual_persons(
                    manual_person_id, display_name,
                    representative_library_id, representative_face_id,
                    created_at, updated_at
                ) VALUES(?1, ?2, ?3, ?4, ?5, ?5)
                "#,
                params![
                    person.manual_person_id,
                    person.display_name,
                    person.representative_library_id,
                    person.representative_face_id,
                    person.updated_at,
                ],
            )?;
        }
        for item in overrides {
            conn.execute(
                r#"
                INSERT INTO people_manual_face_overrides(
                    library_id, face_id, disposition, manual_person_id,
                    propagates_cluster, updated_at
                ) VALUES(?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    item.library_id,
                    item.face_id,
                    item.disposition,
                    item.manual_person_id,
                    if item.propagates_cluster { 1i64 } else { 0i64 },
                    item.updated_at,
                ],
            )?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => conn
            .execute_batch("COMMIT")
            .context("committing portable manual People state"),
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

fn load_manual_person_rows(conn: &Connection) -> Result<Vec<ManualPersonRow>> {
    people_overrides::ensure_schema(conn)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT manual_person_id, display_name,
               representative_library_id, representative_face_id,
               updated_at
        FROM people_manual_persons
        ORDER BY manual_person_id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ManualPersonRow {
            manual_person_id: row.get(0)?,
            display_name: row.get(1)?,
            representative_library_id: row.get(2)?,
            representative_face_id: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("loading portable manual People rows")
}

fn load_manual_override_rows(conn: &Connection) -> Result<Vec<ManualOverrideRow>> {
    people_overrides::ensure_schema(conn)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT library_id, face_id, disposition, manual_person_id,
               propagates_cluster, updated_at
        FROM people_manual_face_overrides
        ORDER BY library_id, face_id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ManualOverrideRow {
            library_id: row.get(0)?,
            face_id: row.get(1)?,
            disposition: row.get(2)?,
            manual_person_id: row.get(3)?,
            propagates_cluster: row.get::<_, i64>(4)? != 0,
            updated_at: row.get(5)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("loading portable manual People override rows")
}

fn attached_portable_roots(conn: &Connection) -> Result<Vec<(String, PathBuf)>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT registry.library_id, registry.root_path
        FROM portable_root_registry registry
        JOIN roots ON roots.path = registry.root_path COLLATE NOCASE
        ORDER BY registry.library_id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            PathBuf::from(row.get::<_, String>(1)?),
        ))
    })?;
    let mut roots = Vec::new();
    for row in rows {
        let (library_id, root) = row?;
        if root.is_dir() && portable::index_db_path(&root).is_file() {
            roots.push((library_id, root));
        }
    }
    Ok(roots)
}

fn is_session_connection(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'portable_root_registry')",
        [],
        |row| row.get::<_, i64>(0),
    )? != 0)
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
