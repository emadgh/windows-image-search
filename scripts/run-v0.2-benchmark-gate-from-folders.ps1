[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateNotNullOrEmpty()]
    [string[]]$FolderRoot,

    [Parameter()]
    [string]$Executable = ".\windows-image-search.exe",

    [Parameter()]
    [string]$OutputDirectory = ".\benchmark-results",

    [Parameter()]
    [ValidateRange(1, 10000)]
    [int]$AnnQueries = 32,

    [Parameter()]
    [ValidateRange(3, 10000)]
    [int]$PreviewSamples = 64,

    [Parameter()]
    [ValidateRange(1, 10000)]
    [int]$RuntimeSamples = 64,

    [Parameter()]
    [ValidateRange(1, 64)]
    [int]$ImageModelQueries = 24,

    [Parameter()]
    [ValidateRange(1, 128)]
    [int]$TextureSamples = 24,

    [Parameter()]
    [ValidateRange(2, 1000000)]
    [int]$MinimumImagesPerGroup = 2,

    [Parameter()]
    [switch]$PreviewGroups
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$OutputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
$manifestGenerator = Join-Path $scriptDirectory "new-material-eval-manifest-from-folders.ps1"
$benchmarkGate = Join-Path $scriptDirectory "run-v0.2-benchmark-gate.ps1"

if (-not (Test-Path -LiteralPath $manifestGenerator -PathType Leaf)) {
    throw "Manifest generator is missing: $manifestGenerator"
}
if (-not (Test-Path -LiteralPath $benchmarkGate -PathType Leaf)) {
    throw "Benchmark gate is missing: $benchmarkGate"
}

$outputRoot = [System.IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $outputRoot | Out-Null
$manifestPath = Join-Path $outputRoot "material-eval.generated.tsv"

& $manifestGenerator `
    -Root $FolderRoot `
    -OutputPath $manifestPath `
    -MinimumImagesPerGroup $MinimumImagesPerGroup `
    -Preview:$PreviewGroups

if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
if ($PreviewGroups) {
    Write-Host "benchmark_gate_started=false"
    exit 0
}

& $benchmarkGate `
    -Executable $Executable `
    -OutputDirectory $outputRoot `
    -AnnQueries $AnnQueries `
    -PreviewSamples $PreviewSamples `
    -RuntimeSamples $RuntimeSamples `
    -ImageModelQueries $ImageModelQueries `
    -TextureSamples $TextureSamples `
    -MaterialEvalManifest $manifestPath

exit $LASTEXITCODE
