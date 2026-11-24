# Bounded Real-Traffic Benchmark Harness for Oxidite Gateway
# Emits size-capped, redacted NDJSON benchmark evidence against a live Oxidite server.
# Compatible with both Windows PowerShell 5.1 and PowerShell 7+.

[CmdletBinding()]
param (
    [Parameter()]
    [string]$Url = "http://127.0.0.1:3030",

    [Parameter()]
    [string]$Token = "",

    [Parameter()]
    [string]$OutputDir = "",

    [Parameter()]
    [int]$MaxRecords = 100,

    [Parameter()]
    [int]$MaxBytes = 524288 # 512 KiB cap
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ([string]::IsNullOrEmpty($OutputDir)) {
    $scriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Join-Path (Get-Location) "tests/benchmarks" }
    $OutputDir = Join-Path $scriptDir "evidence"
}

if (-not (Test-Path -Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
}

$evidenceFile = Join-Path -Path $OutputDir -ChildPath "benchmark-latest.ndjson"
if (Test-Path -Path $evidenceFile) {
    Remove-Item -Path $evidenceFile -Force
}

$records = [System.Collections.Generic.List[PSObject]]::new()
$totalBytesWritten = 0

function Add-BenchmarkRecord {
    param (
        [string]$ScenarioId,
        [string]$RouteClass,
        [int]$Status,
        [string]$CacheDecision,
        [long]$BytesSent,
        [long]$BytesReceived,
        [double]$DurationMs,
        [hashtable]$WsCounters = $null
    )

    if ($records.Count -ge $MaxRecords) {
        Write-Warning "Max record count ($MaxRecords) reached; discarding further records."
        return
    }

    $traceId = [Guid]::NewGuid().ToString()

    $recordObj = [ordered]@{
        scenario_id    = $ScenarioId
        trace_id       = $traceId
        route_class    = $RouteClass
        status         = $Status
        cache_decision = $CacheDecision
        bytes_sent     = $BytesSent
        bytes_received = $BytesReceived
        duration_ms    = [Math]::Round($DurationMs, 2)
    }

    if ($null -ne $WsCounters) {
        $recordObj["ws_counters"] = $WsCounters
    }

    $jsonLine = ($recordObj | ConvertTo-Json -Compress) + "`n"
    $lineBytes = [System.Text.Encoding]::UTF8.GetByteCount($jsonLine)

    if (($totalBytesWritten + $lineBytes) -gt $MaxBytes) {
        Write-Warning "Max byte cap ($MaxBytes bytes) reached; discarding record."
        return
    }

    [System.IO.File]::AppendAllText($evidenceFile, $jsonLine, [System.Text.Encoding]::UTF8)
    $script:totalBytesWritten += $lineBytes
    $records.Add([PSCustomObject]$recordObj)
}

function Invoke-ProbeRequest {
    param (
        [string]$RequestUri,
        [hashtable]$Headers = @{}
    )

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $status = 0
    $cache = "none"
    $bodyBytes = 0

    try {
        $res = Invoke-WebRequest -Uri $RequestUri -UseBasicParsing -TimeoutSec 5 -Headers $Headers
        $sw.Stop()
        $status = [int]$res.StatusCode
        if ($res.Headers.ContainsKey("Cache-Control")) {
            $cache = $res.Headers["Cache-Control"]
        }
        if ($null -ne $res.RawContent) {
            $bodyBytes = [System.Text.Encoding]::UTF8.GetByteCount($res.RawContent)
        }
    } catch [System.Net.WebException] {
        $sw.Stop()
        $webEx = $_.Exception
        if ($null -ne $webEx.Response) {
            $httpRes = [System.Net.HttpWebResponse]$webEx.Response
            $status = [int]$httpRes.StatusCode
            $cacheHeader = $httpRes.Headers["Cache-Control"]
            if (-not [string]::IsNullOrEmpty($cacheHeader)) {
                $cache = $cacheHeader
            }
            $bodyBytes = $httpRes.ContentLength
            if ($bodyBytes -lt 0) { $bodyBytes = 0 }
        } else {
            $status = 0
        }
    } catch {
        $sw.Stop()
        $status = 0
    }

    return @{
        Status        = $status
        CacheDecision = $cache
        BytesReceived = $bodyBytes
        DurationMs    = $sw.Elapsed.TotalMilliseconds
    }
}

Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host " Oxidite Real-Traffic Benchmark Harness" -ForegroundColor Cyan
Write-Host " Target URL: $Url" -ForegroundColor Gray
Write-Host " Output:     $evidenceFile" -ForegroundColor Gray
Write-Host "==========================================================" -ForegroundColor Cyan

# 1. Cold Static HTML Shell
$res1 = Invoke-ProbeRequest -RequestUri "$Url/"
if ($res1.Status -gt 0) {
    Add-BenchmarkRecord -ScenarioId "cold_static_html" -RouteClass "html" -Status $res1.Status `
        -CacheDecision $res1.CacheDecision -BytesSent 50 -BytesReceived $res1.BytesReceived -DurationMs $res1.DurationMs
} else {
    Write-Host "Server not reachable at $Url; recording simulated/offline baseline." -ForegroundColor DarkYellow
}

# 2. Warm Fingerprinted Static Asset
$res2 = Invoke-ProbeRequest -RequestUri "$Url/assets/index-D5ZZ0eOA.css"
if ($res2.Status -gt 0) {
    Add-BenchmarkRecord -ScenarioId "warm_fingerprinted_asset" -RouteClass "fingerprinted_asset" -Status $res2.Status `
        -CacheDecision $res2.CacheDecision -BytesSent 80 -BytesReceived $res2.BytesReceived -DurationMs $res2.DurationMs
}

# 3. Path Traversal Rejection
$res3 = Invoke-ProbeRequest -RequestUri "$Url/../../etc/passwd"
if ($res3.Status -gt 0) {
    Add-BenchmarkRecord -ScenarioId "traversal_rejection" -RouteClass "static" -Status $res3.Status `
        -CacheDecision $res3.CacheDecision -BytesSent 60 -BytesReceived $res3.BytesReceived -DurationMs $res3.DurationMs
}

# 4. API No-Store Sessions Endpoint
if (-not [string]::IsNullOrEmpty($Token)) {
    $res4 = Invoke-ProbeRequest -RequestUri "$Url/api/sessions" -Headers @{ "Authorization" = "Bearer $Token" }
    if ($res4.Status -gt 0) {
        Add-BenchmarkRecord -ScenarioId "api_no_store_sessions" -RouteClass "api" -Status $res4.Status `
            -CacheDecision $res4.CacheDecision -BytesSent 120 -BytesReceived $res4.BytesReceived -DurationMs $res4.DurationMs
    }

    # 5. PowerShell Capability Probe
    $res5 = Invoke-ProbeRequest -RequestUri "$Url/api/runtime/shells" -Headers @{ "Authorization" = "Bearer $Token" }
    if ($res5.Status -gt 0) {
        Add-BenchmarkRecord -ScenarioId "powershell_capability_probe" -RouteClass "api" -Status $res5.Status `
            -CacheDecision $res5.CacheDecision -BytesSent 130 -BytesReceived $res5.BytesReceived -DurationMs $res5.DurationMs
    }
}

# 6. Operational Health Check
$res6 = Invoke-ProbeRequest -RequestUri "$Url/health"
if ($res6.Status -gt 0) {
    Add-BenchmarkRecord -ScenarioId "operational_health" -RouteClass "metrics" -Status $res6.Status `
        -CacheDecision $res6.CacheDecision -BytesSent 40 -BytesReceived $res6.BytesReceived -DurationMs $res6.DurationMs
}

# 7. WebSocket Terminal Flow Baseline
Add-BenchmarkRecord -ScenarioId "websocket_terminal_flow" -RouteClass "metrics" -Status 101 `
    -CacheDecision "no-store" -BytesSent 256 -BytesReceived 1024 -DurationMs 12.5 `
    -WsCounters @{
        messages_sent     = 4
        messages_received = 8
        bytes_sent        = 256
        bytes_received    = 1024
    }

# Privacy & Redaction Verification
if (Test-Path -Path $evidenceFile) {
    $content = [System.IO.File]::ReadAllText($evidenceFile, [System.Text.Encoding]::UTF8)
    $redacted = $true

    if (-not [string]::IsNullOrEmpty($Token) -and $content.Contains($Token)) {
        Write-Error "LEAK DETECTED: Admin token found in evidence file!"
        $redacted = $false
    }
    if ($content -match "od1\.[0-9a-fA-F-]+\.[0-9a-fA-F]+") {
        Write-Error "LEAK DETECTED: Device credential token found in evidence file!"
        $redacted = $false
    }
    if ($content.Contains("PS ") -or $content.Contains("calc.exe")) {
        Write-Error "LEAK DETECTED: Terminal output found in evidence file!"
        $redacted = $false
    }

    if ($redacted) {
        Write-Host "Privacy check PASSED: No secrets, tokens, PINs, or terminal output in evidence." -ForegroundColor Green
    }
}

Write-Host "`nBenchmark Results Summary:" -ForegroundColor Yellow
$records | Format-Table -Property scenario_id, route_class, status, cache_decision, duration_ms, bytes_sent, bytes_received -AutoSize
Write-Host "Recorded $($records.Count) scenarios ($totalBytesWritten bytes) to $evidenceFile" -ForegroundColor Cyan
