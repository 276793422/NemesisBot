@echo off
setlocal

REM ============================================
REM NemesisBot guest build (Windows native)
REM ============================================
REM Cross-compiles nemesis-guest (musl static ELF) for both architectures and
REM runs product-level assertions. This is the Windows-native twin of
REM scripts\build-guest.sh (Linux/CI implementation) - NO bash / git-bash
REM dependency: only cargo, rustup, powershell (inbox), findstr.
REM
REM Usage: scripts\build-guest.bat [target]
REM   (no target)                    dual musl targets + host smoke
REM   x86_64-unknown-linux-musl      build one target only
REM   aarch64-unknown-linux-musl     build one target only
REM
REM Output: bin\bin_guest\x86_64\nemesis-guest + bin\bin_guest\aarch64\nemesis-guest
REM Artifact root follows CARGO_TARGET_DIR (unset = target) - same rule as
REM build-guest.sh, so calls under build-windows.bat's redirected target dir
REM resolve correctly.
REM
REM Product-level assertions per artifact (not "build printed no error"):
REM   1. ELF header: magic 7f 45 4c 46 + e_machine (offset 18 LE: x86_64=3e00,
REM      aarch64=b700) - guards "built but wrong arch"
REM   2. Static link: binary must NOT contain "ld-musl" (VM base image has no
REM      musl loader)
REM   3. sha256 manifest printed for each artifact
REM   4. Host smoke: one v2 exec roundtrip through the host-native binary
REM      (musl ELFs cannot execute on Windows - runtime parity is asserted by
REM      build-guest.sh on Linux hosts)
REM
REM Exit: 0 = all green; 1 = any red light (build failures abort immediately)

cd /d "%~dp0\.."

set TARGETS=x86_64-unknown-linux-musl aarch64-unknown-linux-musl
if not "%~1"=="" set "TARGETS=%~1"

set OUT_ROOT=bin\bin_guest
set PKG=nemesis-guest
set FAIL=0

REM Artifact root single source of truth (mirrors build-guest.sh ART_ROOT)
set "ART_ROOT=target"
if defined CARGO_TARGET_DIR set "ART_ROOT=%CARGO_TARGET_DIR%"

REM Full path: works even when PATH lacks the WindowsPowerShell directory
set "PS=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"

REM ============================================
REM Step 1: rustup target check / install
REM ============================================
echo ===== musl target install check =====
REM Plain substring findstr (no /x): rustup emits LF-only lines and findstr
REM exact-line options (/x, /b+/e) are unreliable on LF-only streams; target
REM triples never contain each other, so substring is exact enough here.
for %%t in (%TARGETS%) do (
    rustup target list --installed 2>nul | findstr /c:"%%t" >nul
    if errorlevel 1 (
        echo   installing %%t ...
        rustup target add %%t
        if errorlevel 1 (
            echo !! rustup target add %%t failed
            exit /b 1
        )
    ) else (
        echo   ok: %%t installed
    )
)

REM ============================================
REM Step 2: host build + stdio smoke (cross-platform behavior baseline)
REM ============================================
echo.
echo ===== host build + stdio smoke =====
cargo build --release -p %PKG%
if errorlevel 1 (
    echo !! host build failed
    exit /b 1
)
set "HOST_BIN=%ART_ROOT%\release\%PKG%.exe"
if not exist "%HOST_BIN%" (
    echo !! host binary missing: %HOST_BIN%
    exit /b 1
)

REM One v2 exec request - response must echo the call_id AND carry the command
REM output (roundtrip proves v2 parse + exec + response shape).
set "SMOKE_WS=%TEMP%\nmb-guest-smoke-%RANDOM%%RANDOM%"
set "SMOKE_RESP=%TEMP%\nmb-guest-smoke-resp-%RANDOM%.txt"
mkdir "%SMOKE_WS%" >nul 2>&1
set "NEMESISBOT_GUEST_WORKSPACE=%SMOKE_WS%"
echo {"v":2,"call_id":"smoke-1","tool":"exec","args":"{\"command\":\"echo guest-build-smoke\"}","context":{}}| "%HOST_BIN%" > "%SMOKE_RESP%" 2>nul

findstr /c:"smoke-1" "%SMOKE_RESP%" >nul
if errorlevel 1 (
    echo !! host smoke failed: call_id not echoed back
    set FAIL=1
) else (
    findstr /c:"guest-build-smoke" "%SMOKE_RESP%" >nul
    if errorlevel 1 (
        echo !! host smoke failed: command output not in response
        set FAIL=1
    ) else (
        echo ok: host smoke passed
    )
)
rmdir /s /q "%SMOKE_WS%" >nul 2>&1
del /q "%SMOKE_RESP%" >nul 2>&1
set "NEMESISBOT_GUEST_WORKSPACE="

REM ============================================
REM Step 3: per-target cross build + copy + assertions
REM ============================================
for %%t in (%TARGETS%) do (
    call :build_one %%t || exit /b 1
)

if not "%FAIL%"=="0" goto :fail
echo.
echo build-guest: ALL GREEN - artifacts in %OUT_ROOT%\
exit /b 0

:fail
echo.
echo build-guest: FAIL - see !! lines above
exit /b 1


:build_one
REM %1 = target triple
set "T=%~1"
for /f "delims=-" %%a in ("%T%") do set "ARCH=%%a"
echo.
echo ===== cross build %T% =====

cargo build --release -p %PKG% --target %T%
if errorlevel 1 (
    echo !! cross build failed: %T%
    exit /b 1
)

set "SRC=%ART_ROOT%\%T%\release\%PKG%"
if not exist "%SRC%" (
    echo !! cross artifact missing: %SRC%
    exit /b 1
)

if not exist "%OUT_ROOT%\%ARCH%" mkdir "%OUT_ROOT%\%ARCH%"
copy /y "%SRC%" "%OUT_ROOT%\%ARCH%\%PKG%" >nul
if errorlevel 1 (
    echo !! copy failed: %SRC% into %OUT_ROOT%\%ARCH%\
    exit /b 1
)
set "BIN=%OUT_ROOT%\%ARCH%\%PKG%"
set "NMB_BIN=%BIN%"

REM expected e_machine low byte: x86_64=0x3e (62), aarch64=0xb7 (183); high byte 0
set "WANT_EM=62"
set "WANT_EM_HEX=3e00"
if "%ARCH%"=="aarch64" (
    set "WANT_EM=183"
    set "WANT_EM_HEX=b700"
)

REM --- 1. ELF header: magic 7f 45 4c 46 + e_machine (offset 18, LE) ---
%PS% -NoProfile -Command "$b=[IO.File]::ReadAllBytes($env:NMB_BIN); if($b.Length -lt 20){exit 1}; if($b[0] -ne 127 -or $b[1] -ne 69 -or $b[2] -ne 76 -or $b[3] -ne 70){exit 1}; if($b[18] -ne %WANT_EM% -or $b[19] -ne 0){exit 1}; exit 0"
if errorlevel 1 (
    echo !! [%ARCH%] ELF header assertion failed - want magic 7f454c46 + e_machine %WANT_EM_HEX%
    set FAIL=1
    ver >nul
    goto :eof
)
echo ok: [%ARCH%] ELF header (magic + e_machine)

REM --- 2. static link: must NOT contain "ld-musl" (VM base image has no musl loader) ---
%PS% -NoProfile -Command "$b=[IO.File]::ReadAllBytes($env:NMB_BIN); $s=[Text.Encoding]::ASCII.GetString($b); if($s.Contains('ld-musl')){exit 1}; exit 0"
if errorlevel 1 (
    echo !! [%ARCH%] contains ld-musl - not a static link
    set FAIL=1
    ver >nul
    goto :eof
)
echo ok: [%ARCH%] static (no ld-musl interpreter)

REM --- 3. sha256 manifest ---
set "SHA="
for /f %%h in ('%PS% -NoProfile -Command "(Get-FileHash -Algorithm SHA256 $env:NMB_BIN).Hash.ToLower()"') do if not defined SHA set "SHA=%%h"
echo sha256[%ARCH%] = %SHA%

REM --- 4. runtime smoke: musl ELFs cannot execute on Windows ---
echo note: [%ARCH%] runtime smoke not possible on Windows host - ELF/static assertions only; runtime parity asserted by build-guest.sh on Linux
goto :eof
