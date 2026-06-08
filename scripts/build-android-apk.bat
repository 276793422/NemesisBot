@echo off
setlocal enabledelayedexpansion

REM ============================================================
REM NemesisBot Android App (APK) Build Script
REM ============================================================
REM One-click pipeline: Rust binary -> JNI packaging -> APK
REM
REM Usage: scripts\build-android-app.bat [options]
REM   No arguments    - Build release APK (default)
REM   --clean         - Clean before building
REM   --debug         - Build debug APK instead of release
REM   --skip-rust     - Skip Rust cross-compile (use existing binary)
REM   --help          - Show help
REM
REM Prerequisites:
REM   - All prerequisites for build-android.bat (NDK, cargo-ndk, Rust target)
REM   - Android SDK + Java 17 (for Gradle)
REM
REM Output: bin\bin_android_apk\nemesisbot-arm64-release.apk
REM         bin\bin_android_apk\nemesisbot-arm64-debug.apk (--debug)
REM ============================================================

cd /d "%~dp0\.."

set CLEAN=0
set RELEASE=1
set SKIP_RUST=0

:parse_args
if "%~1"=="" goto done_parsing
if /i "%~1"=="--clean" (
    set CLEAN=1
    shift
    goto parse_args
)
if /i "%~1"=="--debug" (
    set RELEASE=0
    shift
    goto parse_args
)
if /i "%~1"=="--skip-rust" (
    set SKIP_RUST=1
    shift
    goto parse_args
)
if /i "%~1"=="--help" goto show_help
echo [ERROR] Unknown argument: %~1
echo Use --help for usage information
exit /b 1

:show_help
echo Usage: build-android-app.bat [options]
echo.
echo Options:
echo   --clean      Clean Rust target before building
echo   --debug      Build debug APK (default: release)
echo   --skip-rust  Skip Rust compile, use existing binary
echo   --help       Show this help
exit /b 0

:done_parsing

REM ============================================
REM Banner
REM ============================================
set VERSION=0.0.0.1
for /f "tokens=*" %%i in ('git describe --tags --abbrev=0 2^>nul') do set VERSION=%%i
set GIT_COMMIT=unknown
for /f "tokens=*" %%i in ('git rev-parse --short HEAD 2^>nul') do set GIT_COMMIT=%%i

echo.
echo ============================================
echo  NemesisBot Android App Builder
echo ============================================
echo  Version:  %VERSION%
echo  Commit:   %GIT_COMMIT%
echo  APK Type: %RELEASE%
echo  Skip Rust: %SKIP_RUST%
echo ============================================
echo.

set RUST_BIN=bin\bin_android\arm64-v8a\nemesisbot
set SHELL_DIR=test-tools\android-shell
set JNI_DIR=%SHELL_DIR%\app\src\main\jniLibs\arm64-v8a
set APK_OUT=bin\bin_android_apk

REM ============================================
REM Phase 1: Build Rust binary for Android
REM ============================================
if "%SKIP_RUST%"=="1" (
    if not exist "%RUST_BIN%" (
        echo [ERROR] --skip-rust but binary not found: %RUST_BIN%
        echo   Run without --skip-rust first.
        exit /b 1
    )
    echo [Phase 1/4] Skipping Rust build (--skip-rust)
    for %%A in ("%RUST_BIN%") do echo   Using existing: %%~zA bytes
    echo.
) else (
    echo [Phase 1/4] Building Rust binary for Android arm64-v8a...
    echo.
    set BUILD_ARGS=
    if "%CLEAN%"=="1" set BUILD_ARGS=--clean
    call scripts\build-android.bat !BUILD_ARGS!
    if errorlevel 1 (
        echo.
        echo [ERROR] Rust build failed! See errors above.
        exit /b 1
    )
    if not exist "%RUST_BIN%" (
        echo [ERROR] Rust binary not found after build: %RUST_BIN%
        exit /b 1
    )
    echo.
)

REM ============================================
REM Phase 2: Copy binary to jniLibs
REM ============================================
echo [Phase 2/4] Packaging binary into APK project...

if not exist "%JNI_DIR%" mkdir "%JNI_DIR%"

copy /y "%RUST_BIN%" "%JNI_DIR%\libnemesisbot.so" >nul 2>&1
if errorlevel 1 (
    echo [ERROR] Failed to copy binary to jniLibs
    exit /b 1
)
for %%A in ("%JNI_DIR%\libnemesisbot.so") do (
    set /a SIZE_MB=%%~zA / 1048576
    echo   OK %%~zA bytes ^(!SIZE_MB! MB^) -> jniLibs\arm64-v8a\libnemesisbot.so
)
echo.

REM ============================================
REM Phase 3: Build APK with Gradle
REM ============================================
set GRADLE_TASK=
set APK_SUFFIX=debug
if "%RELEASE%"=="1" (
    set GRADLE_TASK=release
    set APK_SUFFIX=release
)

echo [Phase 3/4] Building Android APK ^(%APK_SUFFIX%^)...

pushd "%SHELL_DIR%"
call build.bat %GRADLE_TASK%
if errorlevel 1 (
    popd
    echo.
    echo [ERROR] Gradle build failed! See errors above.
    exit /b 1
)
popd
echo.

REM ============================================
REM Phase 4: Copy APK to output directory
REM ============================================
echo [Phase 4/4] Collecting APK...

if not exist "%APK_OUT%" mkdir "%APK_OUT%"

set APK_SRC=
if "%RELEASE%"=="1" (
    if exist "%SHELL_DIR%\app\build\outputs\apk\release\app-release.apk" (
        set APK_SRC=%SHELL_DIR%\app\build\outputs\apk\release\app-release.apk
    ) else (
        set APK_SRC=%SHELL_DIR%\app\build\outputs\apk\release\app-release-unsigned.apk
    )
) else (
    set APK_SRC=%SHELL_DIR%\app\build\outputs\apk\debug\app-debug.apk
)

if not exist "%APK_SRC%" (
    echo [ERROR] APK not found at expected location: %APK_SRC%
    exit /b 1
)

set APK_NAME=nemesisbot-arm64-%APK_SUFFIX%.apk
copy /y "%APK_SRC%" "%APK_OUT%\%APK_NAME%" >nul 2>&1
if errorlevel 1 (
    echo [ERROR] Failed to copy APK to output directory
    exit /b 1
)

for %%A in ("%APK_OUT%\%APK_NAME%") do (
    set /a SIZE_MB=%%~zA / 1048576
    echo   OK %%~zA bytes ^(!SIZE_MB! MB^)
)

echo.
echo ============================================
echo  Android App Build Complete
echo ============================================
echo  APK:     %APK_OUT%\%APK_NAME%
echo  Version: %VERSION%
echo  Commit:  %GIT_COMMIT%
echo.
echo  Install:    adb install -r %APK_OUT%\%APK_NAME%
echo  Reinstall:  adb uninstall com.nemesisbot.android ^&^& adb install %APK_OUT%\%APK_NAME%
echo  Launch:     adb shell am start -n com.nemesisbot.android/.MainActivity
echo ============================================
echo.
