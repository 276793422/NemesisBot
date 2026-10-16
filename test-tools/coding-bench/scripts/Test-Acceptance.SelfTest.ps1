# Test-Acceptance.SelfTest.ps1 - R8 correctness self-test for Test-Acceptance.ps1
# Builds a synthetic git "run copy" per case, applies one change class, runs the
# acceptance scanner, and asserts the verdict. Six change classes x
# (inside / outside allowed_paths):
#   1. unstaged edit     (tracked file, working tree only)
#   2. staged edit       (index only)
#   3. committed edit    (agent committed inside the run copy)
#   4. committed-then-modified (commit + further working-tree edit)
#   5. untracked file    (new file, never added)
#   6. deletion          (tracked file removed)
# The R8 bugs these pin: bare `git diff --name-only` missed staged+committed
# changes (out-of-bounds guard blind); `-notlike '??*'` matched every porcelain
# line (dead filter); `-like "$p*"` had no directory boundary ("src" accepted
# "srcfoo/x").
#
# Usage: powershell -NoProfile -File Test-Acceptance.SelfTest.ps1
# Exit 0 = all cases pass; throws on the first failure with the case label.
param()
$ErrorActionPreference = 'Stop'

$script:here = $PSScriptRoot
$script:pass = 0
$script:fail = 0

function New-CaseRepo {
    # synthetic run copy: one tracked file, one clean commit, git identity local
    param([string]$Dir)
    New-Item -ItemType Directory -Path $Dir -Force | Out-Null
    git -C $Dir init -q | Out-Null
    git -C $Dir config user.email selftest@example.invalid
    git -C $Dir config user.name selftest
    Set-Content -Encoding UTF8 (Join-Path $Dir 'src.txt') 'base'
    git -C $Dir add .
    git -C $Dir commit -q -m base | Out-Null
}

function Invoke-Case {
    # run Test-Acceptance.ps1 against a hand-built experiment dir; return acceptance.json
    param([string]$Label, [string]$ExpDir, [string]$TaskId, [string]$AllowedJson, [string]$Expect)
    $taskDir = Join-Path $ExpDir "tasks"
    New-Item -ItemType Directory -Path $taskDir -Force | Out-Null
    # PS5.1 trap: @(ConvertFrom-Json '["src.txt"]') wraps the deserialized array in a
    # PSObject collection adapter; ConvertTo-Json then emits {"value":[...],"Count":1}
    # and allowed_paths becomes an object — the prefix match stringifies it and never
    # matches. Embed the JSON array literal directly instead.
    "{`"allowed_paths`": $AllowedJson}" |
        Set-Content -Encoding UTF8 (Join-Path $taskDir "$TaskId.json")
    $evalDir = Join-Path $ExpDir "evaluators\$TaskId"
    New-Item -ItemType Directory -Path $evalDir -Force | Out-Null
    @{ inject = @(); commands = @(); fail_patterns = @(); runtime_paths = @(); expect_files = @(); require_diff = $false } |
        ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $evalDir 'config.json')
    $runDir = Join-Path $ExpDir "evidence\$TaskId\run1"
    New-Item -ItemType Directory -Path $runDir -Force | Out-Null
    if (-not (Test-Path (Join-Path $runDir 'run.json'))) {
        # caller normally pre-writes run.json with the frozen base_sha; only
        # synthesize a minimal one when absent (standalone ad-hoc use)
        @{ run_copy = (Get-Item $ExpDir).FullName + "\copy"; run_status = 'completed' } |
            ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $runDir 'run.json')
    }

    $out = & powershell -NoProfile -File (Join-Path $script:here 'Test-Acceptance.ps1') `
        -ExperimentDir $ExpDir -TaskId $TaskId -RunNo 1 2>&1
    $acc = Get-Content -Raw -Encoding UTF8 (Join-Path $runDir 'acceptance.json') | ConvertFrom-Json
    if ($acc.acceptance -ne $Expect) {
        $script:fail++
        throw "[$Label] expected acceptance=$Expect got=$($acc.acceptance) " +
              "(oob=$($acc.out_of_bounds -join ',') stray=$($acc.stray_untracked -join ','))`n$out"
    }
    $script:pass++
    Write-Output "  ok: $Label => $($acc.acceptance)"
    return $acc
}

$root = Join-Path ([IO.Path]::GetTempPath()) ("acctest-" + [IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $root -Force | Out-Null
Write-Output "self-test root: $root"

# --- six change classes, target inside allowed (verdict pass) and outside (fail) ---
$cases = @(
    @{ name = 'unstaged-inside';  setup = {
        Set-Content -Encoding UTF8 (Join-Path $c 'src.txt') 'edited' }
       allowed = '["src.txt"]'; expect = 'pass' }
    @{ name = 'unstaged-outside'; setup = {
        Set-Content -Encoding UTF8 (Join-Path $c 'other.txt') 'edited' }
       allowed = '["src.txt"]'; expect = 'fail' }
    @{ name = 'staged-inside';    setup = {
        Set-Content -Encoding UTF8 (Join-Path $c 'src.txt') 'staged'; git -C $c add . }
       allowed = '["src.txt"]'; expect = 'pass' }
    @{ name = 'staged-outside';   setup = {
        Set-Content -Encoding UTF8 (Join-Path $c 'other.txt') 'staged'; git -C $c add . }
       allowed = '["src.txt"]'; expect = 'fail' }
    @{ name = 'committed-inside'; setup = {
        Set-Content -Encoding UTF8 (Join-Path $c 'src.txt') 'committed'
        git -C $c add .; git -C $c commit -q -m agent | Out-Null }
       allowed = '["src.txt"]'; expect = 'pass' }
    @{ name = 'committed-outside'; setup = {
        Set-Content -Encoding UTF8 (Join-Path $c 'other.txt') 'committed'
        git -C $c add .; git -C $c commit -q -m agent | Out-Null }
       allowed = '["src.txt"]'; expect = 'fail' }
    @{ name = 'commit-then-edit-inside'; setup = {
        Set-Content -Encoding UTF8 (Join-Path $c 'src.txt') 'committed'
        git -C $c add .; git -C $c commit -q -m agent | Out-Null
        Set-Content -Encoding UTF8 (Join-Path $c 'src.txt') 'committed+edited' }
       allowed = '["src.txt"]'; expect = 'pass' }
    @{ name = 'commit-then-edit-outside'; setup = {
        Set-Content -Encoding UTF8 (Join-Path $c 'other.txt') 'committed'
        git -C $c add .; git -C $c commit -q -m agent | Out-Null
        Set-Content -Encoding UTF8 (Join-Path $c 'other.txt') 'committed+edited' }
       allowed = '["src.txt"]'; expect = 'fail' }
    @{ name = 'untracked-inside'; setup = {
        Set-Content -Encoding UTF8 (Join-Path $c 'new.txt') 'new' }
       allowed = '["new.txt"]'; expect = 'pass' }
    @{ name = 'untracked-outside'; setup = {
        Set-Content -Encoding UTF8 (Join-Path $c 'litter.txt') 'new' }
       allowed = '["src.txt"]'; expect = 'fail' }
    @{ name = 'deleted-inside';   setup = {
        Remove-Item (Join-Path $c 'src.txt') -Force }
       allowed = '["src.txt"]'; expect = 'pass' }
    @{ name = 'deleted-outside';  setup = {
        git -C $c mv src.txt other.txt | Out-Null }
       allowed = '["src.txt"]'; expect = 'fail' }
)

foreach ($case in $cases) {
    $exp = Join-Path $root $case.name
    $copy = Join-Path $exp 'copy'
    New-Item -ItemType Directory -Path $exp -Force | Out-Null
    New-CaseRepo -Dir $copy
    $c = $copy   # setup closures close over $c
    & $case.setup
    # run.json must carry the FROZEN base (pre-change HEAD) — the driver records
    # it before the agent runs; here the repo is already at the post-change state,
    # so re-freeze the original base commit explicitly (first commit sha).
    $base = (git -C $copy rev-list --max-parents=0 HEAD)
    $runDir = Join-Path $exp "evidence\selftest\run1"
    New-Item -ItemType Directory -Path $runDir -Force | Out-Null
    @{ run_copy = $copy; run_status = 'completed'; base_sha = $base } |
        ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $runDir 'run.json')
    Invoke-Case -Label $case.name -ExpDir $exp -TaskId 'selftest' `
        -AllowedJson $case.allowed -Expect $case.expect | Out-Null
}

# --- boundary: allowed "src" must NOT accept "srcfoo/x" (R8 directory edge) ---
$exp = Join-Path $root 'boundary-srcfoo'
$copy = Join-Path $exp 'copy'
New-Item -ItemType Directory -Path (Join-Path $copy 'srcfoo') -Force | Out-Null
New-CaseRepo -Dir $copy
Set-Content -Encoding UTF8 (Join-Path (Join-Path $copy 'srcfoo') 'x.txt') 'evil'
$base = (git -C $copy rev-list --max-parents=0 HEAD)
$runDir = Join-Path $exp "evidence\selftest\run1"
New-Item -ItemType Directory -Path $runDir -Force | Out-Null
@{ run_copy = $copy; run_status = 'completed'; base_sha = $base } |
    ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $runDir 'run.json')
Invoke-Case -Label 'boundary-srcfoo' -ExpDir $exp -TaskId 'selftest' `
    -AllowedJson '["src"]' -Expect 'fail' | Out-Null

# --- boundary positive: allowed "src/" (trailing slash) accepts src/a.txt ---
$exp = Join-Path $root 'boundary-slash'
$copy = Join-Path $exp 'copy'
New-Item -ItemType Directory -Path (Join-Path $copy 'src') -Force | Out-Null
New-CaseRepo -Dir $copy
Set-Content -Encoding UTF8 (Join-Path (Join-Path $copy 'src') 'a.txt') 'fine'
$base = (git -C $copy rev-list --max-parents=0 HEAD)
$runDir = Join-Path $exp "evidence\selftest\run1"
New-Item -ItemType Directory -Path $runDir -Force | Out-Null
@{ run_copy = $copy; run_status = 'completed'; base_sha = $base } |
    ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $runDir 'run.json')
Invoke-Case -Label 'boundary-slash' -ExpDir $exp -TaskId 'selftest' `
    -AllowedJson '["src/"]' -Expect 'pass' | Out-Null

Write-Output "self-test done: pass=$($script:pass) fail=$($script:fail)"
if ($script:fail -gt 0) { throw "$($script:fail) case(s) failed" }
Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
exit 0
