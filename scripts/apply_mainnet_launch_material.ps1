param(
    [Parameter(Mandatory = $true)][string]$MaterialPath,
    [string]$GenesisPath = "docs/genesis.mainnet.v1.json",
    [string]$NetworkPath = "docs/mainnet-network.v1.json",
    [string]$ReleaseVerificationPath = "docs/release-verification.md",
    [switch]$InPlace
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$validationErrors = New-Object System.Collections.Generic.List[string]

function Test-Hex32 {
    param([string]$Value)
    return $Value -match '^[0-9a-fA-F]{64}$'
}

function Test-Placeholder {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) { return $true }
    $lower = $Value.ToLowerInvariant()
    return $lower.Contains("<") -or
        $lower.Contains("placeholder") -or
        $lower.Contains("example") -or
        $lower.Contains("111111") -or
        $lower.Contains("222222") -or
        $lower.Contains("333333") -or
        $lower.Contains("aaaaaaaa") -or
        $lower.Contains("bbbbbbbb") -or
        $lower.Contains("cccccccc")
}

function Assert-NoPlaceholder {
    param([string]$Name, [string]$Value)
    if (Test-Placeholder $Value) {
        $script:validationErrors.Add("$Name still looks like placeholder: $Value") | Out-Null
    }
}

function Add-ValidationError {
    param([string]$Message)
    $script:validationErrors.Add($Message) | Out-Null
}

function Write-JsonFile {
    param([object]$Value, [string]$Path)
    $json = $Value | ConvertTo-Json -Depth 32
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText((Resolve-Path -LiteralPath $Path), $json, $utf8NoBom)
}

if (!(Test-Path -LiteralPath $MaterialPath -PathType Leaf)) {
    throw "Material file not found: $MaterialPath"
}

$material = Get-Content -LiteralPath $MaterialPath -Raw | ConvertFrom-Json
if ($material.schema -ne "zoka-mainnet-launch-material-v1") {
    throw "Unsupported material schema '$($material.schema)'"
}

if ($null -eq $material.genesis.validators -or $material.genesis.validators.Count -lt 1) {
    Add-ValidationError "Material must include genesis.validators"
}
foreach ($validator in $material.genesis.validators) {
    Assert-NoPlaceholder "validator $($validator.label) consensus_pubkey_hex" $validator.consensus_pubkey_hex
    Assert-NoPlaceholder "validator $($validator.label) operator_pubkey_hex" $validator.operator_pubkey_hex
    if (!(Test-Hex32 $validator.consensus_pubkey_hex)) {
        Add-ValidationError "validator $($validator.label) consensus_pubkey_hex must be 32-byte hex"
    }
    if (!(Test-Hex32 $validator.operator_pubkey_hex)) {
        Add-ValidationError "validator $($validator.label) operator_pubkey_hex must be 32-byte hex"
    }
    if ($validator.consensus_pubkey_hex -eq $validator.operator_pubkey_hex) {
        Add-ValidationError "validator $($validator.label) consensus and operator pubkeys must differ"
    }
}

if ($material.network.status -notin @("launch-ready", "active")) {
    Add-ValidationError "network.status must be launch-ready or active"
}
$activeSeeds = @($material.network.seed_nodes | Where-Object { $_.status -eq "active" })
if ($activeSeeds.Count -lt 3) {
    Add-ValidationError "network.seed_nodes must include at least 3 active seeds"
}
$regions = @($activeSeeds | ForEach-Object { $_.region } | Sort-Object -Unique)
if ($regions.Count -lt 3) {
    Add-ValidationError "active seed nodes must cover at least 3 regions"
}
foreach ($seed in $material.network.seed_nodes) {
    Assert-NoPlaceholder "seed $($seed.label) multiaddr" $seed.multiaddr
    Assert-NoPlaceholder "seed $($seed.label) onion" $seed.onion
    Assert-NoPlaceholder "seed $($seed.label) i2p" $seed.i2p
}
foreach ($rpc in $material.network.public_rpc) {
    Assert-NoPlaceholder "rpc $($rpc.label) url" $rpc.url
    Assert-NoPlaceholder "rpc $($rpc.label) onion" $rpc.onion
}
foreach ($signer in $material.network.snapshot_signers) {
    Assert-NoPlaceholder "snapshot signer" $signer
    if (!(Test-Hex32 $signer)) {
        throw "snapshot signer must be 32-byte hex"
    }
}

if ($material.release.gpg_fingerprint -notmatch '^[0-9a-fA-F ]{40,49}$') {
    Add-ValidationError "release.gpg_fingerprint must be a 40-hex-char GPG fingerprint, spaces allowed"
}

if ($validationErrors.Count -gt 0) {
    Write-Host "Launch material is not ready:"
    foreach ($err in $validationErrors) {
        Write-Host "  - $err"
    }
    exit 1
}

$fingerprint = (($material.release.gpg_fingerprint -replace '\s+', '').ToUpperInvariant() -split '(.{4})' | Where-Object { $_ }) -join ' '

$genesis = Get-Content -LiteralPath $GenesisPath -Raw | ConvertFrom-Json
$genesis.validators = @($material.genesis.validators)
if ($null -ne $material.genesis.pre_mine) {
    $genesis.pre_mine = @($material.genesis.pre_mine)
}
if ($null -ne $material.genesis.initial_accounts) {
    $genesis.initial_accounts = @($material.genesis.initial_accounts)
}

$network = Get-Content -LiteralPath $NetworkPath -Raw | ConvertFrom-Json
$network.status = $material.network.status
$network.seed_nodes = @($material.network.seed_nodes)
$network.public_rpc = @($material.network.public_rpc)
$network.snapshot_signers = @($material.network.snapshot_signers)

$releaseDoc = Get-Content -LiteralPath $ReleaseVerificationPath -Raw
$releaseDoc = $releaseDoc -replace 'Pendiente: <FFFF FFFF FFFF FFFF FFFF FFFF FFFF FFFF FFFF FFFF>', $fingerprint
$releaseDoc = $releaseDoc -replace '> \*\*TODO antes de mainnet launch\*\*: el equipo core publica aqui el GPG\r?\n> fingerprint definitivo \(40 chars hex\)\. Hasta entonces, este es el slot\r?\n> que cualquier verifier compara contra el fingerprint que `gpg --verify`\r?\n> imprime\.\r?\n', ''

if (!$InPlace) {
    Write-Host "Material validates. Pass -InPlace to update:"
    Write-Host "  $GenesisPath"
    Write-Host "  $NetworkPath"
    Write-Host "  $ReleaseVerificationPath"
    exit 0
}

Write-JsonFile -Value $genesis -Path $GenesisPath
Write-JsonFile -Value $network -Path $NetworkPath
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText((Resolve-Path -LiteralPath $ReleaseVerificationPath), $releaseDoc, $utf8NoBom)

Write-Host "Applied launch material."
Write-Host "Next:"
Write-Host "  1. cargo build --release --features randomx --bin zoka --bin zokahd"
Write-Host "  2. target\release\zoka.exe genesis-verify $GenesisPath --json"
Write-Host "  3. target\release\zoka.exe mainnet-network-lint $NetworkPath"
Write-Host "  4. PowerShell -ExecutionPolicy Bypass -File scripts\mainnet_readiness_check.ps1 -BuildRelease"
