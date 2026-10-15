# inject_helper.ps1 - copy an evaluator's inject files into a run copy
# Mirrors Test-Acceptance.ps1 inject logic exactly (Windows-native paths throughout).
param(
    [Parameter(Mandatory=$true)][string]$ExperimentDir,
    [Parameter(Mandatory=$true)][string]$TaskId,
    [Parameter(Mandatory=$true)][string]$DestCopy
)
$ErrorActionPreference = 'Stop'
$cfg = Get-Content -Raw -Encoding UTF8 (Join-Path $ExperimentDir "evaluators\$TaskId\config.json") | ConvertFrom-Json
foreach ($inj in @($cfg.inject)) {
    $src = Join-Path $ExperimentDir "evaluators\$TaskId\$($inj.src)"
    $dst = Join-Path $DestCopy $inj.dst
    $dstDir = Split-Path -Parent $dst
    if (-not (Test-Path $dstDir)) { New-Item -ItemType Directory -Path $dstDir -Force | Out-Null }
    Copy-Item -Force $src $dst
    # freshness: mtime must be newer than any cargo fingerprint build time,
    # else cargo may treat the injected file as already-compiled (stale-binary hazard).
    (Get-Item $dst).LastWriteTime = Get-Date
}
Write-Output "INJECTED $TaskId"
