@echo off
setlocal

REM Switch to project root (parent of scripts/)
cd /d "%~dp0"

if "%~1"=="" goto show_help

if /i "%~1"=="win" (
    call scripts\build-windows.bat
) else if /i "%~1"=="lin" (
    call scripts\build-linux.bat
) else if /i "%~1"=="and" (
    call scripts\build-android.bat
) else if /i "%~1"=="run" (
    call scripts\run-demo.bat
) else if /i "%~1"=="cls" (
    rmdir /s /q "target" 2>nul
    rmdir /s /q "bin" 2>nul
) else (
    echo [ERROR] Unknown argument: %~1
    echo Use --help for usage information
    exit /b 1
)
exit /b %errorlevel%

:show_help
echo Usage: build.bat [win^|lin^|and^|run]
echo.
echo   win  - Build Windows release
echo   lin  - Build Linux release (WSL)
echo   and  - Build Android release
echo   run  - Clean build + run gateway (Windows)
echo   cls  - Delete folders that both target and bin
