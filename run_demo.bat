@echo off
setlocal enabledelayedexpansion

REM ============================================
REM NemesisBot Demo Runner
REM ============================================
REM Clean build + run gateway
REM Steps: Clean -> Build -> Run

echo ============================================
echo  NemesisBot Demo Runner
echo ============================================
echo.

REM ============================================
REM Step 1: Clean old outputs
REM ============================================
echo [Step 1/3] Cleaning old outputs...

if exist "bin\nemesisbot.exe" (
    del /f /q "bin\nemesisbot.exe" 2>nul
    echo   OK Deleted bin\nemesisbot.exe
) else (
    echo   SKIP bin\nemesisbot.exe not found
)

if exist "bin\plugins" (
    rmdir /s /q "bin\plugins" 2>nul
    echo   OK Deleted bin\plugins\
) else (
    echo   SKIP bin\plugins\ not found
)

if exist "bin\tests" (
    rmdir /s /q "bin\tests" 2>nul
    echo   OK Deleted bin\tests\
) else (
    echo   SKIP bin\tests\ not found
)

if exist "crates\nemesis-web\static" (
    rmdir /s /q "crates\nemesis-web\static" 2>nul
    echo   OK Cleaned crates\nemesis-web\static\
) else (
    echo   SKIP crates\nemesis-web\static\ not found
)

echo   OK Clean finished
echo.

REM ============================================
REM Step 2: Build (call build.bat)
REM ============================================
echo [Step 2/3] Building...
echo.

call build.bat
if errorlevel 1 (
    echo.
    echo [ERROR] Build failed!
    pause
    exit /b 1
)

echo.

REM ============================================
REM Step 3: Run gateway
REM ============================================
echo [Step 3/3] Starting gateway...
echo.

bin\nemesisbot.exe gateway
