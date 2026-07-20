@echo off
chcp 65001 >nul
setlocal

echo [1/2] cargo build --release ...
cargo build --release -p exe-sign-tool -p revoke-server
if errorlevel 1 ( echo BUILD FAILED & exit /b 1 )

echo [2/2] packing dist\ ...
if exist dist rmdir /s /q dist
mkdir dist
copy /Y target\release\exe-sign-tool.exe dist\ >nul
copy /Y target\release\revoke-server.exe dist\ >nul
copy /Y scripts\sig-dist\start-server.bat dist\ >nul
copy /Y scripts\sig-dist\test-client.bat dist\ >nul
copy /Y scripts\sig-dist\README.md dist\ >nul

echo.
echo === dist\ packed ===
dir /b dist\
