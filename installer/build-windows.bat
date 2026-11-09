@echo off
setlocal enabledelayedexpansion

REM ============================================
REM NemesisBot 安装包构建脚本（Windows）
REM ============================================
REM 用法: installer\build-windows.bat [选项]
REM   （无参数）     构建 bot + 安装器 + 打包 → dist\NemesisBotSetup-<ver>-x64.exe
REM   --skip-bot     复用既有 bin\bin_windows（不重编主程序）
REM   --clean        传给主仓构建脚本（清理后构建）
REM   --sign <keys>  用 keys.json 签名安装包（绿级信任链；缺省 = 不签，
REM                  安装时显示黄色「未签名」提示）
REM   --anchor <hex> 编译期固化根锚（NEMESIS_BUILD_ROOT_ANCHOR；运行期等价
REM                  方式 = 安装器环境带 NEMESIS_ROOT_ANCHOR）
REM   --out <file>   输出路径（缺省 dist\NemesisBotSetup-<ver>-x64.exe）
REM   --help         显示帮助
REM
REM 全程在 installer\ 独立 workspace 内运作，产物落 installer\dist\，
REM 对外部目录零污染（隔离契约见 installer\README.md）。

cd /d "%~dp0"

set SKIP_BOT=0
set CLEAN=0
set SIGN=
set ANCHOR=
set OUT=

:parse_args
if "%~1"=="" goto done_parse
if /i "%~1"=="--skip-bot" ( set SKIP_BOT=1 & shift & goto parse_args )
if /i "%~1"=="--clean" ( set CLEAN=1 & shift & goto parse_args )
if /i "%~1"=="--sign" ( set SIGN=%~2 & shift & shift & goto parse_args )
if /i "%~1"=="--anchor" ( set ANCHOR=%~2 & shift & shift & goto parse_args )
if /i "%~1"=="--out" ( set OUT=%~2 & shift & shift & goto parse_args )
if /i "%~1"=="--help" goto show_help
echo [ERROR] 未知参数: %~1
echo 用 --help 查看用法
exit /b 1
:done_parse

REM ① 主程序构建（主仓脚本；产物 bin\bin_windows\）。
if %SKIP_BOT%==0 (
    echo [1/5] 构建主程序（scripts\build-windows.bat）...
    set BOT_ARGS=
    if %CLEAN%==1 set BOT_ARGS=--clean
    call "%~dp0..\scripts\build-windows.bat" !BOT_ARGS!
    if errorlevel 1 (
        echo [ERROR] 主程序构建失败
        exit /b 1
    )
) else (
    echo [1/5] 跳过主程序构建（复用既有 bin\bin_windows）
    if not exist "%~dp0..\bin\bin_windows\nemesisbot.exe" (
        echo [ERROR] bin\bin_windows\nemesisbot.exe 缺席——去掉 --skip-bot 先构建
        exit /b 1
    )
)

REM ② 图标进载荷（share\icon.png；桌面入口/展示面引用）。
if not exist "%~dp0..\bin\bin_windows\share" mkdir "%~dp0..\bin\bin_windows\share"
copy /Y "%~dp0..\web\public\favicon.png" "%~dp0..\bin\bin_windows\share\icon.png" >nul
echo [2/5] 图标已同步 share\icon.png

REM ③ 安装器构建（本 workspace release；骨架 = 未签名原始 exe）。
echo [3/5] 构建安装器（cargo build --release）...
cd /d "%~dp0"
if not "%ANCHOR%"=="" set NEMESIS_BUILD_ROOT_ANCHOR=%ANCHOR%
cargo build --release
if errorlevel 1 (
    echo [ERROR] 安装器构建失败
    exit /b 1
)
if not exist "%~dp0target\release\nbsetup-win.exe" (
    echo [ERROR] nbsetup-win.exe 未产出
    exit /b 1
)

REM ④ 版本号（与主仓构建脚本同源：git tag，缺省 0.0.0.1）。
set VERSION=0.0.0.1
for /f "tokens=*" %%i in ('git describe --tags --abbrev=0 2^>nul') do set VERSION=%%i

REM ⑤ 打包 NBSETUP1（payload = bin_windows，骨架 = nbsetup-win.exe）。
if "%OUT%"=="" set OUT=%~dp0dist\NemesisBotSetup-%VERSION%-x64.exe
if not exist "%~dp0dist" mkdir "%~dp0dist"
echo [4/5] 打包 NBSETUP1（版本 %VERSION%）...
set PACK_ARGS=pack --payload "%~dp0..\bin\bin_windows" --skeleton "%~dp0target\release\nbsetup-win.exe" --out "%OUT%" --version %VERSION% --platform windows
if not "%SIGN%"=="" set PACK_ARGS=%PACK_ARGS% --sign "%SIGN%"
"%~dp0target\release\pack.exe" %PACK_ARGS%
if errorlevel 1 (
    echo [ERROR] 打包失败
    exit /b 1
)

REM ⑤b 整包签名（PE 证书表；安装器启动外壳自检的绿级来源——两层信任链
REM 的第二层，第一层 = pack --sign 的 manifest blob 签名）。
if not "%SIGN%"=="" (
    echo 签名整包（外壳自检绿级）...
    "%~dp0target\release\pack.exe" sign "%OUT%" --keys "%SIGN%"
    if errorlevel 1 (
        echo [ERROR] 整包签名失败
        exit /b 1
    )
)

echo [5/5] 检视成品...
"%~dp0target\release\pack.exe" inspect "%OUT%"
echo.
echo 产物: %OUT%
exit /b 0

:show_help
echo NemesisBot 安装包构建（Windows）
echo.
echo 用法: installer\build-windows.bat [选项]
echo   --skip-bot     复用既有 bin\bin_windows
echo   --clean        清理后构建主程序
echo   --sign ^<keys^>  keys.json 签名（绿级；缺省不签 = 黄色提示）
echo   --anchor ^<hex^> 编译期固化根锚
echo   --out ^<file^>   输出路径
exit /b 0
