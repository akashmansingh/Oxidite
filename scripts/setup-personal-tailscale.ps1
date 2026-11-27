<#
.SYNOPSIS
    Idempotent setup for personal always-on Oxidite over Tailscale HTTPS.
    - Installs Tailscale if missing (via winget)
    - Detects exact tailnet DNS name from `tailscale status --json`
    - Configures `tailscale serve --bg 3030` (never Funnel)
    - Secures `%USERPROFILE%\.oxidite` ACLs to current user only
    - Registers a hardened Windows Scheduled Task (`PT0S` unlimited runtime, battery-safe, auto-restart)
#>
[CmdletBinding()]
param(
    [int]$Port = 3030,
    [int]$IdleSuspendSecs = 21600,
    [string]$TaskName = "OxiditeGateway",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$OxiditeDir = Join-Path $env:USERPROFILE ".oxidite"
$LogDir = Join-Path $OxiditeDir "logs"
$DbPath = Join-Path $OxiditeDir "oxidite.db"
$LauncherPath = Join-Path $OxiditeDir "start-oxidited.ps1"

# 1. Ensure Tailscale is installed and on PATH
$tsDefaultDir = Join-Path $env:ProgramFiles "Tailscale"
if (-not (Get-Command tailscale -ErrorAction SilentlyContinue)) {
    $env:Path = [System.Environment]::GetEnvironmentVariable("Path", "Machine") + ";" + [System.Environment]::GetEnvironmentVariable("Path", "User")
    if ((Test-Path (Join-Path $tsDefaultDir "tailscale.exe")) -and ($env:Path -notlike "*$tsDefaultDir*")) {
        $env:Path = "$tsDefaultDir;$env:Path"
    }
}
if (-not (Get-Command tailscale -ErrorAction SilentlyContinue)) {
    Write-Host "[1/6] Installing Tailscale via winget..." -ForegroundColor Cyan
    & winget install --id Tailscale.Tailscale -e --accept-source-agreements --accept-package-agreements
    # -1978335189 (0x8A15001B) = already installed with no newer version available
    if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1978335189) {
        throw "winget failed to install Tailscale (exit $LASTEXITCODE)"
    }
    $env:Path = [System.Environment]::GetEnvironmentVariable("Path", "Machine") + ";" + [System.Environment]::GetEnvironmentVariable("Path", "User")
    if ((Test-Path (Join-Path $tsDefaultDir "tailscale.exe")) -and ($env:Path -notlike "*$tsDefaultDir*")) {
        $env:Path = "$tsDefaultDir;$env:Path"
    }
}

# 2. Verify Tailscale login and auto-detect exact .ts.net DNS name
Write-Host "[2/6] Reading Tailscale status..." -ForegroundColor Cyan
$statusJson = & tailscale status --json 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Host "Tailscale is not logged in yet. Run 'tailscale up' first and re-run this script." -ForegroundColor Yellow
    exit 1
}
$status = $statusJson | ConvertFrom-Json
$dnsName = ($status.Self.DNSName).TrimEnd('.')
if ([string]::IsNullOrWhiteSpace($dnsName)) {
    throw "Could not determine Tailscale MagicDNS name. Ensure MagicDNS & HTTPS are enabled in Tailscale Admin Console."
}
$allowedOrigin = "https://$dnsName"
Write-Host "    Detected Tailscale origin: $allowedOrigin" -ForegroundColor Green

# 3. Build release oxidited + web bundle unless skipped
$ReleaseBin = Join-Path $RepoRoot "target\release\oxidited.exe"
if (-not $SkipBuild) {
    Write-Host "[3/6] Building web bundle and release oxidited..." -ForegroundColor Cyan
    Push-Location (Join-Path $RepoRoot "web")
    try {
        if (Get-Command bun -ErrorAction SilentlyContinue) { & bun run build } else { & npm run build }
        if ($LASTEXITCODE -ne 0) { throw "Web build failed" }
    } finally { Pop-Location }

    Push-Location $RepoRoot
    try {
        & cargo build --release -p oxidite-server --bin oxidited
        if ($LASTEXITCODE -ne 0) { throw "Cargo release build failed" }
    } finally { Pop-Location }
}
if (-not (Test-Path $ReleaseBin)) {
    throw "Release binary not found at $ReleaseBin"
}

# 4. Create data/log directories and restrict NTFS ACLs to current user
Write-Host "[4/6] Securing $OxiditeDir permissions..." -ForegroundColor Cyan
New-Item -ItemType Directory -Path $OxiditeDir, $LogDir -Force | Out-Null
$currentUser = "$env:USERDOMAIN\$env:USERNAME"
& icacls $OxiditeDir /inheritance:r /grant:r "${currentUser}:(OI)(CI)F" "SYSTEM:(OI)(CI)F" | Out-Null

# 5. Configure Tailscale Serve (tailnet-only HTTPS, NEVER Funnel)
Write-Host "[5/6] Configuring tailscale serve --bg $Port..." -ForegroundColor Cyan
& tailscale serve --bg $Port
if ($LASTEXITCODE -ne 0) {
    Write-Warning "tailscale serve returned $LASTEXITCODE. Ensure HTTPS certificates are enabled in Tailscale Admin Console -> DNS."
}

# 6. Write launcher script and register hardened Windows Scheduled Task
Write-Host "[6/6] Registering Windows Scheduled Task '$TaskName'..." -ForegroundColor Cyan
$launcherContent = @"
`$env:OXIDITE_HOST = "127.0.0.1"
`$env:OXIDITE_PORT = "$Port"
`$env:OXIDITE_DB_PATH = "$DbPath"
`$env:OXIDITE_LOG_DIR = "$LogDir"
`$env:OXIDITE_IDLE_SUSPEND_SECS = "$IdleSuspendSecs"
`$env:OXIDITE_ALLOWED_ORIGINS = "$allowedOrigin"
Set-Location "$RepoRoot"
& "$ReleaseBin"
"@
Set-Content -Path $LauncherPath -Value $launcherContent -Encoding UTF8

$action = New-ScheduledTaskAction `
    -Execute "powershell.exe" `
    -Argument "-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$LauncherPath`"" `
    -WorkingDirectory $RepoRoot

$trigger = New-ScheduledTaskTrigger -AtLogOn -User $currentUser

$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero) `
    -RestartCount 999 `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -MultipleInstances IgnoreNew `
    -StartWhenAvailable

$principal = New-ScheduledTaskPrincipal -UserId $currentUser -LogonType Interactive -RunLevel Limited

Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $action `
    -Trigger $trigger `
    -Settings $settings `
    -Principal $principal `
    -Description "Oxidite local gateway behind Tailscale HTTPS ($allowedOrigin)" `
    -Force | Out-Null

# Start (or restart) immediately
try { Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue } catch {}
Start-ScheduledTask -TaskName $TaskName

Start-Sleep -Seconds 2
try {
    $tcp = New-Object System.Net.Sockets.TcpClient
    $tcp.Connect("127.0.0.1", $Port)
    $tcp.Close()
    Write-Host "`n[OK] Oxidite is running on 127.0.0.1:$Port and served at $allowedOrigin" -ForegroundColor Green
} catch {
    Write-Warning "Oxidite task started, but port $Port did not respond yet. Check logs in $LogDir."
}

Write-Host @"

=== Windows Always-On Power Reminder ===
For uninterrupted phone access while away from your laptop:
  1. Control Panel -> Power Options -> Choose what closing the lid does -> 'Do nothing' (Plugged in).
  2. Settings -> System -> Power & battery -> Screen and sleep -> Plugged in sleep: 'Never'.
  3. Keep the laptop plugged in if relying on Modern Standby (S0ix suspends Wi-Fi on battery).
"@ -ForegroundColor Yellow
