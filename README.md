# Windows Image Search

A native, local-first image index and visual search application for Windows, written in Rust.

## v0.2.x features

- Configure one or more folders to index recursively.
- Crash-safe incremental **Rescan** with durable batch commits and live results while indexing.
- Filesystem watching for new, changed, renamed and deleted images without requiring a full rescan.
- Supports JPG/JPEG, PNG, TIF and TIFF.
- Indexed FTS5 text search across filename, path, EXIF/XMP description and keywords.
- Search/filter by dominant color with adjustable tolerance.
- Hybrid image similarity using color distribution, texture/dHash, CLIP semantic similarity and dominant color.
- Two-stage candidate generation for large libraries, with persisted HNSW semantic retrieval and exact hybrid reranking.
- Persistent thumbnail disk cache, viewport-priority loading and bounded GPU/UI texture residency.
- Collections for grouping indexed folders/files.
- Switch between a resizable thumbnail grid and an Explorer-style detailed list.
- Double-click to open an image; context menu can open its containing folder or copy its path.

## First run

1. Start `windows-image-search.exe`.
2. Open **Settings**, add one or more indexed folders, then run **Rescan**.
3. Metadata, color/texture descriptors and cached thumbnail previews are committed incrementally.
4. CLIP embeddings are generated for images that do not have one yet.
5. The CLIP model is downloaded on its first use and cached under the app's local data directory. After it is cached, similarity search can run offline.

Index data is stored under the current Windows user's local application-data directory in `WindowsImageSearch/index.sqlite3`. The index contains file paths, file state, dimensions, extracted text metadata, compact visual descriptors and CLIP embeddings; original images are never copied into the database.

## Search modes

### Text / tags

Type words in the search field. Multiple words use AND semantics and are matched case-insensitively against filename, full path, description and keywords through the indexed FTS5 search path.

### Color

Enable the explicit color filter, choose a target color and adjust tolerance. Color search can be combined with text/tag filtering.

### Similar image

Click **Search by image**, choose a query image, and use the similarity sliders to control color distribution, texture/pattern, CLIP semantic and dominant-color influence. Large libraries use bounded candidate generation and HNSW semantic retrieval before exact hybrid reranking.

## Diagnostic benchmarks

Benchmarks are opt-in CLI diagnostics. They do not change normal GUI/indexing/search behavior and write timestamped reports beside the application database in the `WindowsImageSearch` local-data directory.

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
  -TextureSamples 48
```

Peak working set is process-level resident RAM. Private-memory and GPU-memory values are sampled while the benchmark runs rather than being allocator-level exact maxima. `Win32_VideoController.AdapterRAM` in `system-info.json` is the capacity reported by Windows/WMI and can differ from exact dedicated VRAM on some drivers.

### Library profile

Record the composition of the currently indexed library before interpreting benchmark results:

```powershell
.\windows-image-search.exe --benchmark-library-profile
```

The report includes indexed image count, source files that currently exist or are missing, persisted total source bytes, database size, extension distribution, width/height min/median/P90/max, megapixel min/P50/P90/P95/max, megapixel buckets, and landscape/portrait/square counts. The command uses the current local SQLite index and does not modify production search settings.

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

```powershell
cargo fmt --all -- --check
cargo check --all-targets
cargo test --all-targets
cargo run --release
```

Windows CI reads the package version from `Cargo.toml` and uploads a versioned artifact such as `windows-image-search-v0.2.8-win64`. The ZIP contains `windows-image-search.exe`, `run-v0.2-benchmark-gate.ps1`, `README.md`, `LICENSE` and `VERSION.txt`.

## Privacy

The application is designed for local indexing and local inference. Network access is only required when an embedding model has not yet been cached and must be downloaded.
