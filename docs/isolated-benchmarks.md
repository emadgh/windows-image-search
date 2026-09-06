# Isolated benchmark workspaces

Benchmarking a live collection must not migrate or hydrate its database. Prepare
an online SQLite backup with the application indexer paused:

```powershell
python scripts/prepare-benchmark-workspace.py "$env:TEMP\wis-benchmark" `
  --session "$env:LOCALAPPDATA\WindowsImageSearch\index.sqlite3" `
  --root 'D:\My collection'
scripts/run-indexed-sample-benchmark.ps1 `
  -Executable .\windows-image-search.exe `
  -BenchmarkWorkspace "$env:TEMP\wis-benchmark" -SampleCount 50
python scripts/prepare-benchmark-workspace.py "$env:TEMP\wis-benchmark" --verify
```

The destination must be new. The preparation tool opens source databases with
SQLite `mode=ro`, uses the online backup API, checks backup integrity, and retains
baseline copies and logical SHA-256/table counts. It filters only the working copy
to the selected collection and removes registered roots from that copy. Source
photos remain read-only benchmark inputs. Models are copied to the workspace;
additional benchmark models download there when needed.

The executable requires a marked workspace and an existing contained database.
It directs reports, model caches, preview caches and ANN files there and skips
portable-root hydration. The gate first probes an invalid workspace; an old
executable that ignores the option is rejected before any benchmark starts.
Final verification compares source tables with the baseline and runs SQLite
`quick_check`. Independent application writes can change the source hash; such a
change must be investigated, never reported as unchanged.

For a separate large ANN experiment, clone the prepared workspace first, then run:

```powershell
.\windows-image-search.exe --benchmark-workspace C:\Temp\wis-preview-bank `
  --benchmark-build-preview-vectors
.\windows-image-search.exe --benchmark-workspace C:\Temp\wis-preview-bank --benchmark-ann 50
```

This explicitly replaces vectors **only in the marked copy**, using existing valid
512 px portable previews, CPU/4 threads/batch 32. Missing previews are skipped;
no original preview or database is written. Report this as a preview-derived vector
bank, separately from the paused production-vector snapshot. It is not evidence
of semantic/material relevance or a completed production index.

## Face runtime without identity labels

YuNet and SFace runner manifests accept this optional tab-separated row:

```text
evaluation	runtime-only
```

In this mode, identity/detector evaluation is skipped entirely and reports contain
`quality_status=not_evaluated`. SFace still needs valid face images and five normalized
landmarks; its required person column may contain `unlabeled`, which is ignored.
Stored detector landmarks can supply alignment inputs but are not ground truth.
YuNet image rows do not imply negative labels in this mode. Existing manifests
default to labeled evaluation, and unknown/duplicate evaluation modes are rejected.
Use the face gate's `-BenchmarkWorkspace` option and keep manifests in that workspace.
Face ANN additionally needs registered roots pointing to isolated portable copies.

The image gate accepts `-OnlyBenchmarks` to rerun selected phases, such as
`clip-runtime` or `preview-vector-bank,ann`. The full suite remains the default.
An incomplete image-model comparison saves every attempted model's result and
then returns nonzero; a completed process is not a passing quality evaluation.
Process telemetry uses a nominal 200 ms polling interval, but synchronous GPU
counter collection may delay it. Short-lived process peaks can be missed;
unavailable GPU samples are retained as null rather than reported as zero.

DirectML registration failures must surface explicitly. CLIP production code can
then report its CPU fallback accurately; diagnostic model adapters fail instead of
silently reporting a CPU run as DirectML. Operator placement within an initialized
DirectML session is not measured by these benchmarks.
