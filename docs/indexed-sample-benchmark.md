# Indexed sample benchmark

If the application already has a large indexed library, you do not need to select benchmark images manually.

Run the packaged executable and the benchmark scripts from the same extracted build/source tree:

```powershell
.\scripts\run-indexed-sample-benchmark.ps1 -Executable .\windows-image-search.exe
```

The default is 50 samples. The automatic suite reads the existing application index and runs:

- library profile
- ANN/HNSW recall and latency
- original-image vs cached-preview CLIP
- CPU/DirectML CLIP runtime and batch timings
- alternative image-model transformed-query evaluation
- material-texture transformed-query evaluation

The Rust benchmark implementations choose deterministic samples spread across the available indexed corpus instead of taking one contiguous first-N block. This reduces ordering bias when a 10k+ library is grouped by folder or filename.

To request a different bounded sample size:

```powershell
.\scripts\run-indexed-sample-benchmark.ps1 -Executable .\windows-image-search.exe -SampleCount 100
```

For the image-model comparison, the requested sample count now bounds the corpus as well; a 50-sample run no longer silently expands to a 512-image corpus. Up to 16 deterministic source images from that bounded corpus are transformed into three query variants for each of the five candidate models. The material-texture benchmark remains capped at 128 samples. Other automatic benchmarks use the requested count subject to their own corpus availability.

Long-running child benchmarks emit a 15-second heartbeat with elapsed time, accumulated CPU time, and working-set memory so a stalled model download/session initialization is visible instead of appearing frozen.

The run writes the normal timestamped result directory and ZIP bundle under `benchmark-results`, including per-benchmark output, system information, process RAM/private-memory sampling, and GPU process-memory sampling when Windows exposes the counters.

## Optional labeled same-material evaluation

No TSV is required for the automatic indexed sample suite. A labeled material manifest is still optional when you want to measure retrieval between different images/faces that you explicitly know belong to the same material/design. Arbitrary index order cannot infer those positive groups reliably.

When such a manifest is available:

```powershell
.\scripts\run-indexed-sample-benchmark.ps1 `
  -Executable .\windows-image-search.exe `
  -SampleCount 50 `
  -MaterialEvalManifest .\material-eval.tsv
```
