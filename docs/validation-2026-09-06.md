# Search validation — 2026-09-06

Completed work is published for review; the remaining acceptance gates below are not approved. Further testing stopped at the owner's request. No source photos, model weights, private manifests, or databases are included.

## Build and safeguards

- [x] `cargo check --all-targets --locked --offline` passed.
- [x] `cargo test --all-targets --locked --offline`: 262 passed (257 application, 5 repair), none failed or ignored.
- [x] `cargo build --release --locked --offline` passed; existing unused-code warnings remain.
- [x] Python isolated-workspace regression and PowerShell 5.1 runner/provider-manifest regression passed.
- [x] The subsequently added DirectML diagnostic example compiled and ran in Release. Its successful process exit means probes completed, not that DirectML worked.
- [x] Final read-only SQLite audits found the original session, textures portable database, and people portable database logically unchanged; all returned `quick_check=ok`. This checks application-table content, not byte-for-byte file identity or FTS internals.

Application: v0.3.0-alpha.6. Final executable SHA256: `dee5ec2eb040555dc2b8641c71e9e5af1e78f5459b75e9ac44bb24735c768441`.

The owner paused indexing. Benchmarks used marked temporary workspaces made with SQLite online backups, with discovery roots cleared in working copies. Existing source images/previews were read only. Model, cache, vector-bank, and ANN writes stayed in copies. The session retained 10,576 image rows; textures portable retained 10,461, people portable 118 images and 167 face embeddings. Audit hashes:

| Original | Logical SHA256 |
| --- | --- |
| Session | `1d1fa6b66fac8b7d1641eb9c46fd42ff06a41acdbd3929adfda87ea40b6b5448` |
| Textures portable | `8c6277b82b833350dadd9e9bc1fc8c5ad85cb4637e3f61d4b94333ba55c5645a` |
| People portable | `d28549ecd868c26e3283d232322e7764f71acbb9cd943beda6c10a17f1cd25fb` |

Host: Windows 10 Enterprise 19045, Intel i9-9900K (8 cores/16 threads), 31.92 GiB RAM, NVIDIA GTX 1660, driver 32.0.15.8108. GPU memory counters were unavailable; null values do not mean zero GPU usage. Process-memory polling can miss short-lived peaks.

## Corpus and evidence provenance

The selected textures session collection contains 10,458 existing images: 8,787 JPG, 1,589 JPEG, 64 PNG, 18 TIFF; approximately 273.3 GB. Median image size is 36.619 MP, P95 104.637 MP, maximum 178.548 MP. The original stored CLIP bank contains only 864 vectors, despite the larger catalogue.

[Initial full-suite evidence](validation/2026-09-06/production-snapshot/) used executable SHA256 `d3b46b52e78c435c8cc7a9238cb9558f224746619a2912918f88a3812fcadcfc`. Its six native processes exited zero, but four model downloads failed and the wrapper subsequently failed on an unset LASTEXITCODE. Final code fixes both error reporting paths. Raw baseline fields claiming `production_clip_input=original`, a fixed production CPU backend, and DirectML availability are superseded: cached previews were already used by indexing, backend selection is user configured, and ORT silently fell back to CPU. Those baseline fields must not be interpreted as GPU measurements. Final evidence below uses strict provider initialization and the final executable.

## Results

| Measurement | Result | Interpretation |
| --- | --- | --- |
| 54-image preview cohort: 9 JPG, 9 JPEG, 18 PNG, 18 TIFF | 36,256 ms original vs 1,416 ms preview; 25.603x; cosine mean .985188; ranking agreement R@10 .825926, R@25 .920740, top-1 .666667 | Decode-inclusive, cache/warmth-sensitive; agreement with original-input rankings, not human relevance accuracy |
| CPU CLIP, 50 cached previews, 4 threads | Best batch 8: 38.062 images/s; batch 16: 37.815 | This is not original full-resolution throughput |
| Strict DirectML | Unsupported interface/feature level, `0x887A0004` | Automatic and DXGI device 0 failed inside and outside sandbox; device 1 was an invalid adapter. No valid CPU/GPU comparison |
| Alternative image models | CLIP succeeded; UNICOM B16/B32, Nomic Vision 1.5, ResNet50 weight downloads timed out | Comparison incomplete; final CLI correctly exits nonzero |
| Texture transformations: 10,458 corpus, 50 sources, 150 crop/scale queries | dHash R@10 .4867; material .9000; production blend .7200 | Automatic same-source recovery only; no different-photo/same-material ground truth |
| Offset crops specifically | dHash R@10 0; material .78; production blend .26 | Stronger standalone descriptor result does not justify changing production weights without independent labels |
| YuNet CPU, 118 images | Mean 118.336 ms, P95 288.414 ms; 8.451 images/s | Runtime-only, no positive detection ground truth |
| SFace CPU, 167 faces | Mean 7.698 ms, P95 8.899 ms; 129.911 faces/s | Runtime-only, no identity quality or threshold calibration |
| Face batch support | Batch 1 supported; 2/4/8/16 unsupported in tested models | Errors retained in reports |
| Six visually reviewed texture negatives | CPU YuNet: zero false positives in six images | Small negative smoke test, not broad face accuracy; empty-positive precision/recall fields are not meaningful quality scores |
| Face ANN, 167 vectors, 32 queries | .087 ms ANN vs .019 ms exact; .221x; R@10 100%, R@25 99.75%, R@100 99.78% | Recall passes this rebuild; speed gate fails. Stochastic graph builds can vary |

Evidence: [format previews](validation/2026-09-06/format-preview/), [strict runtime/model failures](validation/2026-09-06/strict-preview-runtime/), [texture suite](validation/2026-09-06/production-snapshot/material-texture.txt), [face runtime, ANN and negatives](validation/2026-09-06/faces/).

### Image ANN: hot graph versus actual candidate API

| Bank | Warm graph query | Exact top-100 | Warm speedup / R@100 | Actual candidate API / exact candidate scan |
| --- | --- | --- | --- | --- |
| 864 original vectors | .633 ms | .363 ms | .57x / 99.2% | 16.994 / .423 ms; .025x; candidate recall .985046 |
| 7,093 preview vectors | 2.165 ms | 3.303 ms | 1.53x / 99.02% | 79.878 / 3.395 ms; .043x; candidate recall .990840 |

The larger bank completed before publication: 7,093 existing valid previews embedded in 264.073 seconds, with 3,365 missing previews skipped. It is not a fully indexed 10,458-vector bank and uses preview-derived embeddings. No missing previews were generated in the original collection. Its actual API requested 3,000 candidates with ef=3,512; candidate measurements include SQLite signature validation and graph reload, excluding query inference and hybrid reranking. Warm graph speed alone does not establish end-to-end acceleration. Production now checks actual vector count before selecting ANN, so the original 864-vector collection uses exact fallback. The 7,093-vector result still exposes an unresolved large-bank API performance problem; this publication does not claim it fixed.

Evidence: [original-bank final run](validation/2026-09-06/final-production-ann/), [completed preview-bank run](validation/2026-09-06/preview-bank/).

## Changes included for review

Search presets, region queries, text/image search, cancellation and bounded worker queues, filter-before-TopK behavior, component explanations, exact/visual duplicate review, people backup/undo and offline synchronization, bounded descriptor caching, and deferred metadata loading are accompanied by regression coverage and workflow documentation. This is a broad review batch; manual UI acceptance has not been performed.

Validation fixes include isolated benchmark workspaces with mandatory markers and containment checks, explicit runtime-only face evaluation, strict DirectML provider errors, equal runtime thread counts, accurate preview provenance, nonzero incomplete-model status, PowerShell manifest parsing/exit handling, query-photo exclusion before face TopK, ANN lifecycle coverage, actual-vector-count fallback, and production candidate API measurements. Model defaults, face thresholds, and material weights were not promoted from these incomplete quality gates.

## Issue disposition

| Issue | Completed evidence | Remaining acceptance gate; disposition |
| --- | --- | --- |
| #28 | Preview cache implementation and JPG/JPEG/PNG/TIFF comparison | Independent relevance approval; keep open |
| #29 | Persistence/lifecycle regressions, small-bank fallback, 864 and 7,093-vector recall/timing | Actual candidate API remains slower than exact; keep open |
| #30 | CPU batches, strict provider probe, model-download diagnostics | Working DirectML, four missing models, representative labeled quality; keep open |
| #32 | Full-corpus crop/scale descriptor comparison | Different-photo/same-material labels deferred by owner; keep open |
| #62 | CPU face runtimes/batches, six negatives, 167-vector ANN | Labeled identities/poses/occlusions, working DirectML, larger-scale storage/crossover and remaining deployment gates; keep open |
| #69 | Aggregated evidence and reproducible isolated runners | Dependent quality, runtime/model and ANN performance gates remain unmet; keep open |

No issue is closed solely because a benchmark process completed. See the [checkbox implementation plan](improvement-checklist.md), [people evaluation](people-collection-evaluation.md), and [isolated reproduction guide](isolated-benchmarks.md). Production rollout and manual UI acceptance remain separate from successful compilation and automated regressions.
