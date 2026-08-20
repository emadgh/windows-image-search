[CmdletBinding()]
param(
    [Parameter(Mandatory, Position = 0)]
    [ValidateNotNullOrEmpty()]
    [string[]]$Root,

    [Parameter()]
    [string]$OutputPath = ".\material-eval.generated.tsv",

    [Parameter()]
    [ValidateRange(2, 1000000)]
    [int]$MinimumImagesPerGroup = 2,

    [Parameter()]
    [switch]$Preview
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$OutputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$extensions = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
@(".jpg", ".jpeg", ".png", ".tif", ".tiff") | ForEach-Object { [void]$extensions.Add($_) }

function Get-SafeGroupText {
    param([Parameter(Mandatory)][string]$Value)
    return ($Value -replace "[`t`r`n]", " ").Trim()
}

function Get-RootDescriptors {
    param([Parameter(Mandatory)][string[]]$Paths)

    $resolved = @(
        $Paths |
            ForEach-Object {
                $item = Get-Item -LiteralPath $_ -ErrorAction Stop
                if (-not $item.PSIsContainer) {
                    throw "Folder root is not a directory: $($_)"
                }
                [pscustomobject]@{
                    FullName = [System.IO.Path]::GetFullPath($item.FullName)
                    Leaf = $item.Name
                }
            } |
            Sort-Object FullName -Unique
    )

    $leafCounts = @{}
    foreach ($item in $resolved) {
        $key = $item.Leaf.ToLowerInvariant()
        if (-not $leafCounts.ContainsKey($key)) {
            $leafCounts[$key] = 0
        }
        $leafCounts[$key]++
    }

    $leafOrdinals = @{}
    $output = @()
    foreach ($item in $resolved) {
        $key = $item.Leaf.ToLowerInvariant()
        if (-not $leafOrdinals.ContainsKey($key)) {
            $leafOrdinals[$key] = 0
        }
        $leafOrdinals[$key]++
        $rootKey = if ($leafCounts[$key] -gt 1) {
            "{0}-{1}" -f $item.Leaf, $leafOrdinals[$key]
        }
        else {
            $item.Leaf
        }
        $output += [pscustomobject]@{
            FullName = $item.FullName
            RootKey = Get-SafeGroupText $rootKey
        }
    }
    return @($output)
}

function Get-SupportedFiles {
    param([Parameter(Mandatory)][string]$Directory)

    return @(
        Get-ChildItem -LiteralPath $Directory -File -Recurse -ErrorAction Stop |
            Where-Object { $extensions.Contains($_.Extension) } |
            Sort-Object FullName |
            ForEach-Object { [System.IO.Path]::GetFullPath($_.FullName) }
    )
}

$roots = @(Get-RootDescriptors -Paths $Root)
$candidates = @()
foreach ($rootInfo in $roots) {
    $children = @(
        Get-ChildItem -LiteralPath $rootInfo.FullName -Directory -ErrorAction Stop |
            Sort-Object FullName
    )
    foreach ($child in $children) {
        $files = @(Get-SupportedFiles -Directory $child.FullName)
        $candidates += [pscustomobject]@{
            RootKey = $rootInfo.RootKey
            BaseGroup = Get-SafeGroupText $child.Name
            Directory = $child.FullName
            Files = $files
        }
    }
}

$groupCounts = @{}
foreach ($candidate in $candidates) {
    $key = $candidate.BaseGroup.ToLowerInvariant()
    if (-not $groupCounts.ContainsKey($key)) {
        $groupCounts[$key] = 0
    }
    $groupCounts[$key]++
}

$accepted = @()
$skipped = @()
foreach ($candidate in $candidates) {
    $baseKey = $candidate.BaseGroup.ToLowerInvariant()
    $group = if ($groupCounts[$baseKey] -gt 1) {
        "{0}/{1}" -f $candidate.RootKey, $candidate.BaseGroup
    }
    else {
        $candidate.BaseGroup
    }
    $group = Get-SafeGroupText $group

    if ($candidate.Files.Count -lt $MinimumImagesPerGroup) {
        $skipped += [pscustomobject]@{
            Group = $group
            Directory = $candidate.Directory
            Images = $candidate.Files.Count
            Reason = "fewer-than-minimum"
        }
        continue
    }

    foreach ($file in $candidate.Files) {
        if ($file -match "[`t`r`n]") {
            throw "Image path contains a tab or newline and cannot be represented safely in TSV: $file"
        }
    }
    $accepted += [pscustomobject]@{
        Group = $group
        Directory = $candidate.Directory
        Files = @($candidate.Files)
    }
}

$accepted = @($accepted | Sort-Object Group, Directory)
$sampleCount = 0
foreach ($group in $accepted) {
    $sampleCount += $group.Files.Count
}

Write-Host "Windows Image Search material-eval folder manifest"
Write-Host "roots=$($roots.Count)"
Write-Host "groups_discovered=$($candidates.Count)"
Write-Host "groups_accepted=$($accepted.Count)"
Write-Host "groups_skipped=$($skipped.Count)"
Write-Host "samples=$sampleCount"
foreach ($group in $accepted) {
    Write-Host ("group`t{0}`t{1}`t{2}" -f $group.Group, $group.Files.Count, $group.Directory)
}
foreach ($group in ($skipped | Sort-Object Group, Directory)) {
    Write-Host ("skipped`t{0}`t{1}`t{2}`t{3}" -f $group.Group, $group.Images, $group.Reason, $group.Directory)
}

if ($accepted.Count -eq 0) {
    throw "No material-evaluation groups contain at least $MinimumImagesPerGroup supported images."
}

if ($Preview) {
    Write-Host "preview=true"
    return
}

$destination = [System.IO.Path]::GetFullPath($OutputPath)
$parent = Split-Path -Parent $destination
if ($parent) {
    New-Item -ItemType Directory -Force -Path $parent | Out-Null
}
$lines = [System.Collections.Generic.List[string]]::new()
foreach ($group in $accepted) {
    foreach ($file in $group.Files) {
        $lines.Add("$($group.Group)`t$file")
    }
}
[System.IO.File]::WriteAllLines(
    $destination,
    $lines,
    [System.Text.UTF8Encoding]::new($false)
)
Write-Host "manifest=$destination"
