from pathlib import Path

people = Path('src/people_clustering.rs')
text = people.read_text(encoding='utf-8')

text = text.replace(
    "use anyhow::{bail, Context, Result};\nuse rusqlite::{params, Connection, OpenFlags, OptionalExtension};\nuse std::collections::{HashMap, HashSet};",
    "use anyhow::{bail, Context, Result};\nuse hnsw_rs::prelude::{AnnT, DistCosine, Hnsw};\nuse rusqlite::{params, Connection, OpenFlags, OptionalExtension};\nuse std::collections::{HashMap, HashSet};"
)

text = text.replace(
    "const READ_BATCH_SIZE: usize = 512;",
    "const READ_BATCH_SIZE: usize = 512;\nconst HNSW_MIN_FACES: usize = 512;\nconst HNSW_MAX_CONNECTIONS: usize = 24;\nconst HNSW_MAX_LAYERS: usize = 16;\nconst HNSW_EF_CONSTRUCTION: usize = 200;\nconst HNSW_CANDIDATE_NEIGHBORS: usize = 64;\nconst HNSW_EF_SEARCH_EXTRA: usize = 192;"
)

old = '''    let mut work: Vec<WorkCluster> = Vec::new();
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
'''

new = '''    let mut work = if faces.len() >= HNSW_MIN_FACES {
        cluster_faces_hnsw(faces, options.similarity_threshold)?
    } else {
        cluster_faces_exact(faces, options.similarity_threshold)?
    };
    prune_weak_members(&mut work, faces, options.similarity_threshold)?;
'''

if old not in text:
    raise SystemExit('cluster_faces seed block not found')
text = text.replace(old, new, 1)

marker = "fn merge_compatible_clusters(clusters: &mut Vec<WorkCluster>, threshold: f32) -> Result<()> {\n"
if marker not in text:
    raise SystemExit('merge marker not found')

helpers = r'''fn cluster_faces_exact(faces: &[ClusterFace], threshold: f32) -> Result<Vec<WorkCluster>> {
    let mut work: Vec<WorkCluster> = Vec::new();
    for (face_index, face) in faces.iter().enumerate() {
        let mut best: Option<(usize, f32)> = None;
        for (cluster_index, cluster) in work.iter().enumerate() {
            let similarity = cosine(&face.values, &cluster.centroid)?;
            if similarity < threshold {
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
    merge_compatible_clusters(&mut work, threshold)?;
    Ok(work)
}

fn cluster_faces_hnsw(faces: &[ClusterFace], threshold: f32) -> Result<Vec<WorkCluster>> {
    if faces.is_empty() {
        return Ok(Vec::new());
    }

    let hnsw = Hnsw::<f32, DistCosine>::new(
        HNSW_MAX_CONNECTIONS,
        faces.len(),
        HNSW_MAX_LAYERS,
        HNSW_EF_CONSTRUCTION,
        DistCosine {},
    );
    let refs: Vec<(&Vec<f32>, usize)> = faces
        .iter()
        .enumerate()
        .map(|(index, face)| (&face.values, index))
        .collect();
    hnsw.parallel_insert(&refs);

    let mut work: Vec<WorkCluster> = Vec::new();
    let mut face_cluster = vec![usize::MAX; faces.len()];
    let k = HNSW_CANDIDATE_NEIGHBORS.saturating_add(1).min(faces.len());
    let ef = k
        .saturating_add(HNSW_EF_SEARCH_EXTRA)
        .min(faces.len())
        .max(k);

    for (face_index, face) in faces.iter().enumerate() {
        let mut candidates = Vec::new();
        for neighbour in hnsw.search(&face.values, k, ef) {
            let neighbour_index = neighbour.d_id;
            if neighbour_index >= face_index || neighbour_index >= face_cluster.len() {
                continue;
            }
            let cluster_index = face_cluster[neighbour_index];
            if cluster_index != usize::MAX {
                candidates.push(cluster_index);
            }
        }
        candidates.sort_unstable();
        candidates.dedup();

        let mut best: Option<(usize, f32)> = None;
        for cluster_index in candidates {
            let similarity = cosine(&face.values, &work[cluster_index].centroid)?;
            if similarity < threshold {
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

        let cluster_index = if let Some((cluster_index, _)) = best {
            add_face_to_cluster(&mut work[cluster_index], face_index, &face.values)?;
            cluster_index
        } else {
            let cluster_index = work.len();
            work.push(WorkCluster {
                members: vec![face_index],
                sum: face.values.clone(),
                centroid: face.values.clone(),
            });
            cluster_index
        };
        face_cluster[face_index] = cluster_index;
    }

    if work.len() <= 1 {
        return Ok(work);
    }

    let mut sets = DisjointSet::new(work.len());
    for left in 0..work.len() {
        let mut candidates = Vec::new();
        for neighbour in hnsw.search(&work[left].centroid, k, ef) {
            if neighbour.d_id >= face_cluster.len() {
                continue;
            }
            let right = face_cluster[neighbour.d_id];
            if right != usize::MAX && right != left {
                candidates.push(right);
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        for right in candidates {
            if right <= left {
                continue;
            }
            if cosine(&work[left].centroid, &work[right].centroid)? >= threshold {
                sets.union(left, right);
            }
        }
    }

    merge_disjoint_clusters(work, &mut sets)
}

fn merge_disjoint_clusters(
    clusters: Vec<WorkCluster>,
    sets: &mut DisjointSet,
) -> Result<Vec<WorkCluster>> {
    if clusters.is_empty() {
        return Ok(Vec::new());
    }
    let dimension = clusters[0].centroid.len();
    let mut grouped: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in 0..clusters.len() {
        grouped.entry(sets.find(index)).or_default().push(index);
    }
    let mut roots = grouped.keys().copied().collect::<Vec<_>>();
    roots.sort_unstable();

    let mut merged = Vec::with_capacity(roots.len());
    for root in roots {
        let indexes = grouped.remove(&root).unwrap_or_default();
        let mut members = Vec::new();
        let mut sum = vec![0.0f32; dimension];
        for index in indexes {
            members.extend(clusters[index].members.iter().copied());
            for (slot, value) in sum.iter_mut().zip(clusters[index].sum.iter()) {
                *slot += *value;
            }
        }
        members.sort_unstable();
        let centroid = normalize(sum.clone())?;
        merged.push(WorkCluster {
            members,
            sum,
            centroid,
        });
    }
    merged.sort_by_key(|cluster| cluster.members.first().copied().unwrap_or(usize::MAX));
    Ok(merged)
}

struct DisjointSet {
    parent: Vec<usize>,
}

impl DisjointSet {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    fn find(&mut self, index: usize) -> usize {
        let parent = self.parent[index];
        if parent == index {
            return index;
        }
        let root = self.find(parent);
        self.parent[index] = root;
        root
    }

    fn union(&mut self, left: usize, right: usize) {
        let left_root = self.find(left);
        let right_root = self.find(right);
        if left_root == right_root {
            return;
        }
        let (keep, attach) = if left_root < right_root {
            (left_root, right_root)
        } else {
            (right_root, left_root)
        };
        self.parent[attach] = keep;
    }
}

'''
text = text.replace(marker, helpers + marker, 1)

# Exercise the HNSW path directly without making every existing small regression test expensive.
test_marker = "    #[test]\n    fn weak_chain_member_is_pruned_instead_of_forced_into_person() {\n"
if test_marker not in text:
    raise SystemExit('test insertion marker not found')
hnsw_test = r'''    #[test]
    fn hnsw_candidate_path_separates_dense_identities_without_quadratic_scan() {
        let mut faces = Vec::new();
        for index in 0..48 {
            faces.push(face(
                "a",
                &format!("alice-{index:03}"),
                unit(1.0, (index as f32) * 0.0005, 0.0),
            ));
            faces.push(face(
                "b",
                &format!("bob-{index:03}"),
                unit((index as f32) * 0.0005, 1.0, 0.0),
            ));
        }
        sort_faces(&mut faces);
        let mut clusters = cluster_faces_hnsw(&faces, 0.90).unwrap();
        prune_weak_members(&mut clusters, &faces, 0.90).unwrap();
        let mut sizes = clusters
            .iter()
            .filter(|cluster| cluster.members.len() >= 2)
            .map(|cluster| cluster.members.len())
            .collect::<Vec<_>>();
        sizes.sort_unstable();
        assert_eq!(sizes, vec![48, 48]);
    }

'''
text = text.replace(test_marker, hnsw_test + test_marker, 1)
people.write_text(text, encoding='utf-8')

store = Path('src/people_store.rs')
store_text = store.read_text(encoding='utf-8')
store_text = store_text.replace('pub const ALGORITHM_REVISION: i64 = 1;', 'pub const ALGORITHM_REVISION: i64 = 2;')
old_dim = '''    for byte in embedding.dimension.to_le_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
'''
new_dim = '''    let dimension = u64::try_from(embedding.dimension)
        .context("People embedding dimension does not fit stable id encoding")?;
    for byte in dimension.to_le_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
'''
if old_dim not in store_text:
    raise SystemExit('stable id dimension block not found')
store_text = store_text.replace(old_dim, new_dim, 1)
store.write_text(store_text, encoding='utf-8')
