# Search and People workflows

## Visual and description queries

Choose **Describe an image**, enter a description, and choose **Find images**.
This uses fastembed's matching CLIP ViT-B/32 text encoder and existing CLIP image
vectors. Its first use downloads a separate text model into the model cache;
later inference is local. Text model inference currently uses CPU with two
threads. English is the intended starting point; multilingual quality has not
been evaluated. Missing image vectors are excluded from description results.
A failed text-model load produces an error instead of color-only results.

For **Similar Image**, expand **Search a region**, enable the rectangle and
adjust its normalized position and size. The yellow preview shows the selected
area. Rerun applies it to an ephemeral crop; the original file is preserved.
Large sources use the existing safe preview policy. Native operations already
in flight may finish before cancellation releases their temporary resources.

Metadata, collection, People, and color filters persist across modes. Visual
and face retrieval apply an eligibility set before the final result limit.
Changing filters after a query narrows the current results; rerun to retrieve
a fresh top set for the new scope. Filtered visual queries use exact semantic
retrieval to avoid starving the eligible set with global ANN candidates.

The inspector's **Why this result** reports the component scores and normalized
weight contributions used for that completed query. A query source pinned to
the top is identified separately. Scores are ranking signals, not calibrated
identity probabilities. Query timings cover decode/descriptor work, inference
including queue wait and model initialization, retrieval including database
reads and component scoring, final ranking, and a separate result-metadata stage. Total time also includes worker
serialization. These are diagnostic wall times, not a cold/warm latency study
or a measurement of thumbnail display completion.

## Duplicate review

Open **Duplicates** and **Scan current filters**. Byte-identical suggestions
use full-file SHA-256 after grouping current source sizes; unavailable files
and detected changes during hashing are skipped. Hashing is cancellable.
Visual suggestions compare dHash and aspect ratio with bounded candidate
retrieval, so they can miss similar files and include false positives. Every
member matches its group anchor; unrelated images are not joined through a
transitive chain. Exact and visual groups can overlap. Open the side-by-side
previews to review them. This workflow does not delete files.

## People recovery and bulk review

Select multiple faces in a Person or in **Unassigned / exceptions** and use
the bulk correction buttons. Assignment targets an existing manual Person.
Local bulk changes run in a transaction; any invalid member rolls the batch
back. A representative and selected face can be compared side by side.

**Backup corrections** saves a versioned JSON file with manual identities and
face overrides only. It refuses to overwrite an existing backup. **Restore
backup** replaces the current corrections, validates the input before writing,
and leaves photos, vectors and automatic clustering data intact. Use backups
from the same libraries: IDs are preserved, not remapped to different photos.
Restore advances timestamps beyond both current and restored values, including
future-dated edits. JSON backups are limited to 16 MiB.

**Undo correction** restores an in-memory recovery point for the preceding
correction. Up to 20 normal correction points are retained for the current
session; use an explicit backup for recovery across restarts. Recovery points
are also kept when an operation reports a partial portable-sync failure.

Pending portable shard writes are recorded locally before corrections. A
refresh retries available pending shards before importing their contents;
unavailable attached libraries retain their local corrections until reconnect.
Already committed shards can be replayed after interruption. Explicitly
detaching a library removes its disposable local People view. Timestamp ties
have a deterministic metadata ordering. This is not a distributed conflict-free
merge protocol: simultaneous edits on separate computers still need review.

## Evaluate an existing People collection

The root diagnostic reads the chosen portable index without initializing or
changing the application's session database:

```powershell
.\windows-image-search.exe --benchmark-face-ann-root 'L:\join\dl\ImageSearch\people'
python scripts\export-people-review.py 'L:\join\dl\ImageSearch\people' target\people-review
```

The exporter creates a new output directory and never overwrites existing
human labels. Its CSV contains proposed face boxes and empty review fields;
automatic clusters are deliberately not treated as truth. Review full images
for missed detections as well. Convert reviewed evidence using the existing
[face benchmark format](face-benchmark-manifest.md). Exact-vs-ANN recall measures
retrieval approximation, not whether two faces belong to the same person.

Preset calibration, identity precision/recall, representative hardware latency
and peak memory, and manual Windows GUI checks remain separate validation tasks.

## Descriptor cache and result metadata

Repeated visual queries can reuse descriptors while the committed catalog revision
is unchanged. The timing panel shows cache hits and retained payload size. A single
catalog is retained under a 64 MiB accounting budget; larger catalogs remain
searchable but bypass retention. This budget does not include allocator overhead,
query vectors, returned metadata, or the application's other caches. Concurrent
index writes invalidate the cache for subsequent snapshots.

Ranking omits descriptions and keywords for the full library and loads them only
for final results. The snapshot remains open through that short, chunked metadata
read so concurrent updates and row-id reuse cannot mix catalog versions.
