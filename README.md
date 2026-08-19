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

Windows CI reads the package version from `Cargo.toml` and uploads a versioned artifact such as `windows-image-search-v0.2.4-win64`. The ZIP contains the stable executable name `windows-image-search.exe`, `README.md`, `LICENSE` and `VERSION.txt`.

## Privacy

The application is designed for local indexing and local inference. Network access is only required when the CLIP model has not yet been cached and must be downloaded.
