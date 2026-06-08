@echo off
setlocal enabledelayedexpansion
chcp 65001 >nul 2>&1

REM ============================================================
REM NemesisBot Android Shell - Release Signing Script
REM ============================================================
REM
REM One-click: read sign.json -> generate keystore -> build
REM signed release APK -> output to bin directory.
REM
REM Usage:  sign.bat            Build signed release APK
REM         sign.bat --check    Check config without building
REM         sign.bat --help     Show help
REM
REM Config: sign.json (in same directory as this script)
REM ============================================================

cd /d "%~dp0"

set "SCRIPT_DIR=%~dp0"
set "APP_DIR=%SCRIPT_DIR%app"
set "JAVA_HOME=C:\Program Files\Android\Android Studio\jbr"
set "ANDROID_HOME=C:\Users\Zoo\AppData\Local\Android\Sdk"
set "KEYTOOL=%JAVA_HOME%\bin\keytool.exe"
set "JARSIGNER=%JAVA_HOME%\bin\jarsigner.exe"
set "CONFIG_FILE=%SCRIPT_DIR%sign.json"
set "KEYSTORE_FILE=%SCRIPT_DIR%nemesisbot-release.jks"
set "PROP_FILE=%SCRIPT_DIR%keystore.properties"

REM ============================================
REM Parse arguments
REM ============================================
if /i "%~1"=="--help" goto :show_help
if /i "%~1"=="--check" goto :check_only

REM ============================================
REM Banner
REM ============================================
echo.
echo ============================================
echo  NemesisBot APK Release Signing
echo ============================================
echo.

REM ============================================
REM Phase 1: Read sign.json
REM ============================================
echo [Phase 1/6] Reading sign.json...

if not exist "%CONFIG_FILE%" (
    echo   Config not found, generating default sign.json...
    call :write_default_config
    echo.
    echo   EDIT sign.json with your signing details, then re-run sign.bat
    echo   Or run again to use defaults.
    goto :eof
)

REM Read JSON values using simple text parsing (no jq dependency)
set "KEY_ALIAS="
set "KEY_PASSWORD="
set "STORE_PASSWORD="
set "VALIDITY_YEARS="
set "DNAME_CN="
set "DNAME_OU="
set "DNAME_O="
set "DNAME_L="
set "DNAME_ST="
set "DNAME_C="
set "OUTPUT_DIR="
set "OUTPUT_NAME="

for /f "usebackq tokens=1,* delims=:" %%a in ("%CONFIG_FILE%") do (
    set "LINE=%%a"
    set "VAL=%%b"
    call :parse_json_line
)

REM Apply defaults for empty fields
if "%KEY_ALIAS%"=="" set "KEY_ALIAS=nemesisbot"
if "%KEY_PASSWORD%"=="" set "KEY_PASSWORD=nemesisbot"
if "%STORE_PASSWORD%"=="" set "STORE_PASSWORD=nemesisbot"
if "%VALIDITY_YEARS%"=="" set "VALIDITY_YEARS=25"
if "%DNAME_CN%"=="" set "DNAME_CN=NemesisBot"
if "%DNAME_OU%"=="" set "DNAME_OU=Dev"
if "%DNAME_O%"=="" set "DNAME_O=NemesisBot"
if "%DNAME_L%"=="" set "DNAME_L=Beijing"
if "%DNAME_ST%"=="" set "DNAME_ST=Beijing"
if "%DNAME_C%"=="" set "DNAME_C=CN"
if "%OUTPUT_DIR%"=="" set "OUTPUT_DIR=..\..\bin\bin_android_apk"
if "%OUTPUT_NAME%"=="" set "OUTPUT_NAME=nemesisbot-arm64-release.apk"

REM Calculate validity in days
set /a VALIDITY_DAYS=VALIDITY_YEARS*365

echo   Key Alias:    %KEY_ALIAS%
echo   Validity:     %VALIDITY_YEARS% years ^(%VALIDITY_DAYS% days^)
echo   DName:        CN=%DNAME_CN%, O=%DNAME_O%, C=%DNAME_C%
echo   Output:       %OUTPUT_DIR%\%OUTPUT_NAME%
echo.

REM ============================================
REM Phase 2: Generate keystore (if not exists)
REM ============================================
echo [Phase 2/6] Checking keystore...

if exist "%KEYSTORE_FILE%" (
    echo   Keystore exists: %KEYSTORE_FILE%
    for %%A in ("%KEYSTORE_FILE%") do echo   Size: %%~zA bytes
) else (
    echo   Generating new keystore...
    if not exist "%KEYTOOL%" (
        echo   [ERROR] keytool not found: %KEYTOOL%
        echo   Install Android Studio or set JAVA_HOME correctly.
        exit /b 1
    )
    "%KEYTOOL%" -genkeypair -v ^
        -keystore "%KEYSTORE_FILE%" ^
        -keyalg RSA ^
        -keysize 2048 ^
        -validity %VALIDITY_DAYS% ^
        -alias %KEY_ALIAS% ^
        -storepass %STORE_PASSWORD% ^
        -keypass %KEY_PASSWORD% ^
        -dname "CN=%DNAME_CN%, OU=%DNAME_OU%, O=%DNAME_O%, L=%DNAME_L%, ST=%DNAME_ST%, C=%DNAME_C%"
    if errorlevel 1 (
        echo   [ERROR] Failed to generate keystore.
        exit /b 1
    )
    echo   Keystore created successfully.
)
echo.

REM ============================================
REM Phase 3: Generate keystore.properties
REM ============================================
echo [Phase 3/6] Writing keystore.properties...

(
    echo storeFile=nemesisbot-release.jks
    echo storePassword=%STORE_PASSWORD%
    echo keyAlias=%KEY_ALIAS%
    echo keyPassword=%KEY_PASSWORD%
) > "%PROP_FILE%"

if errorlevel 1 (
    echo   [ERROR] Failed to write keystore.properties
    exit /b 1
)
echo   OK - keystore.properties written
echo.

REM ============================================
REM Phase 4: Build Release APK
REM ============================================
echo [Phase 4/6] Building signed release APK...

REM Ensure Rust binary is in jniLibs
set "JNI_DIR=%APP_DIR%\src\main\jniLibs\arm64-v8a"
set "JNI_LIB=%JNI_DIR%\libnemesisbot.so"

if not exist "%JNI_LIB%" (
    echo   Rust binary not found, copying...
    call :do_copy_bin
    if errorlevel 1 (
        echo   [ERROR] Failed to copy Rust binary.
        call :cleanup
        exit /b 1
    )
) else (
    for %%A in ("%JNI_LIB%") do echo   Rust binary ready: %%~zA bytes
)

echo   Running Gradle assembleRelease...
cd /d "%SCRIPT_DIR%"
call "%SCRIPT_DIR%gradlew.bat" assembleRelease
if errorlevel 1 (
    echo.
    echo   [ERROR] Gradle build failed.
    call :cleanup
    exit /b 1
)
echo.

REM ============================================
REM Phase 5: Verify and output
REM ============================================
echo [Phase 5/6] Verifying signed APK...

set "APK_SRC=%APP_DIR%\build\outputs\apk\release\app-release.apk"
if not exist "%APK_SRC%" (
    REM Try unsigned variant name
    set "APK_SRC=%APP_DIR%\build\outputs\apk\release\app-release-unsigned.apk"
)

if not exist "!APK_SRC!" (
    echo   [ERROR] APK not found at expected location.
    echo   Expected: %APP_DIR%\build\outputs\apk\release\app-release.apk
    call :cleanup
    exit /b 1
)

REM Verify signature with jarsigner
"%JARSIGNER%" -verify "!APK_SRC!" >nul 2>&1
if errorlevel 1 (
    echo   [WARNING] APK signature verification failed - APK may be unsigned.
    echo   This can happen if keystore.properties was not read by Gradle.
) else (
    echo   APK signature verified OK.
)

REM Copy to output directory
set "OUT_DIR=%SCRIPT_DIR%%OUTPUT_DIR%"
if not exist "%OUT_DIR%" mkdir "%OUT_DIR%"

copy /y "!APK_SRC!" "%OUT_DIR%\%OUTPUT_NAME%" >nul 2>&1
if errorlevel 1 (
    echo   [ERROR] Failed to copy APK to output directory.
    call :cleanup
    exit /b 1
)

for %%A in ("%OUT_DIR%\%OUTPUT_NAME%") do (
    set /a SIZE_MB=%%~zA / 1048576
    echo   APK: %%~zA bytes ^(!SIZE_MB! MB^)
)
echo.

REM ============================================
REM Phase 6: Cleanup
REM ============================================
:cleanup
echo [Phase 6/6] Cleanup...
if exist "%PROP_FILE%" (
    del /q "%PROP_FILE%" 2>nul
    echo   Removed keystore.properties
)
echo.

REM ============================================
REM Done
REM ============================================
echo ============================================
echo  Release Signing Complete
echo ============================================
echo  APK:    %OUT_DIR%\%OUTPUT_NAME%
echo  Key:    %KEYSTORE_FILE%
echo  Alias:  %KEY_ALIAS%
echo.
echo  Install:    adb install -r %OUT_DIR%\%OUTPUT_NAME%
echo  Reinstall:  adb uninstall com.nemesisbot.android ^&^& adb install %OUT_DIR%\%OUTPUT_NAME%
echo  Launch:     adb shell am start -n com.nemesisbot.android/.MainActivity
echo ============================================
echo.
goto :eof

REM ============================================
REM Subroutines
REM ============================================

:do_copy_bin
set "JNI_DIR=%APP_DIR%\src\main\jniLibs\arm64-v8a"
if not exist "%JNI_DIR%" mkdir "%JNI_DIR%"

set "SRC_BIN="
if exist "%SCRIPT_DIR%..\..\bin\bin_android\arm64-v8a\nemesisbot" (
    set "SRC_BIN=%SCRIPT_DIR%..\..\bin\bin_android\arm64-v8a\nemesisbot"
) else if exist "%SCRIPT_DIR%..\..\..\bin\bin_android\arm64-v8a\nemesisbot" (
    set "SRC_BIN=%SCRIPT_DIR%..\..\..\bin\bin_android\arm64-v8a\nemesisbot"
)

if not defined SRC_BIN (
    echo   [ERROR] Rust binary not found. Run build-android.bat first.
    exit /b 1
)

copy /Y "%SRC_BIN%" "%JNI_DIR%\libnemesisbot.so" >nul 2>&1
if errorlevel 1 (
    echo   [ERROR] Failed to copy binary.
    exit /b 1
)
for %%A in ("%JNI_DIR%\libnemesisbot.so") do echo   Copied: %%~zA bytes
exit /b 0

:parse_json_line
REM Simple JSON key-value parser (handles flat structure)
set "L=!LINE!"
set "V=!VAL!"

REM Trim quotes and whitespace from value
set "V=!V:"=!"
set "V=!V: =!"
set "V=!V:,=!"
set "V=!V:}=!"
set "V=!V:]=!"
set "V=!V:}=!"

REM Trim whitespace from key
set "L=!L: =!"

REM Match known keys
if "!L!"=="\"key_alias\"" set "KEY_ALIAS=!V!"
if "!L!"=="\"key_password\"" set "KEY_PASSWORD=!V!"
if "!L!"=="\"store_password\"" set "STORE_PASSWORD=!V!"
if "!L!"=="\"validity_years\"" set "VALIDITY_YEARS=!V!"
if "!L!"=="\"CN\"" set "DNAME_CN=!V!"
if "!L!"=="\"OU\"" set "DNAME_OU=!V!"
if "!L!"=="\"O\"" set "DNAME_O=!V!"
if "!L!"=="\"L\"" set "DNAME_L=!V!"
if "!L!"=="\"ST\"" set "DNAME_ST=!V!"
if "!L!"=="\"C\"" set "DNAME_C=!V!"
if "!L!"=="\"output_dir\"" set "OUTPUT_DIR=!V!"
if "!L!"=="\"output_name\"" set "OUTPUT_NAME=!V!"
exit /b 0

:write_default_config
(
    echo {
    echo   "key_alias": "nemesisbot",
    echo   "key_password": "",
    echo   "store_password": "",
    echo   "validity_years": 25,
    echo   "dname": {
    echo     "CN": "NemesisBot",
    echo     "OU": "Dev",
    echo     "O": "NemesisBot",
    echo     "L": "",
    echo     "ST": "",
    echo     "C": "CN"
    echo   },
    echo   "output_dir": "../../bin/bin_android_apk",
    echo   "output_name": "nemesisbot-arm64-release.apk"
    echo }
) > "%CONFIG_FILE%"
echo   Default sign.json created at: %CONFIG_FILE%
exit /b 0

:check_only
echo.
echo ============================================
echo  Sign Config Check
echo ============================================
echo.
if not exist "%CONFIG_FILE%" (
    echo   sign.json not found. Run sign.bat to generate default config.
    goto :eof
)
echo   Config file: %CONFIG_FILE%
type "%CONFIG_FILE%"
echo.
echo   Keystore: %KEYSTORE_FILE%
if exist "%KEYSTORE_FILE%" (
    echo   [EXISTS]
) else (
    echo   [NOT YET CREATED]
)
echo.
goto :eof

:show_help
echo.
echo Usage: sign.bat [options]
echo.
echo Options:
echo   (none)     Build signed release APK
echo   --check    Show config without building
echo   --help     Show this help
echo.
echo Config file: sign.json (edit before running)
echo   key_alias       - Keystore alias name
echo   key_password    - Key password (empty = default "nemesisbot")
echo   store_password  - Store password (empty = default "nemesisbot")
echo   validity_years  - Certificate validity in years
echo   dname.*         - Certificate distinguished name fields
echo   output_dir      - Output directory (relative to script)
echo   output_name     - Output APK filename
echo.
exit /b 0
