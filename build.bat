@echo off
setlocal enabledelayedexpansion

REM ============================================
REM NemesisBot Rust Build Script
REM ============================================
REM Usage: build.bat [options]
REM   No arguments  - Build release, copy to bin\
REM   --clean       - Clean before building
REM   --skip-plugin - Skip plugin-ui.dll build
REM   --help        - Show help
REM
REM Output layout:
REM   bin\
REM     nemesisbot.exe
REM     plugin_ui.dll
REM     ai-server.exe
REM     cluster-test.exe
REM     integration-test.exe
REM     mcp-server.exe

REM ============================================
REM Parse Arguments
REM ============================================
set CLEAN=0
set SKIP_PLUGIN=0

:parse_args
if "%~1"=="" goto done_parsing
if /i "%~1"=="--clean" (
    set CLEAN=1
    shift
    goto parse_args
)
if /i "%~1"=="--skip-plugin" (
    set SKIP_PLUGIN=1
    shift
    goto parse_args
)
if /i "%~1"=="--help" goto show_help
echo [ERROR] Unknown argument: %~1
echo Use --help for usage information
exit /b 1

:show_help
echo Usage: build.bat [--clean] [--skip-plugin] [--help]
echo.
echo Options:
echo   --clean        Clean target before building
echo   --skip-plugin  Skip plugin-ui.dll build
echo   --help         Show this help
exit /b 0

:done_parsing

REM ============================================
REM Banner
REM ============================================
set VERSION=0.0.0.1
for /f "tokens=*" %%i in ('git describe --tags --abbrev=0 2^>nul') do set VERSION=%%i

set GIT_COMMIT=unknown
for /f "tokens=*" %%i in ('git rev-parse --short HEAD 2^>nul') do set GIT_COMMIT=%%i

for /f "tokens=*" %%i in ('rustc --version 2^>nul') do set RUSTC_VERSION=%%i

echo ============================================
echo  NemesisBot Rust Build
echo ============================================
echo  Version:     %VERSION%
echo  Git Commit:  %GIT_COMMIT%
echo  Rustc:       %RUSTC_VERSION%
echo ============================================
echo.

REM ============================================
REM Step 1: Clean (optional)
REM ============================================
if "%CLEAN%"=="1" (
    echo [Step 1/4] Cleaning target...
    cargo clean 2>nul
    if errorlevel 1 (
        echo   WARN cargo clean had issues, continuing...
    ) else (
        echo   OK Cleaned
    )
    echo.
) else (
    echo [Step 1/4] Clean skipped ^(use --clean to enable^)
    echo.
)

REM ============================================
REM Step 2: Build main workspace (release)
REM ============================================
echo [Step 2/4] Building release...

cargo build --release
if errorlevel 1 (
    echo.
    echo [ERROR] Build failed!
    pause
    exit /b 1
)

echo   OK Build completed
echo.

REM ============================================
REM Step 3: Build plugin-ui DLL (release)
REM ============================================
if "%SKIP_PLUGIN%"=="1" (
    echo [Step 3/4] Plugin DLL skipped ^(--skip-plugin^)
    echo.
    goto step4
)

echo [Step 3/4] Building plugin-ui DLL...

if not exist "plugins\plugin-ui\Cargo.toml" (
    echo   SKIP plugins\plugin-ui\Cargo.toml not found
    echo.
    goto step4
)

pushd plugins\plugin-ui
cargo build --release
if errorlevel 1 (
    popd
    echo   WARN Plugin DLL build failed ^(non-fatal, continuing without plugin^)
    echo.
    goto step4
)
popd
echo   OK Plugin DLL built
echo.

:step4

REM ============================================
REM Step 4: Copy to bin\
REM ============================================
echo [Step 4/4] Copying to bin\...

if not exist "bin" mkdir bin

set COPIED=0

REM Copy main binary
if exist "target\release\nemesisbot.exe" (
    copy /y "target\release\nemesisbot.exe" "bin\" >nul 2>&1
    set /a COPIED+=1
)

REM Copy plugin DLL to bin\plugins\
set DLL_FOUND=0
if not exist "bin\plugins" mkdir "bin\plugins"
if exist "plugins\plugin-ui\target\release\plugin_ui.dll" (
    copy /y "plugins\plugin-ui\target\release\plugin_ui.dll" "bin\plugins\" >nul 2>&1
    set /a COPIED+=1
    set DLL_FOUND=1
) else if exist "plugins\plugin-ui\target\release\plugin-ui.dll" (
    copy /y "plugins\plugin-ui\target\release\plugin-ui.dll" "bin\plugins\" >nul 2>&1
    set /a COPIED+=1
    set DLL_FOUND=1
)

if "!DLL_FOUND!"=="0" if "%SKIP_PLUGIN%"=="0" (
    echo   WARN plugin-ui.dll not found in build output
)

REM Copy test-tools binaries to bin\tests\
if not exist "bin\tests" mkdir "bin\tests"
for %%e in (ai-server.exe cluster-test.exe integration-test.exe mcp-server.exe) do (
    if exist "target\release\%%e" (
        copy /y "target\release\%%e" "bin\tests\" >nul 2>&1
        set /a COPIED+=1
    )
)

echo   OK Copied !COPIED! file^(s^) to bin\
echo.

REM ============================================
REM Summary
REM ============================================
echo ============================================
echo  Build Summary
echo ============================================
echo  Version: %VERSION%
echo  Commit:  %GIT_COMMIT%
echo.

echo  bin\
for %%f in (bin\*) do (
    set FSIZE=%%~zf
    set /a FSIZE_MB=!FSIZE! / 1048576
    echo    %%~nxf ^(!FSIZE_MB! MB^)
)

echo.
echo [SUCCESS] Build completed!
echo.
echo Run: bin\nemesisbot.exe gateway
echo ============================================
echo.
