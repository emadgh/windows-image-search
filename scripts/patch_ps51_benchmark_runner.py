from pathlib import Path

script = Path('scripts/run-v0.2-benchmark-gate.ps1')
text = script.read_text(encoding='utf-8')

marker = "function Invoke-DiagnosticBenchmark {\n"
helper = r'''function ConvertTo-ProcessArgument {
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

    # ProcessStartInfo.ArgumentList is unavailable on Windows PowerShell 5.1 /
    # .NET Framework. Build a correctly quoted command-line token instead.
    $escaped = $Value -replace '(\\*)"', '$1$1\\"'
    $escaped = $escaped -replace '(\\+)$', '$1$1'
    return '"' + $escaped + '"'
}

'''
if helper not in text:
    if marker not in text:
        raise RuntimeError('Invoke-DiagnosticBenchmark marker not found')
    text = text.replace(marker, helper + marker, 1)

old = r'''    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $ExecutablePath
    foreach ($argument in $Arguments) {
        [void]$startInfo.ArgumentList.Add($argument)
    }
'''
new = r'''    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $ExecutablePath
    $startInfo.Arguments = (($Arguments | ForEach-Object {
        ConvertTo-ProcessArgument -Value ([string]$_)
    }) -join ' ')
'''
if old not in text:
    raise RuntimeError('ProcessStartInfo.ArgumentList block not found')
text = text.replace(old, new, 1)
script.write_text(text, encoding='utf-8', newline='\n')

workflow = Path('.github/workflows/windows-build.yml')
wf = workflow.read_text(encoding='utf-8')
needle = "      - name: Format check\n        run: cargo fmt --all -- --check\n"
compat = r'''      - name: Windows PowerShell 5.1 benchmark compatibility
        shell: powershell
        run: |
          $legacyApi = Select-String -Path 'scripts/run-v0.2-benchmark-gate.ps1' -Pattern '\.ArgumentList' -Quiet
          if ($legacyApi) {
            throw 'Benchmark runner still uses ProcessStartInfo.ArgumentList, which is unavailable on Windows PowerShell 5.1.'
          }
          $tokens = $null
          $errors = $null
          [System.Management.Automation.Language.Parser]::ParseFile(
            (Resolve-Path -LiteralPath 'scripts/run-v0.2-benchmark-gate.ps1').Path,
            [ref]$tokens,
            [ref]$errors
          ) | Out-Null
          if ($errors.Count -gt 0) {
            $errors | ForEach-Object { Write-Error $_.Message }
            exit 1
          }
      - name: Format check
        run: cargo fmt --all -- --check
'''
if 'Windows PowerShell 5.1 benchmark compatibility' not in wf:
    if needle not in wf:
        raise RuntimeError('windows-build format step marker not found')
    wf = wf.replace(needle, compat, 1)
workflow.write_text(wf, encoding='utf-8', newline='\n')
