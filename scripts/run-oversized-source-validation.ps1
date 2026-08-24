[CmdletBinding()]
param(
    [Parameter()]
    [string]$Executable = ".\windows-image-search.exe",

    [Parameter(Mandatory = $true)]
    [string]$Source,

    [Parameter()]
    [string]$OutputDirectory = ".\benchmark-results",

    [Parameter()]
    [switch]$SkipIndexing
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$OutputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

function Convert-BytesToMiB {
    param([AllowNull()][object]$Bytes)
    if ($null -eq $Bytes) {
        return $null
    }
    return [math]::Round(([double]$Bytes / 1MB), 2)
}

function ConvertTo-ProcessArgument {
    param(
        [AllowEmptyString()]
        [string]$Value
    )

    if ([string]::IsNullOrEmpty($Value)) {
        return '""'
    }
    if ($Value -notmatch '[\s"]') {
        return $Value
    }

    # ProcessStartInfo.ArgumentList is unavailable on Windows PowerShell 5.1.
    $escaped = $Value -replace '(\\*)"', '$1$1\"'
    $escaped = $escaped -replace '(\\+)$', '$1$1'
    return '"' + $escaped + '"'
}

function Get-SystemSnapshot {
    $os = Get-CimInstance Win32_OperatingSystem | Select-Object -First 1
    $computer = Get-CimInstance Win32_ComputerSystem | Select-Object -First 1
    $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
    $gpu = @(Get-CimInstance Win32_VideoController | ForEach-Object {
        [pscustomobject]@{
            name = $_.Name
            driver_version = $_.DriverVersion
            adapter_ram_reported_bytes = $_.AdapterRAM
        }
    })

    return [pscustomobject]@{
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
            total_physical_memory_mib = Convert-BytesToMiB $computer.TotalPhysicalMemory
        }
        cpu = [pscustomobject]@{
            name = $cpu.Name
            cores = $cpu.NumberOfCores
            logical_processors = $cpu.NumberOfLogicalProcessors
        }
        gpu = $gpu
    }
}

function Invoke-ValidationProcess {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [string]$ExecutablePath,

        [Parameter(Mandatory = $true)]
        [string[]]$Arguments,

        [Parameter(Mandatory = $true)]
        [string]$ResultDirectory
    )

    $stdoutPath = Join-Path $ResultDirectory "$Name.stdout.txt"
    $stderrPath = Join-Path $ResultDirectory "$Name.stderr.txt"
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
    if (-not $process.Start()) {
        throw "Cannot start validation process: $ExecutablePath"
    }

    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    [int64]$peakWorkingSet = 0
    [int64]$peakPrivateMemory = 0
    $nextHeartbeat = [DateTime]::UtcNow.AddSeconds(15)

    do {
        try {
            $process.Refresh()
            $peakWorkingSet = [math]::Max($peakWorkingSet, [int64]$process.WorkingSet64)
            $peakWorkingSet = [math]::Max($peakWorkingSet, [int64]$process.PeakWorkingSet64)
            $peakPrivateMemory = [math]::Max($peakPrivateMemory, [int64]$process.PrivateMemorySize64)
        }
        catch {
            # The process can exit between polling and Refresh().
        }

        if ([DateTime]::UtcNow -ge $nextHeartbeat) {
            try {
                $cpuSeconds = [math]::Round($process.TotalProcessorTime.TotalSeconds, 1)
                $workingSetMiB = Convert-BytesToMiB $process.WorkingSet64
            }
            catch {
                $cpuSeconds = $null
                $workingSetMiB = $null
            }
            $elapsedSeconds = [math]::Round($stopwatch.Elapsed.TotalSeconds, 0)
            Write-Host "[$Name] running: elapsed=${elapsedSeconds}s cpu=${cpuSeconds}s working_set=${workingSetMiB}MiB"
            $nextHeartbeat = [DateTime]::UtcNow.AddSeconds(15)
        }

        $exited = $process.WaitForExit(100)
    } while (-not $exited)

    $process.WaitForExit()
    $stdout = $stdoutTask.Result
    $stderr = $stderrTask.Result
    $exitCode = $process.ExitCode
    $stopwatch.Stop()
    $finishedAt = Get-Date

    $stdout | Set-Content -Path $stdoutPath -Encoding utf8
    $stderr | Set-Content -Path $stderrPath -Encoding utf8
    if (-not [string]::IsNullOrWhiteSpace($stdout)) {
        Write-Host $stdout.TrimEnd()
    }
    if (-not [string]::IsNullOrWhiteSpace($stderr)) {
        Write-Host $stderr.TrimEnd()
    }

    return [pscustomobject]@{
        name = $Name
        process_id = $process.Id
        command = "$ExecutablePath $($Arguments -join ' ')"
        arguments = $Arguments
        started_at = $startedAt.ToString("o")
        finished_at = $finishedAt.ToString("o")
        wall_time_seconds = [math]::Round($stopwatch.Elapsed.TotalSeconds, 3)
        exit_code = $exitCode
        peak_working_set_bytes = $peakWorkingSet
        peak_working_set_mib = Convert-BytesToMiB $peakWorkingSet
        sampled_peak_private_memory_bytes = $peakPrivateMemory
        sampled_peak_private_memory_mib = Convert-BytesToMiB $peakPrivateMemory
        stdout_file = [System.IO.Path]::GetFileName($stdoutPath)
        stderr_file = [System.IO.Path]::GetFileName($stderrPath)
        succeeded = ($exitCode -eq 0)
    }
}

$executablePath = (Resolve-Path -LiteralPath $Executable -ErrorAction Stop).Path
$sourcePath = (Resolve-Path -LiteralPath $Source -ErrorAction Stop).Path
$sourceItem = Get-Item -LiteralPath $sourcePath -ErrorAction Stop
if ($sourceItem.PSIsContainer) {
    throw "Source must be a JPEG file, not a directory: $sourcePath"
}
$extension = [System.IO.Path]::GetExtension($sourcePath).ToLowerInvariant()
if ($extension -ne ".jpg" -and $extension -ne ".jpeg") {
    throw "Oversized validation currently requires a JPEG source: $sourcePath"
}
if ([int64]$sourceItem.Length -le 256MB) {
    throw "Source is $(Convert-BytesToMiB $sourceItem.Length) MiB; choose a JPEG larger than the 256 MiB direct-decode ceiling."
}

$rootOutput = [System.IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $rootOutput | Out-Null
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$resultDirectory = Join-Path $rootOutput "oversized-source-validation-$timestamp"
New-Item -ItemType Directory -Force -Path $resultDirectory | Out-Null

$systemInfo = Get-SystemSnapshot
$systemInfo | ConvertTo-Json -Depth 8 | Set-Content -Path (Join-Path $resultDirectory "system-info.json") -Encoding utf8

$versionLines = @(& $executablePath --version 2>&1)
$versionExitCode = $LASTEXITCODE
$versionLines | Set-Content -Path (Join-Path $resultDirectory "version.txt") -Encoding utf8
if ($versionExitCode -ne 0) {
    throw "Executable --version failed with exit code $versionExitCode"
}

$preview = Invoke-ValidationProcess `
    -Name "oversized-preview" `
    -ExecutablePath $executablePath `
    -Arguments @("--validate-oversized-preview", $sourcePath) `
    -ResultDirectory $resultDirectory

$indexing = $null
if (-not $SkipIndexing.IsPresent) {
    $indexing = Invoke-ValidationProcess `
        -Name "oversized-full-index" `
        -ExecutablePath $executablePath `
        -Arguments @("--validate-oversized-indexing", $sourcePath) `
        -ResultDirectory $resultDirectory
}

$summary = [pscustomobject]@{
    captured_at = (Get-Date).ToString("o")
    app_version = if ($versionLines.Count -gt 0) { [string]$versionLines[0] } else { "unknown" }
    source = [pscustomobject]@{
        path = $sourcePath
        size_bytes = [int64]$sourceItem.Length
        size_mib = Convert-BytesToMiB $sourceItem.Length
        last_write_time_utc = $sourceItem.LastWriteTimeUtc.ToString("o")
    }
    preview = $preview
    indexing = $indexing
    passed = ($preview.succeeded -and ($SkipIndexing.IsPresent -or ($null -ne $indexing -and $indexing.succeeded)))
}

$summaryPath = Join-Path $resultDirectory "summary.json"
$summary | ConvertTo-Json -Depth 10 | Set-Content -Path $summaryPath -Encoding utf8

Write-Host ""
Write-Host "Validation results: $resultDirectory"
Write-Host "Preview peak working set: $($preview.peak_working_set_mib) MiB"
if ($null -ne $indexing) {
    Write-Host "Full-index peak working set: $($indexing.peak_working_set_mib) MiB"
}
Write-Host "summary=$summaryPath"

if (-not $summary.passed) {
    exit 1
}
