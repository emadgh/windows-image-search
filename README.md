# Windows Image Search

A native, local-first image index and visual search application for Windows, written in Rust.

> **v0.3.0-beta.1 — Beta / testing preview.** Validation is incomplete and tests are not final. Manual UI acceptance, labeled material/face quality evaluation, and some hardware/model gates remain outstanding. See [beta release notes](docs/releases/v0.3.0-beta.1.md).

## Current features (v0.3 beta)

- Configure one or more folders to index recursively.
- Portable per-root indexes under `<root>/.imagesearch`, suitable for external/removable drives and drive-letter changes.
- Crash-safe incremental **Rescan** with durable batch commits and live results while indexing.
- Filesystem watching for new, changed, renamed and deleted images without requiring a full rescan.
- Supports JPG/JPEG, PNG, TIF and TIFF.
- Indexed FTS5 text search across filename, path, EXIF/XMP description and keywords.
- Search/filter by dominant color with adjustable tolerance.
- Hybrid image similarity using color distribution, texture/dHash, CLIP semantic similarity and dominant color.
- Explicit natural-language search with the matching CLIP text encoder, temporary region queries, component score explanations, and query-stage timings.
- Duplicate review with full-file SHA-256 groups and separate approximate visual groups; side-by-side previews and no automatic deletion.
- Two-stage candidate generation for large libraries, with persisted HNSW semantic retrieval and exact hybrid reranking.
- Persistent portable thumbnail disk cache, viewport-priority loading and bounded GPU/UI texture residency.
- Collections for grouping indexed folders/files.
- Face search from indexed faces or an external image with a multi-face chooser.
- People grouping, naming, merging, and per-face corrections separate from automatic clusters.
- People bulk assignment/ignore/restore, unassigned-face review, correction backups, session Undo, and retryable portable synchronization.
- Managed YuNet/SFace model setup and CPU/DirectML inference options.
- Switch between a resizable thumbnail grid and an Explorer-style detailed list.
- Double-click to open an image; context menu can open its containing folder or copy its path.

## First run and portable indexes

1. Start `windows-image-search.exe`.
2. Open **Preferences → Collections**, create/select a Collection, and add folders. New roots are scheduled for indexing automatically.
3. A new folder gets a `.imagesearch` directory and can then be populated with **Rescan**. If the folder already contains a valid `.imagesearch/index.sqlite3`, the existing portable index is attached and reused without decoding/rescanning the source images.
4. Metadata, color/texture descriptors, cached thumbnail previews and CLIP embeddings are committed incrementally.
5. The CLIP model is downloaded on its first use and cached under the Windows user's local application-data directory. After it is cached, similarity search can run offline.

Each configured root owns its durable image-search data:

```text
<root>/.imagesearch/
  index.sqlite3
  thumbnails/
  ann-index/
```

The presence of `.imagesearch/index.sqlite3` marks a previously indexed root. Source paths inside the portable database are stored relative to that root, so an external drive can move from a path such as `E:\Materials` to `F:\Materials` without invalidating its image records, descriptors, embeddings or thumbnail identities. Adding the moved folder on another machine rehydrates the application from the portable database instead of performing a full image rescan.

The application still keeps a small local `WindowsImageSearch/index.sqlite3` under the current user's application-data directory. In v0.2.10 this is a rebuildable multi-root session/catalog cache used to union currently attached libraries and retain shared state such as root registration and collections; the root-local `.imagesearch/index.sqlite3` files are the durable source of image index data. Downloaded model files and UI/performance settings also remain local to the Windows user.

Existing v0.2.9 central records are migrated root-by-root when an available folder is attached. Compact visual descriptors and CLIP embeddings are copied directly rather than regenerated. Existing cached thumbnails are copied into the root's portable thumbnail cache when their old cache entries are available. Migration is committed per root, so an interrupted migration can be resumed without deleting the old central data prematurely.

`.imagesearch` and all of its descendants are excluded from recursive image traversal and filesystem-watch indexing events. Changed source files are mirrored back to the portable database in durable batches. Watcher-reported changes are revalidated even if a replacement file preserved its size and modified timestamp, and newly decoded sources receive a stable content fingerprint.

Original source images are never copied into the SQLite database.

## Search modes

### Text / tags

Type words in the search field. Multiple words use AND semantics and are matched case-insensitively against filename, full path, description and keywords through the indexed FTS5 search path.

### Color

Enable the explicit color filter, choose a target color and adjust tolerance. Color search can be combined with text/tag filtering.

### Similar image

Click **Search by image**, choose a query image, and use the similarity sliders to control color distribution, texture/pattern, CLIP semantic and dominant-color influence. Large libraries use bounded candidate generation and HNSW semantic retrieval before exact hybrid reranking.

The Similar Image sidebar provides named presets: **Material / texture** preserves the existing default; **General appearance** and **Possible duplicates** are experimental starting points pending labeled evaluation. Editing controls shows **Custom**. Preset changes take effect on the next search; use **Re-run search** to apply them to the current query.

Scores are ranking similarities, not probabilities or guarantees that files are duplicates. If the semantic model fails, a persistent notice explains that results use texture/color only. Similar Image search remains available during active indexing, using a consistent committed snapshot. Re-run to include newly committed images. Pause/resume remains available while searching.

Use **Cancel search** or the face-search cancellation button to stop a query; switching search modes also cancels it. Cancelled results cannot replace a newer query. An in-flight decoder/model operation can finish before its worker stops. Missing visual descriptors are repaired by **Rescan**, not by running a query.

Interactive CLIP queries are prioritized between indexing batches on the existing single model worker. Queue sizes are bounded; after four queued queries, a waiting indexing batch gets a turn. During active indexing, semantic candidates use exact snapshot retrieval instead of rebuilding a changing ANN cache; this can use more time/memory on large libraries. Face preparation/search and face maintenance retain their separate busy-state restrictions.

### Faces and People

Enable **Detect faces** for participating Collections and set up YuNet/SFace in Preferences. Face Search offers People representatives when available and falls back to indexed face instances. Choose **Face from file…** to select a face from an external image. The People manager supports names, merges, splits, assignments, and representative selection without rewriting automatic clustering state. See [Face Search](docs/face-search.md) and [People management](docs/people-management.md).

See [search workflows](docs/search-workflows.md) for description/region search, duplicate review, and People recovery. The [local People evaluation](docs/people-collection-evaluation.md) records the available corpus and ANN gate results.

### Improvement roadmap

Implementation and validation status is tracked in the [project improvement checklist](docs/improvement-checklist.md).

## Diagnostic benchmarks

Benchmarks are opt-in CLI diagnostics. Before a diagnostic runs, available registered portable roots are attached into the local multi-root session cache so the command sees the same current indexed libraries as the GUI. Benchmark reports are written beside the local session database in the `WindowsImageSearch` application-data directory.

### Run the complete v0.2 benchmark gate

The Windows build includes `run-v0.2-benchmark-gate.ps1` beside the executable. Run it after indexing a representative tile, marble, stone or ceramic library:

```powershell
.\run-v0.2-benchmark-gate.ps1 -Executable .\windows-image-search.exe
```

The runner records the application version, exact benchmark commands/sample counts, timestamps, exit codes and wall time. It also records Windows, CPU, total RAM, GPU/driver and Windows-reported adapter RAM, plus disk model/media information using built-in CIM queries.

Each diagnostic runs as a child process. The runner records peak process working set, sampled private memory, and—when Windows exposes the `GPU Process Memory` performance counters—sampled dedicated and shared GPU memory for that benchmark process. GPU-process memory is recorded as unavailable/null instead of failing the gate when those counters are unsupported or no matching process sample is exposed.

The gate runs the library profile, ANN, cached-preview CLIP, CPU-vs-DirectML, alternative image-model and material-texture diagnostics. Each benchmark gets separate stdout/stderr files plus a combined text file; `manifest.json` contains timing and memory telemetry, `system-info.json` contains hardware context, and `summary.txt` contains the compact result overview.

Results are saved under a timestamped `benchmark-results` directory and compressed into a ZIP suitable for attaching to the performance roadmap issues. Sample counts can be overridden, for example:

```powershell
.\run-v0.2-benchmark-gate.ps1 `
  -Executable .\windows-image-search.exe `
  -AnnQueries 64 `
  -PreviewSamples 96 `
  -RuntimeSamples 96 `
  -ImageModelQueries 32 `
  -TextureSamples 48 `
  -MaterialEvalManifest .\material-eval.tsv
```

Peak working set is process-level resident RAM. Private-memory and GPU-memory values are sampled while the benchmark runs rather than being allocator-level exact maxima. `Win32_VideoController.AdapterRAM` in `system-info.json` is the capacity reported by Windows/WMI and can differ from exact dedicated VRAM on some drivers.

`-MaterialEvalManifest` is optional. When it is omitted, `manifest.json` and `summary.txt` explicitly mark labeled same-material evaluation as not run. When supplied, its path is passed as a discrete process argument, so spaces in the manifest path are supported.

### Labeled same-material evaluation

Use a small manually curated UTF-8 TSV when you need to measure retrieval across *different images of the same material/design*, not only transformed copies of one source image:

```text
group	path
Calacatta Gold	D:\Material Eval\calacatta-face-01.jpg
Calacatta Gold	D:\Material Eval\calacatta-face-02.jpg
Travertine Beige	travertine-face-01.jpg
Travertine Beige	travertine-face-02.jpg
```

Blank lines and lines beginning with `#` are ignored. Relative paths are resolved relative to the TSV file. Every evaluated group must contain at least two distinct indexed images; assigning one image path to different groups is rejected.

Run the labeled benchmark directly:

```powershell
.\windows-image-search.exe --benchmark-material-eval .\material-eval.tsv
```

Or include it in the complete gate with `-MaterialEvalManifest`. Every labeled image is used as a query, the query itself is excluded, and the first *other* image from the same group is the relevant result. The report compares indexed dHash, Gradient, LBP, combined material texture, the current material+dHash blend, stored production CLIP, and CPU embeddings from CLIP B32, UNICOM B16/B32, Nomic Vision v1.5 and ResNet50. It reports Recall@1/5/10/25, MRR and mean first-relevant rank, plus model initialization/throughput and embedding coverage. The command never overwrites production embeddings or changes search defaults.

### Library profile

Record the composition of the currently attached portable libraries before interpreting benchmark results:

```powershell
.\windows-image-search.exe --benchmark-library-profile
```

The report includes indexed image count, source files that currently exist or are missing, persisted total source bytes, database size, extension distribution, width/height min/median/P90/max, megapixel min/P50/P90/P95/max, megapixel buckets, and landscape/portrait/square counts. The command uses the hydrated multi-root session index and does not modify production search settings.

### ANN retrieval

Compare persisted HNSW retrieval against exact brute-force CLIP ranking on the current indexed library:

```powershell
.\windows-image-search.exe --benchmark-ann 32
```

The benchmark reports HNSW prepare/load time, ANN and exact latency, speedup, and Recall@10/50/100.

### Cached-preview CLIP quality

Measure the quality and speed impact of embedding cached image previews instead of full source images:

```powershell
.\windows-image-search.exe --benchmark-clip-preview 64
```

This diagnostic reports pair cosine similarity, retrieval agreement/recall and timing without changing production CLIP input behavior.

### CLIP CPU vs DirectML runtime

Compare available CLIP runtime backends and batch sizes on local hardware:

```powershell
.\windows-image-search.exe --benchmark-clip-runtime 64
```

Production inference remains unchanged; the command is intended to collect evidence before changing backend defaults.

### Alternative image embedding models

Compare supported image models on the same bounded local corpus without replacing the production CLIP embeddings:

```powershell
.\windows-image-search.exe --benchmark-image-models 24
```

The benchmark compares `ClipVitB32`, `UnicomVitB16`, `UnicomVitB32`, `NomicEmbedVisionV15`, and `Resnet50` on CPU. It deterministically builds up to 512 corpus images, creates center-crop, off-center crop/layout-shift, and reduced-resolution queries, and reports embedding dimension, initialization time, corpus throughput, average query embedding latency, Recall@10/25, MRR, and mean rank for each model and transform. A model download/initialization failure is recorded without aborting the remaining comparisons.

The automated ground truth is recovery of the transformed query's original indexed source. Use the results to narrow candidates, but do not change the production model until representative same-material tile/marble/stone evaluation also supports the change. Running this command can download additional model files into the existing local model cache.

### Material texture robustness

Evaluate the compact material descriptor against 64-bit dHash on the current indexed corpus:

```powershell
.\windows-image-search.exe --benchmark-material-texture 24
```

The benchmark deterministically samples indexed images and creates center-crop, off-center crop/layout-shift and reduced-resolution queries. It compares:

- 64-bit dHash
- multi-scale gradient/HOG-like descriptor component
- rotation-normalized LBP component
- combined material descriptor
- the current production material + dHash blend

It reports Recall@1/5/10, MRR, mean source-image rank, descriptor-computation latency and full-corpus ranking latency, including per-transform results. The transformed query's original indexed image is the automated ground truth; representative same-material tests across different tile/marble/stone faces are still required before considering the material-texture roadmap complete.

To print the application version:

```powershell
.\windows-image-search.exe --version
```

## Development

For tests on an existing collection, use an [isolated benchmark workspace](docs/isolated-benchmarks.md).
The runner checks that the executable honors isolation before starting diagnostics.

```powershell
cargo fmt --all -- --check
cargo check --all-targets
cargo test --all-targets
cargo run --release
```

Windows CI reads the package version from `Cargo.toml` and uploads a versioned artifact such as `windows-image-search-v0.2.10-win64`. The ZIP contains `windows-image-search.exe`, `run-v0.2-benchmark-gate.ps1`, `README.md`, `LICENSE` and `VERSION.txt`.

## Privacy

Image indexing and inference run locally. Network access is used for uncached CLIP models, managed face-model downloads, and GitHub update checks/downloads. Automatic update checking and downloading are enabled by default and can be disabled in update settings. After required models are cached, search can run offline.
