param(
    [int]$HalvingInterval = 1051200,
    [string]$InitialRewardZka = "12.000000000000",
    [string]$MaxSupplyZka = "23000000.000000000000"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Parse-ZkaToAtomic([string]$zka) {
    $atomic = [System.Numerics.BigInteger]::Parse("1000000000000")
    $parts = $zka.Split(".")
    $whole = [System.Numerics.BigInteger]::Parse($parts[0])
    $fracRaw = if ($parts.Length -gt 1) { $parts[1] } else { "" }
    if ($fracRaw.Length -gt 12) {
        throw "Too many decimal places in '$zka' (max 12)"
    }
    $frac = $fracRaw.PadRight(12, "0")
    $fracValue = if ([string]::IsNullOrWhiteSpace($frac)) {
        [System.Numerics.BigInteger]::Zero
    } else {
        [System.Numerics.BigInteger]::Parse($frac)
    }
    return ($whole * $atomic) + $fracValue
}

function Format-AtomicToZka([System.Numerics.BigInteger]$value) {
    $atomic = [System.Numerics.BigInteger]::Parse("1000000000000")
    $sign = ""
    $abs = $value
    if ($value -lt 0) {
        $sign = "-"
        $abs = [System.Numerics.BigInteger]::Negate($value)
    }
    $whole = $abs / $atomic
    $frac = ($abs % $atomic).ToString().PadLeft(12, "0")
    return "$sign$whole.$frac"
}

$atomicUnit = [System.Numerics.BigInteger]::Parse("1000000000000")
$initialRewardAtomic = Parse-ZkaToAtomic -zka $InitialRewardZka
$maxSupplyAtomic = Parse-ZkaToAtomic -zka $MaxSupplyZka
$remaining = $maxSupplyAtomic
$minted = [System.Numerics.BigInteger]::Zero
$height = [System.Numerics.BigInteger]::Zero
$era = 0
$rows = New-Object System.Collections.Generic.List[object]

while ($remaining -gt 0) {
    $rewardAtomic = $initialRewardAtomic / [System.Numerics.BigInteger]::Pow(2, $era)
    if ($rewardAtomic -le 0) {
        break
    }

    $eraStart = $height
    $intervalAtomic = $rewardAtomic * $HalvingInterval

    if ($intervalAtomic -lt $remaining) {
        $mintEra = $intervalAtomic
        $blocks = [System.Numerics.BigInteger]::Parse($HalvingInterval.ToString())
        $remaining -= $mintEra
        $minted += $mintEra
        $height += $blocks
        $rows.Add([pscustomobject]@{
                Era         = $era
                StartHeight = $eraStart
                EndHeight   = $height - 1
                Blocks      = $blocks
                RewardZka   = Format-AtomicToZka $rewardAtomic
                MintedZka   = Format-AtomicToZka $mintEra
                CumZka      = Format-AtomicToZka $minted
                Note        = "full era"
            })
        $era++
        continue
    }

    $fullBlocks = $remaining / $rewardAtomic
    if ($fullBlocks -gt 0) {
        $mintFull = $fullBlocks * $rewardAtomic
        $remaining -= $mintFull
        $minted += $mintFull
        $height += $fullBlocks
        $rows.Add([pscustomobject]@{
                Era         = $era
                StartHeight = $eraStart
                EndHeight   = $height - 1
                Blocks      = $fullBlocks
                RewardZka   = Format-AtomicToZka $rewardAtomic
                MintedZka   = Format-AtomicToZka $mintFull
                CumZka      = Format-AtomicToZka $minted
                Note        = "partial era (full-reward blocks)"
            })
    }

    if ($remaining -gt 0) {
        $capStart = $height
        $mintCap = $remaining
        $remaining = [System.Numerics.BigInteger]::Zero
        $minted += $mintCap
        $height += 1
        $rows.Add([pscustomobject]@{
                Era         = $era
                StartHeight = $capStart
                EndHeight   = $capStart
                Blocks      = [System.Numerics.BigInteger]::One
                RewardZka   = Format-AtomicToZka $mintCap
                MintedZka   = Format-AtomicToZka $mintCap
                CumZka      = Format-AtomicToZka $minted
                Note        = "cap block (clipped reward)"
            })
    }

    break
}

"# Emission v1 Simulation"
""
"- Initial reward: $(Format-AtomicToZka $initialRewardAtomic) ZKA"
"- Halving interval: $HalvingInterval blocks"
"- Max supply: $(Format-AtomicToZka $maxSupplyAtomic) ZKA"
"- Minted total: $(Format-AtomicToZka $minted) ZKA"
"- Final rewarded block height: $($height - 1)"
"- First zero-reward block height: $height"
""
"| Era | Start Height | End Height | Blocks | Reward (ZKA/block) | Minted In Row (ZKA) | Cumulative Minted (ZKA) | Note |"
"|---|---:|---:|---:|---:|---:|---:|---|"
foreach ($r in $rows) {
    "| $($r.Era) | $($r.StartHeight) | $($r.EndHeight) | $($r.Blocks) | $($r.RewardZka) | $($r.MintedZka) | $($r.CumZka) | $($r.Note) |"
}
