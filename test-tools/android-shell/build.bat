@echo off
setlocal EnableDelayedExpansion

:: ============================================================
:: NemesisBot Android Shell - Build Script
:: Usage:
::   build.bat          Build debug APK
::   build.bat release  Build release APK
::   build.bat clean    Clean non-project files (build, .gradle, IDE)
::   build.bat copy-bin Copy latest Rust binary to jniLibs
:: ============================================================

set "SCRIPT_DIR=%~dp0"
set "APP_DIR=%SCRIPT_DIR%app"
set "JAVA_HOME=C:\Program Files\Android\Android Studio\jbr"
set "ANDROID_HOME=C:\Users\Zoo\AppData\Local\Android\Sdk"

if /i "%~1"=="clean" goto :do_clean
if /i "%~1"=="copy-bin" goto :do_copy_bin
if /i "%~1"=="release" goto :do_build_release

:: Default: build debug
call :do_build
goto :eof

:: -----------------------------------------------------------
:do_copy_bin
:: -----------------------------------------------------------
echo.
echo [copy-bin] Copying Rust binary to jniLibs...

set "JNI_DIR=%APP_DIR%\src\main\jniLibs\arm64-v8a"
if not exist "%JNI_DIR%" mkdir "%JNI_DIR%"

set "SRC_BIN="
if exist "%SCRIPT_DIR%..\..\bin\bin_android\arm64-v8a\nemesisbot" (
    set "SRC_BIN=%SCRIPT_DIR%..\..\bin\bin_android\arm64-v8a\nemesisbot"
) else if exist "%SCRIPT_DIR%..\..\..\bin\bin_android\arm64-v8a\nemesisbot" (
    set "SRC_BIN=%SCRIPT_DIR%..\..\..\bin\bin_android\arm64-v8a\nemesisbot"
)

if not defined SRC_BIN (
    echo ERROR: Rust binary not found. Run build-android.bat first.
    exit /b 1
)

copy /Y "%SRC_BIN%" "%JNI_DIR%\libnemesisbot.so" >nul 2>&1
if errorlevel 1 (
    echo ERROR: Failed to copy binary.
    exit /b 1
)
for %%A in ("%JNI_DIR%\libnemesisbot.so") do echo   Copied: %%~zA bytes -> %JNI_DIR%\libnemesisbot.so
echo   Done.
goto :eof

:: -----------------------------------------------------------
:do_build
:: -----------------------------------------------------------
echo.
echo ===========================================
echo   NemesisBot Android Shell - Debug Build
echo ===========================================
echo.

set "JNI_LIB=%APP_DIR%\src\main\jniLibs\arm64-v8a\libnemesisbot.so"
if not exist "%JNI_LIB%" (
    echo [1/2] Rust binary not found in jniLibs, copying...
    call :do_copy_bin
    if errorlevel 1 exit /b 1
) else (
    echo [1/2] Rust binary ready.
)

echo [2/2] Building debug APK...
cd /d "%SCRIPT_DIR%"
call "%SCRIPT_DIR%gradlew.bat" assembleDebug
if errorlevel 1 (
    echo.
    echo ERROR: Build failed.
    exit /b 1
)

echo.
echo ===========================================
echo   BUILD SUCCESSFUL
echo ===========================================
for %%A in ("%APP_DIR%\build\outputs\apk\debug\app-debug.apk") do echo   APK: %%~zA bytes
echo   %APP_DIR%\build\outputs\apk\debug\app-debug.apk
echo.
echo   Install: adb install -r app\build\outputs\apk\debug\app-debug.apk
echo   Launch:  adb shell am start -n com.nemesisbot.android/.MainActivity
echo.
goto :eof

:: -----------------------------------------------------------
:do_build_release
:: -----------------------------------------------------------
echo.
echo ===========================================
echo   NemesisBot Android Shell - Release Build
echo ===========================================
echo.

set "JNI_LIB=%APP_DIR%\src\main\jniLibs\arm64-v8a\libnemesisbot.so"
if not exist "%JNI_LIB%" (
    echo [1/2] Rust binary not found in jniLibs, copying...
    call :do_copy_bin
    if errorlevel 1 exit /b 1
) else (
    echo [1/2] Rust binary ready.
)

echo [2/2] Building release APK...
cd /d "%SCRIPT_DIR%"
call "%SCRIPT_DIR%gradlew.bat" assembleRelease
if errorlevel 1 (
    echo.
    echo ERROR: Build failed.
    exit /b 1
)

echo.
echo ===========================================
echo   BUILD SUCCESSFUL
echo ===========================================
echo   %APP_DIR%\build\outputs\apk\release\app-release-unsigned.apk
echo.
goto :eof

:: -----------------------------------------------------------
:do_clean
:: -----------------------------------------------------------
echo.
echo ===========================================
echo   Cleaning non-project files...
echo ===========================================
echo.

echo [1/5] Running gradle clean...
cd /d "%SCRIPT_DIR%"
call "%SCRIPT_DIR%gradlew.bat" clean 2>nul

echo [2/5] Removing build directories...
if exist "%APP_DIR%\build" ( rmdir /s /q "%APP_DIR%\build" && echo   Removed app\build\ )
if exist "%SCRIPT_DIR%build" ( rmdir /s /q "%SCRIPT_DIR%build" && echo   Removed build\ )

echo [3/5] Removing .gradle cache...
if exist "%SCRIPT_DIR%.gradle" ( rmdir /s /q "%SCRIPT_DIR%.gradle" && echo   Removed .gradle\ )

echo [4/5] Removing IDE files...
if exist "%SCRIPT_DIR%.idea" ( rmdir /s /q "%SCRIPT_DIR%.idea" && echo   Removed .idea\ )
del /q "%SCRIPT_DIR%*.iml" 2>nul
del /q "%APP_DIR%\*.iml" 2>nul

echo [5/5] Removing local.properties...
if exist "%SCRIPT_DIR%local.properties" ( del /q "%SCRIPT_DIR%local.properties" && echo   Removed local.properties )

echo.
echo ===========================================
echo   Clean complete.
echo ===========================================
echo   To rebuild:           build.bat
echo   To refresh binary:    build.bat copy-bin
echo   Then build:           build.bat
echo.
goto :eof
