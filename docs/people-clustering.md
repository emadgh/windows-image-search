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
- For fewer than 512 compatible faces, clustering uses the deterministic exact centroid path; this keeps small libraries and regression fixtures exact.
- At 512 faces or more, an in-memory cosine HNSW index bounds candidate retrieval. The graph uses M=24, ef-construction=200, up to 16 layers, and 64 nearest candidate faces per query with extra search breadth.
- HNSW is candidate retrieval only: candidate assignments and cluster merges are accepted using exact cosine similarity against the current cluster centroids.
- Weak-member pruning remains authoritative after candidate clustering, so bridge/chained or ambiguous faces can be detached instead of forcing two identities together.
- A cluster must have at least two members by default; singletons and weak/ambiguous faces remain outliers.
- Incremental/full rebuilds reconcile new memberships with previous Person IDs by overlap so IDs remain stable when possible.
- If one previous cluster splits, at most one resulting cluster inherits its old Person ID; other clusters receive deterministic new IDs.
- Stable Person ID hashing encodes the embedding dimension as a fixed-width 64-bit value, avoiding platform-width-dependent IDs.

## Scaling intent

The large-corpus path avoids scanning every existing cluster centroid for every face and avoids the previous all-pairs cluster merge pass. HNSW narrows both assignment and merge candidates while exact centroid cosine and pruning keep the decision semantics conservative. The HNSW structure is rebuilt in memory from the current SFace embeddings because automatic People state is derived data; persistence remains the People snapshot, not the transient neighbor graph.

## Next integration steps

1. Run clustering automatically after a successful SFace embedding backfill.
2. Expose cluster representatives as unique Person suggestions in Face Search.
3. Add a background full-rebuild action and a separate clustering threshold control.
4. Build manual People management/overrides in #60 without mutating automatic clustering state.
