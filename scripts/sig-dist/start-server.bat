@echo off
chcp 65001 >nul
REM ===========================================================================
REM  Start revoke-server (background). Keys generated on first run, not packed.
REM  Usage: start-server [keys_dir] [bind] [admin_token] [db_path]
REM    default: keys / 0.0.0.0:7878 / random token / data\revoke.db
REM  Uses %~dp0 to locate sibling exe (cwd-independent)
REM ===========================================================================
setlocal enabledelayedexpansion
set DP=%~dp0
set KEYS=%~1
if "%KEYS%"=="" set KEYS=keys
set BIND=%~2
if "%BIND%"=="" set BIND=0.0.0.0:7878
set TOKEN=%~3
if "%TOKEN%"=="" set TOKEN=admin-%RANDOM%
set DB=%~4
if "%DB%"=="" set DB=data\revoke.db

if not exist "%KEYS%\exe_sign.crkey" (
  echo [first time] generating keys to %KEYS% ...
  "%DP%exe-sign-tool.exe" keygen --out "%KEYS%"
)
if not exist data mkdir data
set /p CRKEY=<"%KEYS%\exe_sign.crkey"
set /p SIGNKEY=<"%KEYS%\exe_sign.key"
set /p SYMKEY=<"%KEYS%\exe_sign.sym"

echo.
echo Starting revoke-server: bind=%BIND%  db=%DB%
echo **********************************************
echo  admin token: %TOKEN%  (record this for Web UI login + admin API)
echo **********************************************
PowerShell -Command "Start-Process -WindowStyle Hidden -FilePath '%DP%revoke-server.exe' -ArgumentList '--crkey','%CRKEY%','--sign-key','%SIGNKEY%','--sym-key','%SYMKEY%','--db-url','%DB%','--admin-token','%TOKEN%','--bind','%BIND%'"
echo.
echo Started (hidden). Web UI: http://%BIND%
echo Stop: taskkill /F /IM revoke-server.exe
