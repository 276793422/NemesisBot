@echo off
REM Sign tools build script (v3)
REM   Intermediate output: target/target_sign_tools/  (cargo build dir)
REM   Final tools:         bin/bin_sign_tools/         (dll/exe + keys.json + start-server.bat)
REM   DLL compiled-in root (= keys.json root) + server URL (127.0.0.1:7878)
REM Usage: scripts\build-sign-tools.bat
setlocal enableextensions

set CARGO_TARGET_DIR=target\target_sign_tools
set OUT=bin\bin_sign_tools
set SERVER_URL=http://127.0.0.1:7878

echo === 1/5 build all (intermediate -^> %CARGO_TARGET_DIR%) ===
cargo build -p nemesis-verify -p revoke-server -p verify-loader -p exe-sign-tool
if errorlevel 1 (
    echo [FAIL] build failed
    exit /b 1
)

echo.
echo === 2/5 gen-keys (keys.json + root.pub) ===
%CARGO_TARGET_DIR%\debug\verify-loader.exe gen-keys keys.json
if errorlevel 1 (
    echo [FAIL] gen-keys failed
    exit /b 1
)

echo.
echo === 3/5 read root pub ===
set /p ROOT_PUB=<root.pub
echo root pub: %ROOT_PUB%

echo.
echo === 4/5 build DLL with compiled-in root + server URL (%SERVER_URL%) ===
set NEMESIS_BUILD_ROOT_PUBKEY=%ROOT_PUB%
set NEMESIS_BUILD_REVOCATION_URL=%SERVER_URL%
cargo build -p nemesis-verify
if errorlevel 1 (
    echo [FAIL] DLL compiled-in build failed
    exit /b 1
)

echo.
echo === 5/5 copy tools to %OUT% ===
if exist %OUT% rmdir /s /q %OUT%
if exist %OUT% (
    echo [FAIL] cannot clean %OUT% - close verify-loader/revoke-server holding files, then rerun
    exit /b 1
)
mkdir %OUT%
copy /Y %CARGO_TARGET_DIR%\debug\nemesis_verify.dll %OUT%\ >nul
copy /Y %CARGO_TARGET_DIR%\debug\revoke-server.exe %OUT%\ >nul
copy /Y %CARGO_TARGET_DIR%\debug\verify-loader.exe %OUT%\ >nul
copy /Y %CARGO_TARGET_DIR%\debug\exe-sign-tool.exe %OUT%\ >nul
copy /Y keys.json %OUT%\ >nul
copy /Y root.pub %OUT%\ >nul
echo @echo off > %OUT%\start-server.bat
echo REM Start sign server (DLL built-in 127.0.0.1:7878; admin-token=uat-token) >> %OUT%\start-server.bat
echo revoke-server --keys-file keys.json --bind 127.0.0.1:7878 --admin-token uat-token >> %OUT%\start-server.bat
echo pause >> %OUT%\start-server.bat

echo.
echo === Done ===
echo Intermediate: %CARGO_TARGET_DIR%\
echo Tools in:     %OUT%\
echo   nemesis_verify.dll   compiled-in root=%ROOT_PUB% + server=%SERVER_URL%
echo   revoke-server.exe / verify-loader.exe / exe-sign-tool.exe
echo   keys.json / root.pub / start-server.bat
exit /b 0
