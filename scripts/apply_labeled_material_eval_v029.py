from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:160]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once("Cargo.toml", 'version = "0.2.8"', 'version = "0.2.9"')

replace_once(
    "src/main.rs",
    "if matches!(mode, StartupMode::Version) {",
    "if matches!(&mode, StartupMode::Version) {",
)
replace_once(
    "src/main.rs",
    "if matches!(mode, StartupMode::LibraryProfile) {",
    "if matches!(&mode, StartupMode::LibraryProfile) {",
)

replace_once(
    "src/material_eval.rs",
    """        if line_number == 1
            && group.eq_ignore_ascii_case(\"group\")
            && path_text.eq_ignore_ascii_case(\"path\")
        {""",
    """        if output.is_empty()
            && path_groups.is_empty()
            && group.eq_ignore_ascii_case(\"group\")
            && path_text.eq_ignore_ascii_case(\"path\")
        {""",
)

replace_once(
    "src/material_eval.rs",
    """    let all_indices: Vec<usize> = (0..items.len()).collect();
    let metrics = evaluate_scores(items, &all_indices, |query, candidate| {
        let query_index = items.iter().position(|item| item.rowid == query.rowid)?;
        let candidate_index = items.iter().position(|item| item.rowid == candidate.rowid)?;
        Some(dot(&embeddings[query_index], &embeddings[candidate_index]))
    });""",
    """    let all_indices: Vec<usize> = (0..items.len()).collect();
    let embedding_index: HashMap<usize, usize> = items
        .iter()
        .enumerate()
        .map(|(index, item)| (item.rowid, index))
        .collect();
    let metrics = evaluate_scores(items, &all_indices, |query, candidate| {
        let query_index = *embedding_index.get(&query.rowid)?;
        let candidate_index = *embedding_index.get(&candidate.rowid)?;
        Some(dot(&embeddings[query_index], &embeddings[candidate_index]))
    });""",
)

replace_once(
    "scripts/run-v0.2-benchmark-gate.ps1",
    """    [Parameter()]
    [ValidateRange(1, 128)]
    [int]$TextureSamples = 24
)""",
    """    [Parameter()]
    [ValidateRange(1, 128)]
    [int]$TextureSamples = 24,

    [Parameter()]
    [string]$MaterialEvalManifest = \"\"
)""",
)

replace_once(
    "scripts/run-v0.2-benchmark-gate.ps1",
    """    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $ExecutablePath
    $startInfo.Arguments = ($Arguments -join ' ')
    $startInfo.UseShellExecute = $false""",
    """    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $ExecutablePath
    foreach ($argument in $Arguments) {
        [void]$startInfo.ArgumentList.Add($argument)
    }
    $startInfo.UseShellExecute = $false""",
)

replace_once(
    "scripts/run-v0.2-benchmark-gate.ps1",
    """$benchmarks = @(
    [pscustomobject]@{ Name = \"library-profile\"; Arguments = @(\"--benchmark-library-profile\") },
    [pscustomobject]@{ Name = \"ann\"; Arguments = @(\"--benchmark-ann\", [string]$AnnQueries) },
    [pscustomobject]@{ Name = \"clip-preview\"; Arguments = @(\"--benchmark-clip-preview\", [string]$PreviewSamples) },
    [pscustomobject]@{ Name = \"clip-runtime\"; Arguments = @(\"--benchmark-clip-runtime\", [string]$RuntimeSamples) },
    [pscustomobject]@{ Name = \"image-models\"; Arguments = @(\"--benchmark-image-models\", [string]$ImageModelQueries) },
    [pscustomobject]@{ Name = \"material-texture\"; Arguments = @(\"--benchmark-material-texture\", [string]$TextureSamples) }
)

$results = @()""",
    """$benchmarks = @(
    [pscustomobject]@{ Name = \"library-profile\"; Arguments = @(\"--benchmark-library-profile\") },
    [pscustomobject]@{ Name = \"ann\"; Arguments = @(\"--benchmark-ann\", [string]$AnnQueries) },
    [pscustomobject]@{ Name = \"clip-preview\"; Arguments = @(\"--benchmark-clip-preview\", [string]$PreviewSamples) },
    [pscustomobject]@{ Name = \"clip-runtime\"; Arguments = @(\"--benchmark-clip-runtime\", [string]$RuntimeSamples) },
    [pscustomobject]@{ Name = \"image-models\"; Arguments = @(\"--benchmark-image-models\", [string]$ImageModelQueries) },
    [pscustomobject]@{ Name = \"material-texture\"; Arguments = @(\"--benchmark-material-texture\", [string]$TextureSamples) }
)

$materialEvalRequested = -not [string]::IsNullOrWhiteSpace($MaterialEvalManifest)
$resolvedMaterialEvalManifest = $null
if ($materialEvalRequested) {
    $resolvedMaterialEvalManifest = (Resolve-Path -LiteralPath $MaterialEvalManifest -ErrorAction Stop).Path
    $benchmarks += [pscustomobject]@{
        Name = \"material-eval\"
        Arguments = @(\"--benchmark-material-eval\", $resolvedMaterialEvalManifest)
    }
}

$results = @()""",
)

replace_once(
    "scripts/run-v0.2-benchmark-gate.ps1",
    """    version_exit_code = $versionExitCode
    memory_sampling = [pscustomobject]@{""",
    """    version_exit_code = $versionExitCode
    material_eval = [pscustomobject]@{
        requested = $materialEvalRequested
        manifest = $resolvedMaterialEvalManifest
        status = if ($materialEvalRequested) { \"included\" } else { \"not_run_no_manifest\" }
    }
    memory_sampling = [pscustomobject]@{""",
)

replace_once(
    "scripts/run-v0.2-benchmark-gate.ps1",
    """    \"  Material-texture samples: $TextureSamples\",
    \"\",
    \"Benchmark results:\"""",
    """    \"  Material-texture samples: $TextureSamples\",
    \"  Labeled material eval: $(if ($materialEvalRequested) { $resolvedMaterialEvalManifest } else { 'not run (no -MaterialEvalManifest)' })\",
    \"\",
    \"Benchmark results:\"""",
)

replace_once(
    "README.md",
    """  -TextureSamples 48
```""",
    """  -TextureSamples 48 `
  -MaterialEvalManifest .\\material-eval.tsv
```""",
)

replace_once(
    "README.md",
    """Peak working set is process-level resident RAM. Private-memory and GPU-memory values are sampled while the benchmark runs rather than being allocator-level exact maxima. `Win32_VideoController.AdapterRAM` in `system-info.json` is the capacity reported by Windows/WMI and can differ from exact dedicated VRAM on some drivers.

### Library profile""",
    """Peak working set is process-level resident RAM. Private-memory and GPU-memory values are sampled while the benchmark runs rather than being allocator-level exact maxima. `Win32_VideoController.AdapterRAM` in `system-info.json` is the capacity reported by Windows/WMI and can differ from exact dedicated VRAM on some drivers.

`-MaterialEvalManifest` is optional. When it is omitted, `manifest.json` and `summary.txt` explicitly mark labeled same-material evaluation as not run. When supplied, its path is passed as a discrete process argument, so spaces in the manifest path are supported.

### Labeled same-material evaluation

Use a small manually curated UTF-8 TSV when you need to measure retrieval across *different images of the same material/design*, not only transformed copies of one source image:

```text
group\tpath
Calacatta Gold\tD:\\Material Eval\\calacatta-face-01.jpg
Calacatta Gold\tD:\\Material Eval\\calacatta-face-02.jpg
Travertine Beige\ttravertine-face-01.jpg
Travertine Beige\ttravertine-face-02.jpg
```

Blank lines and lines beginning with `#` are ignored. Relative paths are resolved relative to the TSV file. Every evaluated group must contain at least two distinct indexed images; assigning one image path to different groups is rejected.

Run the labeled benchmark directly:

```powershell
.\\windows-image-search.exe --benchmark-material-eval .\\material-eval.tsv
```

Or include it in the complete gate with `-MaterialEvalManifest`. Every labeled image is used as a query, the query itself is excluded, and the first *other* image from the same group is the relevant result. The report compares indexed dHash, Gradient, LBP, combined material texture, the current material+dHash blend, stored production CLIP, and CPU embeddings from CLIP B32, UNICOM B16/B32, Nomic Vision v1.5 and ResNet50. It reports Recall@1/5/10/25, MRR and mean first-relevant rank, plus model initialization/throughput and embedding coverage. The command never overwrites production embeddings or changes search defaults.

### Library profile""",
)

replace_once(
    "README.md",
    "windows-image-search-v0.2.8-win64",
    "windows-image-search-v0.2.9-win64",
)
