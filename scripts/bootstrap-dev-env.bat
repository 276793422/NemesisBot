@echo off
REM ============================================================================
REM  bootstrap-dev-env.bat  --  NemesisBot dev environment bootstrap (Windows)
REM  Double-click to run. Will trigger UAC (click Yes); the actual install
REM  runs in the elevated PowerShell window that pops up.
REM
REM  Intentionally ASCII-only: cmd.exe reads .bat in the OEM codepage, so any
REM  non-ASCII here could garble. All Chinese user-facing text lives in the
REM  UTF-8 (BOM) .ps1, which PowerShell reads correctly.
REM ============================================================================
echo.
echo Starting dev-environment bootstrap (admin required)...
echo Click "Yes" on the UAC prompt. Watch the elevated window that opens.
echo.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0bootstrap-dev-env.ps1"
echo.
echo Done. If no elevated window appeared, right-click this .bat and
echo choose "Run as administrator".
pause
