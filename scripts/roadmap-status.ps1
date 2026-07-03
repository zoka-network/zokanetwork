param(
    [string]$Roadmap = "docs/sync-network-redesign.md"
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$roadmapPath = if ([System.IO.Path]::IsPathRooted($Roadmap)) {
    $Roadmap
} else {
    Join-Path $root $Roadmap
}

if (-not (Test-Path -LiteralPath $roadmapPath)) {
    throw "Roadmap file not found: $roadmapPath"
}

$phase = "Sin fase"
$items = New-Object System.Collections.Generic.List[object]

foreach ($line in Get-Content -LiteralPath $roadmapPath) {
    if ($line -match '^###\s+(.+)$') {
        $phase = $Matches[1].Trim()
        continue
    }

    if ($line -match '^\s*-\s+\[( |x|X)\]\s+(.+)$') {
        $done = $Matches[1].Trim().ToLowerInvariant() -eq "x"
        $items.Add([pscustomobject]@{
            Phase = $phase
            Status = if ($done) { "DONE" } else { "TODO" }
            Item = $Matches[2].Trim()
        })
    }
}

$total = $items.Count
$doneCount = @($items | Where-Object { $_.Status -eq "DONE" }).Count
$todoCount = $total - $doneCount
$percent = if ($total -gt 0) { [math]::Round(($doneCount * 100.0) / $total, 1) } else { 0 }

Write-Host "Roadmap: $roadmapPath"
Write-Host "Estado global: $doneCount/$total completado ($percent%), $todoCount pendiente(s)"
Write-Host ""

$items |
    Group-Object Phase |
    ForEach-Object {
        $phaseItems = $_.Group
        $phaseDone = @($phaseItems | Where-Object { $_.Status -eq "DONE" }).Count
        $phaseTotal = $phaseItems.Count
        $phaseTodo = $phaseTotal - $phaseDone
        [pscustomobject]@{
            Phase = $_.Name
            Done = "$phaseDone/$phaseTotal"
            Todo = $phaseTodo
        }
    } |
    Format-Table -AutoSize

Write-Host ""
Write-Host "Siguientes TODO:"
$items |
    Where-Object { $_.Status -eq "TODO" } |
    Select-Object -First 12 Phase, Item |
    Format-Table -Wrap
