# v0.3 Face benchmark gate

`run-v0.3-face-benchmark-gate.ps1` runs the production-candidate YuNet detector and SFace embedder benchmark adapters against the same labeled manifests on both CPU and DirectML. It is intended to collect the runtime and quality evidence required by issue #62 without bundling benchmark model weights.

## Inputs

The runner requires one YuNet adapter manifest and one SFace adapter manifest. The provider value inside each input manifest is treated as a template: the gate creates short-lived sibling copies with `cpu` and `directml` selected, then deletes those temporary files in a `finally` block. Keeping the temporary manifests beside the originals preserves relative model/image paths.

YuNet manifest records:

```text
model<TAB>model-path<TAB>provider<TAB>license<TAB>redistributable<TAB>commercial-use<TAB>source
settings<TAB>score-threshold<TAB>nms-threshold<TAB>top-k
image<TAB>image-id<TAB>image-path
gt<TAB>image-id<TAB>x<TAB>y<TAB>width<TAB>height
```

The `settings` row is optional; the production YuNet defaults are used when it is omitted. Include explicit `image` rows for no-face examples so detector false positives are measurable.

SFace manifest records:

```text
model<TAB>model-path<TAB>provider<TAB>license<TAB>redistributable<TAB>commercial-use<TAB>source
face<TAB>face-id<TAB>person-id<TAB>image-path<TAB>left-eye-x<TAB>left-eye-y<TAB>right-eye-x<TAB>right-eye-y<TAB>nose-x<TAB>nose-y<TAB>mouth-left-x<TAB>mouth-left-y<TAB>mouth-right-x<TAB>mouth-right-y
```

All SFace landmark coordinates are normalized to `[0,1]` in the EXIF-oriented source image.

## Run the gate

```powershell
.\run-v0.3-face-benchmark-gate.ps1 `
  -Executable .\windows-image-search.exe `
  -YuNetManifest .\yunet-eval.tsv `
  -SFaceManifest .\sface-eval.tsv
```

By default CPU failures fail the gate, while a DirectML failure is retained as an unavailable runtime result so machines without a usable DirectML device can still produce CPU evidence. On the intended Windows GPU validation machine, require both DirectML runs explicitly:

```powershell
.\run-v0.3-face-benchmark-gate.ps1 `
  -Executable .\windows-image-search.exe `
  -YuNetManifest .\yunet-eval.tsv `
  -SFaceManifest .\sface-eval.tsv `
  -RequireDirectML
```

## Output

Each run creates `benchmark-results/v0.3-face-benchmark-gate-<timestamp>/` plus a ZIP beside it. The bundle contains:

- YuNet CPU and DirectML stdout/stderr/reports
- SFace CPU and DirectML stdout/stderr/reports
- the current populated-index `--benchmark-face-ann 32` report for face-storage and exact-vs-ANN crossover evidence
- `manifest.json` with exact commands, exit codes, wall time, process RAM/private-memory peaks, sampled GPU dedicated/shared memory and source manifest paths
- `system-info.json` with Windows, CPU, RAM, GPU and driver information
- `summary.txt` for a compact CPU-vs-DirectML comparison
- `version.txt` for the exact application build under test

The adapter reports retain the shared #92 evaluator metrics. YuNet reports IoU-based precision/recall/F1, no-face false positives and face-size recall buckets. SFace reports Recall@1/5/10, MRR, same/different-person distance distributions and threshold-sweep results. Both adapters also report model initialization time and persistent-session inference throughput. SFace additionally performs a benchmark-only batch-size sweep for 1/2/4/8/16 aligned faces using the same persistent ONNX session, recording support/failure, warm-up latency, mean/P50 batch latency, throughput, and output-dimension validation for each size. Batch failures are retained as evidence and do not change production embedding behavior.

## Interpretation

Do not select thresholds or execution-provider defaults from synthetic/unit-test data. Run the gate on representative small, angled, occluded, multi-face and low-resolution detector examples plus identity examples spanning pose, lighting, age/time, crop and compression changes. Attach or summarize the generated ZIP in #62 before changing production defaults.

The gate covers model quality, initialization/inference timing, CPU/DirectML execution, batch-size behavior, process/GPU memory, and—through its built-in `--benchmark-face-ann 32` step—face-index storage plus exact-vs-ANN crossover. The storage/crossover result is only production evidence when the current index contains a representative number and distribution of face embeddings; a small or unrepresentative index should be recorded as insufficient evidence rather than used to select an ANN default.
