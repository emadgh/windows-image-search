use crate::{db, face_embedding, people_store, portable};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const READ_BATCH_SIZE: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeopleClusteringOptions {
    pub similarity_threshold: f32,
    pub min_cluster_size: usize,
}

impl Default for PeopleClusteringOptions {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.62,
            min_cluster_size: 2,
        }
    }
}

impl PeopleClusteringOptions {
    pub fn sanitized(self) -> Self {
        Self {
            similarity_threshold: if self.similarity_threshold.is_finite() {
                self.similarity_threshold.clamp(-1.0, 1.0)
            } else {
                Self::default().similarity_threshold
            },
            min_cluster_size: self.min_cluster_size.clamp(2, 1_000_000),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeopleClusteringSummary {
    pub roots_scanned: usize,
    pub roots_unavailable: usize,
    pub faces_loaded: usize,
    pub people_created: usize,
    pub faces_clustered: usize,
    pub outliers: usize,
    pub reused_person_ids: usize,
}

#[derive(Clone, Debug, PartialEq)]
struct ClusterFace {
    library_id: String,
    face_id: String,
    values: Vec<f32>,
}

#[derive(Clone, Debug)]
struct WorkCluster {
    members: Vec<usize>,
    sum: Vec<f32>,
    centroid: Vec<f32>,
}

#[derive(Clone, Debug)]
struct FinalCluster {
    members: Vec<usize>,
    centroid: Vec<f32>,
    representative: usize,
}

pub fn run(
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
    let previous_members = people_store::load_members(&conn).unwrap_or_default();
    let previous = previous_assignment_map(&previous_members);

    let clustered = cluster_faces(&faces, options, &previous, embedding)?;
    let state = people_store::PeopleClusterState {
        embedding: embedding.clone(),
        algorithm_revision: people_store::ALGORITHM_REVISION,
        similarity_threshold: options.similarity_threshold,
    };
    people_store::replace_automatic_snapshot(
        &mut conn,
        &state,
        &clustered.clusters,
        &clustered.members,
    )?;

    Ok(PeopleClusteringSummary {
        roots_scanned,
        roots_unavailable,
        faces_loaded: faces.len(),
        people_created: clustered.clusters.len(),
        faces_clustered: clustered
            .members
            .iter()
            .filter(|member| member.person_id.is_some())
            .count(),
        outliers: clustered
            .members
            .iter()
            .filter(|member| member.is_outlier)
            .count(),
        reused_person_ids: clustered.reused_person_ids,
    })
}

struct ClusteredSnapshot {
    clusters: Vec<people_store::PersonCluster>,
    members: Vec<people_store::PersonClusterMember>,
    reused_person_ids: usize,
}

fn cluster_faces(
    faces: &[ClusterFace],
    options: PeopleClusteringOptions,
    previous: &HashMap<(String, String), String>,
    embedding: &people_store::PeopleEmbeddingRevision,
) -> Result<ClusteredSnapshot> {
    if faces.is_empty() {
        return Ok(ClusteredSnapshot {
            clusters: Vec::new(),
            members: Vec::new(),
            reused_person_ids: 0,
        });
    }
    let dimension = embedding.dimension;
    if faces.iter().any(|face| face.values.len() != dimension) {
        bail!("People clustering received mixed embedding dimensions");
    }

    let mut work: Vec<WorkCluster> = Vec::new();
    for (face_index, face) in faces.iter().enumerate() {
        let mut best: Option<(usize, f32)> = None;
        for (cluster_index, cluster) in work.iter().enumerate() {
            let similarity = cosine(&face.values, &cluster.centroid)?;
            if similarity < options.similarity_threshold {
                continue;
            }
            match best {
                None => best = Some((cluster_index, similarity)),
                Some((best_index, best_similarity)) => {
                    if similarity > best_similarity
                        || (similarity == best_similarity && cluster_index < best_index)
                    {
                        best = Some((cluster_index, similarity));
                    }
                }
            }
        }

        if let Some((cluster_index, _)) = best {
            add_face_to_cluster(&mut work[cluster_index], face_index, &face.values)?;
        } else {
            work.push(WorkCluster {
                members: vec![face_index],
                sum: face.values.clone(),
                centroid: face.values.clone(),
            });
        }
    }

    merge_compatible_clusters(&mut work, options.similarity_threshold)?;
    prune_weak_members(&mut work, faces, options.similarity_threshold)?;

    let mut final_clusters = Vec::new();
    let mut outliers = HashSet::new();
    for cluster in work {
        if cluster.members.len() < options.min_cluster_size {
            outliers.extend(cluster.members);
            continue;
        }
        let representative = choose_representative(&cluster, faces)?;
        final_clusters.push(FinalCluster {
            members: cluster.members,
            centroid: cluster.centroid,
            representative,
        });
    }
    final_clusters.sort_by(|left, right| {
        face_key(&faces[left.representative]).cmp(&face_key(&faces[right.representative]))
    });

    let person_ids = reconcile_person_ids(&final_clusters, faces, previous, embedding)?;
    let reused_person_ids = person_ids.iter().filter(|(_, reused)| *reused).count();
    let mut clusters = Vec::with_capacity(final_clusters.len());
    let mut members = Vec::with_capacity(faces.len());

    for (cluster_index, cluster) in final_clusters.iter().enumerate() {
        let (person_id, _) = &person_ids[cluster_index];
        let representative = &faces[cluster.representative];
        clusters.push(people_store::PersonCluster {
            person_id: person_id.clone(),
            representative_library_id: representative.library_id.clone(),
            representative_face_id: representative.face_id.clone(),
            member_count: cluster.members.len(),
        });
        for &member_index in &cluster.members {
            let face = &faces[member_index];
            members.push(people_store::PersonClusterMember {
                library_id: face.library_id.clone(),
                face_id: face.face_id.clone(),
                person_id: Some(person_id.clone()),
                assignment_similarity: Some(cosine(&face.values, &cluster.centroid)?),
                is_outlier: false,
            });
        }
    }

    for (index, face) in faces.iter().enumerate() {
        if outliers.contains(&index) {
            members.push(people_store::PersonClusterMember {
                library_id: face.library_id.clone(),
                face_id: face.face_id.clone(),
                person_id: None,
                assignment_similarity: None,
                is_outlier: true,
            });
        }
    }
    members.sort_by(|left, right| {
        left.library_id
            .cmp(&right.library_id)
            .then_with(|| left.face_id.cmp(&right.face_id))
    });

    Ok(ClusteredSnapshot {
        clusters,
        members,
        reused_person_ids,
    })
}

fn merge_compatible_clusters(clusters: &mut Vec<WorkCluster>, threshold: f32) -> Result<()> {
    loop {
        let mut best: Option<(usize, usize, f32)> = None;
        for left in 0..clusters.len() {
            for right in (left + 1)..clusters.len() {
                let similarity = cosine(&clusters[left].centroid, &clusters[right].centroid)?;
                if similarity < threshold {
                    continue;
                }
                match best {
                    None => best = Some((left, right, similarity)),
                    Some((best_left, best_right, best_similarity)) => {
                        if similarity > best_similarity
                            || (similarity == best_similarity
                                && (left, right) < (best_left, best_right))
                        {
                            best = Some((left, right, similarity));
                        }
                    }
                }
            }
        }
        let Some((left, right, _)) = best else {
            break;
        };
        let removed = clusters.remove(right);
        for (sum, value) in clusters[left].sum.iter_mut().zip(removed.sum.iter()) {
            *sum += *value;
        }
        clusters[left].members.extend(removed.members);
        clusters[left].members.sort_unstable();
        clusters[left].centroid = normalize(clusters[left].sum.clone())?;
    }
    Ok(())
}

fn prune_weak_members(
    clusters: &mut Vec<WorkCluster>,
    faces: &[ClusterFace],
    threshold: f32,
) -> Result<()> {
    let mut detached = Vec::new();
    for cluster in clusters.iter_mut() {
        loop {
            if cluster.members.len() <= 1 {
                break;
            }
            let centroid = cluster.centroid.clone();
            let weak: Vec<usize> = cluster
                .members
                .iter()
                .copied()
                .filter(|index| {
                    cosine(&faces[*index].values, &centroid)
                        .map(|similarity| similarity < threshold)
                        .unwrap_or(true)
                })
                .collect();
            if weak.is_empty() {
                break;
            }
            let weak_set: HashSet<usize> = weak.iter().copied().collect();
            cluster.members.retain(|index| !weak_set.contains(index));
            detached.extend(weak);
            if cluster.members.is_empty() {
                cluster.sum.fill(0.0);
                cluster.centroid.fill(0.0);
                break;
            }
            cluster.sum = vec![0.0; centroid.len()];
            for &index in &cluster.members {
                for (sum, value) in cluster.sum.iter_mut().zip(faces[index].values.iter()) {
                    *sum += *value;
                }
            }
            cluster.centroid = normalize(cluster.sum.clone())?;
        }
    }
    clusters.retain(|cluster| !cluster.members.is_empty());
    for index in detached {
        clusters.push(WorkCluster {
            members: vec![index],
            sum: faces[index].values.clone(),
            centroid: faces[index].values.clone(),
        });
    }
    clusters.sort_by_key(|cluster| cluster.members.iter().min().copied().unwrap_or(usize::MAX));
    Ok(())
}

fn choose_representative(cluster: &WorkCluster, faces: &[ClusterFace]) -> Result<usize> {
    let mut best = cluster.members[0];
    let mut best_similarity = cosine(&faces[best].values, &cluster.centroid)?;
    for &index in cluster.members.iter().skip(1) {
        let similarity = cosine(&faces[index].values, &cluster.centroid)?;
        if similarity > best_similarity
            || (similarity == best_similarity && face_key(&faces[index]) < face_key(&faces[best]))
        {
            best = index;
            best_similarity = similarity;
        }
    }
    Ok(best)
}

fn reconcile_person_ids(
    clusters: &[FinalCluster],
    faces: &[ClusterFace],
    previous: &HashMap<(String, String), String>,
    embedding: &people_store::PeopleEmbeddingRevision,
) -> Result<Vec<(String, bool)>> {
    let mut overlap_candidates = Vec::new();
    for (cluster_index, cluster) in clusters.iter().enumerate() {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for &member_index in &cluster.members {
            let face = &faces[member_index];
            if let Some(person_id) = previous.get(&(face.library_id.clone(), face.face_id.clone()))
            {
                *counts.entry(person_id.as_str()).or_default() += 1;
            }
        }
        for (person_id, count) in counts {
            overlap_candidates.push((
                count,
                cluster.members.len(),
                person_id.to_owned(),
                cluster_index,
            ));
        }
    }
    overlap_candidates.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.3.cmp(&right.3))
    });

    let mut assigned_cluster = HashSet::new();
    let mut used_person_id = HashSet::new();
    let mut output: Vec<Option<(String, bool)>> = vec![None; clusters.len()];
    for (_, _, person_id, cluster_index) in overlap_candidates {
        if assigned_cluster.contains(&cluster_index) || used_person_id.contains(&person_id) {
            continue;
        }
        assigned_cluster.insert(cluster_index);
        used_person_id.insert(person_id.clone());
        output[cluster_index] = Some((person_id, true));
    }

    for (cluster_index, cluster) in clusters.iter().enumerate() {
        if output[cluster_index].is_some() {
            continue;
        }
        let seed_index = *cluster
            .members
            .iter()
            .min_by_key(|index| face_key(&faces[**index]))
            .context("People cluster has no members")?;
        let seed = &faces[seed_index];
        let person_id = people_store::stable_person_id(embedding, &seed.library_id, &seed.face_id)?;
        output[cluster_index] = Some((person_id, false));
    }

    Ok(output.into_iter().map(Option::unwrap).collect())
}

fn previous_assignment_map(
    members: &[people_store::PersonClusterMember],
) -> HashMap<(String, String), String> {
    members
        .iter()
        .filter_map(|member| {
            member.person_id.as_ref().map(|person_id| {
                (
                    (member.library_id.clone(), member.face_id.clone()),
                    person_id.clone(),
                )
            })
        })
        .collect()
}

fn add_face_to_cluster(cluster: &mut WorkCluster, index: usize, values: &[f32]) -> Result<()> {
    for (sum, value) in cluster.sum.iter_mut().zip(values.iter()) {
        *sum += *value;
    }
    cluster.members.push(index);
    cluster.centroid = normalize(cluster.sum.clone())?;
    Ok(())
}

fn load_root_faces(
    root: &Path,
    revision: &people_store::PeopleEmbeddingRevision,
) -> Result<Vec<ClusterFace>> {
    if !root.is_dir() {
        bail!("portable root unavailable: {}", root.display());
    }
    let db_path = portable::index_db_path(root);
    let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening portable People source {}", db_path.display()))?;
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

    let mut output = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut stmt = conn.prepare(
            r#"
            SELECT f.face_id, e.embedding
            FROM face_embeddings e
            JOIN faces f ON f.face_id = e.face_id
            JOIN face_detection_state s ON s.image_path = f.image_path
            JOIN images i ON i.path = f.image_path
            WHERE (?1 IS NULL OR f.face_id > ?1)
              AND e.model_id = ?2
              AND e.model_version = ?3
              AND e.model_cache_revision = ?4
              AND e.schema_version = ?5
              AND e.alignment_revision = ?6
              AND e.dimension = ?7
              AND e.normalized = 1
              AND e.detector_id = f.detector_id
              AND e.detector_version = f.detector_version
              AND e.detector_cache_revision = f.detector_cache_revision
              AND e.detection_schema_version = f.schema_version
              AND e.source_size = f.source_size
              AND e.source_modified = f.source_modified
              AND s.detector_id = f.detector_id
              AND s.detector_version = f.detector_version
              AND s.detector_cache_revision = f.detector_cache_revision
              AND s.schema_version = f.schema_version
              AND s.source_size = f.source_size
              AND s.source_modified = f.source_modified
              AND i.size = f.source_size
              AND i.modified = f.source_modified
            ORDER BY f.face_id
            LIMIT ?8
            "#,
        )?;
        let rows = stmt.query_map(
            params![
                cursor,
                revision.model_id,
                revision.model_version,
                revision.model_cache_revision,
                face_embedding::SCHEMA_VERSION,
                revision.alignment_revision,
                revision.dimension as i64,
                READ_BATCH_SIZE as i64,
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )?;
        let batch = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        if batch.is_empty() {
            break;
        }
        for (face_id, blob) in batch {
            cursor = Some(face_id.clone());
            let Some(values) = decode_embedding(&blob, revision.dimension) else {
                continue;
            };
            if !is_normalized(&values) {
                continue;
            }
            output.push(ClusterFace {
                library_id: library_id.clone(),
                face_id,
                values,
            });
        }
    }
    Ok(output)
}

fn validate_embedding_revision(revision: &people_store::PeopleEmbeddingRevision) -> Result<()> {
    if revision.model_id.trim().is_empty()
        || revision.model_version.trim().is_empty()
        || revision.model_cache_revision.trim().is_empty()
        || revision.dimension == 0
        || revision.alignment_revision <= 0
    {
        bail!("invalid People embedding revision");
    }
    Ok(())
}

fn sort_faces(faces: &mut [ClusterFace]) {
    faces.sort_by(|left, right| face_key(left).cmp(&face_key(right)));
}

fn face_key(face: &ClusterFace) -> (&str, &str) {
    (&face.library_id, &face.face_id)
}

fn cosine(left: &[f32], right: &[f32]) -> Result<f32> {
    if left.len() != right.len() || left.is_empty() {
        bail!("People clustering cosine dimension mismatch");
    }
    Ok(left
        .iter()
        .zip(right.iter())
        .map(|(a, b)| a * b)
        .sum::<f32>()
        .clamp(-1.0, 1.0))
}

fn normalize(values: Vec<f32>) -> Result<Vec<f32>> {
    let norm_sq = values
        .iter()
        .map(|value| (*value as f64) * (*value as f64))
        .sum::<f64>();
    if norm_sq <= f64::EPSILON {
        bail!("People clustering centroid has zero length");
    }
    let norm = norm_sq.sqrt() as f32;
    Ok(values.into_iter().map(|value| value / norm).collect())
}

fn decode_embedding(bytes: &[u8], dimension: usize) -> Option<Vec<f32>> {
    if bytes.len() != dimension.checked_mul(4)? {
        return None;
    }
    Some(
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect(),
    )
}

fn is_normalized(values: &[f32]) -> bool {
    if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
        return false;
    }
    let norm = values
        .iter()
        .map(|value| (*value as f64) * (*value as f64))
        .sum::<f64>()
        .sqrt();
    (norm - 1.0).abs() <= 1e-3
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision() -> people_store::PeopleEmbeddingRevision {
        people_store::PeopleEmbeddingRevision {
            model_id: "sface".to_owned(),
            model_version: "1".to_owned(),
            model_cache_revision: "revision-a".to_owned(),
            dimension: 3,
            alignment_revision: 2,
        }
    }

    fn unit(x: f32, y: f32, z: f32) -> Vec<f32> {
        normalize(vec![x, y, z]).unwrap()
    }

    fn face(library: &str, id: &str, values: Vec<f32>) -> ClusterFace {
        ClusterFace {
            library_id: library.to_owned(),
            face_id: id.to_owned(),
            values,
        }
    }

    #[test]
    fn obvious_same_people_cluster_and_singletons_remain_outliers() {
        let mut faces = vec![
            face("a", "alice-1", unit(1.0, 0.02, 0.0)),
            face("b", "alice-2", unit(0.99, -0.01, 0.0)),
            face("a", "bob-1", unit(0.0, 1.0, 0.0)),
            face("b", "bob-2", unit(0.02, 0.99, 0.0)),
            face("a", "outlier", unit(0.0, 0.0, 1.0)),
        ];
        sort_faces(&mut faces);
        let snapshot = cluster_faces(
            &faces,
            PeopleClusteringOptions {
                similarity_threshold: 0.8,
                min_cluster_size: 2,
            },
            &HashMap::new(),
            &revision(),
        )
        .unwrap();
        assert_eq!(snapshot.clusters.len(), 2);
        assert_eq!(
            snapshot
                .members
                .iter()
                .filter(|member| member.person_id.is_some())
                .count(),
            4
        );
        assert_eq!(
            snapshot
                .members
                .iter()
                .filter(|member| member.is_outlier)
                .count(),
            1
        );
    }

    #[test]
    fn weak_chain_member_is_pruned_instead_of_forced_into_person() {
        let mut faces = vec![
            face("a", "p1", unit(1.0, 0.0, 0.0)),
            face("a", "p2", unit(0.8, 0.6, 0.0)),
            face("a", "p3", unit(0.28, 0.96, 0.0)),
        ];
        sort_faces(&mut faces);
        let snapshot = cluster_faces(
            &faces,
            PeopleClusteringOptions {
                similarity_threshold: 0.72,
                min_cluster_size: 2,
            },
            &HashMap::new(),
            &revision(),
        )
        .unwrap();
        assert!(snapshot.members.iter().any(|member| member.is_outlier));
    }

    #[test]
    fn incremental_face_reuses_existing_person_id_by_membership_overlap() {
        let mut initial = vec![
            face("a", "face-1", unit(1.0, 0.0, 0.0)),
            face("b", "face-2", unit(0.99, 0.02, 0.0)),
        ];
        sort_faces(&mut initial);
        let first = cluster_faces(
            &initial,
            PeopleClusteringOptions {
                similarity_threshold: 0.8,
                min_cluster_size: 2,
            },
            &HashMap::new(),
            &revision(),
        )
        .unwrap();
        let original_id = first.clusters[0].person_id.clone();
        let previous = previous_assignment_map(&first.members);

        let mut updated = initial;
        updated.push(face("c", "face-3", unit(0.98, -0.02, 0.0)));
        sort_faces(&mut updated);
        let second = cluster_faces(
            &updated,
            PeopleClusteringOptions {
                similarity_threshold: 0.8,
                min_cluster_size: 2,
            },
            &previous,
            &revision(),
        )
        .unwrap();
        assert_eq!(second.clusters[0].person_id, original_id);
        assert_eq!(second.reused_person_ids, 1);
    }

    #[test]
    fn split_reuses_old_person_id_for_only_one_result_cluster() {
        let faces = vec![
            face("a", "a1", unit(1.0, 0.0, 0.0)),
            face("a", "a2", unit(0.99, 0.01, 0.0)),
            face("a", "b1", unit(0.0, 1.0, 0.0)),
            face("a", "b2", unit(0.01, 0.99, 0.0)),
        ];
        let previous = faces
            .iter()
            .map(|face| {
                (
                    (face.library_id.clone(), face.face_id.clone()),
                    "old-person".to_owned(),
                )
            })
            .collect();
        let snapshot = cluster_faces(
            &faces,
            PeopleClusteringOptions {
                similarity_threshold: 0.8,
                min_cluster_size: 2,
            },
            &previous,
            &revision(),
        )
        .unwrap();
        assert_eq!(snapshot.clusters.len(), 2);
        assert_eq!(
            snapshot
                .clusters
                .iter()
                .filter(|cluster| cluster.person_id == "old-person")
                .count(),
            1
        );
    }
}
