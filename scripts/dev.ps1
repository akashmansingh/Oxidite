<#
.SYNOPSIS
    Starts oxidited and the Vite web dev server together, and cleanly stops oxidited on exit.
#>
[CmdletBinding()]
param(
    [string]$BindHost = "127.0.0.1",
    [int]$Port = 3030,
    [int]$IdleSuspendSecs = 21600
)

$ErrorActionPreference = "Stop"
$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$WebDir = Join-Path $RepoRoot "web"
$OxiditeDir = Join-Path $env:USERPROFILE ".oxidite"
$DbPath = Join-Path $OxiditeDir "oxidite-dev.db"

New-Item -ItemType Directory -Path $OxiditeDir -Force | Out-Null

$env:OXIDITE_HOST = $BindHost
$env:OXIDITE_PORT = "$Port"
$env:OXIDITE_DB_PATH = $DbPath
$env:OXIDITE_IDLE_SUSPEND_SECS = "$IdleSuspendSecs"

Write-Host "[dev] Building oxidited..." -ForegroundColor Cyan
Push-Location $RepoRoot
try {
    & cargo build -p oxidite-server --bin oxidited
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}

$Binary = Join-Path $RepoRoot "target\debug\oxidited.exe"
if (-not (Test-Path $Binary)) {
    throw "Expected binary not found at $Binary"
}

Write-Host "[dev] Starting oxidited on http://${BindHost}:${Port} (DB: $DbPath)" -ForegroundColor Green
$ServerProc = Start-Process -FilePath $Binary -WorkingDirectory $RepoRoot -NoNewWindow -PassThru

try {
    # Wait briefly for loopback listener
    $ready = $false
    for ($i = 0; $i -lt 30; $i++) {
        if ($ServerProc.HasExited) {
            throw "oxidited exited unexpectedly with code $($ServerProc.ExitCode)"
        }
        try {
            $client = New-Object System.Net.Sockets.TcpClient
            $client.Connect($BindHost, $Port)
            $client.Close()
            $ready = $true
            break
        } catch {
            Start-Sleep -Milliseconds 200
        }
    }
    if (-not $ready) {
        Write-Warning "[dev] oxidited did not open port $Port within 6s; continuing to launch Vite."
    }

    Push-Location $WebDir
    try {
        if (Get-Command bun -ErrorAction SilentlyContinue) {
            Write-Host "[dev] Launching Vite via bun..." -ForegroundColor Cyan
            & bun run dev
        } else {
            Write-Host "[dev] Launching Vite via npm..." -ForegroundColor Cyan
            & npm run dev
        }
    } finally {
        Pop-Location
    }
} finally {
    if ($ServerProc -and -not $ServerProc.HasExited) {
        Write-Host "`n[dev] Stopping oxidited (PID $($ServerProc.Id))..." -ForegroundColor Yellow
        try {
            Stop-Process -Id $ServerProc.Id -Force -ErrorAction SilentlyContinue
            $ServerProc.WaitForExit(3000) | Out-Null
        } catch {
            # Ignore already exited
        }
    }
}
