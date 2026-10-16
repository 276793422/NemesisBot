# Invoke-ValidationRun.ps1 v2 - drive one validation run (task x run_no) end to end
# Usage:
#   powershell -NoProfile -File Invoke-ValidationRun.ps1 -ExperimentDir <dir> -TaskId CODE-XX -RunNo 1 `
#     -BotPath <nemesisbot.exe> -RuntimeHome <dir>
# Reads $ExperimentDir\tasks\$TaskId.json. Produces $ExperimentDir\evidence\$TaskId\run$RunNo\{run.json,...}
#
# v2 changes (pilot P1 findings):
# - task text via stdin file ('run -'): Start-Process -ArgumentList (PS5.1) does not
#   quote array items, an inline task with spaces gets split by clap.
# - liveness by OS truth (Get-Process -Id poll), NOT the Start-Process Process object:
#   observed in pilot that the object reported HasExited=true + ExitCode=1 while the
#   process was alive and blocked in a tool call (run_checks compile, 30+ min).
# - true exit code via cmd batch echo (%ERRORLEVEL%) into exitcode.txt.
# - kill chain loud + verified: taskkill /T logged to kill.log, survival escalates to
#   per-child Stop-Process, still-alive after that = hard throw.
param(
    [Parameter(Mandatory=$true)][string]$ExperimentDir,
    [Parameter(Mandatory=$true)][string]$TaskId,
    [Parameter(Mandatory=$true)][int]$RunNo,
    [Parameter(Mandatory=$true)][string]$BotPath,
    [Parameter(Mandatory=$true)][string]$RuntimeHome,
    [string]$Arm  # override card.arm (model arm label, e.g. main)
)
$ErrorActionPreference = 'Stop'
$cardPath = Join-Path $ExperimentDir "tasks\$TaskId.json"
if (-not (Test-Path $cardPath)) { throw "task card missing: $cardPath" }
$card = Get-Content -Raw -Encoding UTF8 $cardPath | ConvertFrom-Json

$evDir = Join-Path $ExperimentDir "evidence\$TaskId\run$RunNo"
if (Test-Path $evDir) { throw "evidence dir already exists (slot must be fresh): $evDir" }
New-Item -ItemType Directory -Path $evDir -Force | Out-Null

# 1. fresh run copy (+ junctions per card)
$copy = Join-Path $ExperimentDir "runs\$TaskId-run$RunNo"
& (Join-Path $PSScriptRoot 'New-RunCopy.ps1') -MotherDir $card.mother -DestDir $copy `
    -TargetCache $card.target_cache -NodeModulesCache $card.node_modules_cache |
    Set-Content -Encoding UTF8 (Join-Path $evDir 'runcopy.json')

# 2. env fingerprint BEFORE (shared-device contention evidence)
& (Join-Path $PSScriptRoot 'Dump-ExperimentEnv.ps1') -OutFile (Join-Path $evDir 'env_before.json') | Out-Null

# 2.5 R8: freeze base HEAD right after the run copy exists, BEFORE the agent
# starts. The agent may commit inside the run copy — acceptance must diff the
# full change set against the pre-run base, not against whatever HEAD the
# agent left behind (diff-vs-HEAD would silently under-report committed edits).
$baseSha = (git -C $copy rev-parse HEAD)

# 3. run the bot (cwd = runtime home so --local pins home under it)
$startUtc = (Get-Date).ToUniversalTime().ToString('o')
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$stdoutFile = Join-Path $evDir 'stdout.ndjson'
$stderrFile = Join-Path $evDir 'stderr.log'
$taskFile = Join-Path $evDir 'task.txt'
[IO.File]::WriteAllText($taskFile, $card.user_request, [Text.UTF8Encoding]::new($false))

# batch wrapper: carries the full arg vector (quoted here, immune to PS arg-splitting),
# redirects stdio, and echoes the true exit code (PS Process objects have lied about it).
# NOTE: space between %ERRORLEVEL% and > — "echo %E%>f" with E ending in a digit 1-9
# would be parsed as a handle redirect (classic batch trap).
$batPath = Join-Path $evDir 'run_slot.cmd'
$exitFile = Join-Path $evDir 'exitcode.txt'
$bat = "@echo off`r`n" +
       "`"$BotPath`" --local run - --workspace `"$copy`" --mode build --model $($card.model) " +
       "--max-turns $($card.max_turns) --format json < `"$taskFile`" " +
       "> `"$stdoutFile`" 2> `"$stderrFile`"`r`n" +
       "echo %ERRORLEVEL% > `"$exitFile`"`r`n"
[IO.File]::WriteAllText($batPath, $bat, [Text.UTF8Encoding]::new($false))

$p = Start-Process -FilePath 'cmd.exe' -ArgumentList '/c', "`"$batPath`"" -WorkingDirectory $RuntimeHome `
        -NoNewWindow -PassThru

# liveness poll: OS truth only (fresh Get-Process by PID each tick)
$limitSec = [int]$card.time_limit_minutes * 60
$deadline = $sw.Elapsed.TotalSeconds + $limitSec
$timedOut = $false
while ($true) {
    Start-Sleep -Seconds 5
    $alive = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
    if (-not $alive) { break }
    if ($sw.Elapsed.TotalSeconds -gt $deadline) { $timedOut = $true; break }
}

if ($timedOut) {
    # loud, verified tree kill
    $killLog = Join-Path $evDir 'kill.log'
    $kt = & taskkill /F /T /PID $p.Id 2>&1 | ForEach-Object { "$_" }
    $kt | Set-Content -Encoding UTF8 $killLog
    Start-Sleep -Seconds 3
    if (Get-Process -Id $p.Id -ErrorAction SilentlyContinue) {
        # escalate: kill direct children then the root
        Get-CimInstance Win32_Process -Filter "ParentProcessId=$($p.Id)" | ForEach-Object {
            Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
            "escalated stop: $($_.ProcessId) $($_.Name)" | Add-Content -Encoding UTF8 $killLog
        }
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        "escalated stop root: $($p.Id)" | Add-Content -Encoding UTF8 $killLog
        Start-Sleep -Seconds 2
    }
    if (Get-Process -Id $p.Id -ErrorAction SilentlyContinue) {
        "root $($p.Id) SURVIVED all kill attempts" | Add-Content -Encoding UTF8 $killLog
        throw "run process $($p.Id) survived taskkill and escalation; slot aborted"
    }
}
$sw.Stop()
$exitCode = if ($timedOut) { -1 } else {
    if (Test-Path $exitFile) { [int](Get-Content $exitFile -ErrorAction SilentlyContinue | Select-Object -First 1) } else { $null }
}
$endUtc = (Get-Date).ToUniversalTime().ToString('o')

# 4. post-run capture: full change set vs frozen base + untracked (completeness
# evidence, BEFORE acceptance injects tests). R8: diff against $baseSha (covers
# committed + staged + unstaged) instead of bare `git diff` (working tree vs
# index only — committed agent edits were invisible to both this capture and
# acceptance's tracked-diff scan).
$diffFile = Join-Path $evDir 'workspace.diff'
git -C $copy diff $baseSha | Set-Content -Encoding UTF8 $diffFile
$dirty = @(git -C $copy status --porcelain)
$dirty | Set-Content -Encoding UTF8 (Join-Path $evDir 'dirty.txt')
& (Join-Path $PSScriptRoot 'Dump-ExperimentEnv.ps1') -OutFile (Join-Path $evDir 'env_after.json') | Out-Null

# 5. run record
$runStatus = if ($timedOut) { 'timeout' } elseif ($null -eq $exitCode) { 'unknown_exit' } elseif ($exitCode -eq 0) { 'completed' } else { 'error_exit' }
$codeSha = (git -C $card.mother log -1 --format=%H)
$armLabel = if ($Arm) { $Arm } else { $card.arm }
[PSCustomObject]@{
    experiment_id = (Split-Path -Leaf $ExperimentDir)
    task_id       = $TaskId
    split         = $card.split
    arm           = $armLabel
    run_no        = $RunNo
    code_sha      = $codeSha
    model_id      = $card.model
    start_utc     = $startUtc
    end_utc       = $endUtc
    wall_seconds  = [int]$sw.Elapsed.TotalSeconds
    timed_out     = $timedOut
    exit_code     = $exitCode
    run_status    = $runStatus
    run_copy      = $copy
    base_sha      = $baseSha
    head_after    = (git -C $copy rev-parse HEAD)
    dirty_files   = $dirty
    cost_usd      = $null
    notes         = ''
} | ConvertTo-Json -Depth 3 | Set-Content -Encoding UTF8 (Join-Path $evDir 'run.json')
Write-Output "run done: $TaskId#$RunNo status=$runStatus exit=$exitCode wall=$([int]$sw.Elapsed.TotalSeconds)s"
