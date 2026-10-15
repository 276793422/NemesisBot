# ConvertTo-Scorecard.ps1 - merge evidence dirs into the frozen-header scorecard + metric summary
# Usage: powershell -NoProfile -File ConvertTo-Scorecard.ps1 -ExperimentDir <dir>
param([Parameter(Mandatory=$true)][string]$ExperimentDir)
$ErrorActionPreference = 'Stop'
$rows = @()
Get-ChildItem -Path (Join-Path $ExperimentDir 'evidence') -Directory | ForEach-Object { $_.FullName } | ForEach-Object {
    $taskId = Split-Path -Leaf $_
    Get-ChildItem -Path $_ -Directory | ForEach-Object {
        $runJsonPath = Join-Path $_.FullName 'run.json'
        if (-not (Test-Path $runJsonPath)) { return }
        $r = Get-Content -Raw -Encoding UTF8 $runJsonPath | ConvertFrom-Json
        $acc = $null
        $accPath = Join-Path $_.FullName 'acceptance.json'
        if (Test-Path $accPath) { $acc = (Get-Content -Raw -Encoding UTF8 $accPath | ConvertFrom-Json).acceptance }
        $rows += [PSCustomObject]@{
            experiment_id = $r.experiment_id; task_id = $r.task_id; split = $r.split; arm = $r.arm
            run_no = $r.run_no; code_sha = $r.code_sha; fixture_sha = $r.code_sha
            model_id = $r.model_id; model_version = ''; config_sha = ''
            start_utc = $r.start_utc; end_utc = $r.end_utc; wall_seconds = $r.wall_seconds
            model_cost_usd = $r.cost_usd; approval_count = ''; approval_seconds = ''
            rescue_count = 0; exit_code = $r.exit_code; run_status = $r.run_status
            acceptance = $acc; category = ''; evidence_dir = $_.FullName
        }
    }
}
$csvPath = Join-Path $ExperimentDir 'scorecard.csv'
$rows | Export-Csv -NoTypeInformation -Encoding UTF8 -Path $csvPath

# ---- summary (descriptive + frozen-gate arithmetic) ----
$slots = $rows.Count
$pass = @($rows | Where-Object { $_.acceptance -eq 'pass' }).Count
$holdout = @($rows | Where-Object { $_.split -eq 'holdout' })
$holdoutTasks = @($holdout | ForEach-Object { $_.task_id } | Sort-Object -Unique)
$allPass = 0
foreach ($t in $holdoutTasks) {
    $rs = @($holdout | Where-Object { $_.task_id -eq $t })
    if ($rs.Count -ge 3 -and @($rs | Where-Object { $_.acceptance -eq 'pass' }).Count -eq $rs.Count) { $allPass++ }
}
$stablePct = if ($holdoutTasks.Count -gt 0) { [math]::Round(100.0 * $allPass / $holdoutTasks.Count, 1) } else { $null }
$cost = ($rows | ForEach-Object { if ($_.model_cost_usd) { [double]$_.model_cost_usd } } | Measure-Object -Sum).Sum
$summary = @"
slots=$slots pass=$pass holdout_tasks=$($holdoutTasks.Count) stable_all3_tasks=$allPass stable_pct=$stablePct cost_total=$cost
GATE effect(>=75): $(if ($null -ne $stablePct -and $stablePct -ge 75) {'PASS'} else {'PENDING/FAIL'})
GATE rescue(<=1) & timeout(<=1): see scorecard rows (run_status=timeout / rescue_count)
GATE budget(<=10 USD): cost_total=$cost
"@
$summary | Set-Content -Encoding UTF8 (Join-Path $ExperimentDir 'summary.txt')
Write-Output $summary
Write-Output "scorecard: $csvPath"
