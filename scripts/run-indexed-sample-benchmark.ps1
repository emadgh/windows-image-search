[CmdletBinding()]
param(
    [Parameter()]
    [string]$Executable = ".\windows-image-search.exe",

    [Parameter()]
    [string]$OutputDirectory = ".\benchmark-results",

    [Parameter()]
    [ValidateRange(3, 10000)]
    [int]$SampleCount = 50,

    [Parameter()]
    [string]$MaterialEvalManifest = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$gateScript = Join-Path $PSScriptRoot "run-v0.2-benchmark-gate.ps1"
if (-not (Test-Path -LiteralPath $gateScript -PathType Leaf)) {
    throw "Benchmark gate script not found: $gateScript"
}

# The underlying Rust benchmarks already select deterministic, evenly distributed
# samples from the existing indexed corpus. We intentionally do not take the first
# N rows because path/folder ordering can bias a representative benchmark.
$annQueries = $SampleCount
$previewSamples = [Math]::Max(3, $SampleCount)
$runtimeSamples = $SampleCount
$imageModelQueries = [Math]::Min(64, $SampleCount)
$textureSamples = [Math]::Min(128, $SampleCount)

Write-Host "Windows Image Search indexed-library benchmark"
Write-Host "Requested indexed sample count: $SampleCount"
Write-Host "Sampling: deterministic and spread across the existing index"
Write-Host "ANN queries: $annQueries"
Write-Host "CLIP preview samples: $previewSamples"
Write-Host "CLIP runtime samples: $runtimeSamples"
Write-Host "Image-model query sources: $imageModelQueries"
Write-Host "Material-texture samples: $textureSamples"
if ([string]::IsNullOrWhiteSpace($MaterialEvalManifest)) {
    Write-Host "Labeled same-material evaluation: skipped (optional; no TSV required for the automatic indexed sample suite)"
}

$gateParameters = @{
    Executable = $Executable
    OutputDirectory = $OutputDirectory
    AnnQueries = $annQueries
    PreviewSamples = $previewSamples
    RuntimeSamples = $runtimeSamples
    ImageModelQueries = $imageModelQueries
    TextureSamples = $textureSamples
}

if (-not [string]::IsNullOrWhiteSpace($MaterialEvalManifest)) {
    $gateParameters.MaterialEvalManifest = $MaterialEvalManifest
}

& $gateScript @gateParameters
$exitCode = $LASTEXITCODE
if ($null -eq $exitCode) {
    $exitCode = 0
}
exit $exitCode
