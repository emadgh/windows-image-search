# Issue 225 implementation progress

This branch implements bounded previews for images that are unsafe to decode directly.

Completed implementation:

- Adds the optional `libvips-backend` Cargo feature using `rs-vips`.
- Preserves the bounded scaled-IDCT JPEG path and applies EXIF orientation to its already-bounded pixel buffer.
- Routes bounded PNG and TIFF generation through libvips when enabled and fails closed with a clear diagnostic when the backend is unavailable.
- Routes by both compressed file size and estimated decoded-memory cost so highly compressed, very large-dimension sources cannot bypass the bounded path.
- Reuses the existing portable oversized-preview and 512 px thumbnail cache layout without changing source files, the database schema, or portable index format.
- Builds missing bounded derivatives from the Preview background worker instead of requiring a rescan.
- Commits bounded-preview and thumbnail cache files through same-directory temporary files and atomic rename semantics, with cache reuse/invalidation coverage.
- Covers JPEG, PNG, and TIFF bounded generation, backend selection, cache reuse/invalidation, source cleanup, decode failure, EXIF orientation, and on-demand preview creation.
- Pins the ZIP-aware `update-via-github` implementation so updates can replace the executable together with required sidecar DLLs.
- Builds Windows release/test bundles with the official libvips 8.18.6 runtime DLL set beside `windows-image-search.exe`.
- Publishes release ZIP assets rather than a feature-enabled standalone EXE that would be unusable without its libvips sidecars.
- Adds clean-bundle smoke validation that removes the downloaded libvips directory from `PATH` and launches the staged/extracted executable using only bundled runtime files.

Validation still required before issue #225 can close:

- Finish the final Windows CI run with both default and `libvips-backend` configurations. Some unrelated pre-existing ANN/copy-requirement/face-quality tests have shown intermittent failures and are being kept separate from #225 behavior.
- Add/confirm restart-safety coverage for stale temporary cache files.
- Re-run the existing `scripts/run-oversized-source-validation.ps1` scenario against a representative source larger than 600 MiB and record peak-memory/result evidence.
- Validate the final release ZIP/update path and keep PR #226 Draft until the acceptance checks above are complete.
