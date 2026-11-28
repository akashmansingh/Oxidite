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
$prevEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
$statusJson = & tailscale status --json 2>$null
$statusExit = $LASTEXITCODE
$ErrorActionPreference = $prevEap
if ($statusExit -ne 0 -or [string]::IsNullOrWhiteSpace(($statusJson | Out-String))) {
    Write-Host "Tailscale is not logged in yet. Run 'tailscale up' first and re-run this script." -ForegroundColor Yellow
    exit 1
}
$status = ($statusJson | Out-String) | ConvertFrom-Json
$rawDnsName = if ($status -and $status.Self -and $status.Self.DNSName) { [string]$status.Self.DNSName } else { "" }
$dnsName = $rawDnsName.TrimEnd('.')
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
if ($LASTEXITCODE -ne 0) {
    throw "Failed to apply restrictive NTFS ACLs to $OxiditeDir (icacls exit $LASTEXITCODE)"
}

# 5. Configure Tailscale Serve (tailnet-only HTTPS, NEVER Funnel)
Write-Host "[5/6] Configuring tailscale serve --bg $Port..." -ForegroundColor Cyan
$serveOk = $false
$prevEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
$existingServeJson = & tailscale serve status --json 2>$null
if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace(($existingServeJson | Out-String))) {
    try {
        $existingServe = ($existingServeJson | Out-String) | ConvertFrom-Json
        if ($existingServe.AllowFunnel) {
            $funnelEnabled = $existingServe.AllowFunnel.PSObject.Properties | Where-Object { $_.Value -eq $true }
            if ($funnelEnabled) {
                Write-Warning "Existing Tailscale Funnel configuration detected; resetting to enforce tailnet-only Serve."
                & tailscale funnel reset 2>$null | Out-Null
                & tailscale serve reset 2>$null | Out-Null
            }
        }
    } catch {}
}
& tailscale serve --bg $Port
$serveExit = $LASTEXITCODE
if ($serveExit -eq 0) {
    $verifyServeJson = & tailscale serve status --json 2>$null
    $funnelStillOn = $false
    if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace(($verifyServeJson | Out-String))) {
        try {
            $verifyServe = ($verifyServeJson | Out-String) | ConvertFrom-Json
            if ($verifyServe.AllowFunnel) {
                $funnelStillOn = [bool]($verifyServe.AllowFunnel.PSObject.Properties | Where-Object { $_.Value -eq $true })
            }
        } catch {}
    }
    if ($funnelStillOn) {
        & tailscale funnel reset 2>$null | Out-Null
        Write-Warning "Tailscale Funnel was unexpectedly active after serve; Funnel has been reset. Re-run to verify tailnet-only Serve."
    } else {
        $serveOk = $true
    }
} else {
    Write-Warning "tailscale serve returned $serveExit. Ensure HTTPS certificates are enabled in Tailscale Admin Console -> DNS."
}
$ErrorActionPreference = $prevEap

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
    if ($serveOk) {
        Write-Host "`n[OK] Oxidite is running on 127.0.0.1:$Port and served at $allowedOrigin" -ForegroundColor Green
    } else {
        Write-Warning "`nOxidite is running locally on 127.0.0.1:$Port, but tailscale serve FAILED (not reachable at $allowedOrigin)."
    }
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
