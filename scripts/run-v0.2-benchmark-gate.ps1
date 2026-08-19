[CmdletBinding()]
param(
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
    [int]$TextureSamples = 24
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$OutputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

function Convert-BytesToGiB {
    param([AllowNull()][object]$Bytes)
    if ($null -eq $Bytes) {
        return $null
    }
    try {
        return [math]::Round(([double]$Bytes / 1GB), 2)
    }
    catch {
        return $null
    }
}

function Get-SystemSnapshot {
    $os = Get-CimInstance Win32_OperatingSystem | Select-Object -First 1
    $computer = Get-CimInstance Win32_ComputerSystem | Select-Object -First 1
    $cpus = @(Get-CimInstance Win32_Processor)
    $gpus = @(Get-CimInstance Win32_VideoController)
    $disks = @(Get-CimInstance Win32_DiskDrive)

    [pscustomobject]@{
        captured_at = (Get-Date).ToString("o")
        windows = [pscustomobject]@{
            caption = $os.Caption
            version = $os.Version
            build_number = $os.BuildNumber
            architecture = $os.OSArchitecture
        }
        computer = [pscustomobject]@{
            manufacturer = $computer.Manufacturer
            model = $computer.Model
            total_physical_memory_bytes = [uint64]$computer.TotalPhysicalMemory
            total_physical_memory_gib = Convert-BytesToGiB $computer.TotalPhysicalMemory
        }
        cpu = @($cpus | ForEach-Object {
            [pscustomobject]@{
                name = $_.Name
                cores = $_.NumberOfCores
                logical_processors = $_.NumberOfLogicalProcessors
                max_clock_mhz = $_.MaxClockSpeed
            }
        })
        gpu = @($gpus | ForEach-Object {
            [pscustomobject]@{
                name = $_.Name
                video_processor = $_.VideoProcessor
                driver_version = $_.DriverVersion
                adapter_ram_reported_bytes = $_.AdapterRAM
                adapter_ram_reported_gib = Convert-BytesToGiB $_.AdapterRAM
            }
        })
        disk = @($disks | ForEach-Object {
            [pscustomobject]@{
                model = $_.Model
                media_type = $_.MediaType
                interface_type = $_.InterfaceType
                size_bytes = $_.Size
                size_gib = Convert-BytesToGiB $_.Size
            }
        })
    }
}

function Invoke-DiagnosticBenchmark {
    param(
        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string[]]$Arguments,

        [Parameter(Mandatory)]
        [string]$ExecutablePath,

        [Parameter(Mandatory)]
        [string]$ResultDirectory
    )

    $outputPath = Join-Path $ResultDirectory "$Name.txt"
    $startedAt = Get-Date
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

    Write-Host ""
    Write-Host "=== $Name ==="
    Write-Host "$ExecutablePath $($Arguments -join ' ')"

    & $ExecutablePath @Arguments 2>&1 | Tee-Object -FilePath $outputPath
    $exitCode = $LASTEXITCODE
    $stopwatch.Stop()
    $finishedAt = Get-Date

    [pscustomobject]@{
        name = $Name
        command = "$ExecutablePath $($Arguments -join ' ')"
        arguments = $Arguments
        started_at = $startedAt.ToString("o")
        finished_at = $finishedAt.ToString("o")
        wall_time_seconds = [math]::Round($stopwatch.Elapsed.TotalSeconds, 3)
        exit_code = $exitCode
        output_file = [System.IO.Path]::GetFileName($outputPath)
        succeeded = ($exitCode -eq 0)
    }
}

$resolvedExecutable = Resolve-Path -LiteralPath $Executable -ErrorAction Stop
$executablePath = $resolvedExecutable.Path
$rootOutput = [System.IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $rootOutput | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$resultDirectory = Join-Path $rootOutput "v0.2-benchmark-gate-$timestamp"
New-Item -ItemType Directory -Force -Path $resultDirectory | Out-Null

$gateStartedAt = Get-Date
$systemInfo = Get-SystemSnapshot
$systemInfo | ConvertTo-Json -Depth 8 | Set-Content -Path (Join-Path $resultDirectory "system-info.json") -Encoding utf8

$versionLines = @(& $executablePath --version 2>&1)
$versionExitCode = $LASTEXITCODE
$versionLines | Set-Content -Path (Join-Path $resultDirectory "version.txt") -Encoding utf8
$appVersion = if ($versionLines.Count -gt 0) { [string]$versionLines[0] } else { "unknown" }

$benchmarks = @(
    [pscustomobject]@{ Name = "ann"; Arguments = @("--benchmark-ann", [string]$AnnQueries) },
    [pscustomobject]@{ Name = "clip-preview"; Arguments = @("--benchmark-clip-preview", [string]$PreviewSamples) },
    [pscustomobject]@{ Name = "clip-runtime"; Arguments = @("--benchmark-clip-runtime", [string]$RuntimeSamples) },
    [pscustomobject]@{ Name = "image-models"; Arguments = @("--benchmark-image-models", [string]$ImageModelQueries) },
    [pscustomobject]@{ Name = "material-texture"; Arguments = @("--benchmark-material-texture", [string]$TextureSamples) }
)

$results = @()
foreach ($benchmark in $benchmarks) {
    try {
        $results += Invoke-DiagnosticBenchmark `
            -Name $benchmark.Name `
            -Arguments $benchmark.Arguments `
            -ExecutablePath $executablePath `
            -ResultDirectory $resultDirectory
    }
    catch {
        $failurePath = Join-Path $resultDirectory "$($benchmark.Name).txt"
        $_ | Out-String | Set-Content -Path $failurePath -Encoding utf8
        $results += [pscustomobject]@{
            name = $benchmark.Name
            command = "$executablePath $($benchmark.Arguments -join ' ')"
            arguments = $benchmark.Arguments
            started_at = $null
            finished_at = (Get-Date).ToString("o")
            wall_time_seconds = $null
            exit_code = $null
            output_file = [System.IO.Path]::GetFileName($failurePath)
            succeeded = $false
            runner_error = $_.Exception.Message
        }
    }
}

$gateFinishedAt = Get-Date
$manifest = [pscustomobject]@{
    gate = "v0.2-representative-library"
    generated_at = $gateFinishedAt.ToString("o")
    gate_started_at = $gateStartedAt.ToString("o")
    gate_wall_time_seconds = [math]::Round(($gateFinishedAt - $gateStartedAt).TotalSeconds, 3)
    executable = $executablePath
    application_version = $appVersion
    version_exit_code = $versionExitCode
    sample_counts = [pscustomobject]@{
        ann_queries = $AnnQueries
        preview_samples = $PreviewSamples
        runtime_samples = $RuntimeSamples
        image_model_queries = $ImageModelQueries
        texture_samples = $TextureSamples
    }
    benchmark_results = $results
}
$manifest | ConvertTo-Json -Depth 8 | Set-Content -Path (Join-Path $resultDirectory "manifest.json") -Encoding utf8

$summaryLines = @(
    "Windows Image Search v0.2 benchmark gate",
    "Generated: $($gateFinishedAt.ToString('o'))",
    "Application: $appVersion",
    "Executable: $executablePath",
    "Result directory: $resultDirectory",
    "",
    "Sample counts:",
    "  ANN queries: $AnnQueries",
    "  CLIP preview samples: $PreviewSamples",
    "  CLIP runtime samples: $RuntimeSamples",
    "  Image-model query sources: $ImageModelQueries",
    "  Material-texture samples: $TextureSamples",
    "",
    "Benchmark results:"
)
foreach ($result in $results) {
    $summaryLines += "  $($result.name): success=$($result.succeeded) exit=$($result.exit_code) wall_s=$($result.wall_time_seconds) output=$($result.output_file)"
}
$summaryLines += @(
    "",
    "Hardware details are stored in system-info.json.",
    "Win32_VideoController.AdapterRAM is recorded as reported by Windows/WMI and may not represent exact dedicated VRAM on every driver/GPU.",
    "Use the individual benchmark files plus representative same-material review before changing production defaults."
)
$summaryLines | Set-Content -Path (Join-Path $resultDirectory "summary.txt") -Encoding utf8

$zipPath = "$resultDirectory.zip"
if (Test-Path -LiteralPath $zipPath) {
    Remove-Item -LiteralPath $zipPath -Force
}
Compress-Archive -Path (Join-Path $resultDirectory "*") -DestinationPath $zipPath -Force

$failed = @($results | Where-Object { -not $_.succeeded })
Write-Host ""
Write-Host "Benchmark gate complete."
Write-Host "Result directory: $resultDirectory"
Write-Host "ZIP bundle: $zipPath"
Write-Host "Succeeded: $($results.Count - $failed.Count)/$($results.Count)"

if ($versionExitCode -ne 0 -or $failed.Count -gt 0) {
    exit 1
}
