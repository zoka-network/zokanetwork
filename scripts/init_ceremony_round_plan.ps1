param(
    [Parameter(Mandatory = $true)]
    [string]$OutPath,
    [string]$Label = "zoka-mainnet-v1",
    [string[]]$Contributors = @("contrib-a", "contrib-b", "contrib-c", "contrib-d", "contrib-e")
)

$ErrorActionPreference = "Stop"

if ($Contributors.Count -lt 1) {
    throw "At least one contributor is required"
}

$rounds = @()
for ($i = 0; $i -lt $Contributors.Count; $i++) {
    $rounds += [ordered]@{
        round = ($i + 1)
        contributor_id = $Contributors[$i]
        status = if ($i -eq 0) { "prepared" } else { "pending" }
        package = ""
        return_artifact = ""
        finalized = $false
        notes = ""
    }
}

$plan = [ordered]@{
    version = 1
    label = $Label
    created_at_utc = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    rounds = $rounds
}

$parent = Split-Path -Parent $OutPath
if (-not [string]::IsNullOrWhiteSpace($parent) -and -not (Test-Path -LiteralPath $parent)) {
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
}

$json = $plan | ConvertTo-Json -Depth 10
Set-Content -LiteralPath $OutPath -Value $json -Encoding UTF8

Write-Host "Ceremony round plan initialized:"
Write-Host "  Path : $OutPath"
Write-Host "  Label: $Label"
Write-Host "  Rounds: $($Contributors.Count)"
