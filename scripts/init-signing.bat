@echo off
REM ============================================================================
REM init-signing.bat -- v4 signing system one-time initialization (pure ops
REM script; touches no repository code).
REM
REM Companion of: docs/PLAN/2026-09-23_ci-v4-signing-integration.md (P2 runbook)
REM Same stages / same semantics as scripts/init-signing.sh, ported natively.
REM
REM !! THIS FILE MUST STAY PURE ASCII (no Chinese, no emoji, no box drawing).
REM !! A chcp 65001 codepage switch executed while a batch file is being read
REM !! corrupts cmd.exe's byte-offset tracking (empirically confirmed on this
REM !! machine with in-process AND child-process switches: execution lands
REM !! mid-line of unrelated code). The .sh twin carries the Chinese narration;
REM !! this .bat prints English for the same reason.
REM
REM Stages:
REM   build     compile exe-sign-tool (release)
REM   keygen    key ceremony: keygen -> split-keys -> extract root_cert.der
REM             (all private keys land OUTSIDE the repo, in the ceremony dir:
REM             NMB_KEYCEREMONY_DIR or %USERPROFILE%\nemesis-keyceremony)
REM   secrets   upload intermediate CA material to GitHub Secrets (gh secret set)
REM   cert      drop root_cert.der into certs/ and git add it (you commit --
REM             commit discipline: no auto-commit from scripts)
REM   dispatch  trigger daily-release workflow_dispatch and wait for the build
REM   verify    download artifacts, run 1 positive + 3 negative verify cases
REM             (Valid / Tampered / Untrusted / NoSignature)
REM   clean     destroy plaintext private keys in the ceremony dir (--yes req.)
REM   all       build -> keygen -> secrets -> cert -> dispatch -> verify
REM             (clean is always run separately)
REM
REM Key red line: no private key file produced here ever goes into the repo.
REM
REM Deliberate deviations from the .sh:
REM   - hashing via certutil (native); chmod 600 skipped (NTFS ACLs differ)
REM   - command output capture goes through temp files (for /f inline quoting
REM     is too fragile in cmd)
REM ============================================================================

for %%i in ("%~dp0..") do set "REPO_ROOT=%%~fi"
set "CEREMONY_DIR=%NMB_KEYCEREMONY_DIR%"
if not defined CEREMONY_DIR set "CEREMONY_DIR=%USERPROFILE%\nemesis-keyceremony"
set "STAGE=%~1"
set "EXTRA=%~2"
set "TOOL="
set "PY="

if "%STAGE%"=="" goto :help
if /i "%STAGE%"=="help" goto :help
if /i "%STAGE%"=="-h" goto :help
if /i "%STAGE%"=="--help" goto :help
if /i "%STAGE%"=="build" goto :stage_build
if /i "%STAGE%"=="keygen" goto :stage_keygen
if /i "%STAGE%"=="secrets" goto :stage_secrets
if /i "%STAGE%"=="cert" goto :stage_cert
if /i "%STAGE%"=="dispatch" goto :stage_dispatch
if /i "%STAGE%"=="verify" goto :stage_verify
if /i "%STAGE%"=="clean" goto :stage_clean
if /i "%STAGE%"=="all" goto :stage_all
echo [ERROR] unknown stage: %STAGE% (see: scripts\init-signing.bat help)
exit /b 1

REM ---------------------------------------------------------------------------
:stage_build
echo.
echo ==^> compiling exe-sign-tool (release)
cd /d "%REPO_ROOT%"
cargo build --release -p exe-sign-tool
if errorlevel 1 goto :fail
call :find_tool
if not defined TOOL goto :fail
echo ==^> tool ready: %TOOL%
goto :eof

REM ---------------------------------------------------------------------------
:stage_keygen
call :find_tool
if not defined TOOL (
    echo [ERROR] exe-sign-tool not found -- run the build stage first, or set EXE_SIGN_TOOL
    goto :fail
)
"%TOOL%" split-keys --help >nul 2>&1
if errorlevel 1 (
    echo [ERROR] this exe-sign-tool has no split-keys subcommand -- P0 code not
    echo         implemented yet; finish the code part first, see plan section 4
    goto :fail
)
if exist "%CEREMONY_DIR%\keys.json" (
    echo [ERROR] %CEREMONY_DIR%\keys.json already exists -- refusing to overwrite
    echo         an existing key ceremony; delete the directory manually to redo
    goto :fail
)
if not exist "%CEREMONY_DIR%" mkdir "%CEREMONY_DIR%"
echo.
echo ==^> generating 3-tier chain (root -^> issuing CA -^> leaf) into %CEREMONY_DIR%
"%TOOL%" keygen --out "%CEREMONY_DIR%\keys.json"
if errorlevel 1 goto :fail
echo.
echo ==^> splitting: root (offline) / intermediate CA (into Secrets)
"%TOOL%" split-keys --in "%CEREMONY_DIR%\keys.json" --root-out "%CEREMONY_DIR%\root.offline.json" --issuing-out "%CEREMONY_DIR%\issuing.ci.json" --root-cert-out "%CEREMONY_DIR%\root_cert.der"
if not errorlevel 1 goto :split_ok
echo (split-keys lacks --root-cert-out; extracting root cert from keys.json via python)
call :need_python
if errorlevel 1 goto :fail
call %PY% -c "import json,binascii,sys; d=json.load(open(r'%CEREMONY_DIR%\keys.json')); open(r'%CEREMONY_DIR%\root_cert.der','wb').write(binascii.unhexlify(d['root_cert']))"
if errorlevel 1 goto :fail
:split_ok
echo (note: the .sh's chmod 600 is skipped on Windows; tighten NTFS ACLs manually if needed)
echo.
echo ==^> trust anchor fingerprint (record this; the verifier pins it)
certutil -hashfile "%CEREMONY_DIR%\root_cert.der" SHA256
echo.
echo ============================================================
echo [OK] key ceremony complete. Ceremony dir: %CEREMONY_DIR%
echo    keys.json          all 3 private keys (root+issuing+leaf, full power)
echo    root.offline.json  root key + root cert    ^<-- keep this safe
echo    issuing.ci.json    intermediate CA key+cert ^<-- next stage: Secrets
echo    root_cert.der      root cert public part   ^<-- commit into the repo
echo.
echo [WARN] the ONE remaining manual step (no script can do this for you):
echo    store root.offline.json and keys.json in a password manager
echo    (Bitwarden / 1Password / KeePass) + one offline cold copy (USB drive).
echo    Do NOT run the clean stage until that is confirmed.
echo ============================================================
goto :eof

REM ---------------------------------------------------------------------------
:stage_secrets
if not exist "%CEREMONY_DIR%\issuing.ci.json" (
    echo [ERROR] missing %CEREMONY_DIR%\issuing.ci.json -- run the keygen stage first
    goto :fail
)
call :need_gh
if errorlevel 1 goto :fail
echo.
echo ==^> extracting intermediate CA material (values never echoed; straight into Secrets)
powershell -NoProfile -Command "$d = Get-Content -Raw '%CEREMONY_DIR%\issuing.ci.json' | ConvertFrom-Json; [IO.File]::WriteAllText('%CEREMONY_DIR%\.issuing_sk.tmp', $d.issuing_sk); [IO.File]::WriteAllText('%CEREMONY_DIR%\.issuing_cert.tmp', $d.issuing_cert)"
if errorlevel 1 goto :fail
echo ==^> uploading NMB_ISSUING_SK_HEX / NMB_ISSUING_CERT_HEX (gh encrypts automatically)
call gh secret set NMB_ISSUING_SK_HEX < "%CEREMONY_DIR%\.issuing_sk.tmp"
if errorlevel 1 goto :fail
call gh secret set NMB_ISSUING_CERT_HEX < "%CEREMONY_DIR%\.issuing_cert.tmp"
if errorlevel 1 goto :fail
del "%CEREMONY_DIR%\.issuing_sk.tmp" "%CEREMONY_DIR%\.issuing_cert.tmp" >nul 2>&1
echo.
echo ==^> [OK] Secrets configured (target repo = this directory's git remote)
git -C "%REPO_ROOT%" remote get-url origin
echo    verify at: repo Settings -^> Secrets and variables -^> Actions
goto :eof

REM ---------------------------------------------------------------------------
:stage_cert
if not exist "%CEREMONY_DIR%\root_cert.der" (
    echo [ERROR] missing %CEREMONY_DIR%\root_cert.der -- run the keygen stage first
    goto :fail
)
echo.
echo ==^> placing the root cert (public part) into the repo (certs\root_cert.der)
if not exist "%REPO_ROOT%\certs" mkdir "%REPO_ROOT%\certs"
copy /Y "%CEREMONY_DIR%\root_cert.der" "%REPO_ROOT%\certs\root_cert.der" >nul
if errorlevel 1 goto :fail
echo ==^> trust anchor fingerprint (must match the keygen stage printout)
certutil -hashfile "%REPO_ROOT%\certs\root_cert.der" SHA256
git -C "%REPO_ROOT%" add certs/root_cert.der
if errorlevel 1 goto :fail
echo ==^> staged via git add (per commit discipline YOU run the commit), e.g.:
echo    git commit -m "ADD v4 signing trust root public cert certs/root_cert.der"
goto :eof

REM ---------------------------------------------------------------------------
:stage_dispatch
call :need_gh
if errorlevel 1 goto :fail
echo.
echo ==^> triggering Daily Nightly Release (workflow_dispatch)
cd /d "%REPO_ROOT%"
set "TMPF=%TEMP%\nmb-rid.tmp"
call gh workflow run daily-release.yml > "%TMPF%" 2>&1
if errorlevel 1 goto :fail
type "%TMPF%"
REM run id comes straight from the trigger output URL -- querying the run
REM list races GitHub indexing (lost both a fixed 8s sleep and a 60s retry
REM loop this way; the trigger URL is authoritative and instant).
REM https://github.com/<owner>/<repo>/actions/runs/<id> -- id = slash token 7.
set "RID="
for /f "tokens=7 delims=/" %%n in ('findstr /c:"/actions/runs/" "%TMPF%"') do set "RID=%%n"
del "%TMPF%" >nul 2>&1
if defined RID goto :dispatch_got_rid
echo [ERROR] could not parse the run id from the trigger output -- check the Actions page
goto :fail
:dispatch_got_rid
echo     run id: %RID%
echo ==^> waiting for the build (nightly builds take roughly 10-30 minutes;
echo     Ctrl+C here does NOT cancel the build itself)
call gh run watch %RID% --exit-status
if errorlevel 1 goto :fail
echo     build done. Now run the four-case verify: scripts\init-signing.bat verify %RID%
goto :eof

REM ---------------------------------------------------------------------------
:stage_verify
call :find_tool
if not defined TOOL (
    echo [ERROR] exe-sign-tool not found -- run the build stage first
    goto :fail
)
"%TOOL%" verify --help 2>&1 | findstr /c:"--root-cert" >nul
if errorlevel 1 (
    echo [ERROR] this exe-sign-tool verify lacks --root-cert -- P0.4 code not implemented
    goto :fail
)
call :need_python
if errorlevel 1 goto :fail
call :need_gh
if errorlevel 1 goto :fail
set "RID=%EXTRA%"
REM TMPF must be set OUTSIDE the if block -- %TMPF% inside parens expands at
REM parse time (before the set executes) and would pick up a stale value.
set "TMPF=%TEMP%\nmb-rid2.tmp"
if not defined RID (
    powershell -NoProfile -Command "gh run list --workflow daily-release.yml --limit 5 --json databaseId,conclusion | ConvertFrom-Json | Where-Object { $_.conclusion -eq 'success' } | Select-Object -First 1 -ExpandProperty databaseId" > "%TMPF%" 2>nul
    set /p RID=<"%TMPF%"
    del "%TMPF%" >nul 2>&1
)
if not defined RID (
    echo [ERROR] no recent successful daily-release run found; pass one explicitly:
    echo         init-signing.bat verify ^<run-id^>
    goto :fail
)
set "DL=%TEMP%\nmb-verify-%RANDOM%"
echo.
echo ==^> downloading artifacts of run %RID% into %DL%
call gh run download %RID% -D "%DL%"
if errorlevel 1 goto :verify_fail
set "BIN="
for /f "delims=" %%f in ('dir /s /b "%DL%\nemesisbot.exe" 2^>nul') do if not defined BIN set "BIN=%%f"
if not defined BIN for /f "delims=" %%f in ('dir /s /b "%DL%\nemesisbot" 2^>nul') do if not defined BIN set "BIN=%%f"
if not defined BIN (
    echo [ERROR] no nemesisbot binary found in the artifacts
    goto :verify_fail
)
set "RC=%REPO_ROOT%\certs\root_cert.der"
set "VOUT=%TEMP%\nmb-vout.tmp"
set /a VPASS=0
set /a VFAIL=0

echo.
echo ==^> positive case: signed artifact + repo root cert -^> expect Valid
set "NAME=positive Valid"
set "WANT=Valid"
set "TGT=%BIN%"
call :check

echo ==^> negative 1: flip one byte -^> expect Tampered
powershell -NoProfile -Command "$b = [IO.File]::ReadAllBytes('%BIN%'); $b[[int]($b.Length/2)] = $b[[int]($b.Length/2)] -bxor 0xFF; [IO.File]::WriteAllBytes('%BIN%.tampered', $b)"
if errorlevel 1 goto :verify_fail
set "NAME=negative1 Tampered"
set "WANT=Tampered"
set "TGT=%BIN%.tampered"
call :check

echo ==^> negative 2: different root (freshly generated throwaway chain) -^> expect Untrusted
set "WA=%TEMP%\nmb-wronganchor-%RANDOM%"
mkdir "%WA%"
"%TOOL%" keygen --out "%WA%\wrong.json" >nul 2>&1
if errorlevel 1 goto :verify_fail
call %PY% -c "import json,binascii,sys; d=json.load(open(r'%WA%\wrong.json')); open(r'%WA%\wrong_root.der','wb').write(binascii.unhexlify(d['root_cert']))"
if errorlevel 1 goto :verify_fail
set "NAME=negative2 Untrusted"
set "WANT=Untrusted"
set "TGT=%BIN%"
set "RC=%WA%\wrong_root.der"
call :check

echo ==^> negative 3: never-signed file -^> expect NoSignature
set "NAME=negative3 NoSignature"
set "WANT=NoSignature"
set "TGT=%REPO_ROOT%\certs\root_cert.der"
set "RC=%REPO_ROOT%\certs\root_cert.der"
call :check

rmdir /s /q "%DL%" >nul 2>&1
rmdir /s /q "%WA%" >nul 2>&1
del "%VOUT%" "%BIN%.tampered" >nul 2>&1
echo.
if %VFAIL% equ 0 (
    echo ==^> [OK] all four verify cases behaved as expected -- %VPASS% passed
    goto :eof
)
echo [ERROR] %VFAIL% of 4 verify cases did NOT behave as expected (%VPASS% passed) -- investigate before continuing
goto :fail

:verify_fail
rmdir /s /q "%DL%" >nul 2>&1
goto :fail

REM ---------------------------------------------------------------------------
:stage_clean
if not "%EXTRA%"=="--yes" (
    echo [ERROR] clean destroys ALL plaintext private keys in the ceremony dir -- irreversible.
    echo    After confirming root.offline.json and keys.json are in the password
    echo    manager / cold storage, run:
    echo    scripts\init-signing.bat clean --yes
    goto :fail
)
if not exist "%CEREMONY_DIR%" (
    echo [ERROR] %CEREMONY_DIR% does not exist; nothing to clean
    goto :fail
)
echo will delete:
dir /b "%CEREMONY_DIR%"
rmdir /s /q "%CEREMONY_DIR%"
echo ==^> [OK] local plaintext private keys destroyed
goto :eof

REM ---------------------------------------------------------------------------
:stage_all
call :stage_build
if errorlevel 1 exit /b 1
call :stage_keygen
if errorlevel 1 exit /b 1
call :stage_secrets
if errorlevel 1 exit /b 1
call :stage_cert
if errorlevel 1 exit /b 1
call :stage_dispatch
if errorlevel 1 exit /b 1
call :stage_verify
if errorlevel 1 exit /b 1
echo.
echo all stages done. Final step (after YOU confirm the root key is in the vault):
echo   scripts\init-signing.bat clean --yes
goto :eof

REM ===========================================================================
REM subroutines
REM ===========================================================================

:find_tool
if defined EXE_SIGN_TOOL if exist "%EXE_SIGN_TOOL%" (
    set "TOOL=%EXE_SIGN_TOOL%"
    goto :eof
)
if exist "%REPO_ROOT%\target\release\exe-sign-tool.exe" set "TOOL=%REPO_ROOT%\target\release\exe-sign-tool.exe"
goto :eof

:need_python
if defined PY goto :eof
where python >nul 2>&1 && set "PY=python" && goto :eof
where python3 >nul 2>&1 && set "PY=python3" && goto :eof
echo [ERROR] python required (cert extraction / byte flipping) but not found
exit /b 1

:need_gh
where gh >nul 2>&1 || goto :need_gh_install
call gh auth status >nul 2>&1 && goto :eof
echo [ERROR] gh is not logged in; run first: gh auth login
exit /b 1
:need_gh_install
echo [ERROR] gh CLI (GitHub official command line) not found. Install either way:
echo    1) winget install GitHub.cli        (MSI install; may pop a UAC prompt)
echo    2) portable (no popup): go to https://github.com/cli/cli/releases
echo       download the windows amd64 zip, extract, add the gh.exe dir to PATH
echo    then log in once (device flow: terminal shows a code, paste it in browser):
echo       gh auth login
exit /b 1

REM Shared by all four verify cases. Inputs: NAME / WANT / TGT / RC
REM (+ globals TOOL / VOUT). Matches the first line of the verify output
REM against WANT via findstr on the temp file (never via `echo %GOT%` --
REM tool output must not be re-parsed by cmd). Tallies VPASS / VFAIL.
:check
"%TOOL%" verify --root-cert "%RC%" --target "%TGT%" > "%VOUT%" 2>&1
findstr /b /c:"%WANT%" "%VOUT%" >nul
if errorlevel 1 goto :check_fail
echo   [PASS] %NAME%
set /a VPASS+=1
goto :eof
:check_fail
echo   [FAIL] %NAME% (expected: %WANT%). Tool output:
type "%VOUT%"
set /a VFAIL+=1
goto :eof

:help
echo v4 signing system one-time initialization script (Windows native twin of
echo scripts/init-signing.sh; this file prints English and must stay pure ASCII
echo -- see the header comment for why).
echo.
echo Usage: scripts\init-signing.bat ^<stage^>
echo.
echo stages:
echo   build     compile exe-sign-tool (release)
echo   keygen    key ceremony: keygen -^> split-keys -^> extract root_cert.der
echo             (private keys land OUTSIDE the repo, in the ceremony dir)
echo   secrets   upload intermediate CA material to GitHub Secrets (gh secret set)
echo   cert      root_cert.der into certs/ + git add (you run the commit)
echo   dispatch  trigger daily-release workflow_dispatch and wait
echo   verify    download artifacts; 1 positive + 3 negative verify cases
echo   clean     destroy ceremony-dir plaintext keys (requires --yes)
echo   all       build -^> keygen -^> secrets -^> cert -^> dispatch -^> verify
echo.
echo environment:
echo   NMB_KEYCEREMONY_DIR  ceremony dir (default %%USERPROFILE%%\nemesis-keyceremony)
echo   EXE_SIGN_TOOL        absolute path to exe-sign-tool (default: target\release)
goto :eof

:fail
echo.
echo [ERROR] failed -- see output above
exit /b 1
