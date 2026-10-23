@echo off
setlocal enabledelayedexpansion
rem ============================================================
rem  Copy wasm sample plugins into the gateway's plugins_wasm
rem  directory, so they show up on the Dashboard plugins page.
rem
rem  - never auto-loaded: flip the load switch on the plugins
rem    page to load one (wasm plugins never auto-load,
rem    regardless of sha256)
rem  - no packaging, no staging: straight copy, like dropping
rem    a folder into plugins_wasm by hand
rem
rem  Usage:
rem    scripts\copy-wasm-samples.bat            (all samples)
rem    scripts\copy-wasm-samples.bat textstat   (single sample)
rem ============================================================

set "ROOT=%~dp0.."
set "WASM_DIR=%ROOT%\plugins\wasm"
set "DEST=%ROOT%\bin\bin_windows\plugins_wasm"
set "FILTER=%~1"

if not exist "%ROOT%\bin\bin_windows" mkdir "%ROOT%\bin\bin_windows"
if not exist "%DEST%" mkdir "%DEST%"

for /d %%d in ("%WASM_DIR%\*") do call :stage "%%d"

if not exist "%ROOT%\bin\bin_windows\nemesisbot.exe" (
  echo [WARN] bin\bin_windows\nemesisbot.exe not found. Build first to run the gateway:
  echo         scripts\build-windows.bat
)

echo.
echo Done. Payloads in: %DEST%
echo Open Dashboard plugins page and flip the load switch to test.
exit /b 0

:stage
set "SDIR=%~1"
set "NAME=%~nx1"
if defined FILTER if /i not "%NAME%"=="%FILTER%" goto :eof
if not exist "%SDIR%\plugin.toml.sample" goto :eof
if not exist "%SDIR%\Cargo.toml" goto :eof

rem --- slug from the sample manifest ---
set "SLUG="
for /f "tokens=2 delims==" %%a in ('findstr /b /l /c:"slug" "%SDIR%\plugin.toml.sample" 2^>nul') do set "SLUG=%%a"
if not defined SLUG (
  echo [SKIP] %NAME%: no slug line in plugin.toml.sample
  goto :eof
)
set "SLUG=%SLUG: =%"
set SLUG=%SLUG:"=%

rem --- locate wasm artifact; build if missing ---
set "ART="
for %%w in ("%SDIR%\target\wasm32-wasip2\release\*.wasm") do set "ART=%%w"
if not defined ART (
  echo [BUILD] %NAME%: wasm artifact missing, building ...
  pushd "%SDIR%"
  cargo build --target wasm32-wasip2 --release
  if errorlevel 1 (
    popd
    echo [FAIL] %NAME%: cargo build failed
    goto :eof
  )
  popd
  for %%w in ("%SDIR%\target\wasm32-wasip2\release\*.wasm") do set "ART=%%w"
)
if not defined ART (
  echo [FAIL] %NAME%: no .wasm artifact after build
  goto :eof
)

rem --- sha256 via certutil ---
set "SHA="
for /f "skip=1 delims=" %%h in ('certutil -hashfile "%ART%" SHA256 2^>nul') do (
  if not defined SHA (
    set "H=%%h"
    if /i not "!H:~0,9!"=="CertUtil:" set "SHA=!H: =!"
  )
)
if not defined SHA (
  echo [FAIL] %NAME%: certutil hash failed
  goto :eof
)

rem --- write plugin.toml + copy wasm (normalized name) ---
if not exist "%DEST%\%SLUG%" mkdir "%DEST%\%SLUG%"
copy /y "%ART%" "%DEST%\%SLUG%\plugin.wasm" >nul

set "SRC=%SDIR%\plugin.toml.sample"
set "OUT=%DEST%\%SLUG%\plugin.toml"
powershell -NoProfile -Command "$s=[IO.File]::ReadAllText($env:SRC); $s=$s -replace '(?m)^wasm-sha256[^\r\n]*', ('wasm-sha256 = '+[char]34+$env:SHA+[char]34); [IO.File]::WriteAllText($env:OUT,$s,(New-Object System.Text.UTF8Encoding($false)))"
if errorlevel 1 (
  echo [FAIL] %NAME%: manifest generation failed
  goto :eof
)
echo [OK] %NAME% -^> plugins_wasm\%SLUG%  (sha256 %SHA:~0,12%...)
goto :eof
