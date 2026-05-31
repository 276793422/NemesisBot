@echo off
setlocal enabledelayedexpansion

REM ============================================
REM NemesisBot Rust Build Script (Windows)
REM ============================================
REM Usage: scripts\build-windows.bat [options]
REM   No arguments  - Build release, copy to bin\bin_windows\
REM   --clean       - Clean before building
REM   --skip-plugin - Skip plugin-ui.dll build
REM   --help        - Show help
REM
REM Output layout:
REM   bin\bin_windows\
REM     nemesisbot.exe
REM     plugins\
REM       plugin_ui.dll
REM     tests\
REM       cluster-test.exe
REM       integration-test.exe
REM       mcp-server.exe

REM Switch to project root (parent of scripts/)
cd /d "%~dp0\.."

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
echo Usage: scripts\build-windows.bat [--clean] [--skip-plugin] [--help]
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
echo  NemesisBot Rust Build (Windows)
echo ============================================
echo  Version:     %VERSION%
echo  Git Commit:  %GIT_COMMIT%
echo  Rustc:       %RUSTC_VERSION%
echo  Target Dir:  target\target_windows
echo  Output Dir:  bin\bin_windows
echo ============================================
echo.

REM ============================================
REM Set target directory
REM ============================================
set CARGO_TARGET_DIR=target\target_windows

REM ============================================
REM Step 1: Clean (optional)
REM ============================================
if "%CLEAN%"=="1" (
    echo [Step 1/5] Cleaning target\target_windows...
    cargo clean --target-dir target\target_windows 2>nul
    if errorlevel 1 (
        echo   WARN cargo clean had issues, continuing...
    ) else (
        echo   OK Cleaned
    )
    echo.
) else (
    echo [Step 1/5] Clean skipped ^(use --clean to enable^)
    echo.
)

REM ============================================
REM Step 2: Build Vue frontend (web dashboard)
REM ============================================
echo [Step 2/5] Building Vue frontend...

if exist "web\package.json" (
    pushd web

    REM Detect wrong-platform node_modules (e.g. Linux binaries from WSL build)
    if exist "node_modules\@rollup\rollup-linux-x64-gnu\package.json" (
        if not exist "node_modules\@rollup\rollup-win32-x64-msvc\package.json" (
            echo   Detected Linux node_modules, reinstalling for Windows...
            rmdir /s /q "node_modules" 2>nul
            if exist "package-lock.json" del /q "package-lock.json" 2>nul
        )
    )

    if not exist "node_modules" (
        echo   Installing npm dependencies...
        call npm install --silent
        if errorlevel 1 (
            popd
            echo   WARN npm install failed, skipping Vue build
            echo.
            goto step3
        )
    )
    echo   Running Vite build...
    call npm run build
    if errorlevel 1 (
        popd
        echo   ERROR Vue build failed! See errors above.
        echo.
        pause
        exit /b 1
    )
    popd
    echo   OK Vue frontend built
) else (
    echo   SKIP web\package.json not found
)
echo.

:step3

REM ============================================
REM Step 3: Build main workspace (release)
REM ============================================
echo [Step 3/5] Building release...

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
REM Step 4: Build plugin DLLs (release)
REM ============================================
if "%SKIP_PLUGIN%"=="1" (
    echo [Step 4/5] Plugin DLLs skipped ^(--skip-plugin^)
    echo.
    goto step5
)

echo [Step 4/5] Building plugin DLLs...

REM --- plugin-ui ---
if exist "plugins\plugin-ui\Cargo.toml" (
    echo   Building plugin-ui...
    pushd plugins\plugin-ui
    set CARGO_TARGET_DIR=..\..\target\target_windows\plugins\plugin-ui
    cargo build --release
    if errorlevel 1 (
        popd
        echo   WARN Plugin-ui DLL build failed ^(non-fatal, continuing without plugin^)
        echo.
        set CARGO_TARGET_DIR=target\target_windows
        goto plugin_onnx
    )
    popd
    set CARGO_TARGET_DIR=target\target_windows
    echo   OK Plugin-ui DLL built
) else (
    echo   SKIP plugins\plugin-ui\Cargo.toml not found
)

:plugin_onnx
REM --- plugin-onnx ---
if exist "plugins\plugin-onnx\Cargo.toml" (
    echo   Building plugin-onnx...
    pushd plugins\plugin-onnx
    set CARGO_TARGET_DIR=..\..\target\target_windows\plugins\plugin-onnx
    cargo build --release
    if errorlevel 1 (
        popd
        echo   WARN Plugin-onnx DLL build failed ^(non-fatal, continuing without plugin^)
        echo.
        set CARGO_TARGET_DIR=target\target_windows
        goto step5
    )
    popd
    set CARGO_TARGET_DIR=target\target_windows
    echo   OK Plugin-onnx DLL built
) else (
    echo   SKIP plugins\plugin-onnx\Cargo.toml not found
)
echo.

:step5

REM ============================================
REM Step 5: Copy to bin\bin_windows\
REM ============================================
echo [Step 5/5] Copying to bin\bin_windows\...

if not exist "bin\bin_windows" mkdir "bin\bin_windows"

set COPIED=0

REM Copy main binary
if exist "target\target_windows\release\nemesisbot.exe" (
    copy /y "target\target_windows\release\nemesisbot.exe" "bin\bin_windows\" >nul 2>&1
    set /a COPIED+=1
)

REM Copy plugin DLL to bin\bin_windows\plugins\
set DLL_FOUND=0
if not exist "bin\bin_windows\plugins" mkdir "bin\bin_windows\plugins"
if exist "target\target_windows\plugins\plugin-ui\release\plugin_ui.dll" (
    copy /y "target\target_windows\plugins\plugin-ui\release\plugin_ui.dll" "bin\bin_windows\plugins\" >nul 2>&1
    set /a COPIED+=1
    set DLL_FOUND=1
) else if exist "target\target_windows\plugins\plugin-ui\release\plugin-ui.dll" (
    copy /y "target\target_windows\plugins\plugin-ui\release\plugin-ui.dll" "bin\bin_windows\plugins\" >nul 2>&1
    set /a COPIED+=1
    set DLL_FOUND=1
)

REM Copy plugin-onnx DLL to bin\bin_windows\plugins\
set ONNX_DLL_FOUND=0
if exist "target\target_windows\plugins\plugin-onnx\release\plugin_onnx.dll" (
    copy /y "target\target_windows\plugins\plugin-onnx\release\plugin_onnx.dll" "bin\bin_windows\plugins\" >nul 2>&1
    set /a COPIED+=1
    set ONNX_DLL_FOUND=1
) else if exist "target\target_windows\plugins\plugin-onnx\release\plugin-onnx.dll" (
    copy /y "target\target_windows\plugins\plugin-onnx\release\plugin-onnx.dll" "bin\bin_windows\plugins\" >nul 2>&1
    set /a COPIED+=1
    set ONNX_DLL_FOUND=1
)

if "!ONNX_DLL_FOUND!"=="0" if "%SKIP_PLUGIN%"=="0" (
    echo   WARN plugin-onnx.dll not found in build output
)

if "!DLL_FOUND!"=="0" if "%SKIP_PLUGIN%"=="0" (
    echo   WARN plugin-ui.dll not found in build output
)

REM Copy test-tools binaries to bin\bin_windows\tests\
if not exist "bin\bin_windows\tests" mkdir "bin\bin_windows\tests"
for %%e in (ai-server.exe cluster-test.exe integration-test.exe mcp-server.exe) do (
    if exist "target\target_windows\release\%%e" (
        copy /y "target\target_windows\release\%%e" "bin\bin_windows\tests\" >nul 2>&1
        set /a COPIED+=1
    )
)

echo   OK Copied !COPIED! file^(s^) to bin\bin_windows\
echo.

REM ============================================
REM Summary
REM ============================================
echo ============================================
echo  Build Summary (Windows)
echo ============================================
echo  Version: %VERSION%
echo  Commit:  %GIT_COMMIT%
echo.

echo  bin\bin_windows\
for %%f in (bin\bin_windows\*) do (
    set FSIZE=%%~zf
    set /a FSIZE_MB=!FSIZE! / 1048576
    echo    %%~nxf ^(!FSIZE_MB! MB^)
)

echo.
echo [SUCCESS] Build completed!
echo.
echo Run: bin\bin_windows\nemesisbot.exe gateway
echo ============================================
echo.
