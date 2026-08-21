use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};

pub const ALGORITHM_REVISION: i64 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeopleEmbeddingRevision {
    pub model_id: String,
    pub model_version: String,
    pub model_cache_revision: String,
    pub dimension: usize,
    pub alignment_revision: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeopleClusterState {
    pub embedding: PeopleEmbeddingRevision,
    pub algorithm_revision: i64,
    pub similarity_threshold: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonCluster {
    pub person_id: String,
    pub representative_library_id: String,
    pub representative_face_id: String,
    pub member_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PersonClusterMember {
    pub library_id: String,
    pub face_id: String,
    pub person_id: Option<String>,
    pub assignment_similarity: Option<f32>,
    pub is_outlier: bool,
}

pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS people_cluster_state (
            singleton_id INTEGER PRIMARY KEY NOT NULL CHECK(singleton_id = 1),
            model_id TEXT NOT NULL,
            model_version TEXT NOT NULL,
            model_cache_revision TEXT NOT NULL,
            dimension INTEGER NOT NULL,
            alignment_revision INTEGER NOT NULL,
            algorithm_revision INTEGER NOT NULL,
            similarity_threshold REAL NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        );

        CREATE TABLE IF NOT EXISTS people_clusters (
            person_id TEXT PRIMARY KEY NOT NULL,
            representative_library_id TEXT NOT NULL,
            representative_face_id TEXT NOT NULL,
            member_count INTEGER NOT NULL CHECK(member_count > 0),
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        );

        CREATE TABLE IF NOT EXISTS people_cluster_members (
            library_id TEXT NOT NULL,
            face_id TEXT NOT NULL,
            person_id TEXT,
            assignment_similarity REAL,
            is_outlier INTEGER NOT NULL DEFAULT 0 CHECK(is_outlier IN (0, 1)),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
            PRIMARY KEY(library_id, face_id),
            FOREIGN KEY(person_id) REFERENCES people_clusters(person_id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_people_cluster_members_person
            ON people_cluster_members(person_id);
        CREATE INDEX IF NOT EXISTS idx_people_cluster_members_outlier
            ON people_cluster_members(is_outlier, person_id);
        "#,
    )?;
    Ok(())
}

pub fn replace_automatic_snapshot(
    conn: &mut Connection,
    state: &PeopleClusterState,
    clusters: &[PersonCluster],
    members: &[PersonClusterMember],
) -> Result<()> {
    validate_snapshot(state, clusters, members)?;
    ensure_schema(conn)?;

    let tx = conn.transaction()?;
    tx.execute("DELETE FROM people_cluster_members", [])?;
    tx.execute("DELETE FROM people_clusters", [])?;
    tx.execute("DELETE FROM people_cluster_state", [])?;

    tx.execute(
        r#"
        INSERT INTO people_cluster_state(
            singleton_id,
            model_id, model_version, model_cache_revision,
            dimension, alignment_revision,
            algorithm_revision, similarity_threshold,
            updated_at
        ) VALUES(1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, unixepoch())
        "#,
        params![
            state.embedding.model_id,
            state.embedding.model_version,
            state.embedding.model_cache_revision,
            state.embedding.dimension as i64,
            state.embedding.alignment_revision,
            state.algorithm_revision,
            state.similarity_threshold,
        ],
    )?;

    for cluster in clusters {
        tx.execute(
            r#"
            INSERT INTO people_clusters(
                person_id,
                representative_library_id, representative_face_id,
                member_count, created_at, updated_at
            ) VALUES(?1, ?2, ?3, ?4, unixepoch(), unixepoch())
            "#,
            params![
                cluster.person_id,
                cluster.representative_library_id,
                cluster.representative_face_id,
                cluster.member_count as i64,
            ],
        )?;
    }

    for member in members {
        tx.execute(
            r#"
            INSERT INTO people_cluster_members(
                library_id, face_id, person_id,
                assignment_similarity, is_outlier, updated_at
            ) VALUES(?1, ?2, ?3, ?4, ?5, unixepoch())
            "#,
            params![
                member.library_id,
                member.face_id,
                member.person_id,
                member.assignment_similarity,
                if member.is_outlier { 1 } else { 0 },
            ],
        )?;
    }

    tx.commit()
        .context("committing People clustering snapshot")?;
    Ok(())
}

pub fn load_state(conn: &Connection) -> Result<Option<PeopleClusterState>> {
    ensure_schema(conn)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT model_id, model_version, model_cache_revision,
               dimension, alignment_revision,
               algorithm_revision, similarity_threshold
        FROM people_cluster_state
        WHERE singleton_id = 1
        "#,
    )?;
    let mut rows = stmt.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(PeopleClusterState {
        embedding: PeopleEmbeddingRevision {
            model_id: row.get(0)?,
            model_version: row.get(1)?,
            model_cache_revision: row.get(2)?,
            dimension: row.get::<_, i64>(3)?.max(0) as usize,
            alignment_revision: row.get(4)?,
        },
        algorithm_revision: row.get(5)?,
        similarity_threshold: row.get(6)?,
    }))
}

pub fn load_clusters(conn: &Connection) -> Result<Vec<PersonCluster>> {
    ensure_schema(conn)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT person_id, representative_library_id, representative_face_id, member_count
        FROM people_clusters
        ORDER BY member_count DESC, person_id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(PersonCluster {
            person_id: row.get(0)?,
            representative_library_id: row.get(1)?,
            representative_face_id: row.get(2)?,
            member_count: row.get::<_, i64>(3)?.max(0) as usize,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("loading People clusters")
}

pub fn load_members(conn: &Connection) -> Result<Vec<PersonClusterMember>> {
    ensure_schema(conn)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT library_id, face_id, person_id, assignment_similarity, is_outlier
        FROM people_cluster_members
        ORDER BY library_id, face_id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(PersonClusterMember {
            library_id: row.get(0)?,
            face_id: row.get(1)?,
            person_id: row.get(2)?,
            assignment_similarity: row.get(3)?,
            is_outlier: row.get::<_, i64>(4)? != 0,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("loading People cluster members")
}

pub fn stable_person_id(
    embedding: &PeopleEmbeddingRevision,
    seed_library_id: &str,
    seed_face_id: &str,
) -> Result<String> {
    validate_embedding_revision(embedding)?;
    if seed_library_id.trim().is_empty() || seed_face_id.trim().is_empty() {
        bail!("People cluster seed must have a library id and face id");
    }

    let mut hash = 0xcbf29ce484222325u64;
    for value in [
        embedding.model_id.as_str(),
        embedding.model_version.as_str(),
        embedding.model_cache_revision.as_str(),
        seed_library_id,
        seed_face_id,
    ] {
        for byte in value.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let dimension = u64::try_from(embedding.dimension)
        .context("People embedding dimension does not fit stable id encoding")?;
    for byte in dimension.to_le_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for byte in embedding.alignment_revision.to_le_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Ok(format!("person-{hash:016x}"))
}

fn validate_snapshot(
    state: &PeopleClusterState,
    clusters: &[PersonCluster],
    members: &[PersonClusterMember],
) -> Result<()> {
    validate_embedding_revision(&state.embedding)?;
    if state.algorithm_revision <= 0 {
        bail!("People clustering algorithm revision must be positive");
    }
    if !state.similarity_threshold.is_finite()
        || !(-1.0..=1.0).contains(&state.similarity_threshold)
    {
        bail!("People clustering similarity threshold must be finite and within [-1, 1]");
    }

    let mut cluster_ids = HashSet::new();
    for cluster in clusters {
        if cluster.person_id.trim().is_empty()
            || cluster.representative_library_id.trim().is_empty()
            || cluster.representative_face_id.trim().is_empty()
            || cluster.member_count == 0
        {
            bail!("People cluster contains invalid identity or representative metadata");
        }
        if !cluster_ids.insert(cluster.person_id.as_str()) {
            bail!("duplicate People cluster id: {}", cluster.person_id);
        }
    }

    let mut member_keys = HashSet::new();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut representatives = HashSet::new();
    for member in members {
        if member.library_id.trim().is_empty() || member.face_id.trim().is_empty() {
            bail!("People cluster member must have a library id and face id");
        }
        if !member_keys.insert((member.library_id.as_str(), member.face_id.as_str())) {
            bail!(
                "duplicate People cluster member: {}/{}",
                member.library_id,
                member.face_id
            );
        }
        if member.is_outlier {
            if member.person_id.is_some() || member.assignment_similarity.is_some() {
                bail!("outlier face cannot also have a Person assignment");
            }
            continue;
        }
        let Some(person_id) = member.person_id.as_deref() else {
            bail!("non-outlier face must have a Person assignment");
        };
        if !cluster_ids.contains(person_id) {
            bail!("People cluster member references unknown person id: {person_id}");
        }
        let Some(similarity) = member.assignment_similarity else {
            bail!("assigned People cluster member must have assignment similarity");
        };
        if !similarity.is_finite() || !(-1.0..=1.0).contains(&similarity) {
            bail!("People assignment similarity must be finite and within [-1, 1]");
        }
        *counts.entry(person_id).or_default() += 1;
        representatives.insert((
            person_id,
            member.library_id.as_str(),
            member.face_id.as_str(),
        ));
    }

    for cluster in clusters {
        let actual = counts.get(cluster.person_id.as_str()).copied().unwrap_or(0);
        if actual != cluster.member_count {
            bail!(
                "People cluster {} declares {} members but snapshot contains {actual}",
                cluster.person_id,
                cluster.member_count
            );
        }
        if !representatives.contains(&(
            cluster.person_id.as_str(),
            cluster.representative_library_id.as_str(),
            cluster.representative_face_id.as_str(),
        )) {
            bail!("People cluster representative is not a member of its cluster");
        }
    }
    Ok(())
}

fn validate_embedding_revision(embedding: &PeopleEmbeddingRevision) -> Result<()> {
    if embedding.model_id.trim().is_empty()
        || embedding.model_version.trim().is_empty()
        || embedding.model_cache_revision.trim().is_empty()
        || embedding.dimension == 0
        || embedding.alignment_revision <= 0
    {
        bail!("invalid People embedding revision");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> PeopleClusterState {
        PeopleClusterState {
            embedding: PeopleEmbeddingRevision {
                model_id: "sface".to_owned(),
                model_version: "1".to_owned(),
                model_cache_revision: "1-abcdef".to_owned(),
                dimension: 128,
                alignment_revision: 2,
            },
            algorithm_revision: ALGORITHM_REVISION,
            similarity_threshold: 0.62,
        }
    }

    fn clustered_snapshot() -> (Vec<PersonCluster>, Vec<PersonClusterMember>) {
        let clusters = vec![PersonCluster {
            person_id: "person-a".to_owned(),
            representative_library_id: "library-1".to_owned(),
            representative_face_id: "face-1".to_owned(),
            member_count: 2,
        }];
        let members = vec![
            PersonClusterMember {
                library_id: "library-1".to_owned(),
                face_id: "face-1".to_owned(),
                person_id: Some("person-a".to_owned()),
                assignment_similarity: Some(1.0),
                is_outlier: false,
            },
            PersonClusterMember {
                library_id: "library-2".to_owned(),
                face_id: "face-2".to_owned(),
                person_id: Some("person-a".to_owned()),
                assignment_similarity: Some(0.81),
                is_outlier: false,
            },
            PersonClusterMember {
                library_id: "library-2".to_owned(),
                face_id: "face-outlier".to_owned(),
                person_id: None,
                assignment_similarity: None,
                is_outlier: true,
            },
        ];
        (clusters, members)
    }

    #[test]
    fn automatic_snapshot_round_trips_cross_root_membership_and_outliers() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        let (clusters, members) = clustered_snapshot();
        replace_automatic_snapshot(&mut conn, &state(), &clusters, &members).unwrap();

        assert_eq!(load_state(&conn).unwrap(), Some(state()));
        assert_eq!(load_clusters(&conn).unwrap(), clusters);
        assert_eq!(load_members(&conn).unwrap(), members);
    }

    #[test]
    fn replacing_snapshot_removes_stale_automatic_rows() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        let (clusters, members) = clustered_snapshot();
        replace_automatic_snapshot(&mut conn, &state(), &clusters, &members).unwrap();

        let replacement_cluster = PersonCluster {
            person_id: "person-b".to_owned(),
            representative_library_id: "library-3".to_owned(),
            representative_face_id: "face-3".to_owned(),
            member_count: 1,
        };
        let replacement_member = PersonClusterMember {
            library_id: "library-3".to_owned(),
            face_id: "face-3".to_owned(),
            person_id: Some("person-b".to_owned()),
            assignment_similarity: Some(1.0),
            is_outlier: false,
        };
        replace_automatic_snapshot(
            &mut conn,
            &state(),
            std::slice::from_ref(&replacement_cluster),
            std::slice::from_ref(&replacement_member),
        )
        .unwrap();

        assert_eq!(load_clusters(&conn).unwrap(), vec![replacement_cluster]);
        assert_eq!(load_members(&conn).unwrap(), vec![replacement_member]);
    }

    #[test]
    fn snapshot_rejects_representative_that_is_not_a_member() {
        let mut conn = Connection::open_in_memory().unwrap();
        let cluster = PersonCluster {
            person_id: "person-a".to_owned(),
            representative_library_id: "library-1".to_owned(),
            representative_face_id: "missing-face".to_owned(),
            member_count: 1,
        };
        let member = PersonClusterMember {
            library_id: "library-1".to_owned(),
            face_id: "face-1".to_owned(),
            person_id: Some("person-a".to_owned()),
            assignment_similarity: Some(1.0),
            is_outlier: false,
        };
        let err = replace_automatic_snapshot(
            &mut conn,
            &state(),
            std::slice::from_ref(&cluster),
            std::slice::from_ref(&member),
        )
        .unwrap_err();
        assert!(err.to_string().contains("representative"));
    }

    #[test]
    fn stable_person_id_is_deterministic_and_revision_sensitive() {
        let embedding = state().embedding;
        let first = stable_person_id(&embedding, "library-1", "face-1").unwrap();
        assert_eq!(
            first,
            stable_person_id(&embedding, "library-1", "face-1").unwrap()
        );
        let mut changed = embedding.clone();
        changed.model_cache_revision.push_str("-new");
        assert_ne!(
            first,
            stable_person_id(&changed, "library-1", "face-1").unwrap()
        );
    }
}
