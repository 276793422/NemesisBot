# New-RunCopy.ps1 - fresh per-run copy of a mother fixture (mother stays read-only)
# Optional junctions: TargetCache (cargo target), NodeModulesCache (web/node_modules)
# Usage: powershell -NoProfile -File New-RunCopy.ps1 -MotherDir <dir> -DestDir <dir> [-TargetCache <dir>] [-NodeModulesCache <dir>]
param(
    [Parameter(Mandatory=$true)][string]$MotherDir,
    [Parameter(Mandatory=$true)][string]$DestDir,
    [string]$TargetCache,
    [string]$NodeModulesCache
)
$ErrorActionPreference = 'Stop'
if (-not (Test-Path $MotherDir)) { throw "mother missing: $MotherDir" }
if (Test-Path $DestDir) { throw "dest exists (each run needs a brand-new dir): $DestDir" }
robocopy $MotherDir $DestDir /E /NFL /NDL /NJH /NJS | Out-Null
if ($LASTEXITCODE -ge 8) { throw "robocopy failed rc=$LASTEXITCODE" }
if ($TargetCache) {
    if (-not (Test-Path $TargetCache)) { throw "TargetCache missing: $TargetCache" }
    New-Item -ItemType Junction -Path (Join-Path $DestDir 'target') -Target $TargetCache | Out-Null
}
if ($NodeModulesCache -and (Test-Path (Join-Path $DestDir 'web'))) {
    if (-not (Test-Path $NodeModulesCache)) { throw "NodeModulesCache missing: $NodeModulesCache" }
    New-Item -ItemType Junction -Path (Join-Path $DestDir 'web\node_modules') -Target $NodeModulesCache | Out-Null
}
$dirty = git -C $DestDir status --porcelain
if ($dirty) { throw "run copy not clean right after extraction:`n$dirty" }
[PSCustomObject]@{ dest=$DestDir; target_junction=$TargetCache; ok=$true } | ConvertTo-Json -Compress
