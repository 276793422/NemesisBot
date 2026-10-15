# prep-mothers.ps1 - materialize bench mother fixtures from pins (idempotent, content-verified)
# Reads fixtures/mothers.json (single source of truth). Two source families:
#   github    = codeload tarball download at frozen SHA (lock-version + verify-by-content)
#   main-repo = `git archive <pin>` from this repository's own history
# After extraction: optional fixture patch (git apply), git init + snapshot commit,
# then tree fingerprint verification against tree_sha256 (content-addressed; mismatch = loud fail).
# Mothers land in .coding-bench/mothers/ (gitignored) - the repo stores the spec, never the bytes.
# NOTE: keep this file ASCII-only (PowerShell 5.1 reads no-BOM files as ANSI).
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File prep-mothers.ps1                     # all mothers
#   powershell -NoProfile -ExecutionPolicy Bypass -File prep-mothers.ps1 -Only B-YAMLRUST    # subset
#   powershell -NoProfile -ExecutionPolicy Bypass -File prep-mothers.ps1 -MothersDir <dir> -RepoRoot <dir>
param(
    [string]$MothersDir,
    [string]$RepoRoot,
    [string[]]$Only
)
$ErrorActionPreference = 'Stop'
$BenchRoot = Split-Path -Parent $PSScriptRoot                      # test-tools/coding-bench
if (-not $RepoRoot)   { $RepoRoot   = (Split-Path -Parent (Split-Path -Parent $BenchRoot)) }  # repo root
if (-not $MothersDir) { $MothersDir = Join-Path $RepoRoot '.coding-bench\mothers' }
$spec = Get-Content -Raw -Encoding UTF8 (Join-Path $BenchRoot 'fixtures\mothers.json') | ConvertFrom-Json

# Windows bsdtar by full path: a GNU tar (e.g. MSYS) earlier in PATH treats "C:/..." as a
# remote host spec ("Cannot connect to C:"). System32\tar.exe is present since Win10 1803.
$TAR = Join-Path $env:SystemRoot 'System32\tar.exe'
if (-not (Test-Path $TAR)) { throw "windows bsdtar not found: $TAR" }

function Get-TreeFingerprint($dir) {
    # sha256 of `git ls-files -s` output - content-addressed (blob hashes depend only on
    # file contents + modes), independent of git version / commit timestamps.
    # Line endings normalized to LF explicitly (machine-independent; byte-identical to
    # `git ls-files -s | sha256sum` on any platform).
    $lines = @(git -C $dir ls-files -s)
    $raw = ($lines -join "`n")
    if ($raw) { $raw += "`n" }
    $bytes = [Text.UTF8Encoding]::new($false).GetBytes($raw)
    $sha = [Security.Cryptography.SHA256]::Create()
    ([BitConverter]::ToString($sha.ComputeHash($bytes)) -replace '-', '').ToLower()
}

New-Item -ItemType Directory -Path $MothersDir -Force | Out-Null
$fail = 0
foreach ($prop in $spec.mothers.PSObject.Properties) {
    $name = $prop.Name
    if ($Only -and $Only -notcontains $name) { continue }
    $m = $prop.Value
    $dest = Join-Path $MothersDir $name
    Write-Host "== $name =="
    if (Test-Path $dest) {
        $fp = Get-TreeFingerprint $dest
        if ($fp -eq $m.tree_sha256) { Write-Host "   exists, fingerprint OK -> skip"; continue }
        Write-Warning "   exists but fingerprint MISMATCH ($fp) - delete $dest and re-run prep"
        $fail++
        continue
    }
    New-Item -ItemType Directory -Path $dest -Force | Out-Null
    try {
        if ($m.source -eq 'github') {
            # repo owner/name from the URL (https://github.com/<o>/<r>)
            $parts = $m.repo -split '/'
            $owner = $parts[-2]; $repo = $parts[-1]
            $tgz = Join-Path $env:TEMP "codingbench_$name.tgz"
            $url = "https://codeload.github.com/$owner/$repo/tar.gz/$($m.pin)"
            & curl.exe -sSL -o $tgz $url
            if ($LASTEXITCODE -ne 0) { throw "download failed: $url (rc=$LASTEXITCODE)" }
            & $TAR -xzf $tgz -C $dest --strip-components=1
            if ($LASTEXITCODE -ne 0) { throw "tar extract failed rc=$LASTEXITCODE" }
            Remove-Item $tgz -Force
        } elseif ($m.source -eq 'main-repo') {
            # No binary pipes through PowerShell 5.1: native-to-native pipelines are
            # decoded/re-encoded as text, corrupting the tar stream ("Damaged tar
            # archive"). Write the archive to a file, extract from the file.
            $tarFile = Join-Path $env:TEMP "codingbench_$name.tar"
            git -C $RepoRoot archive -o $tarFile $m.pin
            if ($LASTEXITCODE -ne 0) { throw "git archive $($m.pin) failed (commit present in this repo's history?)" }
            & $TAR -xf $tarFile -C $dest
            if ($LASTEXITCODE -ne 0) { throw "tar extract failed rc=$LASTEXITCODE" }
            Remove-Item $tarFile -Force
        } else { throw "unknown source family: $($m.source)" }

        if ($m.patch) {
            $patchPath = Join-Path $BenchRoot "fixtures\$($m.patch)"
            git -C $dest init -q -b main
            git -C $dest apply --whitespace=nowarn $patchPath
            if ($LASTEXITCODE -ne 0) { throw "patch apply failed: $($m.patch)" }
        } else {
            git -C $dest init -q -b main
        }
        git -C $dest add -A
        git -C $dest -c user.name=fixture -c user.email=fixture@local commit -qm "fixture $name @ $($m.pin)"
        $fp = Get-TreeFingerprint $dest
        if ($fp -ne $m.tree_sha256) { throw "fingerprint mismatch after materialization: got $fp want $($m.tree_sha256)" }
        Write-Host "   materialized + fingerprint OK ($($m.pin))"
    } catch {
        # clean the half-built dir so the next run retries from scratch (existing-valid
        # mothers are only skipped when the fingerprint matches)
        if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
        Write-Warning "   FAILED: $_"
        $fail++
    }
}
if ($fail -gt 0) { throw "$fail mother(s) failed prep - see warnings above" }
Write-Host "prep-mothers: all requested mothers OK"
