# Windows Image Search

A native, local-first image index and search application for Windows, written in Rust.

## v0.1.0 features

- Configure one or more folders to index recursively.
- Incremental **Rescan** detects new, changed and deleted images.
- Supports JPG/JPEG, PNG, TIF and TIFF.
- Search by filename, path, EXIF/XMP description and keywords.
- Search/filter by dominant color with adjustable tolerance.
- Search by another image using local CLIP ViT-B/32 embeddings and cosine similarity.
- Switch between a resizable thumbnail grid and an Explorer-style detailed list.
- Double-click to open an image; context menu can open its containing folder or copy its path.

## First run

1. Start `windows-image-search.exe`.
2. Click **Add folder** and select one or more image folders.
3. Click **Rescan**.
4. Basic metadata/color indexing starts immediately. Visual embeddings are then generated for images that do not have one yet.
5. The CLIP model is downloaded on its first use and cached under the app's local data directory. After it is cached, similarity search can run offline.

Index data is stored under the current Windows user's local application-data directory in `WindowsImageSearch/index.sqlite3`. The index contains file paths, file state, dimensions, extracted text metadata, a compact color descriptor and CLIP embeddings; original images are never copied into the database.

## Search modes

### Text / tags

Type words in the search field. Multiple words use AND semantics and are matched case-insensitively against filename, full path, description and keywords.

### Color

Enable **Color**, choose a target color and adjust **Tolerance**. Color search can be combined with text/tag filtering.

### Similar image

Click **Search by image**, choose a query image, and results are ranked by CLIP cosine similarity. Text and color filters can still be applied to the ranked result set.

## Development

```powershell
cargo fmt --all -- --check
cargo check --all-targets
cargo run --release
```

Windows CI builds a release executable and uploads a `windows-image-search-win64` artifact.

## Privacy

The application is designed for local indexing and local inference. Network access is only required when the CLIP model has not yet been cached and must be downloaded.
