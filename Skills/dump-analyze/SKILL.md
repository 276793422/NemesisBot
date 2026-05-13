---
name: dump-analyze
description: This skill should be used when the user asks to "analyze crash dump", "analyze dump files", "analyze .dmp", "check crash dumps", "debug crash", "analyze crash", "cdb analyze", "windbg analyze", or mentions dump analysis, crash analysis, or debugging crash dumps (.dmp files).
version: 1.0.0
---

# Dump Analyze Skill

## Overview

This skill provides crash dump analysis capabilities using Windows CDB (Console Debugger). It can analyze individual `.dmp` files or batch-analyze all dumps in a directory, producing structured reports with exception info, call stacks, faulting module, and symbol resolution.

## When This Skill Applies

Use this skill when users need to:
- Analyze crash dump (.dmp) files
- Identify crash locations and call stacks
- Determine which module/function caused a crash
- Batch-analyze multiple dump files
- Extract exception codes and faulting addresses from dumps

## Prerequisites

- **CDB (cdb.exe)**: Installed at `C:\Zoo\Tools\WinDBG_x64\cdb.exe`
- **Symbol files (.pdb)**: Must be available alongside the crashed binaries
- **Windows platform**: CDB is a Windows-only tool

## Quick Start

### Analyze a Single Dump File

```bash
# From bash (Git Bash / WSL)
powershell -ExecutionPolicy Bypass -File "C:/Lenovo/SmartConnectFrame/Skills/dump-analyze/scripts/analyze-dump.ps1" \
    -DumpFile "C:\Lenovo\SmartConnectFrame\out\Debug_x64\Log\Dumps\SmartConnect_20260411_003649_424.dmp" \
    -SymbolPath "C:\Lenovo\SmartConnectFrame\out\Debug_x64"
```

### Analyze All Dumps in a Directory

```bash
powershell -ExecutionPolicy Bypass -File "C:/Lenovo/SmartConnectFrame/Skills/dump-analyze/scripts/analyze-dump.ps1" \
    -DumpFolder "C:\Lenovo\SmartConnectFrame\out\Debug_x64\Log\Dumps" \
    -SymbolPath "C:\Lenovo\SmartConnectFrame\out\Debug_x64" \
    -OutputFile "C:\Lenovo\SmartConnectFrame\out\Debug_x64\Log\Dumps\analysis_report.txt"
```

### Analyze with Microsoft Public Symbols

```bash
powershell -ExecutionPolicy Bypass -File "C:/Lenovo/SmartConnectFrame/Skills/dump-analyze/scripts/analyze-dump.ps1" \
    -DumpFolder "C:\Lenovo\SmartConnectFrame\out\Debug_x64\Log\Dumps" \
    -SymbolPath "C:\Lenovo\SmartConnectFrame\out\Debug_x64"
```

## Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| `-DumpFile` | One of DumpFile/DumpFolder | Path to a single .dmp file |
| `-DumpFolder` | One of DumpFile/DumpFolder | Path to directory containing .dmp files |
| `-SymbolPath` | No | Symbol search path (default: project Debug_x64 output) |
| `-OutputFile` | No | Output report file path (default: `{DumpFolder}\analysis_report.txt`) |
| `-CdbPath` | No | Path to cdb.exe (default: `C:\Zoo\Tools\WinDBG_x64\cdb.exe`) |
| `-MaxFrames` | No | Max call stack frames to show (default: 50) |
| `-SummaryOnly` | No | Switch: only output summary table, skip detailed analysis |

## Output Format

### Summary Table (per dump file)

```
================================================================================
  Dump: SmartConnect_20260411_003649_424.dmp
================================================================================
  Exception Code  : 0x40010006 (DBG_PRINTEXCEPTION_C)
  Exception Address: 0x00007FFB355D73FA
  Faulting Module : WinAPI.dll
  Faulting Offset : 0x00000000000173FA
  Thread ID       : 157484
  Process         : SmartConnectServer.exe

  Call Stack (Top 5):
    #00 WinAPI!SDK_Stop+0x3A
    #01 WinAPI!DllMain+0x1E
    #02 ntdll!LdrpCallInitRoutine+0x6E
    #03 ntdll!LdrShutdownProcess+0x1D0
    #04 kernel32!ExitProcess+0x50

  Likely Root Cause: SDK_Stop() triggered broadcast callback during DLL unload
================================================================================
```

### Full Analysis (default)

Includes the summary plus:
- Full `!analyze -v` output
- Complete call stack with parameters and line numbers
- Loaded module list
- Thread information

## Smart Symbol Resolution

The script automatically:
1. Looks for `.pdb` files next to the crashed binary
2. Appends Microsoft public symbol server if `SRV*` syntax is used
3. Reports unresolved symbols (missing PDB) with `???` markers

## Common Use Cases

### 1. Quick Crash Summary

Get a quick overview of all crashes:

```bash
powershell -ExecutionPolicy Bypass -File "C:/Lenovo/SmartConnectFrame/Skills/dump-analyze/scripts/analyze-dump.ps1" \
    -DumpFolder "C:\Lenovo\SmartConnectFrame\out\Debug_x64\Log\Dumps" \
    -SummaryOnly
```

### 2. Full Detailed Analysis

Get complete analysis with call stacks:

```bash
powershell -ExecutionPolicy Bypass -File "C:/Lenovo/SmartConnectFrame/Skills/dump-analyze/scripts/analyze-dump.ps1" \
    -DumpFolder "C:\Lenovo\SmartConnectFrame\out\Debug_x64\Log\Dumps" \
    -OutputFile "C:\Lenovo\SmartConnectFrame\out\Debug_x64\Log\Dumps\full_analysis.txt"
```

### 3. Analyze Specific Dump

Focus on one particular dump:

```bash
powershell -ExecutionPolicy Bypass -File "C:/Lenovo/SmartConnectFrame/Skills/dump-analyze/scripts/analyze-dump.ps1" \
    -DumpFile "C:\Lenovo\SmartConnectFrame\out\Debug_x64\Log\Dumps\SmartConnect_20260411_003649_424.dmp" \
    -MaxFrames 100
```

## Troubleshooting

- **"cdb.exe not found"**: Verify `C:\Zoo\Tools\WinDBG_x64\cdb.exe` exists, or pass `-CdbPath`
- **All symbols show `???`**: PDB files are missing or don't match the binary version
- **"Unable to open dump file"**: Check the file path and permissions
- **Empty output**: The dump file may be corrupted or from a different architecture

## Reference Files

- `references/cdb-commands.md` - CDB command reference for dump analysis
- `references/exception-codes.md` - Common Windows exception codes

## Scripts

- `scripts/analyze-dump.ps1` - Main analysis script (PowerShell)
