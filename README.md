# Windows Image Search

Native local image indexing and search for Windows, written in Rust.

## v0.1.0 goals

- Configurable indexed folders with incremental rescan
- JPG, PNG and TIFF support
- CLIP image-to-image similarity search
- Dominant-color filtering
- Metadata / keyword / description search
- Resizable thumbnail grid and detailed list views

All image index data stays local. The CLIP vision model is downloaded on first semantic-index use and cached for subsequent offline use.
