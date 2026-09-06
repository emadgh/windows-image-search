# People collection: local technical evaluation

Measured on 2026-09-06 using the Release v0.3.0-alpha.6 local build and the
user-provided `people` collection. The latest measurements used a paused, isolated copy of the portable index;
source images were read without modification. No photographs, image filenames, or identity labels are
included in this report.

| Check | Result |
| --- | ---: |
| Indexed images | 118 |
| Stored detected faces / embeddings | 167 / 167 |
| Malformed or non-normalized stored vectors | 0 |
| Current compatible searchable vectors | 167 |
| Images represented by searchable faces | 96 |
| Embedding revision groups | 1 |
| Embedding dimension | 128 |
| Independently labeled identities | 0 |

The remaining 22 indexed images have no current searchable face. These counts
alone cannot establish whether they contain missed faces, intentionally contain
no faces, or need a different detector threshold. Identity accuracy and detector
recall remain **not evaluated**.

## Exact versus ANN

The existing Rust benchmark sampled 32 queries from the 167 current vectors.
These times measure in-memory vector retrieval, excluding image decoding,
inference, database loading, and UI rendering. ANN recall is measured against
exact vector ranking, not human identity labels.

| Metric | Exact | HNSW ANN |
| --- | ---: | ---: |
| Mean query time | 0.019 ms | 0.087 ms |
| P50 | 0.018 ms | 0.086 ms |
| P95 | 0.019 ms | 0.092 ms |
| Recall@25 against exact ranking | Reference | 99.75% |
| Recall@100 against exact ranking | Reference | 99.78% |

Reported ANN speedup was **0.221x**, below the 1.50x gate. Recall gates passed
in this rebuild, but passing recall alone does not establish a speed crossover.
An earlier rebuild had lower recall; construction is stochastic. Production Face Search therefore remains exact. Corpus
sizes from 1,000 through 100,000 were unavailable; no larger-library crossover
or scaling claim follows from this small collection. HNSW construction took
4.384 ms and its serialized graph/data occupied 177,717 bytes in this run.

Hardware: Windows 10 Enterprise build 19045, Intel Core i9-9900K (8 cores,
16 logical processors), 31.92 GiB RAM, NVIDIA GTX 1660. The current ONNX Runtime
cannot initialize DirectML on this host; automatic and explicit device-0 probes
both returned 0x887A0004, including outside the sandbox. CPU remains usable.
Sub-millisecond timings vary with system load.

The latest runtime-only face run measured YuNet mean/P95 of 118.336/288.414 ms
on 118 images and SFace inference mean/P95 of 7.698/8.899 ms on 167 aligned
faces. These measure different stages and are not directly comparable.
Both supplied ONNX exports accepted batch 1 and rejected batches 2/4/8/16.
A separate six-image, visually reviewed negative texture smoke set produced
zero false positives on CPU. It does not establish broad detector accuracy.

See the [full validation report](validation-2026-09-06.md) for telemetry, commands,
model fingerprints, limitations and issue decisions.

## Review material

The local, ignored directory `target/people-review-20260906` contains:

- `face-ann-release.txt`: full raw benchmark output.
- `summary.json`: read-only inventory and vector checks.
- `face-labels.csv`: 167 proposed face boxes with empty human review fields.
- `README.txt`: review instructions and limitations.

The worksheet is not ground truth and is not a benchmark manifest. Review
identities independently and inspect full images for missed detections before
using the existing labeled benchmark pipeline. See
[search workflows](search-workflows.md) for commands and model-download behavior.
