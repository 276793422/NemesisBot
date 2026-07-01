@echo off
REM NemesisBot customize build entry (single command, mode-driven).
REM
REM   scripts\customize.bat            configure (TUI) -> save on exit -> build -> copy
REM   scripts\customize.bat iot        load minimal-iot preset -> build minimal IoT
REM   scripts\customize.bat desktop    load desktop preset -> build
REM   scripts\customize.bat <preset>   load any preset -> build
REM   scripts\customize.bat config     configure (TUI) only, no build
REM   scripts\customize.bat build      build only (existing .config, or full default)
REM
REM Output copied to bin\bin_customize\nemesisbot.exe (matches bin\bin_windows etc).
setlocal enabledelayedexpansion

set "SCRIPT_DIR=%~dp0"
set "ROOT=%SCRIPT_DIR%.."
cd /d "%ROOT%"

set "MODE=%~1"
if "%MODE%"=="" set "MODE=__configure_build"

REM 1) Ensure the configurator binary exists
set "CFG=target\debug\nemesis-build-config.exe"
if not exist "%CFG%" (
    echo [customize] building configurator nemesis-build-config...
    cargo build -p nemesis-build-config
    if errorlevel 1 ( echo [customize] FAILED to build configurator & exit /b 1 )
)

REM 2) Mode dispatch (goto labels - avoid 'and' inside if-blocks)
if /I "%MODE%"=="config"            goto :do_configure
if /I "%MODE%"=="build"             goto :do_build
if /I "%MODE%"=="__configure_build" goto :do_configure_build
if /I "%MODE%"=="iot"               goto :do_iot
set "PRESET=%MODE%"
goto :do_preset_build

:do_iot
set "PRESET=minimal-iot"

:do_preset_build
echo [customize] mode: load preset '%PRESET%' then build
"%CFG%" --root "%ROOT%" load %PRESET%
if errorlevel 1 ( echo [customize] FAILED to load preset '%PRESET%' & exit /b 1 )
goto :do_build

:do_configure
echo [customize] mode: configure (TUI only)
"%CFG%" --root "%ROOT%"
exit /b %errorlevel%

:do_configure_build
echo [customize] mode: configure (TUI) then build
echo [customize] opening TUI - toggle with Space, press q to save and exit
"%CFG%" --root "%ROOT%"
if errorlevel 1 ( echo [customize] configurator exited with error & exit /b 1 )

:do_build
"%CFG%" --root "%ROOT%" has-config
if errorlevel 1 (
    echo [customize] no .config - full default build, profile=release
    set "PROFILE=release"
    cargo build --profile release -p nemesisbot
    if errorlevel 1 ( echo [customize] BUILD FAILED & exit /b 1 )
) else (
    for /f "delims=" %%F in ('%CFG% export --features') do set "FEATS=%%F"
    for /f "delims=" %%P in ('%CFG% export --profile') do set "PROFILE=%%P"
    echo [customize] customized build - profile=!PROFILE! features=[!FEATS!]
    cargo build --profile !PROFILE! -p nemesisbot --no-default-features --features "!FEATS!"
    if errorlevel 1 ( echo [customize] BUILD FAILED & exit /b 1 )
)

if not exist bin\bin_customize mkdir bin\bin_customize
if not exist "target\!PROFILE!\nemesisbot.exe" (
    echo [customize] WARN: target\!PROFILE!\nemesisbot.exe not found, skip copy
    exit /b 1
)
copy /y "target\!PROFILE!\nemesisbot.exe" "bin\bin_customize\nemesisbot.exe" >nul
echo [customize] DONE -^> bin\bin_customize\nemesisbot.exe
exit /b 0
