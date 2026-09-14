# Issue 225 implementation progress

This branch introduces the first implementation slice for issue #225.

- Adds the optional `libvips-backend` Cargo feature using `rs-vips`.
- Preserves the existing scaled JPEG decoder for oversized JPEG sources.
- Routes bounded oversized PNG and TIFF generation through libvips when enabled.
- Fails closed for unsafe PNG/TIFF sources when the feature is unavailable.
- Reuses the existing portable oversized-preview and 512 px thumbnail cache layout.
- Builds a missing bounded preview derivative from the Preview background worker instead of requiring a rescan first.
- Writes the 512 px thumbnail cache through a temporary file before committing the final cache path.
- Adds backend-selection, cache reuse/invalidation, on-demand preview, and bounded PNG regression tests.
- Adds Windows CI coverage that downloads the official libvips runtime, builds/tests the feature, and bundles the DLL set beside the executable.

Still to complete before issue #225 can close: dimension-based unsafe routing during indexing for highly compressed sources, TIFF fixture coverage, final Cargo.lock/release packaging, and clean-machine artifact validation.
