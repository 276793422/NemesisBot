@echo off
REM install-clawhub-skill.bat
REM 从 ClawHub (https://clawhub.ai) 安装 skill 到 NemesisBot (Windows 版本)

if "%2"=="" (
    echo Usage: %0 ^<author^> ^<skill-name^> [output-name]
    echo.
    echo Example:
    echo   %0 steipete weather
    echo   %0 steipete weather weather-clawhub
    exit /b 1
)

set AUTHOR=%1
set SKILL_NAME=%2
set OUTPUT_NAME=%3
if "%OUTPUT_NAME%"=="" set OUTPUT_NAME=%SKILL_NAME%

REM NemesisBot skills 目录
set SKILL_DIR=%USERPROFILE%\.nemesisbot\workspace\skills\%OUTPUT_NAME%

echo 📦 Installing '%SKILL_NAME%' from '%AUTHOR%'...
echo.

REM 创建目录
echo 📁 Creating directory: %SKILL_DIR%
if not exist "%SKILL_DIR%" mkdir "%SKILL_DIR%"

REM 下载 SKILL.md
set SKILL_URL=https://raw.githubusercontent.com/openclaw/skills/main/skills/%AUTHOR%/%SKILL_NAME%/SKILL.md
echo 📥 Downloading from: %SKILL_URL%

curl -f -o "%SKILL_DIR%\SKILL.md" "%SKILL_URL%"

if %ERRORLEVEL% EQU 0 (
    echo.
    echo ✅ Skill '%OUTPUT_NAME%' installed successfully!
    echo.
    echo 验证安装:
    echo   nemesisbot skills list
    echo   nemesisbot skills show %OUTPUT_NAME%
    echo.
    echo 使用:
    echo   nemesisbot agent
    echo.
) else (
    echo.
    echo ❌ Failed to download skill
    echo 请检查:
    echo   1. 作者名称是否正确
    echo   2. Skill 名称是否正确
    echo   3. 网络连接是否正常
    echo.
    exit /b 1
)
