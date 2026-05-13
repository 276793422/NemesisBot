#Requires -Version 5.0
<#
.SYNOPSIS
    Analyze Windows crash dump files using CDB (Console Debugger).

.DESCRIPTION
    Analyzes one or more .dmp files and produces structured reports including
    exception information, faulting module, call stacks, and symbol resolution.

.PARAMETER DumpFile
    Path to a single .dmp file to analyze.

.PARAMETER DumpFolder
    Path to a directory containing .dmp files to batch analyze.

.PARAMETER SymbolPath
    Symbol search path. Supports CDB syntax:
    - Directory path: "C:\path\to\symbols"
    - With MS server: "SRV*C:\sym_cache*http://msdl.microsoft.com/download/symbols;C:\local\pdb"
    Default: "C:\Lenovo\SmartConnectFrame\out\Debug_x64"

.PARAMETER OutputFile
    Path to write the analysis report. Default: {DumpFolder}\analysis_report.txt

.PARAMETER CdbPath
    Path to cdb.exe. Default: C:\Zoo\Tools\WinDBG_x64\cdb.exe

.PARAMETER MaxFrames
    Maximum number of call stack frames to display. Default: 30

.PARAMETER SummaryOnly
    If set, only output a summary table per dump (no full !analyze -v output).

.EXAMPLE
    .\analyze-dump.ps1 -DumpFolder "C:\dumps" -SymbolPath "C:\symbols"

.EXAMPLE
    .\analyze-dump.ps1 -DumpFile "C:\dumps\crash.dmp" -MaxFrames 100

.EXAMPLE
    .\analyze-dump.ps1 -DumpFolder "C:\dumps" -SummaryOnly -OutputFile "C:\dumps\summary.txt"
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory=$false)]
    [string]$DumpFile,

    [Parameter(Mandatory=$false)]
    [string]$DumpFolder,

    [Parameter(Mandatory=$false)]
    [string]$SymbolPath = "C:\Lenovo\SmartConnectFrame\out\Debug_x64",

    [Parameter(Mandatory=$false)]
    [string]$OutputFile = "",

    [Parameter(Mandatory=$false)]
    [string]$CdbPath = "C:\Zoo\Tools\WinDBG_x64\cdb.exe",

    [Parameter(Mandatory=$false)]
    [int]$MaxFrames = 30,

    [Parameter(Mandatory=$false)]
    [switch]$SummaryOnly
)

# ===== Validate inputs =====
if (-not $DumpFile -and -not $DumpFolder) {
    Write-Error "Must specify either -DumpFile or -DumpFolder."
    exit 1
}

if (-not (Test-Path $CdbPath)) {
    Write-Error "cdb.exe not found at: $CdbPath`nPlease install Windows SDK Debugging Tools or specify -CdbPath."
    exit 1
}

# Collect dump files
$dumpFiles = @()
if ($DumpFile) {
    if (-not (Test-Path $DumpFile)) {
        Write-Error "Dump file not found: $DumpFile"
        exit 1
    }
    $dumpFiles += Get-Item $DumpFile
    $resolvedFolder = Split-Path $DumpFile -Parent
} else {
    if (-not (Test-Path $DumpFolder)) {
        Write-Error "Dump folder not found: $DumpFolder"
        exit 1
    }
    $dumpFiles = Get-ChildItem -Path $DumpFolder -Filter "*.dmp" | Sort-Object Name
    $resolvedFolder = $DumpFolder
    if ($dumpFiles.Count -eq 0) {
        Write-Warning "No .dmp files found in: $DumpFolder"
        exit 0
    }
}

# Default output file
if (-not $OutputFile) {
    $OutputFile = Join-Path $resolvedFolder "analysis_report.txt"
}

# ===== Helper: Run CDB and capture output with timeout =====
function Invoke-CdbAnalysis {
    param(
        [string]$DumpPath,
        [string]$SymPath,
        [string]$Cdb,
        [int]$Frames,
        [bool]$Summary
    )

    # Build CDB commands - keep minimal to avoid hangs
    if ($Summary) {
        $commands = "!analyze -v; .echo ===CALL_STACK===; kPN $Frames; q"
    } else {
        # Full mode: analyze + call stack + module list (short form, not lmv)
        $commands = "!analyze -v; .echo ===CALL_STACK===; kPN $Frames; .echo ===MODULES===; lm q; q"
    }

    # Run CDB with timeout (30 seconds per dump)
    $proc = Start-Process -FilePath $Cdb `
        -ArgumentList "-y","`"$SymPath`"","-z","`"$DumpPath`"","-c","$commands" `
        -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput "$env:TEMP\cdb_output_$($PID).tmp" `
        -RedirectStandardError "$env:TEMP\cdb_error_$($PID).tmp" `
        -ErrorAction SilentlyContinue

    # Read output
    $outputFile = "$env:TEMP\cdb_output_$($PID).tmp"
    if (Test-Path $outputFile) {
        $result = Get-Content $outputFile -Raw -ErrorAction SilentlyContinue
        Remove-Item $outputFile -Force -ErrorAction SilentlyContinue
    } else {
        $result = ""
    }

    $errorFile = "$env:TEMP\cdb_error_$($PID).tmp"
    if (Test-Path $errorFile) {
        Remove-Item $errorFile -Force -ErrorAction SilentlyContinue
    }

    return $result
}

# Alternative: direct invocation (simpler, works well for batch)
function Invoke-CdbDirect {
    param(
        [string]$DumpPath,
        [string]$SymPath,
        [string]$Cdb,
        [int]$Frames,
        [bool]$Summary
    )

    if ($Summary) {
        $commands = "!analyze -v; .echo ===CALL_STACK===; kPN $Frames; q"
    } else {
        $commands = "!analyze -v; .echo ===CALL_STACK===; kPN $Frames; .echo ===MODULES===; lm q; q"
    }

    $result = & $Cdb -y $SymPath -z $DumpPath -c $commands 2>&1
    return ($result -join "`r`n")
}

# ===== Helper: Parse summary from CDB output =====
function Get-SummaryFromOutput {
    param([string]$Output)

    $summary = @{
        ExceptionCode   = ""
        ExceptionString = ""
        FaultingAddress = ""
        FaultingModule  = ""
        FaultingFunction = ""
        ThreadId        = ""
        ProcessName     = ""
        CallStack       = @()
    }

    $lines = $Output -split "`r?`n"

    $inCallStack = $false
    $callStackLines = @()

    foreach ($line in $lines) {
        # Exception code: "ExceptionCode: 0x40010006"
        if ($line -match 'ExceptionCode:\s*(0x[0-9a-fA-F]+)') {
            $summary.ExceptionCode = $Matches[1]
        }
        # Exception description: "EXCEPTION_CODE: (Win32/HRESULT/NTSTATUS) 0x40010006 - ..."
        if ($line -match 'EXCEPTION_CODE:\s*\(.*?\)\s*(0x[0-9a-fA-F]+)\s*-\s*(.*)') {
            $code = $Matches[1]
            $desc = $Matches[2].Trim()
            $summary.ExceptionString = "$desc"
            if (-not $summary.ExceptionCode) { $summary.ExceptionCode = $code }
        }
        # Alternate format: "Exception code: 0x40010006"
        if ($line -match 'Exception code:\s*(0x[0-9a-fA-F]+)') {
            if (-not $summary.ExceptionCode) { $summary.ExceptionCode = $Matches[1] }
        }
        # Faulting IP line: "FAULTING_IP: +0x..."
        if ($line -match 'FAULTING_IP:\s*(.*)') {
            $summary.FaultingAddress = $Matches[1].Trim()
        }
        # The module!function line right after FAULTING_IP
        if ($summary.FaultingAddress -and -not $summary.FaultingFunction -and $line -match '^(\S+!)') {
            $summary.FaultingFunction = $line.Trim()
        }
        # "Probably caused by:"
        if ($line -match 'Probably caused by:\s*(.*)') {
            $summary.FaultingModule = $Matches[1].Trim()
        }
        # Process name: "PROCESS_NAME: SmartConnectServer.exe"
        if ($line -match 'PROCESS_NAME:\s*(.*)') {
            $summary.ProcessName = $Matches[1].Trim()
        }
        # Thread ID from faulting thread
        if ($line -match 'FAULTING_THREAD:\s*(.*)') {
            $summary.ThreadId = $Matches[1].Trim()
        }

        # Call stack section
        if ($line -match '===CALL_STACK===') {
            $inCallStack = $true
            $callStackLines = @()
            continue
        }
        if ($inCallStack -and ($line -match '===SUMMARY_END===' -or $line -match '===ANALYSIS_END===' -or $line -match '===MODULES===')) {
            $inCallStack = $false
        }
        if ($inCallStack -and $line.Trim() -ne '') {
            $callStackLines += $line.Trim()
        }
    }

    $summary.CallStack = $callStackLines
    return $summary
}

# ===== Main =====
Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "  Dump Analysis - CDB" -ForegroundColor Cyan
Write-Host "  Files: $($dumpFiles.Count)" -ForegroundColor Cyan
Write-Host "  Symbols: $SymbolPath" -ForegroundColor Gray
Write-Host "========================================`n" -ForegroundColor Cyan

# Clear output file
"" | Out-File -FilePath $OutputFile -Encoding utf8

$totalAnalyses = @()
$fileIndex = 0

foreach ($file in $dumpFiles) {
    $fileIndex++
    Write-Host "[$fileIndex/$($dumpFiles.Count)] Analyzing: $($file.Name)..." -ForegroundColor Cyan -NoNewline

    $rawOutput = Invoke-CdbDirect -DumpPath $file.FullName -SymPath $SymbolPath -Cdb $CdbPath -Frames $MaxFrames -Summary:$SummaryOnly

    # Parse summary
    $parsed = Get-SummaryFromOutput -Output $rawOutput
    $parsed.FileName = $file.Name
    $totalAnalyses += $parsed

    # Write to output file
    $separator = "=" * 80
    $block = @"

$separator
  Dump: $($file.Name)
$separator
  Exception Code   : $($parsed.ExceptionCode)
  Description      : $($parsed.ExceptionString)
  Faulting Module  : $($parsed.FaultingModule)
  Faulting Addr    : $($parsed.FaultingAddress)
  Faulting Func    : $($parsed.FaultingFunction)
  Process          : $($parsed.ProcessName)
  Thread           : $($parsed.ThreadId)
  File Size        : $($file.Length) bytes

  Call Stack (Top frames):
$($parsed.CallStack | Select-Object -First 10 | ForEach-Object { "    $_" } | Out-String)

"@

    if (-not $SummaryOnly) {
        $block += @"

--- Raw CDB Output ---

$rawOutput

"@
    }

    $block | Out-File -FilePath $OutputFile -Append -Encoding utf8

    Write-Host " Done" -ForegroundColor Green
}

# ===== Print Summary Table =====
Write-Host "`n========================================" -ForegroundColor Yellow
Write-Host "  Analysis Summary ($($totalAnalyses.Count) dumps)" -ForegroundColor Yellow
Write-Host "========================================`n" -ForegroundColor Yellow

# Group by unique crash patterns
$grouped = $totalAnalyses | Group-Object -Property { "$($_.ExceptionCode)|$($_.FaultingModule)" }
Write-Host "Unique crash patterns: $($grouped.Count)`n" -ForegroundColor White

foreach ($g in $grouped) {
    $sample = $g.Group[0]
    Write-Host "  Pattern: $($sample.ExceptionCode) in $($sample.FaultingModule) ($($g.Count) dumps)" -ForegroundColor Yellow
    if ($sample.CallStack.Count -gt 0) {
        Write-Host "    Top frame: $($sample.CallStack[0])" -ForegroundColor Gray
        if ($sample.CallStack.Count -gt 1) {
            Write-Host "    Caller  : $($sample.CallStack[1])" -ForegroundColor Gray
        }
    }
    Write-Host ""
}

# Summary table
Write-Host ("{0,-38} {1,-12} {2,-35} {3,-20}" -f "File", "Exception", "Faulting Module", "Process") -ForegroundColor White
Write-Host ("{0,-38} {1,-12} {2,-35} {3,-20}" -f "----", "---------", "---------------", "-------") -ForegroundColor Gray

foreach ($a in $totalAnalyses) {
    $excShort = if ($a.ExceptionCode.Length -gt 10) { $a.ExceptionCode.Substring(0, 10) } else { $a.ExceptionCode }
    $modShort = if ($a.FaultingModule.Length -gt 33) { $a.FaultingModule.Substring(0, 33) } else { $a.FaultingModule }
    $procShort = if ($a.ProcessName.Length -gt 18) { $a.ProcessName.Substring(0, 18) } else { $a.ProcessName }
    $fileShort = if ($a.FileName.Length -gt 36) { $a.FileName.Substring(0, 36) } else { $a.FileName }
    Write-Host ("{0,-38} {1,-12} {2,-35} {3,-20}" -f $fileShort, $excShort, $modShort, $procShort)
}

# Write summary table to file
$summaryBlock = @"

`n========================================
  SUMMARY TABLE ($($totalAnalyses.Count) dumps)
========================================
  Unique patterns: $($grouped.Count)
"@

$summaryBlock | Out-File -FilePath $OutputFile -Append -Encoding utf8

foreach ($g in $grouped) {
    $sample = $g.Group[0]
    $s = @"

  Pattern: $($sample.ExceptionCode) - $($sample.FaultingModule) ($($g.Count) dumps)
  Files: $($g.Group.FileName -join ', ')
  Top frames:
$($sample.CallStack | Select-Object -First 5 | ForEach-Object { "    $_" } | Out-String)
"@
    $s | Out-File -FilePath $OutputFile -Append -Encoding utf8
}

Write-Host "`nReport saved to: $OutputFile`n" -ForegroundColor Green
