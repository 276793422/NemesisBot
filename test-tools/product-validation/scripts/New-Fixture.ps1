# New-Fixture.ps1 - extract a repo snapshot into a mother fixture (fresh git repo, no fix history)
# Usage: powershell -NoProfile -File New-Fixture.ps1 -RepoDir C:\AI\NemesisBot_Rust -Commit <sha> -DestDir <dir>
param(
    [Parameter(Mandatory=$true)][string]$RepoDir,
    [Parameter(Mandatory=$true)][string]$Commit,
    [Parameter(Mandatory=$true)][string]$DestDir
)
$ErrorActionPreference = 'Stop'
if (Test-Path $DestDir) { throw "dest already exists: $DestDir" }
New-Item -ItemType Directory -Path $DestDir -Force | Out-Null
# git archive = tracked files only (respects .gitignore; skips submodules by design)
git -C $RepoDir archive $Commit | tar -x -C $DestDir
if ($LASTEXITCODE -ne 0) { throw "git archive failed rc=$LASTEXITCODE" }
git -C $DestDir init -q -b main
git -C $DestDir add -A
git -C $DestDir -c user.name=fixture -c user.email=fixture@local commit -qm "snapshot $Commit"
$files = (git -C $DestDir ls-files | Measure-Object).Count
[PSCustomObject]@{ dest=$DestDir; commit=$Commit; tracked_files=$files } | ConvertTo-Json -Compress
