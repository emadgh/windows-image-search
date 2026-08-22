# People clustering persistence contract

People clustering is a derived, central catalog built from authoritative face embeddings stored in each portable root `.imagesearch` database.

## Storage boundaries

- Face detections and SFace embeddings remain portable with the indexed root.
- Automatic People clusters live in the central session database so one identity may span multiple roots.
- A face is addressed by `(library_id, face_id)` rather than an absolute path.
- Automatic clustering tables are disposable/rebuildable derived state.
- Future user edits from issue #60 (names, manual merges/splits, ignored faces, representative overrides) must be stored separately and must not be deleted by an automatic rebuild.

## Clustering behavior

- Only current, normalized embeddings from one compatible SFace model/cache/alignment revision participate in a run.
- The one-shot Face Search threshold and People clustering threshold are independent settings.
- People settings persist the identity threshold and minimum faces per Person; the defaults are `0.62` and `2`.
- The automatic snapshot records both values. A model/cache/alignment/algorithm/threshold/minimum-size mismatch forces a full rebuild rather than incrementally mixing incompatible clustering semantics.
- For fewer than 512 compatible faces, full clustering uses the deterministic exact centroid path; this keeps small libraries and regression fixtures exact.
- At 512 faces or more, an in-memory cosine HNSW index bounds candidate retrieval. The graph uses M=24, ef-construction=200, up to 16 layers, and 64 nearest candidate faces per query with extra search breadth.
- HNSW is candidate retrieval only: candidate assignments and cluster merges are accepted using exact cosine similarity against the current cluster centroids.
- Weak-member pruning remains authoritative after candidate clustering, so bridge/chained or ambiguous faces can be detached instead of forcing two identities together.
- A cluster must have at least two members by default; singletons and weak/ambiguous faces remain outliers.
- Full rebuilds reconcile new memberships with previous Person IDs by overlap so IDs remain stable when possible.
- If one previous cluster splits, at most one resulting cluster inherits its old Person ID; other clusters receive deterministic new IDs.
- Stable Person ID hashing encodes the embedding dimension as a fixed-width 64-bit value, avoiding platform-width-dependent IDs.

## Incremental maintenance

After a successful SFace backfill, the automatic path performs an incremental People update rather than a full recluster when the stored snapshot is compatible.

- Already assigned faces are preserved.
- Newly indexed faces and previous outliers are the candidate set.
- Candidates are compared with the current Person representative embeddings using exact cosine similarity.
- A candidate attaches only when its best Person is above the configured threshold and is separated from the second-best qualifying Person by at least `0.04`; ambiguous candidates remain unassigned for the next clustering step.
- Remaining candidates are clustered among themselves, allowing a previous singleton/outlier plus a newly indexed matching face to form a new Person group.
- If a previously known face disappeared or became stale, the update falls back to a full rebuild so stale memberships cannot survive source deletion or embedding invalidation.
- If no compatible snapshot exists, or clustering settings/model revisions changed, the update also falls back to a full rebuild.

This path still scans current embedding keys to detect additions/deletions, but it avoids rebuilding the global HNSW/centroid grouping when only new faces were added.

## Scaling intent

The large-corpus full-rebuild path avoids scanning every existing cluster centroid for every face and avoids the previous all-pairs cluster merge pass. HNSW narrows both assignment and merge candidates while exact centroid cosine and pruning keep the decision semantics conservative. The HNSW structure is rebuilt in memory from the current SFace embeddings because automatic People state is derived data; persistence remains the People snapshot, not the transient neighbor graph.

## Runtime integration

- A successful SFace embedding backfill schedules the incremental People update automatically as a separate background maintenance step. A clustering failure does not invalidate the completed detection/embedding work.
- Settings exposes separate `Identity threshold` and `Minimum faces per Person` controls.
- Settings also exposes `Rebuild People groups from current embeddings` for an explicit full rebuild without re-running YuNet or SFace inference.
- Face Search first reads automatic People clusters and shows one representative crop per Person. Each Person card exposes its cluster face count.
- If no People snapshot exists yet, Face Search falls back to the previous searchable face-instance gallery, so Face Search remains usable before clustering is available.
- Completing People maintenance refreshes the Face Search suggestion source.

## Regression coverage

The People tests include in-memory clustering/reconciliation cases plus persisted SQLite/portable-index scenarios. Persisted coverage verifies that a newly indexed face joins an existing Person without changing its stable Person ID, a previous outlier can form a new Person when a matching face arrives later, and changing the clustering threshold forces a compatible full rebuild rather than mixing old and new snapshot semantics. Test temporary directories use only the Rust standard library, so this feature adds no test/runtime dependency solely for temporary files.

## Remaining #59 work

1. Record practical clustering quality/performance evidence on a real multi-person library before closing #59.
2. Build manual People management/overrides in #60 without mutating automatic clustering state.
