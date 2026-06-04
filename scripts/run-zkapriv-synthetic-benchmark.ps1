param(
    [string]$InputCounts = "1,2,4,8",
    [switch]$SkipNativeBuild,
    [switch]$KeepInstalled,
    [switch]$V2I1,
    [string]$OutputDirectory = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$androidRoot = Join-Path $repoRoot "zkapriv"
$nativeBuild = Join-Path $androidRoot "native\build-android.ps1"
$adb = (Get-Command adb -ErrorAction SilentlyContinue).Source
if (-not $adb) {
    $adb = Join-Path $env:LOCALAPPDATA "Android\Sdk\platform-tools\adb.exe"
}
if (-not (Test-Path -LiteralPath $adb)) {
    throw "adb was not found. Install Android platform-tools or add adb to PATH."
}

$deviceLines = @((& $adb devices) | Where-Object { $_ -match "^\S+\s+device$" })
if ($deviceLines.Count -ne 1) {
    throw "Connect exactly one authorized Android device before running this benchmark."
}
$serial = ($deviceLines[0] -split "\s+")[0]

if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $repoRoot "benchmark-output"
}
$resolvedOutput = [System.IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $resolvedOutput | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logPath = Join-Path $resolvedOutput "zkapriv-synthetic-$stamp.log"
$appApk = Join-Path $androidRoot "app\build\outputs\apk\debug\app-debug.apk"
$testApk = Join-Path $androidRoot "app\build\outputs\apk\androidTest\debug\app-debug-androidTest.apk"
$benchmarkApplicationId = "com.zoka.zkapriv.benchmark"
$benchmarkTestApplicationId = "$benchmarkApplicationId.test"
$testRunner = "$benchmarkTestApplicationId/androidx.test.runner.AndroidJUnitRunner"
$benchmarkClass = if ($V2I1) {
    "com.zoka.zkapriv.MobileProverBenchmarkInstrumentedTest#benchmarkSyntheticPrivateTxV2I1Lab"
} else {
    "com.zoka.zkapriv.MobileProverBenchmarkInstrumentedTest#benchmarkSyntheticPrivateTransactions"
}
$resultPattern = if ($V2I1) { "V2_I1_RESULT=" } else { "BENCH_RESULT=" }

if (-not $SkipNativeBuild) {
    & $nativeBuild
    if ($LASTEXITCODE -ne 0) {
        throw "Android JNI build failed."
    }
}

& $adb -s $serial logcat -c

Push-Location $androidRoot
try {
    & .\gradlew.bat :app:assembleDebug :app:assembleDebugAndroidTest
    if ($LASTEXITCODE -ne 0) {
        throw "Android benchmark APK build failed."
    }
} finally {
    Pop-Location
}

& $adb -s $serial install -r $appApk
if ($LASTEXITCODE -ne 0) {
    throw "Android benchmark APK installation failed."
}
& $adb -s $serial install -r $testApk
if ($LASTEXITCODE -ne 0) {
    throw "Android benchmark test APK installation failed."
}

$instrumentOutput = & $adb -s $serial shell am instrument -w -r `
    -e class $benchmarkClass `
    -e zokaBenchInputs $InputCounts `
    $testRunner
$instrumentText = ($instrumentOutput | Out-String)
$instrumentOutput | ForEach-Object { Write-Host $_ }
if ($LASTEXITCODE -ne 0 -or $instrumentText -notmatch "(?m)^OK \(\d+ tests?\)") {
    throw "Android synthetic benchmark failed."
}

$log = & $adb -s $serial logcat -d -s "ZokaMobileBenchmark:I" "*:S"
$log | Set-Content -LiteralPath $logPath
$results = $log | Select-String $resultPattern
if (-not $results) {
    throw "Benchmark completed but no $resultPattern lines were found in logcat."
}

Write-Host "Synthetic benchmark results:"
$results | ForEach-Object { Write-Host $_.Line }
Write-Host "Saved: $logPath"

if (-not $KeepInstalled) {
    & $adb -s $serial uninstall $benchmarkTestApplicationId | Out-Null
    & $adb -s $serial uninstall $benchmarkApplicationId | Out-Null
}
