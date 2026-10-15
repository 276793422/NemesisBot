# check_gates.ps1 - structural check for CODE-FEATCFG: cfg feature gates wired in lib.rs
param([string]$LibPath)
$ErrorActionPreference = 'Stop'
if (-not (Test-Path $LibPath)) { Write-Output "LIB-MISSING: $LibPath"; exit 1 }
$missing = @()
foreach ($f in @('web', 'webhook', 'rpc')) {
    $pat = 'cfg\(feature\s*=\s*"' + $f + '"\)'
    if (-not (Select-String -Path $LibPath -Pattern $pat -Quiet)) { $missing += $f }
}
if ($missing.Count -gt 0) { Write-Output ("GATES-MISSING: " + ($missing -join ',')); exit 1 }
Write-Output 'GATES-OK'
exit 0
