[CmdletBinding()]
param(
    [Parameter()]
    [string]$Executable = ".\windows-image-search.exe",

    [Parameter(Mandatory = $true)]
    [string]$YuNetManifest,

    [Parameter(Mandatory = $true)]
    [string]$SFaceManifest,

    [Parameter()]
    [string]$OutputDirectory = ".\benchmark-results",

    [Parameter()]
    [switch]$RequireDirectML
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$OutputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$script:GpuCounterUnavailableReason = $null

function Convert-BytesToGiB {
    param([AllowNull()][object]$Bytes)
    if ($null -eq $Bytes) { return $null }
    try { return [math]::Round(([double]$Bytes / 1GB), 2) }
    catch { return $null }
}

function Convert-BytesToMiB {
    param([AllowNull()][object]$Bytes)
    if ($null -eq $Bytes) { return $null }
    try { return [math]::Round(([double]$Bytes / 1MB), 2) }
    catch { return $null }
}

function Get-SystemSnapshot {
    $os = Get-CimInstance Win32_OperatingSystem | Select-Object -First 1
    $computer = Get-CimInstance Win32_ComputerSystem | Select-Object -First 1
    $cpus = @(Get-CimInstance Win32_Processor)
    $gpus = @(Get-CimInstance Win32_VideoController)

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
    }
}

function Get-GpuProcessMemorySample {
    param([Parameter(Mandatory = $true)][int]$TargetProcessId)

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
            if ($instanceName -notmatch "(^|_)pid_$TargetProcessId(_|$)") { continue }
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

function ConvertTo-ProcessArgument {
    param([AllowEmptyString()][string]$Value)
    if ([string]::IsNullOrEmpty($Value)) { return '""' }
    if ($Value -notmatch '[\s"]') { return $Value }
    $escaped = $Value -replace '(\\*)"', '$1$1\"'
    $escaped = $escaped -replace '(\\+)$', '$1$1'
    return '"' + $escaped + '"'
}

function New-ProviderManifest {
    param(
        [Parameter(Mandatory = $true)][string]$SourcePath,
        [Parameter(Mandatory = $true)][ValidateSet('cpu', 'directml')][string]$Provider,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $resolved = (Resolve-Path -LiteralPath $SourcePath -ErrorAction Stop).Path
    $directory = Split-Path -Parent $resolved
    $temporaryPath = Join-Path $directory (".wis-$Label-$Provider-$([Guid]::NewGuid().ToString('N')).tsv")
    $lines = [System.IO.File]::ReadAllLines($resolved)
    $output = New-Object System.Collections.Generic.List[string]
    $replaced = $false

    foreach ($line in $lines) {
        $current = $line
        if (-not $replaced -and $line -match '^\s*model\t') {
            $columns = @($line -split "`t", -1)
            if ($columns.Count -lt 3) {
                throw "Model row in $resolved does not contain a provider column."
            }
            $columns[2] = $Provider
            $current = $columns -join "`t"
            $replaced = $true
        }
        $output.Add($current)
    }

    if (-not $replaced) {
        throw "No model row found in face benchmark manifest: $resolved"
    }

    [System.IO.File]::WriteAllLines(
        $temporaryPath,
        $output,
        [System.Text.UTF8Encoding]::new($false)
    )
    return $temporaryPath
}

function Invoke-FaceBenchmark {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$ExecutablePath,
        [Parameter(Mandatory = $true)][string]$ResultDirectory,
        [Parameter(Mandatory = $true)][string]$Provider
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
    $startInfo.Arguments = (($Arguments | ForEach-Object {
        ConvertTo-ProcessArgument -Value ([string]$_)
    }) -join ' ')
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.CreateNoWindow = $true

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) { throw "Cannot start benchmark process: $ExecutablePath" }

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
            # Process can exit between polling and Refresh().
        }

        if ([DateTime]::UtcNow -ge $nextGpuSample) {
            $gpuSample = Get-GpuProcessMemorySample -TargetProcessId $process.Id
            $gpuCounterAvailable = $gpuSample.counter_available
            if ($null -ne $gpuSample.error) { $gpuCounterError = $gpuSample.error }
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

    [pscustomobject]@{
        name = $Name
        provider = $Provider
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

$executablePath = (Resolve-Path -LiteralPath $Executable -ErrorAction Stop).Path
$resolvedYuNetManifest = (Resolve-Path -LiteralPath $YuNetManifest -ErrorAction Stop).Path
$resolvedSFaceManifest = (Resolve-Path -LiteralPath $SFaceManifest -ErrorAction Stop).Path
$rootOutput = [System.IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $rootOutput | Out-Null

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$resultDirectory = Join-Path $rootOutput "v0.3-face-benchmark-gate-$timestamp"
New-Item -ItemType Directory -Force -Path $resultDirectory | Out-Null

$gateStartedAt = Get-Date
$systemInfo = Get-SystemSnapshot
$systemInfo | ConvertTo-Json -Depth 8 | Set-Content -Path (Join-Path $resultDirectory "system-info.json") -Encoding utf8

$versionLines = @(& $executablePath --version 2>&1)
$versionExitCode = $LASTEXITCODE
$versionLines | Set-Content -Path (Join-Path $resultDirectory "version.txt") -Encoding utf8
$appVersion = if ($versionLines.Count -gt 0) { [string]$versionLines[0] } else { "unknown" }

$tempManifests = @()
$results = @()
try {
    $yunetCpu = New-ProviderManifest -SourcePath $resolvedYuNetManifest -Provider cpu -Label yunet
    $yunetDirectMl = New-ProviderManifest -SourcePath $resolvedYuNetManifest -Provider directml -Label yunet
    $sfaceCpu = New-ProviderManifest -SourcePath $resolvedSFaceManifest -Provider cpu -Label sface
    $sfaceDirectMl = New-ProviderManifest -SourcePath $resolvedSFaceManifest -Provider directml -Label sface
    $tempManifests = @($yunetCpu, $yunetDirectMl, $sfaceCpu, $sfaceDirectMl)

    $benchmarks = @(
        [pscustomobject]@{ Name = 'yunet-cpu'; Provider = 'cpu'; Arguments = @('--benchmark-yunet', $yunetCpu) },
        [pscustomobject]@{ Name = 'yunet-directml'; Provider = 'directml'; Arguments = @('--benchmark-yunet', $yunetDirectMl) },
        [pscustomobject]@{ Name = 'sface-cpu'; Provider = 'cpu'; Arguments = @('--benchmark-sface', $sfaceCpu) },
        [pscustomobject]@{ Name = 'sface-directml'; Provider = 'directml'; Arguments = @('--benchmark-sface', $sfaceDirectMl) },
        [pscustomobject]@{ Name = 'face-ann'; Provider = 'index'; Arguments = @('--benchmark-face-ann', '32') }
    )

    foreach ($benchmark in $benchmarks) {
        try {
            $results += Invoke-FaceBenchmark `
                -Name $benchmark.Name `
                -Provider $benchmark.Provider `
                -Arguments $benchmark.Arguments `
                -ExecutablePath $executablePath `
                -ResultDirectory $resultDirectory
        }
        catch {
            $failurePath = Join-Path $resultDirectory "$($benchmark.Name).runner-error.txt"
            $_ | Out-String | Set-Content -Path $failurePath -Encoding utf8
            $results += [pscustomobject]@{
                name = $benchmark.Name
                provider = $benchmark.Provider
                process_id = $null
                command = "$executablePath $($benchmark.Arguments -join ' ')"
                arguments = $benchmark.Arguments
                started_at = $null
                finished_at = (Get-Date).ToString("o")
                wall_time_seconds = $null
                exit_code = $null
                output_file = [System.IO.Path]::GetFileName($failurePath)
                peak_working_set_mib = $null
                sampled_peak_private_memory_mib = $null
                sampled_peak_gpu_dedicated_mib = $null
                sampled_peak_gpu_shared_mib = $null
                succeeded = $false
                runner_error = $_.Exception.Message
            }
        }
    }
}
finally {
    foreach ($temporary in $tempManifests) {
        if (Test-Path -LiteralPath $temporary) {
            Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
        }
    }
}

$gateFinishedAt = Get-Date
$manifest = [pscustomobject]@{
    gate = "v0.3-face-model-runtime"
    generated_at = $gateFinishedAt.ToString("o")
    gate_started_at = $gateStartedAt.ToString("o")
    gate_wall_time_seconds = [math]::Round(($gateFinishedAt - $gateStartedAt).TotalSeconds, 3)
    executable = $executablePath
    application_version = $appVersion
    version_exit_code = $versionExitCode
    source_manifests = [pscustomobject]@{
        yunet = $resolvedYuNetManifest
        sface = $resolvedSFaceManifest
    }
    directml_required = [bool]$RequireDirectML
    memory_sampling = [pscustomobject]@{
        process_poll_interval_ms = 200
        gpu_poll_interval_seconds = 1
        working_set = "Process.WorkingSet64/PeakWorkingSet64"
        private_memory = "sampled Process.PrivateMemorySize64"
        gpu_memory = "Windows GPU Process Memory performance counters when available"
    }
    benchmark_results = $results
}
$manifest | ConvertTo-Json -Depth 10 | Set-Content -Path (Join-Path $resultDirectory "manifest.json") -Encoding utf8

$summaryLines = @(
    "Windows Image Search v0.3 face benchmark gate",
    "Generated: $($gateFinishedAt.ToString('o'))",
    "Application: $appVersion",
    "Executable: $executablePath",
    "YuNet manifest: $resolvedYuNetManifest",
    "SFace manifest: $resolvedSFaceManifest",
    "DirectML required: $([bool]$RequireDirectML)",
    "",
    "Benchmark results:"
)
foreach ($result in $results) {
    $summaryLines += "  $($result.name): success=$($result.succeeded) exit=$($result.exit_code) wall_s=$($result.wall_time_seconds) peak_ram_mib=$($result.peak_working_set_mib) peak_private_mib=$($result.sampled_peak_private_memory_mib) peak_gpu_dedicated_mib=$($result.sampled_peak_gpu_dedicated_mib) output=$($result.output_file)"
}
$summaryLines += @(
    "",
    "Each model is evaluated through the same labeled manifest twice: CPU and DirectML.",
    "The model-adapter reports contain detector/identity quality metrics, init latency and persistent-session inference throughput.",
    "Peak working set is process-level RAM; private and GPU memory are sampled while each child benchmark runs.",
    "A DirectML failure is recorded as unavailable unless -RequireDirectML is supplied."
)
$summaryLines | Set-Content -Path (Join-Path $resultDirectory "summary.txt") -Encoding utf8

$zipPath = "$resultDirectory.zip"
if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
Compress-Archive -Path (Join-Path $resultDirectory "*") -DestinationPath $zipPath -Force

$cpuFailures = @($results | Where-Object { $_.provider -eq 'cpu' -and -not $_.succeeded })
$directMlFailures = @($results | Where-Object { $_.provider -eq 'directml' -and -not $_.succeeded })
$indexFailures = @($results | Where-Object { $_.provider -eq 'index' -and -not $_.succeeded })

Write-Host ""
Write-Host "Face benchmark gate complete."
Write-Host "Result directory: $resultDirectory"
Write-Host "ZIP bundle: $zipPath"
Write-Host "CPU succeeded: $(2 - $cpuFailures.Count)/2"
Write-Host "DirectML succeeded: $(2 - $directMlFailures.Count)/2"
Write-Host "Face ANN index benchmark succeeded: $($indexFailures.Count -eq 0)"

if ($versionExitCode -ne 0 -or $cpuFailures.Count -gt 0 -or $indexFailures.Count -gt 0 -or ($RequireDirectML -and $directMlFailures.Count -gt 0)) {
    exit 1
}
