# compare-baseline.ps1 - baseline regression gate for coding-bench scorecards
# Semantics (user-approved, mirrors agent-bench --compare): per-task pass_rate regression
# and task-coverage shrink gate the run (exit 1); slot-count shrink and new tasks are
# informational only. Latency never gates (cross-machine noise).
# NOTE: keep this file ASCII-only (PowerShell 5.1 reads no-BOM files as ANSI).
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File compare-baseline.ps1 -Scorecard <csv> -Baseline <json>
param(
    [Parameter(Mandatory=$true)][string]$Scorecard,
    [Parameter(Mandatory=$true)][string]$Baseline
)
$ErrorActionPreference = 'Stop'
$bl = Get-Content -Raw -Encoding UTF8 $Baseline | ConvertFrom-Json
$rows = @(Import-Csv $Scorecard)

$cur = @{}
foreach ($r in $rows) {
    if (-not $cur.ContainsKey($r.task_id)) {
        $cur[$r.task_id] = @{ pass = 0; slots = 0 }
    }
    $cur[$r.task_id].slots++
    if ($r.acceptance -eq 'pass') { $cur[$r.task_id].pass++ }
}

$regressions = @()
$missing = @()
$warnings = @()
$improved = @()
foreach ($prop in $bl.tasks.PSObject.Properties) {
    $tid = $prop.Name
    $b = $prop.Value
    if (-not $cur.ContainsKey($tid)) { $missing += $tid; continue }
    $c = $cur[$tid]
    $bRate = if ($b.slots -gt 0) { $b.pass / $b.slots } else { 0 }
    $cRate = if ($c.slots -gt 0) { $c.pass / $c.slots } else { 0 }
    if ($cRate -lt $bRate) {
        $regressions += "$tid pass_rate $($b.pass)/$($b.slots) -> $($c.pass)/$($c.slots)"
    } elseif ($b.pass -eq 0 -and $c.pass -gt 0) {
        $improved += $tid
    }
    if ($c.slots -lt $b.slots) { $warnings += "$tid slots $($b.slots) -> $($c.slots) (informational)" }
}
$newTasks = @($cur.Keys | Where-Object { $bl.tasks.PSObject.Properties.Name -notcontains $_ })

Write-Host "baseline: $($bl.baseline_id) (model $($bl.model))"
foreach ($r in $regressions) { Write-Host "REGRESSION  $r" }
foreach ($m in $missing)     { Write-Host "MISSING     $m (in baseline, absent from this run)" }
foreach ($w in $warnings)    { Write-Host "note        $w" }
foreach ($i in $improved)    { Write-Host "improved    $i" }
foreach ($n in $newTasks)    { Write-Host "new         $n (not in baseline, informational)" }

if ($regressions.Count -gt 0 -or $missing.Count -gt 0) {
    Write-Host "COMPARE: FAIL ($($regressions.Count) regression(s), $($missing.Count) missing task(s))"
    exit 1
}
Write-Host "COMPARE: PASS (no pass_rate regression, full task coverage)"
exit 0
