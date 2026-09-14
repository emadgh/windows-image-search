# Bounded large-image decoding with libvips

Issue #225 adds an optional libvips backend for source images that must not be decoded at full resolution.

## Backend selection

The existing scaled JPEG decoder remains the bounded backend for oversized JPEG files. PNG and TIFF use libvips when the `libvips-backend` Cargo feature is enabled. Without that feature, those formats fail closed instead of falling back to an unbounded full-resolution decode.

libvips uses its `thumbnail` operation with a 2048 px bounding box. Orientation metadata is honored by the libvips thumbnail operation by default. The resulting bounded image is written through the existing oversized-preview cache and also seeds the existing 512 px portable thumbnail cache. Source files are never modified.

## Windows build dependency

The Rust bindings link against `libvips.lib`, and the application needs the matching libvips DLLs at runtime. Set `VIPS_LIB_DIR` to the directory containing `libvips.lib` before building with:

```powershell
cargo build --release --features libvips-backend
```

The Windows CI job downloads the official libvips 8.18.6 x64 web bundle, locates `libvips.lib` and `libvips-42.dll`, adds the runtime directory to `PATH`, runs both non-libvips and libvips test configurations, and copies the runtime DLL set beside the release executable in the test artifact.

The feature is intentionally optional while the existing scaled JPEG path is retained for comparison and regression testing. Release packaging will only switch to the libvips-enabled build after the Windows CI/runtime bundle is validated on this branch.
