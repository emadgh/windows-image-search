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
- Clusters use deterministic ordering, centroid similarity and weak-member pruning.
- A cluster must have at least two members by default; singletons and weak/ambiguous faces remain outliers.
- Incremental/full rebuilds reconcile new memberships with previous Person IDs by overlap so IDs remain stable when possible.
- If one previous cluster splits, at most one resulting cluster inherits its old Person ID; other clusters receive deterministic new IDs.

## Next integration steps

1. Run clustering automatically after a successful SFace embedding backfill.
2. Expose cluster representatives as unique Person suggestions in Face Search.
3. Add a background full-rebuild action and a separate clustering threshold control.
4. Build manual People management/overrides in #60 without mutating automatic clustering state.
