@echo off
setlocal enabledelayedexpansion

REM Switch to project root (parent of scripts/)
cd /d "%~dp0\.."

REM ============================================
REM NemesisBot Rust Android Cross-Compile Script
REM ============================================
REM Usage: scripts/build-android.bat [options]
REM   No arguments    - Build release for arm64-v8a
REM   --clean         - Clean before building
REM   --skip-plugin   - Skip plugin .so build
REM   --target <arch> - Target architecture (default: arm64-v8a)
REM                      Supported: arm64-v8a, armeabi-v7a, x86_64, x86
REM   --api <level>   - Android API level (default: 36)
REM   --help          - Show help
REM
REM Prerequisites:
REM   - Android NDK installed (use setup-android.bat)
REM   - cargo-ndk installed: cargo install cargo-ndk
REM   - Rust Android target: rustup target add aarch64-linux-android
REM
REM Output layout:
REM   bin\bin_android\
REM     arm64-v8a\
REM       nemesisbot
REM       plugins\
REM         libplugin_ui.so
REM         libplugin_onnx.so

REM ============================================
REM Parse Arguments
REM ============================================
set CLEAN=0
set SKIP_PLUGIN=0
set TARGET_ARCH=arm64-v8a
set API_LEVEL=36

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
if /i "%~1"=="--target" (
    if "%~2"=="" (
        echo [ERROR] --target requires an architecture argument
        exit /b 1
    )
    set TARGET_ARCH=%~2
    shift
    shift
    goto parse_args
)
if /i "%~1"=="--api" (
    if "%~2"=="" (
        echo [ERROR] --api requires a level argument
        exit /b 1
    )
    set API_LEVEL=%~2
    shift
    shift
    goto parse_args
)
if /i "%~1"=="--help" goto show_help
echo [ERROR] Unknown argument: %~1
echo Use --help for usage information
exit /b 1

:show_help
echo Usage: build-android.bat [options]
echo.
echo Options:
echo   --clean         Clean target\target_android before building
echo   --skip-plugin   Skip plugin .so build
echo   --target arch   Target architecture (default: arm64-v8a)
echo                   Supported: arm64-v8a, armeabi-v7a, x86_64, x86
echo   --api level     Android API level (default: 36)
echo   --help          Show this help
echo.
echo Prerequisites:
echo   - Android NDK installed (use setup-android.bat)
echo   - cargo-ndk: cargo install cargo-ndk
echo   - Rust target: rustup target add aarch64-linux-android
exit /b 0

:done_parsing

REM ============================================
REM Map target architecture to Rust target triple
REM ============================================
set RUST_TARGET=
if "%TARGET_ARCH%"=="arm64-v8a"    set RUST_TARGET=aarch64-linux-android
if "%TARGET_ARCH%"=="armeabi-v7a"  set RUST_TARGET=armv7-linux-androideabi
if "%TARGET_ARCH%"=="x86_64"       set RUST_TARGET=x86_64-linux-android
if "%TARGET_ARCH%"=="x86"          set RUST_TARGET=i686-linux-android

if "%RUST_TARGET%"=="" (
    echo [ERROR] Unsupported architecture: %TARGET_ARCH%
    echo   Supported: arm64-v8a, armeabi-v7a, x86_64, x86
    exit /b 1
)

REM ============================================
REM Banner
REM ============================================
set VERSION=0.0.0.1
for /f "tokens=*" %%i in ('git describe --tags --abbrev=0 2^>nul') do set VERSION=%%i

set GIT_COMMIT=unknown
for /f "tokens=*" %%i in ('git rev-parse --short HEAD 2^>nul') do set GIT_COMMIT=%%i

for /f "tokens=*" %%i in ('rustc --version 2^>nul') do set RUSTC_VERSION=%%i

echo ============================================
echo  NemesisBot Android Cross-Compile
echo ============================================
echo  Version:     %VERSION%
echo  Git Commit:  %GIT_COMMIT%
echo  Rustc:       %RUSTC_VERSION%
echo  Target:      %TARGET_ARCH% ^(%RUST_TARGET%^)
echo  API Level:   %API_LEVEL%
echo  Target Dir:  target\target_android
echo  Output Dir:  bin\bin_android\%TARGET_ARCH%
echo ============================================
echo.

REM ============================================
REM Prerequisites Check
REM ============================================
echo [Prerequisites] Checking tools...

where cargo-ndk >nul 2>&1
if errorlevel 1 (
    echo   [MISS] cargo-ndk not found
    echo.
    echo   Install with: cargo install cargo-ndk
    exit /b 1
) else (
    for /f "tokens=*" %%i in ('cargo ndk --version 2^>nul') do echo   [OK]   cargo-ndk: %%i
)

rustup target list --installed 2>nul | findstr /c:"%RUST_TARGET%" >nul 2>&1
if errorlevel 1 (
    echo   [MISS] Rust target %RUST_TARGET% not installed
    echo.
    echo   Install with: rustup target add %RUST_TARGET%
    exit /b 1
) else (
    echo   [OK]   Rust target: %RUST_TARGET%
)

if "%ANDROID_NDK_HOME%"=="" (
    echo   [WARN] ANDROID_NDK_HOME not set
    echo          cargo-ndk will try to auto-detect NDK
) else (
    echo   [OK]   ANDROID_NDK_HOME: %ANDROID_NDK_HOME%
)

echo.

REM ============================================
REM Set environment for Android build
REM ============================================
set CARGO_TARGET_DIR=target\target_android
set CARGO_NDK_PLATFORM=%API_LEVEL%

REM ============================================
REM Step 1: Clean (optional)
REM ============================================
if "%CLEAN%"=="1" (
    echo [Step 1/4] Cleaning target\target_android...
    if exist "target\target_android" (
        rmdir /s /q "target\target_android" 2>nul
        echo   OK Cleaned
    ) else (
        echo   OK Nothing to clean
    )
    echo.
) else (
    echo [Step 1/4] Clean skipped ^(use --clean to enable^)
    echo.
)

REM ============================================
REM Step 2: Build Vue frontend (web dashboard)
REM ============================================
echo [Step 2/4] Building Vue frontend...

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

    REM Always run npm install to ensure all declared dependencies are present.
    REM npm skips already-installed packages, so this is fast when nothing is missing.
    echo   Checking npm dependencies...
    call npm install --silent
    if errorlevel 1 (
        popd
        echo   WARN npm install failed, skipping Vue build
        echo.
        goto step3_android
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

:step3_android

REM ============================================
REM Step 3: Build main workspace (release, Android)
REM ============================================
echo [Step 3/4] Building Android release ^(%TARGET_ARCH%^)...

echo   Injecting version info...
set NEMESISBOT_VERSION=%VERSION%
set NEMESISBOT_GIT_COMMIT=%GIT_COMMIT%

cargo ndk -t %TARGET_ARCH% build --release -p nemesisbot
if errorlevel 1 (
    echo.
    echo [ERROR] Android build failed!
    pause
    exit /b 1
)

echo   OK Build completed
echo.

REM ============================================
REM Step 4: Copy to bin\bin_android\
REM ============================================
echo [Step 4/4] Copying to bin\bin_android\%TARGET_ARCH%\...

set BIN_DIR=bin\bin_android\%TARGET_ARCH%
if not exist "%BIN_DIR%" mkdir "%BIN_DIR%"
if not exist "%BIN_DIR%\plugins" mkdir "%BIN_DIR%\plugins"
if not exist "%BIN_DIR%\tests" mkdir "%BIN_DIR%\tests"

set COPIED=0

REM Copy main binary
set RELEASE_DIR=target\target_android\%RUST_TARGET%\release
if exist "%RELEASE_DIR%\nemesisbot" (
    copy /y "%RELEASE_DIR%\nemesisbot" "%BIN_DIR%\" >nul 2>&1
    set /a COPIED+=1
    echo   OK nemesisbot copied
) else (
    echo   WARN nemesisbot not found in %RELEASE_DIR%
)

REM Copy plugin .so files (built separately with NDK if available)
for %%s in (libplugin_ui.so libplugin-ui.so libplugin_onnx.so libplugin-onnx.so) do (
    if exist "%RELEASE_DIR%\%%s" (
        copy /y "%RELEASE_DIR%\%%s" "%BIN_DIR%\plugins\" >nul 2>&1
        set /a COPIED+=1
        echo   OK %%s copied
    )
)

REM Copy test-tools binaries
for %%e in (cluster-test integration-test mcp-server) do (
    if exist "%RELEASE_DIR%\%%e" (
        copy /y "%RELEASE_DIR%\%%e" "%BIN_DIR%\tests\" >nul 2>&1
        set /a COPIED+=1
    )
)

echo   OK Copied !COPIED! file^(s^) to %BIN_DIR%\
echo.

REM ============================================
REM Summary
REM ============================================
echo ============================================
echo  Android Build Summary
echo ============================================
echo  Version:   %VERSION%
echo  Commit:    %GIT_COMMIT%
echo  Target:    %TARGET_ARCH%
echo  API Level: %API_LEVEL%
echo.

if exist "%BIN_DIR%\nemesisbot" (
    for %%f in ("%BIN_DIR%\nemesisbot") do (
        set FSIZE=%%~zf
        set /a FSIZE_MB=!FSIZE! / 1048576
        echo  nemesisbot ^(!FSIZE_MB! MB^)
    )
)

echo.
echo [SUCCESS] Android build completed!
echo.
echo Output: %BIN_DIR%\nemesisbot
echo.
echo Deploy to device:
echo   adb push %BIN_DIR%\nemesisbot /data/local/tmp/
echo   adb shell chmod +x /data/local/tmp/nemesisbot
echo   adb shell /data/local/tmp/nemesisbot gateway
echo ============================================
echo.
