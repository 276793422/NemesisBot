# Test-Acceptance.ps1 - completeness + hidden-machine acceptance for one run slot
# Reads $ExperimentDir\evaluators\$TaskId\config.json:
#   { "inject": [{"src":"...","dst":"..."}], "commands": [{"argv":[...],"cwd":"web"}],
#     "fail_patterns": ["..."], "runtime_paths": ["logs/","memory/",...],
#     "expect_files": [{"path":"logs/x.csv","contains":["header,line"]}], "require_diff": true }
# Writes acceptance.json next to run.json in the evidence dir.
param(
    [Parameter(Mandatory=$true)][string]$ExperimentDir,
    [Parameter(Mandatory=$true)][string]$TaskId,
    [Parameter(Mandatory=$true)][int]$RunNo
)
$ErrorActionPreference = 'Stop'
# PS5.1 trap: @($cfg.missing_key) yields @($null) — Count=1 null element. Downstream:
# Join-Path crashes (expect_files), "-like *$null*" matches everything (runtime_paths =>
# completeness always true; fail_patterns => -match null matches anything). Filter nulls
# at every config-array consumption point via Arr().
function Arr($v) { @($v | Where-Object { $null -ne $_ }) }
$evDir  = Join-Path $ExperimentDir "evidence\$TaskId\run$RunNo"
$run    = Get-Content -Raw -Encoding UTF8 (Join-Path $evDir 'run.json') | ConvertFrom-Json
$copy   = $run.run_copy
$cfg    = Get-Content -Raw -Encoding UTF8 (Join-Path $ExperimentDir "evaluators\$TaskId\config.json") | ConvertFrom-Json

# --- 1. completeness: untracked non-runtime files only (agent's tracked edits are the task itself)
$runtime = @(Arr $cfg.runtime_paths) + @('target/', '.git/', 'config/', 'sessions/', 'logs/', 'memory/', 'uploads/')
# Global baseline = bot-owned runtime dirs on ANY --workspace run (manifest evidence_policy:
# logs/ holds checkpoints.git shadow repo / turn_wal / spill / lease — appeared in B-repo
# copies too, pilot UUIDV7 run1; memory/, uploads/ likewise bot-managed). Per-card
# runtime_paths add task-specific entries on top. Agent edits to shipped files appear as
# tracked diff, never as untracked files inside these dirs.
$card = Get-Content -Raw -Encoding UTF8 (Join-Path $ExperimentDir "tasks\$TaskId.json") | ConvertFrom-Json
$allowed = Arr $card.allowed_paths
$diffPaths = @(git -C $copy diff --name-only)
# Untracked != junk: a NEW source file the agent created is a legitimate deliverable
# (pilot UIALIGN run1: web/src/composables/useUserBubbleAlign.ts untracked, real work).
# stray = untracked non-runtime AND (outside allowed_paths when declared, else not
# sharing a >=2-segment path prefix with any tracked-diff path => repo-root litter).
$stray = @()
foreach ($line in @(git -C $copy status --porcelain -uall)) {
    if ($line -notlike '??*') { continue }
    $f = $line.Substring(3).Trim()
    $isRuntime = $false
    foreach ($r in $runtime) { if ($f -like "$r*") { $isRuntime = $true; break } }
    if ($isRuntime) { continue }
    $isDeliverable = $false
    if ($allowed.Count -gt 0) {
        foreach ($p in $allowed) { if ($f -like "$p*") { $isDeliverable = $true; break } }
    } elseif ($diffPaths.Count -gt 0) {
        $segs = ($f -replace '\\', '/') -split '/'
        if ($segs.Count -ge 2) {
            $pref = ($segs[0..1] -join '/')
            foreach ($d in $diffPaths) {
                $dn = $d -replace '\\', '/'
                if ($dn -like "$pref/*" -or $dn -eq $pref) { $isDeliverable = $true; break }
            }
        }
    }
    if (-not $isDeliverable) { $stray += $f }
}
$complete = ($stray.Count -eq 0)
# allowed_paths guard on the tracked diff (diff paths must live under allowed prefixes, if declared)
if ($cfg.require_diff -and $diffPaths.Count -eq 0) { $complete = $false }
$outOfBounds = @()
if ($allowed.Count -gt 0) {
    foreach ($f in $diffPaths) {
        $ok = $false
        foreach ($p in $allowed) { if ($f -like "$p*") { $ok = $true; break } }
        if (-not $ok) { $outOfBounds += $f }
    }
}

# --- 2. inject hidden acceptance tests, then run pre-registered commands
$injectLog = @()
foreach ($inj in (Arr $cfg.inject)) {
    $src = Join-Path $ExperimentDir "evaluators\$TaskId\$($inj.src)"
    $dst = Join-Path $copy $inj.dst
    $dstDir = Split-Path -Parent $dst
    if (-not (Test-Path $dstDir)) { New-Item -ItemType Directory -Path $dstDir -Force | Out-Null }
    Copy-Item -Force $src $dst
    # freshness: mtime must be newer than any cargo fingerprint build time (stale-binary guard)
    (Get-Item $dst).LastWriteTime = Get-Date
    $injectLog += $inj.dst
}
$env:CARGO_TARGET_DIR = Join-Path $copy 'target'
$cmdsOk = $true
$cmdResults = @()
foreach ($c in (Arr $cfg.commands)) {
    $workDir = $copy
    if ($c.cwd) { $workDir = Join-Path $copy $c.cwd }
    Push-Location $workDir
    # PS5.1 trap: with EAP=Stop, native stderr lines via 2>&1 are ErrorRecords and the
    # FIRST one throws (observed: cargo "Compiling ..." killed the run before any result).
    # Temporarily drop to Continue around the native call only.
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $c.argv[0] @($c.argv | Select-Object -Skip 1) 2>&1 | ForEach-Object { "$_" }
        $rc = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prevEap
        Pop-Location
    }
    $tail = ($out | Select-Object -Last 25) -join "`n"
    $failed = ($rc -ne 0)
    if (-not $failed) {
        foreach ($pat in (Arr $cfg.fail_patterns)) {
            if ($tail -match $pat) { $failed = $true; break }
        }
    }
    if ($failed) { $cmdsOk = $false }
    $cmdResults += [PSCustomObject]@{ argv=($c.argv -join ' '); rc=$rc; failed=$failed; tail=$tail }
    $tail | Set-Content -Encoding UTF8 (Join-Path $evDir ('accept_' + [IO.Path]::GetFileNameWithoutExtension($c.argv[-1]) + '.log'))
}
# expect_files: post-run artifact checks (path relative to run copy, must contain each string)
$filesOk = $true
$fileResults = @()
foreach ($f in (Arr $cfg.expect_files)) {
    $fp = Join-Path $copy $f.path
    $ok = Test-Path $fp
    if ($ok) {
        $content = Get-Content -Raw -Encoding UTF8 $fp
        foreach ($needle in @($f.contains)) { if ($content -notlike "*$needle*") { $ok = $false } }
    }
    if (-not $ok) { $filesOk = $false }
    $fileResults += [PSCustomObject]@{ path=$f.path; ok=$ok }
}
$verdict = 'pass'
if (-not $complete) { $verdict = 'fail' }
if (-not $cmdsOk)  { $verdict = 'fail' }
if (-not $filesOk) { $verdict = 'fail' }
if ($outOfBounds.Count -gt 0) { $verdict = 'fail' }
[PSCustomObject]@{
    task_id = $TaskId; run_no = $RunNo
    acceptance = $verdict
    complete = $complete
    stray_untracked = $stray
    out_of_bounds = $outOfBounds
    commands_ok = $cmdsOk
    files_ok = $filesOk
    file_results = $fileResults
    injected = $injectLog
    command_results = $cmdResults
} | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 (Join-Path $evDir 'acceptance.json')
Write-Output "acceptance: $TaskId#$RunNo => $verdict (complete=$complete cmds_ok=$cmdsOk files_ok=$filesOk oob=$($outOfBounds.Count))"
