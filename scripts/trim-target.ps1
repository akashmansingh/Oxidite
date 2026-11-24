<#
.SYNOPSIS
Trims stale test binaries and build artifacts in target/ to keep disk usage strictly under 1 GB
without needing a full `cargo clean` (which requires re-downloading and re-compiling all dependencies).
#>
[CmdletBinding()]
param(
    [switch]$Aggressive
)

$targetDir = Join-Path $PSScriptRoot "..\target"
if (-not (Test-Path $targetDir)) {
    Write-Host "Target directory does not exist."
    return
}

$beforeBytes = (Get-ChildItem $targetDir -Recurse -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum

# 1. Remove test executables in deps (test runs generate ~500MB of one-off executables)
$depsDir = Join-Path $targetDir "debug\deps"
if (Test-Path $depsDir) {
    Get-ChildItem -Path $depsDir -Filter "*.exe" -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -notlike "oxidited*.exe" } | Remove-Item -Force -ErrorAction SilentlyContinue

    if ($Aggressive) {
        # In aggressive mode, also remove duplicate older revisions of rlibs/rmeta
        Get-ChildItem -Path $depsDir -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -like "*-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]*" } |
            Group-Object { ($_.Name -split "-[0-9a-f]+")[0] + $_.Extension } |
            Where-Object { $_.Count -gt 1 } |
            ForEach-Object {
                $sorted = $_.Group | Sort-Object LastWriteTime -Descending
                $sorted | Select-Object -Skip 1 | Remove-Item -Force -ErrorAction SilentlyContinue
            }
    }
}

# 2. Clear incremental cache if any accumulated
$incrementalDir = Join-Path $targetDir "debug\incremental"
if (Test-Path $incrementalDir) {
    Remove-Item $incrementalDir -Recurse -Force -ErrorAction SilentlyContinue
}

$afterBytes = (Get-ChildItem $targetDir -Recurse -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
$freedMB = [math]::Round(($beforeBytes - $afterBytes) / 1MB, 2)
$currentGB = [math]::Round($afterBytes / 1GB, 2)

Write-Host "Target trimmed: Freed $freedMB MB. Current target size: $currentGB GB."
