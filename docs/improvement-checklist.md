# Project improvement checklist

Scope: balanced, local-first Windows image search. Preserve Rust, egui, SQLite,
portable indexes, current production models, and existing default search weights.
Checked items describe implemented changes, not unperformed validation.

## Milestone 1 — Make the current product dependable

- [x] Add named similarity presets and derive Custom from edited controls.
- [x] Keep Material / texture as the existing default; label new weights experimental.
- [x] Keep semantic-model failure visible with the completed search results.
- [x] Clear previous visual results when starting a replacement visual query.
- [x] Describe similarity scores without implying identity probability.
- [x] Update onboarding, face search, People status, and network documentation.
- [ ] Calibrate experimental presets using representative labeled queries.
- [x] Explain per-component scores in the inspector.
- [x] Keep compatible filters across modes and apply eligibility during retrieval.
- [x] Add request identities and cooperative cancellation to visual search, face search, and external face preparation.
- [x] Prioritize interactive CLIP requests between production indexing batches, with bounded queues and a four-query fairness limit.
- [x] Move descriptor backfill out of queries (Rescan already performs background backfill).
- [x] Allow Similar Image search during active base indexing without requiring pause; keep descriptor/embedding reads in one committed snapshot.
- [x] Add backup/restore and undo for manual People corrections.
- [x] Test pending-shard replay, detach/reattach after interruption, timestamp ties, and future-dated restore (multi-machine conflict resolution remains separate).

## Milestone 2 — Measure and improve retrieval

- [ ] Maintain held-out sets for duplicates, materials, semantic scenes, and identities.
- [ ] Measure decode, inference, database reads, retrieval, reranking, and display separately.
- [ ] Record cold/warm P50/P95 latency, throughput, and peak memory with hardware context.
- [x] Cache descriptors by committed catalog revision within a 64 MiB retained-payload budget.
- [x] Hydrate description/keywords only for final results, using the same committed snapshot.
- [ ] Enable revision-partitioned face ANN only after existing crossover gates pass:
      speedup >= 1.5x, Recall@25 >= 0.98, Recall@100 >= 0.95.
- [ ] Retain exact small-library search and fallback; oversample before image collapsing.
- [x] Extract visual retrieval from indexing and external-face preparation from UI.

## Milestone 3 — Add focused search workflows

- [x] Add explicit natural-language image search with the matching CLIP text encoder.
- [x] Add side-by-side People review, bulk corrections, unassigned faces, and undo.
- [x] Add duplicate comparison with separate byte-identical and visually similar groups.
- [x] Verify byte identity with full-file cryptographic hashes; never auto-delete.
- [x] Add temporary query-region selection for existing visual retrieval.
- [ ] Revisit OCR, video, and additional formats only after these workflows stabilize.

## Acceptance and validation

- [ ] Superseded searches cannot replace current results.
- [x] Verify read-only search snapshots retain consistent descriptors and chunked embeddings while another connection commits (501-row regression test).
- [ ] Missing models remain visibly degraded; recovery clears the indicator.
- [ ] Drive moves preserve image and People references.
- [ ] Corrections survive reclustering, restart, and reattachment.
- [ ] Interrupted writes retain previously committed work.
- [ ] Incompatible model revisions are never compared.
- [x] Run `cargo fmt --all -- --check` and `git diff --check`.
- [x] Run `cargo check --all-targets --locked` (passed; existing unused-code/import and compatibility warnings remain).
- [x] Run `cargo test --all-targets --locked --offline` (262 passed: 257 application + 5 repair-tool tests; includes cancellation, scoped face retrieval, concurrent snapshots, backup validation, and offline correction replay).
- [x] Compile/link Release and smoke-check the fresh executable with `--version` (v0.3.0-alpha.6, exit 0). The final Release build completed successfully; executable SHA-256 and benchmark evidence are recorded in the validation report.
- [ ] Manually verify preset selection, Custom, rerun, fallback, and recovery in Windows UI.

## Initial implementation notes

General appearance uses color/texture/semantic/dominant weights 20/15/60/5;
Possible duplicates uses 20/70/5/5. Both disable strict color rejection and remain
experimental until the evaluation milestone. Material retains 44/31/20/5 and its
existing color gate. Texture still uses the existing material+dHash blend: the
duplicate preset is a review aid, not an exact-duplicate detector.

Selecting a preset changes controls only; rerun applies them. Search settings
currently live in the UI session; this change does not introduce persistence or
silently change the startup default. Model download failure is attached to the
result payload rather than inferred from transient status messages.

## Cancellation implementation

Visual progress, errors, and completion carry request IDs. Face preparation and
search completion use the same session contract; obsolete messages are rejected
before they can change UI state. Switching search modes cancels active queries.
Cancel buttons release the UI immediately. CLIP waits poll cancellation every
100 ms; queued cancelled CLIP requests are skipped. Face scans check between rows
and external preparation between inference stages/faces. Native decode, model
initialization, inference, and ANN calls already in flight finish their current
operation. Visual workers serialize cache access so replacements cannot race an
older ANN rebuild. Queries no longer backfill library descriptors; use Rescan.

Manual Windows UI validation remains pending, including cancelling immediately
before completion, changing modes while preparing a face, and starting another
query after cancelling.

## Concurrent visual search

Similar Image can run during base indexing, including pause/resume changes while
the query is active. CLIP uses one model-owning worker with at most 8 commands in
the channel and 8 staged commands, plus the running operation. Interactive work
is preferred at batch boundaries; four consecutive queries yield to a waiting
background batch. Cancelled queued requests are discarded. A full interactive
queue reports an error instead of blocking the UI worker indefinitely.

Search opens a read-only database connection and pins a WAL snapshot only after
query inference. Descriptors, vector chunks and final result metadata share that snapshot,
released after result-only metadata hydration. During active indexing, exact semantic retrieval replaces
ANN retrieval to avoid rebuilding/reading a changing cache. Large-library
latency and memory should be benchmarked before optimizing this path further.
Face search/preparation still follow their existing busy restrictions.

Manual validation pending: run a large rescan, start/cancel/re-run visual queries,
pause/resume indexing during a query, and confirm both indexing progress and
search results remain usable. Test real CPU/DirectML throughput and memory.

## Expanded workflows

See [Search and People workflows](search-workflows.md) for description search,
region selection, score explanations, duplicate review, bulk correction and
recovery behavior. Visual retrieval lives in `src/visual_search.rs`; external
face preparation lives in `src/external_face_query.rs`. The matching text model
has been wired and compiled; real text-image quality and its first-download
Windows UX remain unvalidated.

Query timing instrumentation includes result-metadata loading and descriptor cache
hits/retained bytes. Database/component scoring are currently combined and display
timing is not measured. Descriptor caching and result-only text metadata loading
are implemented; representative end-to-end performance measurement remains open.

People recovery tests cover future-dated backups, invalid input preserving
committed corrections, offline refresh/reconnect, and replay of a committed shard
with an uncleared pending marker. Existing detach/reattach regression also passes.
Timestamp tie behavior is deterministic; multi-machine concurrent-edit conflict
resolution is not claimed complete.

The supplied `people` collection contains 118 images and 167 stored face vectors.
The local review export found no malformed or non-normalized vectors. Identity
accuracy and detector recall remain unmeasured without independent labels.

The real [People collection evaluation](people-collection-evaluation.md) measured
167 compatible faces in 96 images. The latest paused-copy ANN speedup was
0.221x; recall passed in this rebuild, but the speed gate still failed. Keeping exact production
search is the validated outcome for this collection; enabling face ANN is not
a missing toggle to turn on without further evidence.

## Retrieval memory follow-up

A single revision-keyed descriptor cache retains up to 64 MiB of accounted record
and owned-buffer capacity. This is not a cap on process memory, allocator overhead,
CLIP vectors, temporary query data, or returned metadata. Larger catalogs bypass
retention. Cache hits share descriptor records instead of cloning the full library.
Only candidate records are cloned; descriptions and keywords load only after the
final top set is selected (at most 2,000 results), in cancellable 500-row chunks.

SQLite triggers change a random revision token transactionally for image inserts,
updates and deletes, including descriptor/vector changes. Random tokens distinguish
divergent database copies. The token is read inside the same snapshot as query
vectors and result metadata. Legacy databases without revision tracking bypass the
cache until normal application initialization installs the tracking schema.

Regression coverage verifies cache reuse, invalidation, snapshot stability during
a concurrent commit, rollback behavior, deletion and budget bypass. The existing
501-row snapshot regression now also verifies deferred metadata and cancellation.

## Representative-library validation, 2026-09-06

- [x] Prepare isolated online backups after the user paused indexing.
- [x] Profile all 10,458 textures images and distinguish 864 stored CLIP vectors from image count.
- [x] Run 50-query ANN against the stored bank, including graph reload/candidate API costs.
- [x] Compare original and 512 px CLIP input on 50 spread samples and a 54-image JPEG/PNG/TIFF cohort.
- [x] Run CPU/DirectML requests for batches 1/4/8/16/32, then verify strict provider behavior on previews.
- [x] Attempt all five image models and retain four model-download failures.
- [x] Evaluate 150 crop/scale queries against all 10,458 stored material descriptors.
- [x] Run YuNet/SFace CPU runtime and native batch compatibility tests on the People collection.
- [x] Attempt DirectML with strict errors and probe automatic/device-0/device-1 selection outside the sandbox.
- [x] Run a six-image, visually reviewed negative face-detection smoke test.
- [ ] Validate different-image/same-material relevance and identity accuracy with independent labels (user deferred this dataset).
- [ ] Obtain working DirectML initialization and the four unavailable alternative model weights.

The [validation report](validation-2026-09-06.md) separates passing checks,
measured limitations, failed attempts and issue decisions. Diagnostic execution
alone does not satisfy the quality gates.
