# run-bench.ps1 - one-command coding-bench entry (prep -> run xN -> score -> optional compare)
# Runs real-LLM coding tasks against frozen mother fixtures. NOT a CI gate - manual, cost-bearing
# (each slot is a real agent run; see README for the cost warning).
# NOTE: keep this file ASCII-only (PowerShell 5.1 reads no-BOM files as ANSI).
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File run-bench.ps1 -Tasks B-YAMLRUST                    # smoke
#   powershell -NoProfile -ExecutionPolicy Bypass -File run-bench.ps1                                      # full suite
#   powershell -NoProfile -ExecutionPolicy Bypass -File run-bench.ps1 -Tasks B-NUMINT -RunsPerTask 3 -Compare
#   powershell -NoProfile -ExecutionPolicy Bypass -File run-bench.ps1 -DryRun                              # plan only
param(
    [string[]]$Tasks,             # default: all 16 from fixtures/tasks
    [int]$RunsPerTask = 0,        # 0 = manifest shape (holdout x3, dev x1)
    [string]$Model,               # override card model (cards carry the baseline model otherwise)
    [string]$BotPath,             # default: target/release/nemesisbot.exe, else bin/bin_windows/nemesisbot.exe
    [string]$RuntimeHome,         # default: <repo>/.coding-bench/runtime (needs .nemesisbot/config.json - see README)
    [string]$Baseline = '',       # default: fixtures baseline.json when -Compare is set
    [switch]$Compare,
    [string]$RunId,
    [switch]$SkipPrep,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'
$BenchRoot = Split-Path -Parent $PSScriptRoot
$RepoRoot  = Split-Path -Parent (Split-Path -Parent $BenchRoot)
$BenchWork = Join-Path $RepoRoot '.coding-bench'
$MothersDir = Join-Path $BenchWork 'mothers'
$CacheDir   = Join-Path $BenchWork 'cache'

if (-not $BotPath) {
    foreach ($cand in @(
        (Join-Path $RepoRoot 'target\release\nemesisbot.exe'),
        (Join-Path $RepoRoot 'bin\bin_windows\nemesisbot.exe'))) {
        if (Test-Path $cand) { $BotPath = $cand; break }
    }
}
if (-not (Test-Path $BotPath)) { throw "nemesisbot.exe not found - build first or pass -BotPath" }
if (-not $RuntimeHome) { $RuntimeHome = Join-Path $BenchWork 'runtime' }
if (-not (Test-Path (Join-Path $RuntimeHome '.nemesisbot\config.json'))) {
    throw "runtime home missing config: $RuntimeHome\.nemesisbot\config.json - see README (model_list entry for the card model + protocol)"
}

# 1. mothers (idempotent, content-verified)
if (-not $SkipPrep) {
    & (Join-Path $PSScriptRoot 'prep-mothers.ps1') -MothersDir $MothersDir -RepoRoot $RepoRoot
}

# 2. fresh run dir + materialized tasks/evaluators (placeholders resolved per-machine)
if (-not $RunId) { $RunId = 'bench-' + (Get-Date).ToString('yyyyMMdd-HHmmss') }
$runDir = Join-Path $BenchWork $RunId
if (Test-Path $runDir) { throw "run dir exists: $runDir" }
New-Item -ItemType Directory -Path (Join-Path $runDir 'tasks') -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $runDir 'evaluators') -Force | Out-Null
Copy-Item -Recurse -Force (Join-Path $BenchRoot 'fixtures\evaluators\*') (Join-Path $runDir 'evaluators\')
$evalAbs = Join-Path $runDir 'evaluators'
# resolve {{EVAL}} in evaluator configs (Test-Acceptance runs their argv verbatim)
Get-ChildItem (Join-Path $runDir 'evaluators') -Recurse -Filter 'config.json' | ForEach-Object {
    $cfgRaw = Get-Content -Raw -Encoding UTF8 $_.FullName
    if ($cfgRaw -match '\{\{EVAL\}\}') {
        $cfgRaw -replace '\{\{EVAL\}\}', ($evalAbs -replace '\\', '/') | Set-Content -Encoding UTF8 $_.FullName
    }
}

$cards = @(Get-ChildItem (Join-Path $BenchRoot 'fixtures\tasks') -Filter '*.json')
$roster = @()
foreach ($cf in $cards) {
    # resolve placeholders FIRST, then parse: cache handling and warnings must speak the
    # same resolved paths the run copy will actually use
    $json = $raw = Get-Content -Raw -Encoding UTF8 $cf.FullName
    $json = $json -replace '\{\{MOTHERS\}\}', ($MothersDir -replace '\\', '/') `
                 -replace '\{\{CACHE\}\}',    ($CacheDir   -replace '\\', '/') `
                 -replace '\{\{EVAL\}\}',     ($evalAbs    -replace '\\', '/')
    $card = $json | ConvertFrom-Json
    if ($Tasks -and $Tasks -notcontains $card.task_id) { continue }
    # drop target_cache when the cache dir was never prewarmed (cold compile, still correct)
    if ($card.target_cache -and -not (Test-Path $card.target_cache)) {
        Write-Warning "target cache missing ($($card.target_cache)) - slot will cold-compile (slower, still correct)"
        $json = $json -replace ('"target_cache":\s*"' + [regex]::Escape($card.target_cache) + '"'), '"target_cache": null'
    }
    if ($Model) {
        $json = $json -replace '"model":\s*"[^"]*"', ('"model": "' + $Model + '"')
    }
    $json | Set-Content -Encoding UTF8 (Join-Path $runDir "tasks\$($card.task_id).json")
    $runs = if ($RunsPerTask -gt 0) { $RunsPerTask } elseif ($card.split -eq 'holdout') { 3 } else { 1 }
    $roster += [PSCustomObject]@{ task_id = $card.task_id; split = $card.split; runs = $runs }
}
if ($roster.Count -eq 0) { throw "no tasks matched (-Tasks $($Tasks -join ','))" }

Write-Host "run dir: $runDir"
Write-Host "bot:     $BotPath"
Write-Host "roster:  $($roster.Count) task(s), $(($roster | Measure-Object -Property runs -Sum).Sum) slot(s)"
if ($DryRun) { $roster | Format-Table -AutoSize | Out-String | Write-Host; return }

# 3. run + accept each slot
foreach ($t in $roster) {
    for ($i = 1; $i -le $t.runs; $i++) {
        Write-Host "== $($t.task_id) run$i =="
        & (Join-Path $PSScriptRoot 'Invoke-ValidationRun.ps1') -ExperimentDir $runDir -TaskId $t.task_id -RunNo $i -BotPath $BotPath -RuntimeHome $RuntimeHome
        & (Join-Path $PSScriptRoot 'Test-Acceptance.ps1')     -ExperimentDir $runDir -TaskId $t.task_id -RunNo $i
    }
}

# 4. scorecard
& (Join-Path $PSScriptRoot 'ConvertTo-Scorecard.ps1') -ExperimentDir $runDir

# 5. optional baseline compare (exit 1 on regression / coverage shrink)
if ($Compare) {
    $bl = if ($Baseline) { $Baseline } else { Join-Path $BenchRoot 'baseline.json' }
    & (Join-Path $PSScriptRoot 'compare-baseline.ps1') -Scorecard (Join-Path $runDir 'scorecard.csv') -Baseline $bl
    exit $LASTEXITCODE
}
