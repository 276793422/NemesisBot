@echo off
REM NemesisBot External Channel - Output Example
REM This script reads AI responses from stdin

:loop
set /p line=
if defined line (
    echo AI: %line%
)
goto loop
