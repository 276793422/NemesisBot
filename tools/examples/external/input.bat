@echo off
REM NemesisBot External Channel - Input Example
REM This script reads from stdin and outputs to stdout

:loop
set /p line=
if defined line (
    echo %line%
)
goto loop
