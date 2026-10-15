# check_uicontract.ps1 - structural check for frontend feature tasks (CODE-UIALIGN / CODE-UITHEME)
# Requires each listed symbol to appear somewhere under the given source root.
param([string]$SrcRoot, [string[]]$Symbols)
$ErrorActionPreference = 'Stop'
if (-not (Test-Path $SrcRoot)) { Write-Output "SRC-MISSING: $SrcRoot"; exit 1 }
$missing = @()
foreach ($s in $Symbols) {
    # -Quiet with piped input returns one boolean PER FILE — a bare .Count counts
    # $false entries too. Only a $true entry means the symbol was actually found.
    $hits = @(Get-ChildItem -Path $SrcRoot -Recurse -Include *.vue,*.ts -File |
        Select-String -Pattern ([regex]::Escape($s)) -Quiet)
    if (-not ($hits -contains $true)) { $missing += $s }
}
if ($missing.Count -gt 0) { Write-Output ("SYMBOLS-MISSING: " + ($missing -join ',')); exit 1 }
Write-Output 'SYMBOLS-OK'
exit 0
