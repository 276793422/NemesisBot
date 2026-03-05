@echo off
setlocal enabledelayedexpansion

SET PATH=C:\AI\golang\go1.25.7\bin;%PATH%

REM ============================================
REM NemesisBot Build Script
REM ============================================

REM ============================================
REM Parse Command Line Arguments
REM ============================================
REM Usage: build.bat [output_filename] [powershell]
REM   output_filename: Name of the compiled binary (default: nemesisbot.exe)
REM   powershell: Optional flag to use PowerShell instead of cmd.exe

set BUILD_TAGS=
set OUTPUT_NAME=nemesisbot.exe

REM Parse arguments
:parse_args
if "%~1"=="" (
    goto done_parsing
)
if /i "%~1"=="powershell" (
    set BUILD_TAGS=-tags powershell
    echo [INFO] PowerShell build enabled
    shift
    goto parse_args
)
set OUTPUT_NAME=%~1
shift
goto parse_args

:done_parsing
if "%OUTPUT_NAME%"=="nemesisbot.exe" (
    echo [INFO] No output filename specified, using default: nemesisbot.exe
) else (
    echo [INFO] Output filename specified: %OUTPUT_NAME%
)
if not "%BUILD_TAGS%"=="" (
    echo [INFO] Building with PowerShell support
) else (
    echo [INFO] Building with cmd.exe (default, more reliable)
)
echo.

echo ============================================
echo NemesisBot Build Script
echo ============================================
echo.

REM ============================================
REM Dynamic Build Variables
REM ============================================
echo [INFO] Gathering build information...
echo.

REM Get version from git tag (or use default)
set VERSION=0.0.0.1
for /f "tokens=*" %%i in ('git describe --tags --abbrev=0 2^>nul') do set VERSION=%%i

REM Get git commit hash (short)
set GIT_COMMIT=unknown
for /f "tokens=*" %%i in ('git rev-parse --short HEAD 2^>nul') do set GIT_COMMIT=%%i

REM Get build time using PowerShell
for /f "delims=" %%i in ('powershell -Command "Get-Date -Format yyyy/MM/dd"') do set BUILD_TIME=%%i

REM Get Go version
for /f "tokens=3" %%i in ('go version') do set GO_VERSION_RAW=%%i
set GO_VERSION=%GO_VERSION_RAW:go=%

REM Display build information
echo Build Information:
echo   - Version:    %VERSION%
echo   - Git Commit: %GIT_COMMIT%
echo   - Build Time: %BUILD_TIME%
echo   - Go Version: %GO_VERSION%
echo.

REM Step 1: Copy workspace to nemesisbot/
echo [Step 1/3] Nothing
echo.

REM Step 2: Build with dynamic ldflags
echo [Step 2/3] Building %OUTPUT_NAME%...
echo.

go build %BUILD_TAGS% -ldflags "-X main.version=%VERSION% -X main.gitCommit=%GIT_COMMIT% -X main.buildTime=%BUILD_TIME% -X main.goVersion=%GO_VERSION% -s -w" -o %OUTPUT_NAME% .\nemesisbot\

if errorlevel 1 (
    echo.
    echo [ERROR] Build failed!
    echo Please check the error messages above.
    pause
    exit /b 1
)

echo [OK] Build completed successfully
echo.

REM Step 3: Clean up workspace and default directories
echo [Step 3/3] Nothing
echo.

REM ============================================
REM Build Summary
REM ============================================
echo ============================================
echo Build Summary
echo ============================================
if exist "%OUTPUT_NAME%" (
    echo Output file: %OUTPUT_NAME%
    for %%A in ("%OUTPUT_NAME%") do (
        set size=%%~zA
        set /a sizeMB=!size! / 1048576
        echo File size: !sizeMB! MB
    )
    echo.
    echo Build Info:
    echo   Version:    %VERSION%
    echo   Git Commit: %GIT_COMMIT%
    echo   Build Time: %BUILD_TIME%
    echo   Go Version: %GO_VERSION%
    echo.
    echo [SUCCESS] Build completed successfully!
    echo.
    echo You can now run: .\%OUTPUT_NAME% gateway
) else (
    echo [ERROR] Output file not found!
)
echo ============================================
echo.


