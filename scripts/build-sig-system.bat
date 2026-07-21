@echo off
REM 签名验证体系 v3 — 自动编译脚本
REM 产物输出 dist\：nemesis_verify.dll（固化 root + server URL 127.0.0.1）/ revoke-server.exe /
REM                verify-loader.exe / exe-sign-tool.exe / keys.json / root.pub
REM 用法：scripts\build-sig-system.bat
setlocal enableextensions

set SERVER_URL=http://127.0.0.1:7878

echo === 1/5 build 全部（首次，无固化）===
cargo build -p nemesis-verify -p revoke-server -p verify-loader -p exe-sign-tool
if errorlevel 1 (
    echo [FAIL] build 全部失败
    exit /b 1
)

echo.
echo === 2/5 gen-keys（keys.json + root.pub）===
target\debug\verify-loader gen-keys keys.json
if errorlevel 1 (
    echo [FAIL] gen-keys 失败
    exit /b 1
)

echo.
echo === 3/5 读 root pub（root.pub）===
set /p ROOT_PUB=<root.pub
echo root pub: %ROOT_PUB%

echo.
echo === 4/5 build DLL 固化 root + server URL（%SERVER_URL%）===
set NEMESIS_BUILD_ROOT_PUBKEY=%ROOT_PUB%
set NEMESIS_BUILD_REVOCATION_URL=%SERVER_URL%
cargo build -p nemesis-verify
if errorlevel 1 (
    echo [FAIL] DLL 固化 build 失败
    exit /b 1
)

echo.
echo === 5/5 拷贝产物到 dist + 生成 start-server.bat ===
if exist dist rmdir /s /q dist
mkdir dist
copy /Y target\debug\nemesis_verify.dll dist\ >nul
copy /Y target\debug\revoke-server.exe dist\ >nul
copy /Y target\debug\verify-loader.exe dist\ >nul
copy /Y target\debug\exe-sign-tool.exe dist\ >nul
copy /Y keys.json dist\ >nul
copy /Y root.pub dist\ >nul
echo @echo off > dist\start-server.bat
echo REM 启动签名服务端（DLL 内置 server 指向 127.0.0.1:7878）>> dist\start-server.bat
echo revoke-server --keys-file keys.json --bind 127.0.0.1:7878 --admin-token uat-token >> dist\start-server.bat
echo pause >> dist\start-server.bat

echo.
echo === 完成 ===
echo 产物在 dist\：
echo   nemesis_verify.dll  固化 root=%ROOT_PUB% + server=%SERVER_URL%
echo   revoke-server.exe   启动: revoke-server --keys-file keys.json --bind 127.0.0.1:7878 --admin-token ^<your-token^>
echo   verify-loader.exe   gen-keys / sign / verify / verify-self / view / verify-dll
echo   exe-sign-tool.exe   keygen / sign / verify
echo   keys.json + root.pub
exit /b 0
