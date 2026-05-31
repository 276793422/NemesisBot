@echo off
setlocal enabledelayedexpansion

REM Switch to project root (parent of scripts/)
cd /d "%~dp0\.."

REM ============================================
REM NemesisBot Android Environment Setup Script
REM ============================================
REM Detects and installs all dependencies required
REM to cross-compile NemesisBot for Android on Windows.
REM
REM Usage:
REM   scripts\setup-android.bat           # Detect + install
REM   scripts\setup-android.bat --dry-run # Detect only, no install
REM   scripts\setup-android.bat --help    # Show help
REM
REM Dependencies checked/installed:
REM   - Android Studio (optional, IDE)
REM   - Android SDK Command-Line Tools
REM   - Android NDK
REM   - Rust Android targets
REM   - cargo-ndk

REM ============================================
REM Parse Arguments
REM ============================================
set DRY_RUN=0

:parse_args
if "%~1"=="" goto done_parsing
if /i "%~1"=="--dry-run" (
    set DRY_RUN=1
    shift
    goto parse_args
)
if /i "%~1"=="--help" goto show_help
echo [ERROR] Unknown argument: %~1
echo Use --help for usage information
exit /b 1

:show_help
echo Usage: scripts\setup-android.bat [options]
echo.
echo Options:
echo   --dry-run  Detect missing dependencies without installing
echo   --help     Show this help
exit /b 0

:done_parsing

REM ============================================
REM Banner
REM ============================================
echo ============================================
echo  NemesisBot Android Environment Setup
echo ============================================
if "%DRY_RUN%"=="1" (
    echo  Mode: DRY RUN (detect only^)
) else (
    echo  Mode: INSTALL
)
echo  OS: Windows
echo ============================================
echo.

set ISSUES=0

REM ============================================
REM Helper macros
REM ============================================
REM We use subroutine-style printing via labels isn't convenient in bat,
REM so we use inline echo with prefixes.

REM ============================================
REM Step 1: Check Android Studio
REM ============================================
echo [Step 1/5] Android Studio

set STUDIO_FOUND=0
set "STUDIO_PATH="

REM Check common install locations
for %%p in (
    "%LOCALAPPDATA%\Android Studio"
    "%ProgramFiles%\Android\Android Studio"
    "%ProgramFiles(x86)%\Android\Android Studio"
    "C:\Program Files\Android\Android Studio"
) do (
    if exist %%p\bin\studio64.exe (
        set STUDIO_FOUND=1
        set "STUDIO_PATH=%%p"
    )
)

REM Also check registry for install path
if "%STUDIO_FOUND%"=="0" (
    for /f "tokens=2*" %%a in ('reg query "HKLM\SOFTWARE\Android Studio" /v Path 2^>nul') do (
        if exist "%%b\bin\studio64.exe" (
            set STUDIO_FOUND=1
            set "STUDIO_PATH=%%b"
        )
    )
)

if "%STUDIO_FOUND%"=="1" (
    echo   [OK]   Android Studio found: !STUDIO_PATH!
) else (
    echo   [SKIP] Android Studio not found (optional, only needed for IDE^)
)

echo.

REM ============================================
REM Step 2: Detect Android SDK
REM ============================================
echo [Step 2/5] Android SDK

set "SDK_HOME="
set SDK_FOUND=0

REM Check ANDROID_HOME env var first
if not "%ANDROID_HOME%"=="" (
    if exist "%ANDROID_HOME%" (
        set "SDK_HOME=%ANDROID_HOME%"
        set SDK_FOUND=1
    )
)

REM Check ANDROID_SDK_ROOT
if "%SDK_FOUND%"=="0" (
    if not "%ANDROID_SDK_ROOT%"=="" (
        if exist "%ANDROID_SDK_ROOT%" (
            set "SDK_HOME=%ANDROID_SDK_ROOT%"
            set SDK_FOUND=1
        )
    )
)

REM Auto-detect from common locations
if "%SDK_FOUND%"=="0" (
    for %%p in (
        "%LOCALAPPDATA%\Android\Sdk"
        "%USERPROFILE%\AppData\Local\Android\Sdk"
        "%ANDROID_HOME%"
    ) do (
        if exist %%p (
            set "SDK_HOME=%%~p"
            set SDK_FOUND=1
        )
    )
)

if "%SDK_FOUND%"=="1" (
    echo   [OK]   Android SDK found: !SDK_HOME!
) else (
    echo   [MISS] Android SDK not found
    echo.
    echo   The Android SDK provides build tools, platform tools, and the
    echo   SDK manager needed to install NDK and other components.
    echo.
    echo   Install options:
    echo     1. Install Android Studio (includes SDK): https://developer.android.com/studio
    echo     2. Install SDK command-line tools only:
    echo        https://developer.android.com/studio#command-line-tools-only
    echo.
    set /a ISSUES+=1
)

echo.

REM ============================================
REM Step 3: Detect Android NDK
REM ============================================
echo [Step 3/5] Android NDK

set "NDK_HOME="
set NDK_FOUND=0

REM Check ANDROID_NDK_HOME env var
if not "%ANDROID_NDK_HOME%"=="" (
    if exist "%ANDROID_NDK_HOME%\ndk-build" (
        set "NDK_HOME=%ANDROID_NDK_HOME%"
        set NDK_FOUND=1
    )
    REM Also check if it points directly to an NDK version dir
    if not exist "%ANDROID_NDK_HOME%\ndk-build" (
        if exist "%ANDROID_NDK_HOME%\toolchains\llvm\prebuilt" (
            set "NDK_HOME=%ANDROID_NDK_HOME%"
            set NDK_FOUND=1
        )
    )
)

REM Check inside SDK directory
if "%SDK_FOUND%"=="1" (
    if "%NDK_FOUND%"=="0" (
        if exist "!SDK_HOME!\ndk" (
            REM Find the latest NDK version
            set "LATEST_NDK="
            for /f "delims=" %%v in ('dir /b /ad "!SDK_HOME!\ndk" 2^>nul ^| sort') do (
                set "LATEST_NDK=%%v"
            )
            if not "!LATEST_NDK!"=="" (
                set "NDK_HOME=!SDK_HOME!\ndk\!LATEST_NDK!"
                set NDK_FOUND=1
            )
        )
    )
)

REM Also check common standalone locations
if "%NDK_FOUND%"=="0" (
    for %%p in (
        "%LOCALAPPDATA%\Android\Sdk\ndk"
        "C:\Android\NDK"
    ) do (
        if exist %%p (
            set "LATEST_NDK="
            for /f "delims=" %%v in ('dir /b /ad %%p 2^>nul ^| sort') do (
                set "LATEST_NDK=%%v"
            )
            if not "!LATEST_NDK!"=="" (
                set "NDK_HOME=%%~p\!LATEST_NDK!"
                set NDK_FOUND=1
            )
        )
    )
)

if "%NDK_FOUND%"=="1" (
    REM Extract NDK version from directory name
    for %%f in ("!NDK_HOME!") do set NDK_DIR_NAME=%%~nxf
    echo   [OK]   Android NDK found: !NDK_HOME!
    echo         Version: !NDK_DIR_NAME!
) else (
    echo   [MISS] Android NDK not found
    echo.
    if "%SDK_FOUND%"=="1" (
        echo   Found SDK at: !SDK_HOME!
        echo.
        if "%DRY_RUN%"=="0" (
            echo   Attempting to install NDK via sdkmanager...
            echo.
            set "SDKMANAGER=!SDK_HOME!\cmdline-tools\latest\bin\sdkmanager.bat"
            if not exist "!SDKMANAGER!" (
                REM Try finding sdkmanager in other locations
                for /f "delims=" %%s in ('dir /s /b "!SDK_HOME!\sdkmanager.bat" 2^>nul') do (
                    set "SDKMANAGER=%%s"
                )
            )
            if exist "!SDKMANAGER!" (
                echo   Using: !SDKMANAGER!
                echo.
                echo   Installing NDK (accepting licenses^)...
                call "!SDKMANAGER!" --install "ndk;30.0.14904198" 2>&1
                if errorlevel 1 (
                    echo   [WARN] NDK installation via sdkmanager failed
                    echo   Try manually:
                    echo     !SDKMANAGER! --install "ndk;30.0.14904198"
                ) else (
                    echo   [OK]   NDK installed successfully
                    REM Re-detect NDK
                    if exist "!SDK_HOME!\ndk\30.0.14904198" (
                        set "NDK_HOME=!SDK_HOME!\ndk\30.0.14904198"
                        set NDK_FOUND=1
                    )
                )
            ) else (
                echo   [WARN] sdkmanager not found in SDK directory
                echo.
                echo   Install options:
                echo     1. Install via Android Studio: SDK Manager -> NDK (Side by side^)
                echo     2. Install via command line:
                echo        !SDK_HOME!\cmdline-tools\latest\bin\sdkmanager --install "ndk;30.0.14904198"
                echo     3. Download manually: https://developer.android.com/ndk/downloads
            )
        ) else (
            echo   DRY RUN: would attempt to install NDK via sdkmanager
        )
    ) else (
        echo   Install options:
        echo     1. Install Android Studio (includes SDK Manager^):
        echo        https://developer.android.com/studio
        echo     2. Download NDK manually:
        echo        https://developer.android.com/ndk/downloads
    )
    set /a ISSUES+=1
)

echo.

REM ============================================
REM Step 4: Set environment variables
REM ============================================
echo [Step 4/5] Environment Variables

set ENV_CHANGED=0

REM ANDROID_NDK_HOME
if "%NDK_FOUND%"=="1" (
    if not "%ANDROID_NDK_HOME%"=="!NDK_HOME!" (
        echo   [SET]  ANDROID_NDK_HOME=!NDK_HOME!
        set "ANDROID_NDK_HOME=!NDK_HOME!"
        set ENV_CHANGED=1
    ) else (
        echo   [OK]   ANDROID_NDK_HOME already set
    )
) else (
    echo   [SKIP] Cannot set ANDROID_NDK_HOME (NDK not found^)
)

REM ANDROID_HOME / ANDROID_SDK_ROOT
if "%SDK_FOUND%"=="1" (
    if not "%ANDROID_HOME%"=="!SDK_HOME!" (
        echo   [SET]  ANDROID_HOME=!SDK_HOME!
        set "ANDROID_HOME=!SDK_HOME!"
        set ENV_CHANGED=1
    ) else (
        echo   [OK]   ANDROID_HOME already set
    )
)

REM Persist to user environment (current session only, user must add permanently)
if "%ENV_CHANGED%"=="1" (
    echo.
    echo   [INFO] Environment variables set for this session.
    echo   To persist, add these to System Environment Variables:
    if "%NDK_FOUND%"=="1" (
        echo     ANDROID_NDK_HOME=!NDK_HOME!
    )
    if "%SDK_FOUND%"=="1" (
        echo     ANDROID_HOME=!SDK_HOME!
    )
    echo.
    echo   Or run this from an admin prompt to set permanently:
    if "%NDK_FOUND%"=="1" (
        echo     setx ANDROID_NDK_HOME "!NDK_HOME!" /M
    )
    if "%SDK_FOUND%"=="1" (
        echo     setx ANDROID_HOME "!SDK_HOME!" /M
    )
)

echo.

REM ============================================
REM Step 5: Rust toolchain for Android
REM ============================================
echo [Step 5/5] Rust Android Targets

set TARGETS_OK=0
set "NEEDED_TARGETS=aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android"
set MISSING_COUNT=0

for %%t in (%NEEDED_TARGETS%) do (
    rustup target list --installed 2>nul | findstr /c:"%%t" >nul 2>&1
    if not errorlevel 1 (
        echo   [OK]   %%t
    ) else (
        echo   [MISS] %%t
        set /a MISSING_COUNT+=1
        set "MISSING_!MISSING_COUNT!=%%t"
    )
)

if !MISSING_COUNT! gtr 0 (
    echo.
    if "%DRY_RUN%"=="0" (
        echo   Installing !MISSING_COUNT! missing target^(s^)...
        for /l %%i in (1,1,!MISSING_COUNT!) do (
            echo   Installing !MISSING_%%i!...
            rustup target add !MISSING_%%i!
            if not errorlevel 1 (
                echo   [OK]   !MISSING_%%i! installed
            ) else (
                echo   [FAIL] !MISSING_%%i! install failed
            )
        )
    ) else (
        echo   DRY RUN: would install !MISSING_COUNT! Rust target^(s^)
    )
)

REM Check cargo-ndk
echo.
where cargo-ndk >nul 2>&1
if not errorlevel 1 (
    for /f "tokens=*" %%i in ('cargo ndk --version 2^>nul') do echo   [OK]   cargo-ndk: %%i
) else (
    echo   [MISS] cargo-ndk not found
    if "%DRY_RUN%"=="0" (
        echo   Installing cargo-ndk...
        cargo install cargo-ndk
        if not errorlevel 1 (
            echo   [OK]   cargo-ndk installed
        ) else (
            echo   [FAIL] cargo-ndk install failed
            set /a ISSUES+=1
        )
    ) else (
        echo   DRY RUN: would install cargo-ndk via: cargo install cargo-ndk
    )
)

echo.

REM ============================================
REM Summary
REM ============================================
echo ============================================
echo  Setup Summary
echo ============================================

if "%DRY_RUN%"=="1" (
    echo  Mode: DRY RUN (no changes made^)
    echo.
)

if "%NDK_FOUND%"=="1" (
    echo  Android NDK:  OK ^(!NDK_HOME!^)
) else (
    echo  Android NDK:  MISSING
)

if "%SDK_FOUND%"=="1" (
    echo  Android SDK:  OK ^(!SDK_HOME!^)
) else (
    echo  Android SDK:  MISSING
)

if "%STUDIO_FOUND%"=="1" (
    echo  Android Studio: OK
) else (
    echo  Android Studio: Not installed (optional^)
)

echo.

if !ISSUES!==0 (
    echo  All dependencies satisfied!
    echo.
    echo  You can now build for Android:
    echo    scripts\build-android.bat
) else (
    echo  !ISSUES! issue^(s^) found. See details above.
    echo.
    if not "%SDK_FOUND%"=="1" (
        echo  Quick start:
        echo    1. Install Android Studio: https://developer.android.com/studio
        echo    2. Open Android Studio -> SDK Manager -> install NDK
        echo    3. Re-run this script to verify
    )
)

echo ============================================
echo.
