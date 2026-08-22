# People filters and image-detail integration

Issue #61 extends the existing image-level search pipeline instead of replacing it.

## Filter semantics

Named People are optional filters composed with the existing collection, text, explicit-color and image-similarity result source.

- **ANY selected person**: an image passes when it contains at least one selected Person.
- **ALL selected people**: an image passes only when every selected Person occurs in that parent image.
- No selected Person means the People filter is inactive.
- Stale/deleted Person IDs are dropped when a filter snapshot is rebuilt.
- Detached and ignored faces never contribute to People filter membership.

## Performance contract

`visible_indices()` must remain a lightweight in-memory pass. It must not open SQLite databases, enumerate face crops, or calculate face embeddings per frame.

When People selection changes, the selection is resolved into per-Person `HashSet<PathBuf>` collections and one final matching-image set. Rendering then performs only a path membership check.

Portable root databases are opened at most once per root during a resolve operation. Current searchable face records are resolved with a prepared statement reused for all selected face IDs in that root.

## Image details

Face information is loaded on demand for the image whose details are being inspected rather than being added to every `ImageSummary` row.

The detail payload contains:

- current detected-face count;
- named effective People found in the image;
- per-Person face count for navigation/badges.

The detail lookup validates current face-detection state against the current indexed image row. Effective People names and manual corrections come from the central session catalog.

## UI direction

The Search sidebar will expose a compact People section with named identities, multi-select, and an explicit ANY/ALL control. Result/details face UI remains optional; normal Grid/Details rendering should not eagerly allocate or render every detected face crop.
