@echo off
chcp 65001 >nul
REM ===========================================================================
REM  Test signing client: keygen + sign + verify (local + optional cloud)
REM  Usage: test-client [cloud_url]
REM    no arg = local only; with URL = local + cloud
REM  Uses %~dp0 to locate sibling exe (cwd-independent)
REM ===========================================================================
setlocal
set DP=%~dp0
set KEYS=keys
set CLOUD=%~1

if not exist "%KEYS%\exe_sign.key" (
  echo [keygen] generating keys to %KEYS% ...
  "%DP%exe-sign-tool.exe" keygen --out "%KEYS%"
)

REM test file (copy of exe-sign-tool itself, avoid corrupting original)
copy /Y "%DP%exe-sign-tool.exe" test-app.exe >nul

echo.
echo [1] sign (publisher=TestPub)
"%DP%exe-sign-tool.exe" sign test-app.exe --key-dir "%KEYS%" --publisher TestPub
if errorlevel 1 ( echo SIGN FAILED & exit /b 1 )

echo.
echo [2] verify local
"%DP%exe-sign-tool.exe" verify test-app.exe --key-dir "%KEYS%"

if not "%CLOUD%"=="" (
  echo.
  echo [3] verify cloud %CLOUD%
  "%DP%exe-sign-tool.exe" verify test-app.exe --key-dir "%KEYS%" --cloud-url "%CLOUD%"
)

echo.
echo Done. test-app.exe signed (local verify should be Valid).
