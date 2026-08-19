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
    [int]$TextureSamples = 24,

    [Parameter()]
    [string]$MaterialEvalManifest = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$OutputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$script:GpuCounterUnavailableReason = $null

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

function Convert-BytesToMiB {
    param([AllowNull()][object]$Bytes)
    if ($null -eq $Bytes) {
        return $null
    }
    try {
        return [math]::Round(([double]$Bytes / 1MB), 2)
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

function Get-GpuProcessMemorySample {
    param(
        [Parameter(Mandatory)]
        [int]$TargetProcessId
    )

    if ($null -ne $script:GpuCounterUnavailableReason) {
        return [pscustomobject]@{
            counter_available = $false
            process_sample_found = $false
            dedicated_bytes = $null
            shared_bytes = $null
            error = $script:GpuCounterUnavailableReason
        }
    }

    try {
        $sampleSet = Get-Counter -Counter @(
            '\GPU Process Memory(*)\Dedicated Usage',
            '\GPU Process Memory(*)\Shared Usage'
        ) -MaxSamples 1 -ErrorAction Stop

        [double]$dedicated = 0
        [double]$shared = 0
        $found = $false
        foreach ($sample in $sampleSet.CounterSamples) {
            $instanceName = [string]$sample.InstanceName
            if ($instanceName -notmatch "(^|_)pid_$TargetProcessId(_|$)") {
                continue
            }
            $found = $true
            if ([string]$sample.Path -match '(?i)\\dedicated usage$') {
                $dedicated += [double]$sample.CookedValue
            }
            elseif ([string]$sample.Path -match '(?i)\\shared usage$') {
                $shared += [double]$sample.CookedValue
            }
        }

        return [pscustomobject]@{
            counter_available = $true
            process_sample_found = $found
            dedicated_bytes = if ($found) { [int64][math]::Max(0, $dedicated) } else { $null }
            shared_bytes = if ($found) { [int64][math]::Max(0, $shared) } else { $null }
            error = $null
        }
    }
    catch {
        $script:GpuCounterUnavailableReason = $_.Exception.Message
        return [pscustomobject]@{
            counter_available = $false
            process_sample_found = $false
            dedicated_bytes = $null
            shared_bytes = $null
            error = $script:GpuCounterUnavailableReason
        }
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

    $stdoutPath = Join-Path $ResultDirectory "$Name.stdout.txt"
    $stderrPath = Join-Path $ResultDirectory "$Name.stderr.txt"
    $combinedPath = Join-Path $ResultDirectory "$Name.txt"
    $startedAt = Get-Date
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

    Write-Host ""
    Write-Host "=== $Name ==="
    Write-Host "$ExecutablePath $($Arguments -join ' ')"

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $ExecutablePath
    foreach ($argument in $Arguments) {
        [void]$startInfo.ArgumentList.Add($argument)
    }
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.CreateNoWindow = $true

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw "Cannot start benchmark process: $ExecutablePath"
    }

    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    [int64]$peakWorkingSet = 0
    [int64]$peakPrivateMemory = 0
    [AllowNull()][object]$peakGpuDedicated = $null
    [AllowNull()][object]$peakGpuShared = $null
    [AllowNull()][object]$gpuCounterAvailable = $null
    $gpuProcessSampleFound = $false
    $gpuCounterError = $null
    $nextGpuSample = [DateTime]::UtcNow

    do {
        try {
            $process.Refresh()
            $peakWorkingSet = [math]::Max($peakWorkingSet, [int64]$process.WorkingSet64)
            $peakWorkingSet = [math]::Max($peakWorkingSet, [int64]$process.PeakWorkingSet64)
            $peakPrivateMemory = [math]::Max($peakPrivateMemory, [int64]$process.PrivateMemorySize64)
        }
        catch {
            # The process may have exited between polling and Refresh().
        }

        if ([DateTime]::UtcNow -ge $nextGpuSample) {
            $gpuSample = Get-GpuProcessMemorySample -TargetProcessId $process.Id
            $gpuCounterAvailable = $gpuSample.counter_available
            if ($null -ne $gpuSample.error) {
                $gpuCounterError = $gpuSample.error
            }
            if ($gpuSample.process_sample_found) {
                $gpuProcessSampleFound = $true
                if ($null -eq $peakGpuDedicated -or $gpuSample.dedicated_bytes -gt $peakGpuDedicated) {
                    $peakGpuDedicated = $gpuSample.dedicated_bytes
                }
                if ($null -eq $peakGpuShared -or $gpuSample.shared_bytes -gt $peakGpuShared) {
                    $peakGpuShared = $gpuSample.shared_bytes
                }
            }
            $nextGpuSample = [DateTime]::UtcNow.AddSeconds(1)
        }

        $exited = $process.WaitForExit(200)
    } while (-not $exited)

    $process.WaitForExit()
    $stdout = $stdoutTask.Result
    $stderr = $stderrTask.Result
    $exitCode = $process.ExitCode
    $stopwatch.Stop()
    $finishedAt = Get-Date

    $stdout | Set-Content -Path $stdoutPath -Encoding utf8
    $stderr | Set-Content -Path $stderrPath -Encoding utf8
    @($stdout, $stderr) | Set-Content -Path $combinedPath -Encoding utf8
    if (-not [string]::IsNullOrWhiteSpace($stdout)) {
        Write-Host $stdout.TrimEnd()
    }
    if (-not [string]::IsNullOrWhiteSpace($stderr)) {
        Write-Host $stderr.TrimEnd()
    }

    [pscustomobject]@{
        name = $Name
        process_id = $process.Id
        command = "$ExecutablePath $($Arguments -join ' ')"
        arguments = $Arguments
        started_at = $startedAt.ToString("o")
        finished_at = $finishedAt.ToString("o")
        wall_time_seconds = [math]::Round($stopwatch.Elapsed.TotalSeconds, 3)
        exit_code = $exitCode
        stdout_file = [System.IO.Path]::GetFileName($stdoutPath)
        stderr_file = [System.IO.Path]::GetFileName($stderrPath)
        output_file = [System.IO.Path]::GetFileName($combinedPath)
        peak_working_set_bytes = $peakWorkingSet
        peak_working_set_mib = Convert-BytesToMiB $peakWorkingSet
        sampled_peak_private_memory_bytes = $peakPrivateMemory
        sampled_peak_private_memory_mib = Convert-BytesToMiB $peakPrivateMemory
        gpu_process_memory_counter_available = $gpuCounterAvailable
        gpu_process_sample_found = $gpuProcessSampleFound
        sampled_peak_gpu_dedicated_bytes = $peakGpuDedicated
        sampled_peak_gpu_dedicated_mib = Convert-BytesToMiB $peakGpuDedicated
        sampled_peak_gpu_shared_bytes = $peakGpuShared
        sampled_peak_gpu_shared_mib = Convert-BytesToMiB $peakGpuShared
        gpu_counter_error = $gpuCounterError
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
    [pscustomobject]@{ Name = "library-profile"; Arguments = @("--benchmark-library-profile") },
    [pscustomobject]@{ Name = "ann"; Arguments = @("--benchmark-ann", [string]$AnnQueries) },
    [pscustomobject]@{ Name = "clip-preview"; Arguments = @("--benchmark-clip-preview", [string]$PreviewSamples) },
    [pscustomobject]@{ Name = "clip-runtime"; Arguments = @("--benchmark-clip-runtime", [string]$RuntimeSamples) },
    [pscustomobject]@{ Name = "image-models"; Arguments = @("--benchmark-image-models", [string]$ImageModelQueries) },
    [pscustomobject]@{ Name = "material-texture"; Arguments = @("--benchmark-material-texture", [string]$TextureSamples) }
)

$materialEvalRequested = -not [string]::IsNullOrWhiteSpace($MaterialEvalManifest)
$resolvedMaterialEvalManifest = $null
if ($materialEvalRequested) {
    $resolvedMaterialEvalManifest = (Resolve-Path -LiteralPath $MaterialEvalManifest -ErrorAction Stop).Path
    $benchmarks += [pscustomobject]@{
        Name = "material-eval"
        Arguments = @("--benchmark-material-eval", $resolvedMaterialEvalManifest)
    }
}

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
        $failurePath = Join-Path $resultDirectory "$($benchmark.Name).runner-error.txt"
        $_ | Out-String | Set-Content -Path $failurePath -Encoding utf8
        $results += [pscustomobject]@{
            name = $benchmark.Name
            process_id = $null
            command = "$executablePath $($benchmark.Arguments -join ' ')"
            arguments = $benchmark.Arguments
            started_at = $null
            finished_at = (Get-Date).ToString("o")
            wall_time_seconds = $null
            exit_code = $null
            stdout_file = $null
            stderr_file = $null
            output_file = [System.IO.Path]::GetFileName($failurePath)
            peak_working_set_bytes = $null
            peak_working_set_mib = $null
            sampled_peak_private_memory_bytes = $null
            sampled_peak_private_memory_mib = $null
            gpu_process_memory_counter_available = $null
            gpu_process_sample_found = $false
            sampled_peak_gpu_dedicated_bytes = $null
            sampled_peak_gpu_dedicated_mib = $null
            sampled_peak_gpu_shared_bytes = $null
            sampled_peak_gpu_shared_mib = $null
            gpu_counter_error = $null
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
    material_eval = [pscustomobject]@{
        requested = $materialEvalRequested
        manifest = $resolvedMaterialEvalManifest
        status = if ($materialEvalRequested) { "included" } else { "not_run_no_manifest" }
    }
    memory_sampling = [pscustomobject]@{
        process_poll_interval_ms = 200
        gpu_poll_interval_seconds = 1
        working_set = "Process.WorkingSet64/PeakWorkingSet64"
        private_memory = "sampled Process.PrivateMemorySize64"
        gpu_memory = "Windows GPU Process Memory performance counters when available"
    }
    sample_counts = [pscustomobject]@{
        ann_queries = $AnnQueries
        preview_samples = $PreviewSamples
        runtime_samples = $RuntimeSamples
        image_model_queries = $ImageModelQueries
        texture_samples = $TextureSamples
    }
    benchmark_results = $results
}
$manifest | ConvertTo-Json -Depth 10 | Set-Content -Path (Join-Path $resultDirectory "manifest.json") -Encoding utf8

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
    "  Labeled material eval: $(if ($materialEvalRequested) { $resolvedMaterialEvalManifest } else { 'not run (no -MaterialEvalManifest)' })",
    "",
    "Benchmark results:"
)
foreach ($result in $results) {
    $summaryLines += "  $($result.name): success=$($result.succeeded) exit=$($result.exit_code) wall_s=$($result.wall_time_seconds) peak_ram_mib=$($result.peak_working_set_mib) peak_private_mib=$($result.sampled_peak_private_memory_mib) peak_gpu_dedicated_mib=$($result.sampled_peak_gpu_dedicated_mib) output=$($result.output_file)"
}
$summaryLines += @(
    "",
    "Hardware details are stored in system-info.json; library composition is stored in library-profile.txt.",
    "Peak working set is process-level RAM. Private memory and GPU dedicated/shared memory are sampled while each benchmark child process runs.",
    "GPU memory uses Windows GPU Process Memory performance counters and is null when the counter set or process sample is unavailable.",
    "Win32_VideoController.AdapterRAM in system-info.json is Windows/WMI-reported adapter capacity and may not equal exact dedicated VRAM on every driver/GPU.",
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
