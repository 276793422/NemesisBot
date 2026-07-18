@echo off
REM ===========================================================================
REM  exe-sign-tool 一键脚本（Windows）
REM  功能：给指定可执行文件加签名，并自动验证检测。
REM  用法：quick-sign.bat ^<executable-path^>
REM  例：  quick-sign.bat C:\path\to\app.exe
REM        quick-sign.bat target\debug\nemesisbot.exe
REM ===========================================================================
setlocal enabledelayedexpansion

if "%~1"=="" (
  echo Usage: quick-sign.bat ^<executable-path^>
  echo   给指定可执行文件加签名，然后验证检测。
  echo   首次运行自动生成密钥（keygen），之后复用 keys\ 目录。
  echo.
  echo   例: quick-sign.bat C:\path\to\app.exe
  exit /b 1
)

set "TARGET=%~f1"
set "SCRIPT_DIR=%~dp0"
set "KEYS=%SCRIPT_DIR%keys"

if not exist "%TARGET%" (
  echo ERROR: file not found: %TARGET%
  exit /b 1
)

REM 切到项目根（cargo run 需在 workspace 根执行）
cd /d "%SCRIPT_DIR%..\.."

REM [1/3] 密钥：首次生成，之后复用
if not exist "%KEYS%\exe_sign.key" (
  echo [1/3] keygen -^> %KEYS%
  cargo run -q -p exe-sign-tool -- keygen --out "%KEYS%"
  if errorlevel 1 ( echo KEYGEN FAILED & exit /b 1 )
) else (
  echo [1/3] reuse existing keys: %KEYS%
)

REM [2/3] 加签名（原地追加 4KB envelope；--key-dir 自动找 key+sym）
echo [2/3] sign: %TARGET%
cargo run -q -p exe-sign-tool -- sign "%TARGET%" --key-dir "%KEYS%"
if errorlevel 1 ( echo SIGN FAILED & exit /b 1 )

REM [3/3] 验证检测（--key-dir 自动找 pub+sym）
echo [3/3] verify:
cargo run -q -p exe-sign-tool -- verify "%TARGET%" --key-dir "%KEYS%"
if errorlevel 1 ( echo VERIFY FAILED -^> 文件可能已被篡改或密钥不匹配 & exit /b 1 )

echo.
echo DONE: %TARGET%  signed and verified OK.
exit /b 0
