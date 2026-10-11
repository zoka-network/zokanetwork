<#
.SYNOPSIS
Build the isolated zoka-pq crate for five platforms and optionally measure an
authorized ARMv7 Android device. Never opens wallets or installs an APK.
.EXAMPLE
./scripts/check-pq-platforms.ps1 -RunAndroidBench -Iterations 100 -AndroidRuns 3
#>
[CmdletBinding()]
param(
    [ValidateRange(1, 100000)][int]$Iterations = 100,
    [ValidateRange(1, 10)][int]$AndroidRuns = 3,
    [switch]$RunAndroidBench,
    [string]$DeviceSerial = '',
    [string]$AndroidNdkHome = $env:ANDROID_NDK_HOME,
    [string]$OutputDirectory = '',
    [string]$TargetDirectory = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$pqRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$pqCrate = Join-Path $pqRoot 'crates\zoka-pq'
$pqManifest = Join-Path $pqCrate 'Cargo.toml'
$pqLock = Join-Path $pqCrate 'Cargo.lock'
$pqUtf8 = [Text.UTF8Encoding]::new($false)
$pqSteps = [Collections.Generic.List[object]]::new()
$pqStamp = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss')
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $pqRoot "docs\artifacts\pq-$pqStamp" }
if (-not $TargetDirectory) { $TargetDirectory = Join-Path $pqRoot 'target-pq-platforms' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$TargetDirectory = [IO.Path]::GetFullPath($TargetDirectory)
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null

function Find-PqTool {
    param([string]$Name, [string]$Fallback)
    $tool = Get-Command $Name -ErrorAction SilentlyContinue
    if ($tool) { return $tool.Source }
    if (Test-Path -LiteralPath $Fallback) { return $Fallback }
    throw "Missing tool: $Name"
}

function Write-PqJson {
    param([string]$Name, [object]$Value)
    [IO.File]::WriteAllText((Join-Path $OutputDirectory $Name), ($Value | ConvertTo-Json -Depth 30), $pqUtf8)
}

function Invoke-PqTool {
    param([string]$Name, [string]$Executable, [string[]]$Arguments)
    $timer = [Diagnostics.Stopwatch]::StartNew()
    # Windows PowerShell treats native stderr as ErrorRecord objects. Preserve
    # compiler diagnostics in the log and judge success using the native exit code.
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $lines = @(& $Executable @Arguments 2>&1 | ForEach-Object { $_.ToString() })
        $nativeCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
        $timer.Stop()
    }
    $output = $lines -join "`n"
    [IO.File]::WriteAllText((Join-Path $OutputDirectory "$Name.log"), $output, $pqUtf8)
    $pqSteps.Add([pscustomobject]@{
        name = $Name
        exit_code = $nativeCode
        elapsed_seconds = [Math]::Round($timer.Elapsed.TotalSeconds, 3)
        executable = [IO.Path]::GetFileName($Executable)
        arguments = $Arguments
    })
    Write-Host "$Name : exit $nativeCode ($([Math]::Round($timer.Elapsed.TotalSeconds, 2)) s)"
    if ($nativeCode -ne 0) { throw "$Name failed; see $OutputDirectory\$Name.log" }
    return $output
}

function Save-PqBenchmark {
    param([string]$Name, [string]$Output, [string]$ExpectedArch, [string]$ExpectedOs)
    $measurement = $Output | ConvertFrom-Json
    if ($measurement.schema_version -ne 1 -or $measurement.architecture -ne $ExpectedArch -or $measurement.os -ne $ExpectedOs) {
        throw "Unexpected benchmark format/platform: $Name"
    }
    if ($measurement.iterations -ne $Iterations -or @($measurement.operations).Count -eq 0) {
        throw "Incomplete benchmark: $Name"
    }
    Write-PqJson "$Name.json" $measurement
}

$cargo = Find-PqTool 'cargo' (Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe')
$rustc = Find-PqTool 'rustc' (Join-Path $env:USERPROFILE '.cargo\bin\rustc.exe')
$rustup = Find-PqTool 'rustup' (Join-Path $env:USERPROFILE '.cargo\bin\rustup.exe')
if (-not (Test-Path -LiteralPath $pqLock)) { throw 'The isolated crate requires its committed Cargo.lock.' }
if (-not $AndroidNdkHome) {
    $AndroidNdkHome = Join-Path $env:LOCALAPPDATA 'Android\Sdk\ndk\27.3.13750724'
}
if (-not (Test-Path -LiteralPath (Join-Path $AndroidNdkHome 'source.properties'))) { throw 'Android NDK not found; set -AndroidNdkHome.' }

$savedTarget = $env:CARGO_TARGET_DIR
$savedNdk = $env:ANDROID_NDK_HOME
$savedPath = $env:PATH
$env:CARGO_TARGET_DIR = $TargetDirectory
$env:ANDROID_NDK_HOME = $AndroidNdkHome
$env:PATH = "$(Split-Path -Parent $cargo);$savedPath"
$summary = [ordered]@{
    schema_version = 1
    captured_at_utc = [DateTime]::UtcNow.ToString('o')
    manifest = 'crates/zoka-pq/Cargo.toml'
    lock_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $pqLock).Hash
    iterations = $Iterations
    android_runs = $AndroidRuns
    android_requested = [bool]$RunAndroidBench
    status = 'running'
    execution_limits = 'Linux x64/ARM64 libraries are cross-compiled only. Android ARM64 is linked only unless run separately on an ARM64 device.'
}

Push-Location $pqCrate
try {
    $summary.rustc = Invoke-PqTool 'rustc-version' $rustc @('-vV')
    $summary.cargo = Invoke-PqTool 'cargo-version' $cargo @('--version')
    $summary.cargo_ndk = Invoke-PqTool 'cargo-ndk-version' $cargo @('ndk', '--version')
    $installed = Invoke-PqTool 'installed-targets' $rustup @('target', 'list', '--installed')
    $targets = @('x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu', 'aarch64-linux-android', 'armv7-linux-androideabi')
    foreach ($pqTarget in $targets) {
        if (($installed -split "`r?`n") -notcontains $pqTarget) { throw "Install the missing Rust target: $pqTarget" }
        $null = Invoke-PqTool "check-$pqTarget" $cargo @('check', '--manifest-path', $pqManifest, '--locked', '--all-targets', '--target', $pqTarget)
        $null = Invoke-PqTool "lib-$pqTarget" $cargo @('build', '--manifest-path', $pqManifest, '--locked', '--release', '--lib', '--target', $pqTarget)
    }
    $null = Invoke-PqTool 'tests-windows' $cargo @('test', '--manifest-path', $pqManifest, '--locked')
    $null = Invoke-PqTool 'build-bench-windows' $cargo @('build', '--manifest-path', $pqManifest, '--locked', '--release', '--bin', 'pq-bench')
    $hostBinary = Join-Path $TargetDirectory 'release\pq-bench.exe'
    Save-PqBenchmark 'benchmark-windows' (Invoke-PqTool 'benchmark-windows' $hostBinary @('--iterations', "$Iterations")) 'x86_64' 'windows'

    foreach ($abi in @('arm64-v8a', 'armeabi-v7a')) {
        $null = Invoke-PqTool "link-benchmark-$abi" $cargo @('ndk', '-t', $abi, '-P', '26', 'build', '--manifest-path', $pqManifest, '--locked', '--release', '--bin', 'pq-bench')
    }

    if ($RunAndroidBench) {
        $adb = Find-PqTool 'adb' (Join-Path $env:LOCALAPPDATA 'Android\Sdk\platform-tools\adb.exe')
        $devices = Invoke-PqTool 'adb-devices' $adb @('devices')
        $authorized = @($devices -split "`r?`n" | Where-Object { $_ -match '^\S+\s+device$' } | ForEach-Object { ($_ -split '\s+')[0] })
        if (-not $DeviceSerial) {
            if ($authorized.Count -ne 1) { throw 'Specify -DeviceSerial when more than one device is connected.' }
            $DeviceSerial = $authorized[0]
        }
        if ($authorized -notcontains $DeviceSerial) { throw 'Selected Android device is not connected and authorized.' }
        $adbPrefix = @('-s', $DeviceSerial)
        $abis = Invoke-PqTool 'android-abis' $adb ($adbPrefix + @('shell', 'getprop', 'ro.product.cpu.abilist'))
        if (($abis.Trim() -split ',') -notcontains 'armeabi-v7a') { throw 'ARMv7 execution requires an armeabi-v7a device.' }
        $summary.android = [ordered]@{
            model = (Invoke-PqTool 'android-model' $adb ($adbPrefix + @('shell', 'getprop', 'ro.product.model'))).Trim()
            abis = $abis.Trim()
            api = (Invoke-PqTool 'android-api' $adb ($adbPrefix + @('shell', 'getprop', 'ro.build.version.sdk'))).Trim()
            kernel = (Invoke-PqTool 'android-kernel' $adb ($adbPrefix + @('shell', 'uname', '-a'))).Trim()
        }

        # cargo-ndk consumes Cargo's JSON output. Find the library test harness
        # in the isolated target directory after a successful build instead.
        $null = Invoke-PqTool 'link-tests-armeabi-v7a' $cargo @('ndk', '-t', 'armeabi-v7a', '-P', '26', 'test', '--manifest-path', $pqManifest, '--locked', '--release', '--lib', '--no-run')
        $androidTestDirectory = Join-Path $TargetDirectory 'armv7-linux-androideabi\release\deps'
        $testBinaries = @(Get-ChildItem -LiteralPath $androidTestDirectory -File | Where-Object { $_.Name -match '^zoka_pq-[0-9a-f]+$' } | ForEach-Object { $_.FullName })
        if ($testBinaries.Count -ne 1) { throw 'Expected exactly one Android library test harness in the isolated target directory. Use a fresh -TargetDirectory if old artifacts remain.' }
        $testStream = [IO.File]::OpenRead($testBinaries[0])
        try {
            $elfHeader = New-Object byte[] 4
            if ($testStream.Read($elfHeader, 0, 4) -ne 4 -or [BitConverter]::ToString($elfHeader) -ne '7F-45-4C-46') { throw 'Android test harness is not an ELF executable.' }
        } finally { $testStream.Dispose() }

        # A unique temporary directory avoids overwriting any existing device file.
        $remoteDirectory = "/data/local/tmp/zoka-pq-$pqStamp"
        $null = Invoke-PqTool 'android-mkdir' $adb ($adbPrefix + @('shell', 'mkdir', $remoteDirectory))
        $remoteFiles = [Collections.Generic.List[string]]::new()
        try {
            $testIndex = 0
            foreach ($testBinary in $testBinaries) {
                $remoteTest = "$remoteDirectory/test-$testIndex"
                $remoteFiles.Add($remoteTest)
                $null = Invoke-PqTool "android-push-test-$testIndex" $adb ($adbPrefix + @('push', $testBinary, $remoteTest))
                $null = Invoke-PqTool "android-chmod-test-$testIndex" $adb ($adbPrefix + @('shell', 'chmod', '700', $remoteTest))
                $null = Invoke-PqTool "android-tests-$testIndex" $adb ($adbPrefix + @('shell', $remoteTest, '--test-threads=1'))
                $testIndex++
            }
            $remoteBenchmark = "$remoteDirectory/pq-bench"
            $remoteFiles.Add($remoteBenchmark)
            $androidBinary = Join-Path $TargetDirectory 'armv7-linux-androideabi\release\pq-bench'
            $summary.android.benchmark_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $androidBinary).Hash
            $null = Invoke-PqTool 'android-push-benchmark' $adb ($adbPrefix + @('push', $androidBinary, $remoteBenchmark))
            $null = Invoke-PqTool 'android-chmod-benchmark' $adb ($adbPrefix + @('shell', 'chmod', '700', $remoteBenchmark))
            for ($run = 1; $run -le $AndroidRuns; $run++) {
                $name = "benchmark-android-armv7-$run"
                Save-PqBenchmark $name (Invoke-PqTool $name $adb ($adbPrefix + @('shell', $remoteBenchmark, '--iterations', "$Iterations"))) 'arm' 'android'
            }
        } finally {
            # Delete only the exact temporary files created by this run; no APK or
            # application data is read, changed, or removed, and no recursive rm.
            foreach ($remoteFile in $remoteFiles) {
                $null = Invoke-PqTool "cleanup-$([IO.Path]::GetFileName($remoteFile))" $adb ($adbPrefix + @('shell', 'rm', '-f', $remoteFile))
            }
            $null = Invoke-PqTool 'cleanup-android-directory' $adb ($adbPrefix + @('shell', 'rmdir', $remoteDirectory))
        }
    }
    $summary.status = 'passed'
} catch {
    $summary.status = 'failed'
    $summary.error = $_.Exception.Message
    throw
} finally {
    $summary.steps = @($pqSteps.ToArray())
    $summary.completed_at_utc = [DateTime]::UtcNow.ToString('o')
    Write-PqJson 'summary.json' $summary
    $env:CARGO_TARGET_DIR = $savedTarget
    $env:ANDROID_NDK_HOME = $savedNdk
    $env:PATH = $savedPath
    Pop-Location
    Write-Host "Evidence: $OutputDirectory"
}
