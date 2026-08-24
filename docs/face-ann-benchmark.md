# Face index storage and ANN crossover benchmark

The opt-in `--benchmark-face-ann` diagnostic measures the current persisted face-embedding corpus without changing Face Search behavior or writing to portable indexes.

```powershell
.\windows-image-search.exe --benchmark-face-ann 32
```

The optional number is the deterministic query sample count per measured corpus size. The default is 32.

## Revision isolation

The benchmark reads only current, normalized face embeddings whose detector/source state still matches the current `faces`, `face_detection_state`, and `images` records. Vectors are grouped by the same embedding revision fields used by production Face Search: model id, model version, model cache revision, embedding schema version, alignment revision, and dimension. Different revision groups are never inserted into the same HNSW graph.

## Corpus sizes and metrics

For each revision group the benchmark measures available sizes from 1k, 5k, 10k, 25k, 50k, and 100k faces, plus the full available tail when it is not exactly one of those sizes. A smaller library is still measured at its actual size when at least two searchable faces exist.

For every measured size it builds an in-memory cosine HNSW graph and compares it with exact normalized cosine ranking. The report includes:

- searchable face count and unique source-image count
- raw embedding bytes and bytes per face
- a deterministic logical face/embedding payload lower-bound; SQLite page/index overhead is intentionally excluded
- HNSW build time and serialized graph+data bytes
- exact and ANN average/P50/P95 query latency
- exact-to-ANN speedup
- Recall@10, Recall@25, and Recall@100 against exact ranking

The benchmark excludes the query face itself from both exact and ANN rankings.

## Crossover rule

For diagnostic reporting, the first measured size is marked as the crossover only when all of these are true:

- exact-to-ANN speedup is at least 1.50x
- Recall@25 is at least 98%
- Recall@100 is at least 95%

Otherwise the report writes `not_reached`. These are benchmark evidence thresholds, not a production behavior change. Production Face Search remains exact until a separate reviewed change uses representative-library results to justify ANN.

## Runtime memory

The Rust report records logical vector/payload sizes and serialized HNSW size. When invoked through `run-v0.3-face-benchmark-gate.ps1`, the parent gate also samples process working set, private memory, and Windows GPU process memory when those counters are available. This supplies runtime-memory evidence without introducing platform-specific memory APIs into the benchmark core.

Refs #62 and #173.
