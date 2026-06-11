param(
    [int]$DurationSeconds = 300,
    [int]$IntervalSeconds = 2,
    [string]$PackageName = "com.zoka.zkapriv",
    [string]$ProcessName = "com.zoka.zkapriv",
    [string]$OutputDirectory = ""
)

$ErrorActionPreference = "Stop"

if ($DurationSeconds -lt 1) {
    throw "DurationSeconds must be greater than zero."
}
if ($IntervalSeconds -lt 1) {
    throw "IntervalSeconds must be greater than zero."
}

$adb = (Get-Command adb -ErrorAction SilentlyContinue).Source
if (-not $adb) {
    $adb = Join-Path $env:LOCALAPPDATA "Android\Sdk\platform-tools\adb.exe"
}
if (-not (Test-Path -LiteralPath $adb)) {
    throw "adb was not found. Install Android platform-tools or add adb to PATH."
}

function Invoke-Adb {
    param([string[]]$Arguments)

    (& $adb @Arguments 2>$null | Out-String).Trim()
}

$deviceLines = @((& $adb devices) | Where-Object { $_ -match "^\S+\s+device$" })
if ($deviceLines.Count -ne 1) {
    throw "Connect exactly one authorized Android device before running this benchmark."
}
$serial = ($deviceLines[0] -split "\s+")[0]

if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $PSScriptRoot "..\benchmark-output"
}
$resolvedOutput = [System.IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $resolvedOutput | Out-Null

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$csvPath = Join-Path $resolvedOutput "zkapriv-android-$stamp.csv"
$metadataPath = Join-Path $resolvedOutput "zkapriv-android-$stamp.txt"
$deviceMemInfo = Invoke-Adb @('-s', $serial, 'shell', 'cat', '/proc/meminfo')
$memTotal = ($deviceMemInfo -split "`r?`n" | Where-Object { $_ -match "^MemTotal:" } | Select-Object -First 1)

$metadata = @(
    "captured_at_utc=$([DateTime]::UtcNow.ToString('o'))"
    "serial=$serial"
    "model=$(Invoke-Adb @('-s', $serial, 'shell', 'getprop', 'ro.product.model'))"
    "abis=$(Invoke-Adb @('-s', $serial, 'shell', 'getprop', 'ro.product.cpu.abilist'))"
    "android_sdk=$(Invoke-Adb @('-s', $serial, 'shell', 'getprop', 'ro.build.version.sdk'))"
    "mem_total=$memTotal"
    "package=$PackageName"
    "process=$ProcessName"
    "duration_seconds=$DurationSeconds"
    "interval_seconds=$IntervalSeconds"
)
Set-Content -LiteralPath $metadataPath -Value $metadata

Write-Host "Monitoring $ProcessName on $serial for $DurationSeconds seconds."
Write-Host "Start one LOCAL proving transfer in ZKAPriv now."
Write-Host "CSV: $csvPath"

$started = Get-Date
$samples = [System.Collections.Generic.List[object]]::new()
while (((Get-Date) - $started).TotalSeconds -lt $DurationSeconds) {
    $elapsed = [math]::Round(((Get-Date) - $started).TotalSeconds, 1)
    $pidValue = Invoke-Adb @("-s", $serial, "shell", "pidof", $ProcessName)
    $pidValue = ($pidValue -split "\s+" | Select-Object -First 1)

    $rssKb = $null
    $pssKb = $null
    if ($pidValue -match "^\d+$") {
        $status = Invoke-Adb @("-s", $serial, "shell", "cat", "/proc/$pidValue/status")
        if ($status -match "(?m)^VmRSS:\s+(\d+)\s+kB") {
            $rssKb = [int64]$matches[1]
        }

        $meminfo = Invoke-Adb @("-s", $serial, "shell", "dumpsys", "meminfo", $ProcessName)
        if ($meminfo -match "(?m)^\s*TOTAL PSS:\s+(\d+)") {
            $pssKb = [int64]$matches[1]
        } elseif ($meminfo -match "(?m)^\s*TOTAL\s+(\d+)") {
            $pssKb = [int64]$matches[1]
        }
    }

    $cpuPercent = $null
    $cpuInfo = Invoke-Adb @("-s", $serial, "shell", "dumpsys", "cpuinfo")
    $escapedProcess = [regex]::Escape($ProcessName)
    if ($cpuInfo -match "(?m)^\s*([0-9.]+)%\s+\d+/$escapedProcess(?:\s|:)") {
        $cpuPercent = [double]$matches[1]
    }

    $batteryTempC = $null
    $battery = Invoke-Adb @("-s", $serial, "shell", "dumpsys", "battery")
    if ($battery -match "(?m)^\s*temperature:\s*(\d+)") {
        $batteryTempC = [math]::Round(([double]$matches[1] / 10.0), 1)
    }

    $samples.Add([pscustomobject]@{
        captured_at_utc = [DateTime]::UtcNow.ToString("o")
        elapsed_seconds = $elapsed
        pid = $pidValue
        rss_kb = $rssKb
        pss_kb = $pssKb
        cpu_percent = $cpuPercent
        battery_temp_c = $batteryTempC
    })
    Start-Sleep -Seconds $IntervalSeconds
}

$samples | Export-Csv -LiteralPath $csvPath -NoTypeInformation
$peakRssKb = ($samples | Measure-Object -Property rss_kb -Maximum).Maximum
$peakPssKb = ($samples | Measure-Object -Property pss_kb -Maximum).Maximum
$peakTempC = ($samples | Measure-Object -Property battery_temp_c -Maximum).Maximum

Write-Host "Done. samples=$($samples.Count) peak_rss_kb=$peakRssKb peak_pss_kb=$peakPssKb peak_temp_c=$peakTempC"
