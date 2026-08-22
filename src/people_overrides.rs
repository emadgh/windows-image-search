use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceOverrideDisposition {
    Assigned,
    Detached,
    Ignored,
}

impl FaceOverrideDisposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Assigned => "assigned",
            Self::Detached => "detached",
            Self::Ignored => "ignored",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "assigned" => Ok(Self::Assigned),
            "detached" => Ok(Self::Detached),
            "ignored" => Ok(Self::Ignored),
            other => bail!("unknown People face override disposition: {other}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualPerson {
    pub manual_person_id: String,
    pub display_name: String,
    pub representative_library_id: Option<String>,
    pub representative_face_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceOverride {
    pub library_id: String,
    pub face_id: String,
    pub disposition: FaceOverrideDisposition,
    pub manual_person_id: Option<String>,
    pub propagates_cluster: bool,
}

pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS people_manual_persons (
            manual_person_id TEXT PRIMARY KEY NOT NULL,
            display_name TEXT NOT NULL DEFAULT '',
            representative_library_id TEXT,
            representative_face_id TEXT,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
            CHECK(
                (representative_library_id IS NULL AND representative_face_id IS NULL)
                OR
                (representative_library_id IS NOT NULL AND representative_face_id IS NOT NULL)
            )
        );

        CREATE TABLE IF NOT EXISTS people_manual_face_overrides (
            library_id TEXT NOT NULL,
            face_id TEXT NOT NULL,
            disposition TEXT NOT NULL CHECK(disposition IN ('assigned', 'detached', 'ignored')),
            manual_person_id TEXT,
            propagates_cluster INTEGER NOT NULL DEFAULT 0 CHECK(propagates_cluster IN (0, 1)),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
            PRIMARY KEY(library_id, face_id),
            FOREIGN KEY(manual_person_id)
                REFERENCES people_manual_persons(manual_person_id)
                ON DELETE CASCADE,
            CHECK(
                (disposition = 'assigned' AND manual_person_id IS NOT NULL)
                OR
                (disposition IN ('detached', 'ignored') AND manual_person_id IS NULL)
            )
        );

        CREATE INDEX IF NOT EXISTS idx_people_manual_overrides_person
            ON people_manual_face_overrides(manual_person_id, disposition);
        CREATE INDEX IF NOT EXISTS idx_people_manual_overrides_disposition
            ON people_manual_face_overrides(disposition, library_id, face_id);
        "#,
    )?;

    let has_propagates_cluster = {
        let mut stmt = conn.prepare("PRAGMA table_info(people_manual_face_overrides)")?;
        let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut found = false;
        for column in columns {
            if column? == "propagates_cluster" {
                found = true;
                break;
            }
        }
        found
    };
    if !has_propagates_cluster {
        conn.execute(
            "ALTER TABLE people_manual_face_overrides ADD COLUMN propagates_cluster INTEGER NOT NULL DEFAULT 0 CHECK(propagates_cluster IN (0, 1))",
            [],
        )?;
    }
    Ok(())
}

pub fn create_person(
    conn: &Connection,
    display_name: &str,
    seed_library_id: &str,
    seed_face_id: &str,
) -> Result<ManualPerson> {
    ensure_schema(conn)?;
    validate_face_key(seed_library_id, seed_face_id)?;
    let manual_person_id = new_manual_person_id(seed_library_id, seed_face_id);
    let display_name = display_name.trim();
    conn.execute(
        r#"
        INSERT INTO people_manual_persons(
            manual_person_id, display_name,
            representative_library_id, representative_face_id,
            created_at, updated_at
        ) VALUES(?1, ?2, NULL, NULL, unixepoch(), unixepoch())
        "#,
        params![manual_person_id, display_name],
    )?;
    load_person(conn, &manual_person_id)?.context("new manual Person was not persisted")
}

pub fn rename_person(conn: &Connection, manual_person_id: &str, display_name: &str) -> Result<()> {
    ensure_schema(conn)?;
    validate_person_id(manual_person_id)?;
    let changed = conn.execute(
        "UPDATE people_manual_persons SET display_name = ?2, updated_at = unixepoch() WHERE manual_person_id = ?1",
        params![manual_person_id, display_name.trim()],
    )?;
    if changed == 0 {
        bail!("manual Person does not exist: {manual_person_id}");
    }
    Ok(())
}

pub fn assign_face(
    conn: &Connection,
    library_id: &str,
    face_id: &str,
    manual_person_id: &str,
) -> Result<()> {
    assign_face_with_propagation(conn, library_id, face_id, manual_person_id, false)
}

pub fn anchor_face(
    conn: &Connection,
    library_id: &str,
    face_id: &str,
    manual_person_id: &str,
) -> Result<()> {
    assign_face_with_propagation(conn, library_id, face_id, manual_person_id, true)
}

fn assign_face_with_propagation(
    conn: &Connection,
    library_id: &str,
    face_id: &str,
    manual_person_id: &str,
    propagates_cluster: bool,
) -> Result<()> {
    ensure_schema(conn)?;
    validate_face_key(library_id, face_id)?;
    validate_person_id(manual_person_id)?;
    if load_person(conn, manual_person_id)?.is_none() {
        bail!("manual Person does not exist: {manual_person_id}");
    }
    upsert_face_override(
        conn,
        library_id,
        face_id,
        FaceOverrideDisposition::Assigned,
        Some(manual_person_id),
        propagates_cluster,
    )
}

pub fn detach_face(conn: &Connection, library_id: &str, face_id: &str) -> Result<()> {
    ensure_schema(conn)?;
    validate_face_key(library_id, face_id)?;
    upsert_face_override(
        conn,
        library_id,
        face_id,
        FaceOverrideDisposition::Detached,
        None,
        false,
    )
}

pub fn ignore_face(conn: &Connection, library_id: &str, face_id: &str) -> Result<()> {
    ensure_schema(conn)?;
    validate_face_key(library_id, face_id)?;
    upsert_face_override(
        conn,
        library_id,
        face_id,
        FaceOverrideDisposition::Ignored,
        None,
        false,
    )
}

pub fn clear_face_override(conn: &Connection, library_id: &str, face_id: &str) -> Result<bool> {
    ensure_schema(conn)?;
    validate_face_key(library_id, face_id)?;
    Ok(conn.execute(
        "DELETE FROM people_manual_face_overrides WHERE library_id = ?1 AND face_id = ?2",
        params![library_id, face_id],
    )? > 0)
}

pub fn set_representative(
    conn: &Connection,
    manual_person_id: &str,
    library_id: &str,
    face_id: &str,
) -> Result<()> {
    ensure_schema(conn)?;
    validate_person_id(manual_person_id)?;
    validate_face_key(library_id, face_id)?;
    let assigned: Option<i64> = conn
        .query_row(
            r#"
            SELECT 1
            FROM people_manual_face_overrides
            WHERE library_id = ?1
              AND face_id = ?2
              AND disposition = 'assigned'
              AND manual_person_id = ?3
            LIMIT 1
            "#,
            params![library_id, face_id, manual_person_id],
            |row| row.get(0),
        )
        .optional()?;
    if assigned.is_none() {
        bail!("manual representative must be explicitly assigned to the Person first");
    }
    let changed = conn.execute(
        r#"
        UPDATE people_manual_persons
        SET representative_library_id = ?2,
            representative_face_id = ?3,
            updated_at = unixepoch()
        WHERE manual_person_id = ?1
        "#,
        params![manual_person_id, library_id, face_id],
    )?;
    if changed == 0 {
        bail!("manual Person does not exist: {manual_person_id}");
    }
    Ok(())
}

pub fn clear_representative(conn: &Connection, manual_person_id: &str) -> Result<()> {
    ensure_schema(conn)?;
    validate_person_id(manual_person_id)?;
    let changed = conn.execute(
        r#"
        UPDATE people_manual_persons
        SET representative_library_id = NULL,
            representative_face_id = NULL,
            updated_at = unixepoch()
        WHERE manual_person_id = ?1
        "#,
        params![manual_person_id],
    )?;
    if changed == 0 {
        bail!("manual Person does not exist: {manual_person_id}");
    }
    Ok(())
}

pub fn merge_people(conn: &mut Connection, keep_id: &str, merge_ids: &[String]) -> Result<usize> {
    ensure_schema(conn)?;
    validate_person_id(keep_id)?;
    if load_person(conn, keep_id)?.is_none() {
        bail!("manual Person does not exist: {keep_id}");
    }

    let mut unique = merge_ids
        .iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty() && value != keep_id)
        .collect::<Vec<_>>();
    unique.sort();
    unique.dedup();
    if unique.is_empty() {
        return Ok(0);
    }
    for id in &unique {
        validate_person_id(id)?;
        if load_person(conn, id)?.is_none() {
            bail!("manual Person does not exist: {id}");
        }
    }

    let tx = conn.transaction()?;
    let mut reassigned = 0usize;
    for id in &unique {
        reassigned += tx.execute(
            r#"
            UPDATE people_manual_face_overrides
            SET manual_person_id = ?1, updated_at = unixepoch()
            WHERE manual_person_id = ?2 AND disposition = 'assigned'
            "#,
            params![keep_id, id],
        )?;
        tx.execute(
            "DELETE FROM people_manual_persons WHERE manual_person_id = ?1",
            params![id],
        )?;
    }
    tx.execute(
        "UPDATE people_manual_persons SET updated_at = unixepoch() WHERE manual_person_id = ?1",
        params![keep_id],
    )?;
    tx.commit().context("committing manual People merge")?;
    Ok(reassigned)
}

pub fn delete_person(conn: &Connection, manual_person_id: &str) -> Result<bool> {
    ensure_schema(conn)?;
    validate_person_id(manual_person_id)?;
    Ok(conn.execute(
        "DELETE FROM people_manual_persons WHERE manual_person_id = ?1",
        params![manual_person_id],
    )? > 0)
}

pub fn load_people(conn: &Connection) -> Result<Vec<ManualPerson>> {
    ensure_schema(conn)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT manual_person_id, display_name,
               representative_library_id, representative_face_id
        FROM people_manual_persons
        ORDER BY
            CASE WHEN display_name = '' THEN 1 ELSE 0 END,
            display_name COLLATE NOCASE,
            manual_person_id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ManualPerson {
            manual_person_id: row.get(0)?,
            display_name: row.get(1)?,
            representative_library_id: row.get(2)?,
            representative_face_id: row.get(3)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("loading manual People")
}

pub fn load_person(conn: &Connection, manual_person_id: &str) -> Result<Option<ManualPerson>> {
    ensure_schema(conn)?;
    validate_person_id(manual_person_id)?;
    conn.query_row(
        r#"
        SELECT manual_person_id, display_name,
               representative_library_id, representative_face_id
        FROM people_manual_persons
        WHERE manual_person_id = ?1
        "#,
        params![manual_person_id],
        |row| {
            Ok(ManualPerson {
                manual_person_id: row.get(0)?,
                display_name: row.get(1)?,
                representative_library_id: row.get(2)?,
                representative_face_id: row.get(3)?,
            })
        },
    )
    .optional()
    .context("loading manual Person")
}

pub fn load_face_overrides(conn: &Connection) -> Result<Vec<FaceOverride>> {
    ensure_schema(conn)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT library_id, face_id, disposition, manual_person_id, propagates_cluster
        FROM people_manual_face_overrides
        ORDER BY library_id, face_id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        let disposition_text = row.get::<_, String>(2)?;
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            disposition_text,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, i64>(4)? != 0,
        ))
    })?;
    let mut output = Vec::new();
    for row in rows {
        let (library_id, face_id, disposition, manual_person_id, propagates_cluster) = row?;
        output.push(FaceOverride {
            library_id,
            face_id,
            disposition: FaceOverrideDisposition::parse(&disposition)?,
            manual_person_id,
            propagates_cluster,
        });
    }
    Ok(output)
}

fn upsert_face_override(
    conn: &Connection,
    library_id: &str,
    face_id: &str,
    disposition: FaceOverrideDisposition,
    manual_person_id: Option<&str>,
    propagates_cluster: bool,
) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO people_manual_face_overrides(
            library_id, face_id, disposition, manual_person_id, propagates_cluster, updated_at
        ) VALUES(?1, ?2, ?3, ?4, ?5, unixepoch())
        ON CONFLICT(library_id, face_id) DO UPDATE SET
            disposition = excluded.disposition,
            manual_person_id = excluded.manual_person_id,
            propagates_cluster = excluded.propagates_cluster,
            updated_at = unixepoch()
        "#,
        params![
            library_id,
            face_id,
            disposition.as_str(),
            manual_person_id,
            if propagates_cluster { 1i64 } else { 0i64 }
        ],
    )?;
    Ok(())
}

fn validate_face_key(library_id: &str, face_id: &str) -> Result<()> {
    if library_id.trim().is_empty() || face_id.trim().is_empty() {
        bail!("People face override requires a library id and face id");
    }
    Ok(())
}

fn validate_person_id(manual_person_id: &str) -> Result<()> {
    if manual_person_id.trim().is_empty() {
        bail!("manual Person id cannot be empty");
    }
    Ok(())
}

fn new_manual_person_id(seed_library_id: &str, seed_face_id: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hash = 0xcbf29ce484222325u64;
    for value in [seed_library_id, seed_face_id, &nanos.to_string()] {
        for byte in value.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("manual-person-{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::people_store;

    fn auto_state() -> people_store::PeopleClusterState {
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

    #[test]
    fn manual_overrides_round_trip_and_revert() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        let person = create_person(&conn, "Alice", "library-a", "face-1").unwrap();
        assign_face(&conn, "library-a", "face-1", &person.manual_person_id).unwrap();
        ignore_face(&conn, "library-a", "face-noise").unwrap();
        detach_face(&conn, "library-b", "face-wrong").unwrap();

        let people = load_people(&conn).unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].display_name, "Alice");
        let overrides = load_face_overrides(&conn).unwrap();
        assert_eq!(overrides.len(), 3);
        assert!(overrides.iter().any(|item| {
            item.face_id == "face-1"
                && item.disposition == FaceOverrideDisposition::Assigned
                && item.manual_person_id.as_deref() == Some(person.manual_person_id.as_str())
        }));
        assert!(overrides.iter().any(|item| {
            item.face_id == "face-noise" && item.disposition == FaceOverrideDisposition::Ignored
        }));
        assert!(clear_face_override(&conn, "library-a", "face-noise").unwrap());
        assert_eq!(load_face_overrides(&conn).unwrap().len(), 2);
    }

    #[test]
    fn representative_must_be_explicitly_assigned() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        let person = create_person(&conn, "Alice", "library-a", "face-1").unwrap();
        assert!(
            set_representative(&conn, &person.manual_person_id, "library-a", "face-1").is_err()
        );
        assign_face(&conn, "library-a", "face-1", &person.manual_person_id).unwrap();
        set_representative(&conn, &person.manual_person_id, "library-a", "face-1").unwrap();
        let loaded = load_person(&conn, &person.manual_person_id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.representative_face_id.as_deref(), Some("face-1"));
    }

    #[test]
    fn merge_reassigns_faces_and_removes_source_people() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        let alice = create_person(&conn, "Alice", "library-a", "a1").unwrap();
        let duplicate = create_person(&conn, "Alice duplicate", "library-b", "a2").unwrap();
        assign_face(&conn, "library-a", "a1", &alice.manual_person_id).unwrap();
        assign_face(&conn, "library-b", "a2", &duplicate.manual_person_id).unwrap();

        let moved = merge_people(
            &mut conn,
            &alice.manual_person_id,
            std::slice::from_ref(&duplicate.manual_person_id),
        )
        .unwrap();
        assert_eq!(moved, 1);
        assert!(load_person(&conn, &duplicate.manual_person_id)
            .unwrap()
            .is_none());
        let overrides = load_face_overrides(&conn).unwrap();
        assert_eq!(overrides.len(), 2);
        assert!(overrides
            .iter()
            .all(|item| item.manual_person_id.as_deref() == Some(alice.manual_person_id.as_str())));
    }

    #[test]
    fn deleting_manual_person_reverts_assigned_faces_to_automatic_behavior() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        let person = create_person(&conn, "Alice", "library-a", "a1").unwrap();
        assign_face(&conn, "library-a", "a1", &person.manual_person_id).unwrap();
        ignore_face(&conn, "library-a", "noise").unwrap();
        assert!(delete_person(&conn, &person.manual_person_id).unwrap());
        let overrides = load_face_overrides(&conn).unwrap();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].disposition, FaceOverrideDisposition::Ignored);
    }

    #[test]
    fn automatic_snapshot_replacement_does_not_delete_manual_overrides() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        let person = create_person(&conn, "Alice", "library-a", "face-1").unwrap();
        assign_face(&conn, "library-a", "face-1", &person.manual_person_id).unwrap();

        let clusters = vec![people_store::PersonCluster {
            person_id: "auto-person-a".to_owned(),
            representative_library_id: "library-a".to_owned(),
            representative_face_id: "face-1".to_owned(),
            member_count: 2,
        }];
        let members = vec![
            people_store::PersonClusterMember {
                library_id: "library-a".to_owned(),
                face_id: "face-1".to_owned(),
                person_id: Some("auto-person-a".to_owned()),
                assignment_similarity: Some(1.0),
                is_outlier: false,
            },
            people_store::PersonClusterMember {
                library_id: "library-a".to_owned(),
                face_id: "face-2".to_owned(),
                person_id: Some("auto-person-a".to_owned()),
                assignment_similarity: Some(0.91),
                is_outlier: false,
            },
        ];
        people_store::replace_automatic_snapshot(&mut conn, &auto_state(), &clusters, &members)
            .unwrap();

        let loaded = load_person(&conn, &person.manual_person_id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.display_name, "Alice");
        assert_eq!(load_face_overrides(&conn).unwrap().len(), 1);

        people_store::replace_automatic_snapshot(&mut conn, &auto_state(), &[], &[]).unwrap();
        assert!(load_person(&conn, &person.manual_person_id)
            .unwrap()
            .is_some());
        assert_eq!(load_face_overrides(&conn).unwrap().len(), 1);
    }
}
