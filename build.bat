@echo off
setlocal enabledelayedexpansion

SET PATH=C:\AI\golang\go1.25.7\bin;%PATH%

REM ============================================
REM NemesisBot Build Script
REM ============================================

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
echo [Step 1/4] Copying workspace to nemesisbot/...
if exist "workspace" (
    xcopy "workspace" ".\nemesisbot\workspace\" /E /I /Y /Q
    if errorlevel 1 (
        echo [ERROR] Failed to copy workspace directory
        pause
        exit /b 1
    )
    echo [OK] Workspace copied successfully
) else (
    echo [WARNING] workspace directory not found in current directory, skipping copy
)
echo.

REM Step 1.5: Copy default to nemesisbot/
echo [Step 1.5/3] Copying default to nemesisbot/...
if exist "default" (
    xcopy "default" ".\nemesisbot\default\" /E /I /Y /Q
    if errorlevel 1 (
        echo [ERROR] Failed to copy default directory
        pause
        exit /b 1
    )
    echo [OK] Default directory copied successfully
) else (
    echo [WARNING] default directory not found in current directory, skipping copy
)
echo.

REM Step 1.6: Copy config to nemesisbot/
echo [Step 1.6/3] Copying config to nemesisbot/...
if exist "config" (
    xcopy "config" ".\nemesisbot\config\" /E /I /Y /Q
    if errorlevel 1 (
        echo [ERROR] Failed to copy config directory
        pause
        exit /b 1
    )
    echo [OK] Config directory copied successfully
) else (
    echo [WARNING] config directory not found in current directory, skipping copy
)
echo.

REM Step 2: Build with dynamic ldflags
echo [Step 2/3] Building nemesisbot.exe...
echo.

go build -ldflags "-X main.version=%VERSION% -X main.gitCommit=%GIT_COMMIT% -X main.buildTime=%BUILD_TIME% -X main.goVersion=%GO_VERSION% -s -w" -o nemesisbot.exe .\nemesisbot\

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
echo [Step 3/3] Cleaning up embedded directories...
if exist ".\nemesisbot\workspace" (
    rmdir /S /Q ".\nemesisbot\workspace"
    if errorlevel 1 (
        echo [WARNING] Failed to remove workspace directory
        echo Please manually delete: .\nemesisbot\workspace
    ) else (
        echo [OK] Workspace directory removed
    )
) else (
    echo [INFO] No workspace directory to clean
)

if exist ".\nemesisbot\default" (
    rmdir /S /Q ".\nemesisbot\default"
    if errorlevel 1 (
        echo [WARNING] Failed to remove default directory
        echo Please manually delete: .\nemesisbot\default
    ) else (
        echo [OK] Default directory removed
    )
) else (
    echo [INFO] No default directory to clean
)

if exist ".\nemesisbot\config" (
    rmdir /S /Q ".\nemesisbot\config"
    if errorlevel 1 (
        echo [WARNING] Failed to remove config directory
        echo Please manually delete: .\nemesisbot\config
    ) else (
        echo [OK] Config directory removed
    )
) else (
    echo [INFO] No config directory to clean
)
echo.

REM ============================================
REM Build Summary
REM ============================================
echo ============================================
echo Build Summary
echo ============================================
if exist "nemesisbot.exe" (
    echo Output file: nemesisbot.exe
    for %%A in ("nemesisbot.exe") do (
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
    echo You can now run: .\nemesisbot.exe gateway
) else (
    echo [ERROR] Output file not found!
)
echo ============================================
echo.


