$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Load only the provider-manifest helper, without launching any diagnostic.
$tokens = $null
$errors = $null
$runner = Join-Path $PSScriptRoot 'run-v0.3-face-benchmark-gate.ps1'
$ast = [System.Management.Automation.Language.Parser]::ParseFile($runner, [ref]$tokens, [ref]$errors)
if ($errors.Count -gt 0) { throw 'Face runner syntax errors' }
$helper = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'New-ProviderManifest' }, $true)
if ($null -eq $helper) { throw 'Provider helper missing' }
. ([scriptblock]::Create($helper.Extent.Text))

$directory = Join-Path $env:TEMP ('wis-runner-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $directory | Out-Null
$source = Join-Path $directory 'source.tsv'
$converted = $null
try {
    $rows = @("evaluation`truntime-only", "model`tC:\test model.onnx`tcpu`texternal-review`tfalse`tfalse`tuser-supplied")
    [System.IO.File]::WriteAllLines($source, $rows)
    $converted = New-ProviderManifest -SourcePath $source -Provider directml -Label test
    $output = [System.IO.File]::ReadAllLines($converted)
    if ($output[0] -ne $rows[0]) { throw 'Evaluation mode changed' }
    $columns = @($output[1] -split "`t")
    if ($columns.Count -ne 7 -or $columns[2] -ne 'directml' -or $columns[1] -ne 'C:\test model.onnx') { throw 'Model provider rewrite failed' }
    if ([System.IO.File]::ReadAllLines($source)[1] -ne $rows[1]) { throw 'Source manifest was modified' }
    Write-Output 'Provider manifest rewrite: passed'
}
finally {
    if ($converted -and (Test-Path -LiteralPath $converted)) { Remove-Item -LiteralPath $converted }
    if (Test-Path -LiteralPath $source) { Remove-Item -LiteralPath $source }
    Remove-Item -LiteralPath $directory
}
