@echo off
setlocal enabledelayedexpansion

REM ============================================
REM NemesisBot Rust Build Script (Windows)
REM ============================================
REM Usage: scripts\build-windows.bat [options]
REM   No arguments  - Build release, copy to bin\bin_windows\
REM   --clean       - Clean before building
REM   --skip-plugin - Skip plugin-ui.dll build
REM   --help        - Show help
REM
REM Output layout:
REM   bin\bin_windows\
REM     nemesisbot.exe
REM     plugins\
REM       plugin_ui.dll
REM       eval_monitor_dll.dll
REM       plugin_wasm.dll
REM       plugin_vm.dll  (image script + guest ELFs embedded; released on first use)
REM       plugin_roaming.dll  (QUIC relay server + rendezvous registry; QUIC
REM                           plugin M1 — iroh stack lives here, host exe
REM                           zero increment; absent = relay honestly degraded)
REM       plugin_resources.dll  (add-on resource carrier; persona supermarket
REM                           upstream snapshot embedded at build time —
REM                           FILE cache (pre-downloaded) > baseline fallback;
REM                           absent = shop pure online)
REM     plugins_wasm\        (WASM plugin payload dir, preset empty + README;
REM                           installed plugins land here, NOT loaded by default)
REM     tests\
REM       cluster-test.exe
REM       integration-test.exe
REM       mcp-server.exe

REM Switch to project root (parent of scripts/)
cd /d "%~dp0\.."

REM ============================================
REM Parse Arguments
REM ============================================
set CLEAN=0
set SKIP_PLUGIN=0
set SKIP_ZIWEI=0

:parse_args
if "%~1"=="" goto done_parsing
if /i "%~1"=="--clean" (
    set CLEAN=1
    shift
    goto parse_args
)
if /i "%~1"=="--skip-plugin" (
    set SKIP_PLUGIN=1
    shift
    goto parse_args
)
if /i "%~1"=="--skip-ziwei" (
    set SKIP_ZIWEI=1
    shift
    goto parse_args
)
if /i "%~1"=="--help" goto show_help
echo [ERROR] Unknown argument: %~1
echo Use --help for usage information
exit /b 1

:show_help
echo Usage: scripts\build-windows.bat [--clean] [--skip-plugin] [--skip-ziwei] [--help]
echo.
echo Options:
echo   --clean        Clean target before building
echo   --skip-plugin  Skip plugin-ui.dll build
echo   --skip-ziwei   Skip ziwei payload DLLs (dual-arch inject payloads)
echo   --help         Show this help
exit /b 0

:done_parsing

REM ============================================
REM Banner
REM ============================================
set VERSION=0.0.0.1
for /f "tokens=*" %%i in ('git describe --tags --abbrev=0 2^>nul') do set VERSION=%%i

set GIT_COMMIT=unknown
for /f "tokens=*" %%i in ('git rev-parse --short HEAD 2^>nul') do set GIT_COMMIT=%%i

REM Compile-time pricing table embed: value set in the pre-download block
REM below (after clean, before any cargo build) - see nemesis-data\build.rs
REM (download latest LiteLLM table for embedding; falls back to repo snapshot
REM on failure)

for /f "tokens=*" %%i in ('rustc --version 2^>nul') do set RUSTC_VERSION=%%i

echo ============================================
echo  NemesisBot Rust Build (Windows)
echo ============================================
echo  Version:     %VERSION%
echo  Git Commit:  %GIT_COMMIT%
echo  Rustc:       %RUSTC_VERSION%
echo  Target Dir:  target\target_windows
echo  Output Dir:  bin\bin_windows
echo ============================================
echo.

REM ============================================
REM Set target directory
REM ============================================
set CARGO_TARGET_DIR=target\target_windows

REM ============================================
REM Step 1: Clean (optional)
REM ============================================
if "%CLEAN%"=="1" (
    echo [Step 1/5] Cleaning target\target_windows...
    cargo clean --target-dir target\target_windows 2>nul
    if errorlevel 1 (
        echo   WARN cargo clean had issues, continuing...
    ) else (
        echo   OK Cleaned
    )
    echo.
) else (
    echo [Step 1/5] Clean skipped ^(use --clean to enable^)
    echo.
)

REM ============================================
REM Step 2: Build Vue frontend (web dashboard)
REM ============================================
echo [Step 2/5] Building Vue frontend...

if exist "web\package.json" (
    pushd web

    REM Detect wrong-platform node_modules (e.g. Linux binaries from WSL build)
    if exist "node_modules\@rollup\rollup-linux-x64-gnu\package.json" (
        if not exist "node_modules\@rollup\rollup-win32-x64-msvc\package.json" (
            echo   Detected Linux node_modules, reinstalling for Windows...
            rmdir /s /q "node_modules" 2>nul
            if exist "package-lock.json" del /q "package-lock.json" 2>nul
        )
    )

    REM Always run npm install to ensure all declared dependencies are present.
    REM npm skips already-installed packages, so this is fast when nothing is missing.
    echo   Checking npm dependencies...
    call npm install --silent
    if errorlevel 1 (
        popd
        echo   WARN npm install failed, skipping Vue build
        echo.
        goto step25
    )
    echo   Cleaning stale Vite assets + env...
    echo   ^(orphaned hashed chunks from prior builds get embedded into the binary via include_dir!, bloating it ~2MB^)
    if exist "..\crates\nemesis-web\static\assets" rmdir /s /q "..\crates\nemesis-web\static\assets"
    if exist ".env" del /q ".env"
    echo   Running Vite build...
    call npm run build
    if errorlevel 1 (
        popd
        echo   ERROR Vue build failed! See errors above.
        echo.
        pause
        exit /b 1
    )
    popd
    echo   OK Vue frontend built
) else (
    echo   SKIP web\package.json not found
)
echo.

REM --- ziwei payload DLLs (dual-arch inject payloads, x64+x86; embedded into
REM     nemesisbot at compile time via nemesis-ziwei build.rs - this build must
REM     precede `cargo build --release`. Failure = non-fatal: absent file =
REM     zero-byte embed + honest runtime error, per decision 9) ---
if "%SKIP_ZIWEI%"=="1" (
    echo   Ziwei payload skipped ^(--skip-ziwei^)
    goto step25
)
if exist "plugins\ziwei-agent-dll\Cargo.toml" (
    echo   Building ziwei payload ^(x64^)...
    pushd plugins\ziwei-agent-dll
    set CARGO_TARGET_DIR=target
    rustup target list --installed | findstr /c:"x86_64-pc-windows-msvc" >nul
    if errorlevel 1 (
        echo   WARN x86_64-pc-windows-msvc target missing ^(rustup target add x86_64-pc-windows-msvc^)
        popd
        set CARGO_TARGET_DIR=target\target_windows
        goto ziwei_x86
    )
    cargo build --release --target x86_64-pc-windows-msvc
    if errorlevel 1 (
        echo   WARN ziwei x64 payload build failed ^(non-fatal, embed stays zero-byte^)
        popd
        set CARGO_TARGET_DIR=target\target_windows
        goto ziwei_x86
    )
    if not exist bin mkdir bin
    copy /y target\x86_64-pc-windows-msvc\release\ziwei_agent_dll.dll bin\ziwei_agent_x64.dll >nul
    popd
    set CARGO_TARGET_DIR=target\target_windows
    echo   OK ziwei x64 payload built
) else (
    echo   SKIP plugins\ziwei-agent-dll\Cargo.toml not found
    goto step25
)
:ziwei_x86
if exist "plugins\ziwei-agent-dll\Cargo.toml" (
    echo   Building ziwei payload ^(x86^)...
    pushd plugins\ziwei-agent-dll
    set CARGO_TARGET_DIR=target
    rustup target list --installed | findstr /c:"i686-pc-windows-msvc" >nul
    if errorlevel 1 (
        echo   WARN i686-pc-windows-msvc target missing ^(rustup target add i686-pc-windows-msvc^)
        popd
        set CARGO_TARGET_DIR=target\target_windows
        goto step25
    )
    cargo build --release --target i686-pc-windows-msvc
    if errorlevel 1 (
        echo   WARN ziwei x86 payload build failed ^(non-fatal, embed stays zero-byte^)
        popd
        set CARGO_TARGET_DIR=target\target_windows
        goto step25
    )
    if not exist bin mkdir bin
    copy /y target\i686-pc-windows-msvc\release\ziwei_agent_dll.dll bin\ziwei_agent_x86.dll >nul
    popd
    set CARGO_TARGET_DIR=target\target_windows
    echo   OK ziwei x86 payload built
)
echo.
:step25

REM ============================================
REM Step 2.5: Pre-download LiteLLM price table (single network touch per build)
REM ============================================
REM nemesis-data\build.rs embeds the price table at compile time and this
REM script runs cargo TWICE (main + vm-display pass). The old random-refresh
REM env made BOTH passes re-download - on a bad network that meant two
REM identical failures. Pre-download once here:
REM   success -> NEMESIS_PRICES_FILE points build.rs at the local file
REM              (zero network in either pass; refresh = size+mtime
REM              fingerprint, so a later pass only reruns when the table
REM              actually changed)
REM   failure -> warn once, leave both envs unset: every cargo pass stays
REM              offline on the bundled snapshot (no second download try)
REM Mirror list mirrors crates\nemesis-data\src\pricing_filter.rs PRICE_MIRROR_URLS.
set "NEMESIS_PRICES_FILE="
set "NEMESIS_PRICES_REFRESH="
set "PRICING_CACHE=%CD%\target\pricing\litellm.json"
set "CURL_EXE=%SystemRoot%\System32\curl.exe"

echo [Step 2.5/5] Pre-downloading LiteLLM price table...
if not exist "%CURL_EXE%" (
    REM No curl: fixed refresh still prevents the second pass from rerunning
    REM build.rs - the first pass does its own single download inside build.rs.
    set "NEMESIS_PRICES_REFRESH=%GIT_COMMIT%_legacy"
    echo   SKIP curl.exe not found - build.rs will fetch once itself
    goto step3
)
if not exist "%CD%\target\pricing" mkdir "%CD%\target\pricing"
for %%u in ("https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json" "https://fastly.jsdelivr.net/gh/BerriAI/litellm@main/model_prices_and_context_window.json" "https://cdn.jsdelivr.net/gh/BerriAI/litellm@main/model_prices_and_context_window.json") do (
    if exist "%PRICING_CACHE%" goto pricing_done
    echo   trying %%~u ...
    "%CURL_EXE%" -fsSL --connect-timeout 15 --max-time 90 -o "%PRICING_CACHE%" %%~u
    if errorlevel 1 del /q "%PRICING_CACHE%" 2>nul
)
:pricing_done
if not exist "%PRICING_CACHE%" (
    echo   WARN price table download failed - this build embeds the bundled snapshot; later passes will NOT retry
    goto step3
)
for %%f in ("%PRICING_CACHE%") do (
    set "NEMESIS_PRICES_FILE=%PRICING_CACHE%"
    set "NEMESIS_PRICES_REFRESH=%%~zf_%%~tf"
    set "PRICING_SIZE=%%~zf"
)
echo   OK price table cached - %PRICING_SIZE% bytes ^(both cargo passes read this file, no further downloads^)
echo.

:step3

REM ============================================
REM Step 3: Build main workspace (release)
REM ============================================
echo [Step 3/5] Building release...

REM --- selfupdate trust anchor (compile-time; same source as CI daily-release:
REM     SHA-256 of certs\root_cert.der. Absent file = no anchor = self-update
REM     apply refuses to execute (fail-closed), build itself unaffected) ---
set "NEMESIS_BUILD_ROOT_ANCHOR="
if exist "certs\root_cert.der" (
    for /f "delims=" %%h in ('powershell -NoProfile -Command "$h=Get-FileHash -Algorithm SHA256 'certs\root_cert.der';$h.Hash.ToLower()"') do set "NEMESIS_BUILD_ROOT_ANCHOR=%%h"
    if defined NEMESIS_BUILD_ROOT_ANCHOR (
        echo   OK selfupdate anchor injected: !NEMESIS_BUILD_ROOT_ANCHOR!
    ) else (
        echo   WARN anchor hash failed - no compile-time anchor, self-update refuses ^(fail-closed^)
    )
) else (
    echo   WARN certs\root_cert.der not found - no compile-time anchor, self-update refuses ^(fail-closed^)
)

cargo build --release --features git-https
if errorlevel 1 (
    echo.
    echo [ERROR] Build failed!
    pause
    exit /b 1
)

REM vm-display pass (plugin-vm host loader; plugin-vm package = single distribution
REM dependency of the whole VM stack, packaged exe must carry the display loader.
REM Incremental: only the nemesisbot crate recompiles; failure = keep default exe, non-fatal)
echo   Building nemesisbot with vm-display (plugin-vm loader)...
cargo build --release -p nemesisbot --features vm-display,git-https
if errorlevel 1 (
    echo   WARN vm-display build failed, keeping default exe ^(VM display relay unavailable^)
)

echo   OK Build completed
echo.

REM ============================================
REM Step 4: Build plugin DLLs (release)
REM ============================================
if "%SKIP_PLUGIN%"=="1" (
    echo [Step 4/5] Plugin DLLs skipped ^(--skip-plugin^)
    echo.
    goto step5
)

echo [Step 4/5] Building plugin DLLs...

REM --- persona snapshot pre-download (对齐价目表 FILE 模式：脚本在此把上游
REM     tarball 预下载到缓存 = 本构建会话唯一网络点，成功 ->
REM     NEMESIS_PERSONA_FILE 指缓存 + NEMESIS_PERSONA_REFRESH=<40位SHA>
REM     （manifest 溯源 commit + rerun-if-env-changed 重跑信号——同 commit
REM     的后续 pass 读缓存零网络）；ls-remote 或下载任一失败 -> 两变量落空 =
REM     plugin-resources build.rs 仓库基线兜底 fail-open（cargo:warning，绝不
REM     炸构建）。日常 cargo 构建不带这些变量 = 纯离线基线；
REM     NEMESIS_PERSONA_OFFLINE=1 可强制基线） ---
set "NEMESIS_PERSONA_FILE="
set "NEMESIS_PERSONA_REFRESH="
set "PERSONA_CACHE=%CD%\target\persona\agency-agents.tar.gz"
set "PERSONA_UPSTREAM_COMMIT="
for /f "tokens=1" %%i in ('git ls-remote https://github.com/msitarzewski/agency-agents.git main 2^>nul') do (
    if not defined PERSONA_UPSTREAM_COMMIT set PERSONA_UPSTREAM_COMMIT=%%i
)
if not defined PERSONA_UPSTREAM_COMMIT (
    echo   Persona snapshot: upstream probe failed ^(offline?^) - baseline fallback
    goto persona_done
)
if not exist "%CURL_EXE%" (
    REM No curl: fixed refresh still pins the fingerprint - build.rs does its
    REM own single download inside the plugin pass (prices legacy shape).
    set "NEMESIS_PERSONA_REFRESH=!PERSONA_UPSTREAM_COMMIT!"
    echo   SKIP curl.exe not found - build.rs will fetch once itself
    goto persona_done
)
if not exist "%CD%\target\persona" mkdir "%CD%\target\persona" >nul 2>&1
echo   Persona snapshot: downloading upstream tarball ^(!PERSONA_UPSTREAM_COMMIT!^)...
"%CURL_EXE%" -fsSL --connect-timeout 15 --max-time 300 -o "%PERSONA_CACHE%" "https://codeload.github.com/msitarzewski/agency-agents/tar.gz/refs/heads/main"
if errorlevel 1 (
    del /q "%PERSONA_CACHE%" 2>nul
    echo   WARN persona tarball download failed - baseline fallback
    goto persona_done
)
set "NEMESIS_PERSONA_FILE=%PERSONA_CACHE%"
set "NEMESIS_PERSONA_REFRESH=!PERSONA_UPSTREAM_COMMIT!"
echo   OK persona tarball cached - commit !PERSONA_UPSTREAM_COMMIT! ^(plugin pass reads this file, no further downloads^)
:persona_done

REM --- plugin-ui ---
if exist "plugins\plugin-ui\Cargo.toml" (
    echo   Building plugin-ui...
    pushd plugins\plugin-ui
    set CARGO_TARGET_DIR=..\..\target\target_windows\plugins\plugin-ui
    cargo build --release
    if errorlevel 1 (
        popd
        echo   WARN Plugin-ui DLL build failed ^(non-fatal, continuing without plugin^)
        echo.
        set CARGO_TARGET_DIR=target\target_windows
        goto plugin_onnx
    )
    popd
    set CARGO_TARGET_DIR=target\target_windows
    echo   OK Plugin-ui DLL built
) else (
    echo   SKIP plugins\plugin-ui\Cargo.toml not found
)

:plugin_onnx
REM --- plugin-onnx ---
if exist "plugins\plugin-onnx\Cargo.toml" (
    echo   Building plugin-onnx...
    pushd plugins\plugin-onnx
    set CARGO_TARGET_DIR=..\..\target\target_windows\plugins\plugin-onnx
    cargo build --release
    if errorlevel 1 (
        popd
        echo   WARN Plugin-onnx DLL build failed ^(non-fatal, continuing without plugin^)
        echo.
        set CARGO_TARGET_DIR=target\target_windows
        goto plugin_eval_monitor
    )
    popd
    set CARGO_TARGET_DIR=target\target_windows
    echo   OK Plugin-onnx DLL built
) else (
    echo   SKIP plugins\plugin-onnx\Cargo.toml not found
)
echo.

:plugin_eval_monitor
REM --- plugin-eval-monitor (eval sandbox monitor DLL; 2026-08-18 deployment:
REM     resolved next to exe via plugins\ dir, no longer bound to repo absolute path) ---
if exist "plugins\plugin-eval-monitor\Cargo.toml" (
    echo   Building plugin-eval-monitor...
    pushd plugins\plugin-eval-monitor
    set CARGO_TARGET_DIR=..\..\target\target_windows\plugins\plugin-eval-monitor
    cargo build --release
    if errorlevel 1 (
        popd
        echo   WARN Plugin-eval-monitor DLL build failed ^(non-fatal, eval feature unavailable^)
        echo.
        set CARGO_TARGET_DIR=target\target_windows
        goto plugin_wasm
    )
    popd
    set CARGO_TARGET_DIR=target\target_windows
    echo   OK Plugin-eval-monitor DLL built
) else (
    echo   SKIP plugins\plugin-eval-monitor\Cargo.toml not found
)
echo.

:plugin_wasm
REM --- plugin-wasm (WASM plugin host shell; resolved next to exe via plugins\ dir; absent = subsystem honestly disabled) ---
if exist "plugins\plugin-wasm\Cargo.toml" (
    echo   Building plugin-wasm...
    pushd plugins\plugin-wasm
    set CARGO_TARGET_DIR=..\..\target\target_windows\plugins\plugin-wasm
    cargo build --release
    if errorlevel 1 (
        popd
        echo   WARN Plugin-wasm DLL build failed ^(non-fatal, wasm plugin subsystem unavailable^)
        echo.
        set CARGO_TARGET_DIR=target\target_windows
        goto plugin_roaming
    )
    popd
    set CARGO_TARGET_DIR=target\target_windows
    echo   OK Plugin-wasm DLL built
) else (
    echo   SKIP plugins\plugin-wasm\Cargo.toml not found
)
echo.

:plugin_roaming
REM --- plugin-roaming (QUIC relay server + rendezvous registry; QUIC plugin
REM     M1 2026-10-23: iroh-relay/rcgen/rustls/axum stack lives entirely in
REM     this DLL, host binary zero increment; absent = server-side relay
REM     honestly degraded + CLI dynamic-hide, never a load failure) ---
if exist "plugins\plugin-roaming\Cargo.toml" (
    echo   Building plugin-roaming...
    pushd plugins\plugin-roaming
    set CARGO_TARGET_DIR=..\..\target\target_windows\plugins\plugin-roaming
    cargo build --release
    if errorlevel 1 (
        popd
        echo   WARN Plugin-roaming DLL build failed ^(non-fatal, QUIC relay server unavailable^)
        echo.
        set CARGO_TARGET_DIR=target\target_windows
        goto plugin_vm
    )
    popd
    set CARGO_TARGET_DIR=target\target_windows
    echo   OK Plugin-roaming DLL built
) else (
    echo   SKIP plugins\plugin-roaming\Cargo.toml not found
)
echo.

:plugin_vm
REM --- guest binaries (musl cross-compile; MUST precede plugin-vm build ---
REM     resources-embed discipline: image script + guest ELFs are embedded into
REM     the DLL at compile time; with NMB_VM_REQUIRE_RESOURCES=1 absent
REM     resources fail the build, so guests must exist first. build.rs requires
REM     BOTH arches, so the guard checks each. Guest build uses the
REM     Windows-native scripts\build-guest.bat (no bash dependency). Failure
REM     log is shown, not swallowed - a silent guest miss used to surface only
REM     as the confusing plugin-vm panic below) ---
set GUEST_NEED_BUILD=0
if not exist "bin\bin_guest\x86_64\nemesis-guest" set GUEST_NEED_BUILD=1
if not exist "bin\bin_guest\aarch64\nemesis-guest" set GUEST_NEED_BUILD=1
if "%GUEST_NEED_BUILD%"=="1" (
    if not exist "target" mkdir "target"
    echo   Building guest binaries ^(scripts\build-guest.bat, log: target\build-guest.log^)...
    call scripts\build-guest.bat > target\build-guest.log 2>&1
    if errorlevel 1 (
        echo   ---- guest build log ----
        type target\build-guest.log
        echo   WARN guest build failed - plugin-vm build below will fail fail-closed ^(NMB_VM_REQUIRE_RESOURCES=1^)
    )
)

REM --- plugin-vm (VM display relay DLL into plugins\ subdir; single-file deploy:
REM     image script + guest ELFs embedded at compile time, released on first use
REM     by the host — no plugin_vm\ resource dir. NMB_VM_REQUIRE_RESOURCES=1 =
REM     fail-closed: missing resources refuse to package) ---
if exist "plugins\plugin-vm\Cargo.toml" (
    echo   Building plugin-vm...
    pushd plugins\plugin-vm
    set CARGO_TARGET_DIR=..\..\target\target_windows\plugins\plugin-vm
    set NMB_VM_REQUIRE_RESOURCES=1
    cargo build --release
    if errorlevel 1 (
        popd
        echo   WARN Plugin-vm DLL build failed ^(non-fatal, VM display relay unavailable^)
        echo.
        set CARGO_TARGET_DIR=target\target_windows
        goto plugin_resources
    )
    popd
    set CARGO_TARGET_DIR=target\target_windows
    set NMB_VM_REQUIRE_RESOURCES=
    echo   OK Plugin-vm DLL built
) else (
    echo   SKIP plugins\plugin-vm\Cargo.toml not found
)
echo.

:plugin_resources
REM --- plugin-resources（附加资源载体 C-ABI 插件；人格超市上游快照编译期内嵌：
REM     Step 4 预下载缓存（NEMESIS_PERSONA_FILE）> 缓存缺席 = 仓库基线兜底
REM     （fail-open，build.rs 内 cargo:warning，绝不炸构建）；插件缺席 =
REM     人格超市纯在线（现有行为）。未签名的 dev 产物经 loader 签名闸放行
REM     仅限 dev 形态，release 出包走签名流程） ---
if exist "plugins\plugin-resources\Cargo.toml" (
    echo   Building plugin-resources...
    pushd plugins\plugin-resources
    set CARGO_TARGET_DIR=..\..\target\target_windows\plugins\plugin-resources
    cargo build --release
    if errorlevel 1 (
        popd
        echo   WARN Plugin-resources DLL build failed ^(non-fatal, persona supermarket snapshot unavailable^)
        echo.
        set CARGO_TARGET_DIR=target\target_windows
        goto step5
    )
    popd
    set CARGO_TARGET_DIR=target\target_windows
    echo   OK Plugin-resources DLL built
) else (
    echo   SKIP plugins\plugin-resources\Cargo.toml not found
)
echo.

:step5

REM ============================================
REM Step 5: Copy to bin\bin_windows\
REM ============================================
echo [Step 5/5] Copying to bin\bin_windows\...

if not exist "bin\bin_windows" mkdir "bin\bin_windows"

set COPIED=0

REM Copy main binary
if exist "target\target_windows\release\nemesisbot.exe" (
    copy /y "target\target_windows\release\nemesisbot.exe" "bin\bin_windows\" >nul 2>&1
    set /a COPIED+=1
)

REM Copy plugin DLL to bin\bin_windows\plugins\
set DLL_FOUND=0
if not exist "bin\bin_windows\plugins" mkdir "bin\bin_windows\plugins"
if exist "target\target_windows\plugins\plugin-ui\release\plugin_ui.dll" (
    copy /y "target\target_windows\plugins\plugin-ui\release\plugin_ui.dll" "bin\bin_windows\plugins\" >nul 2>&1
    set /a COPIED+=1
    set DLL_FOUND=1
) else if exist "target\target_windows\plugins\plugin-ui\release\plugin-ui.dll" (
    copy /y "target\target_windows\plugins\plugin-ui\release\plugin-ui.dll" "bin\bin_windows\plugins\" >nul 2>&1
    set /a COPIED+=1
    set DLL_FOUND=1
)

REM Copy plugin-onnx DLL to bin\bin_windows\plugins\
set ONNX_DLL_FOUND=0
if exist "target\target_windows\plugins\plugin-onnx\release\plugin_onnx.dll" (
    copy /y "target\target_windows\plugins\plugin-onnx\release\plugin_onnx.dll" "bin\bin_windows\plugins\" >nul 2>&1
    set /a COPIED+=1
    set ONNX_DLL_FOUND=1
) else if exist "target\target_windows\plugins\plugin-onnx\release\plugin-onnx.dll" (
    copy /y "target\target_windows\plugins\plugin-onnx\release\plugin-onnx.dll" "bin\bin_windows\plugins\" >nul 2>&1
    set /a COPIED+=1
    set ONNX_DLL_FOUND=1
)

if "!ONNX_DLL_FOUND!"=="0" if "%SKIP_PLUGIN%"=="0" (
    echo   WARN plugin-onnx.dll not found in build output
)

REM Copy plugin-wasm DLL to bin\bin_windows\plugins\
REM (WASM plugin host shell; resolved next to exe via plugins\ dir; absent = subsystem honestly disabled)
if exist "target\target_windows\plugins\plugin-wasm\release\plugin_wasm.dll" (
    copy /y "target\target_windows\plugins\plugin-wasm\release\plugin_wasm.dll" "bin\bin_windows\plugins\" >nul 2>&1
    set /a COPIED+=1
) else (
    if "%SKIP_PLUGIN%"=="0" echo   WARN plugin_wasm.dll not found in build output ^(wasm plugin subsystem unavailable^)
)

REM Preset empty plugins_wasm\ dir next to exe (2026-10-22 discipline: WASM
REM plugin payloads live in a user-visible sibling of plugins\; install puts
REM them here; plugins are NOT loaded by default — load switch lives in
REM workspace config/config.plugin.wasm.json, toggled from Dashboard/CLI).
if not exist "bin\bin_windows\plugins_wasm" mkdir "bin\bin_windows\plugins_wasm"
if not exist "bin\bin_windows\plugins_wasm\README.txt" (
    echo WASM plugin payload directory.> "bin\bin_windows\plugins_wasm\README.txt"
    echo.>> "bin\bin_windows\plugins_wasm\README.txt"
    echo Install via the Dashboard Plugins page, or:  nemesisbot plugin install ^<dir^>>> "bin\bin_windows\plugins_wasm\README.txt"
    echo Hand-copying a payload dir here also works; run "nemesisbot plugin list" to see it.>> "bin\bin_windows\plugins_wasm\README.txt"
    echo.>> "bin\bin_windows\plugins_wasm\README.txt"
    echo Plugins here are NOT loaded by default. Open the load switch on the>> "bin\bin_windows\plugins_wasm\README.txt"
    echo Dashboard Plugins page or run:  nemesisbot plugin enable ^<slug^>>> "bin\bin_windows\plugins_wasm\README.txt"
    echo Switch state is persisted in workspace config/config.plugin.wasm.json.>> "bin\bin_windows\plugins_wasm\README.txt"
    set /a COPIED+=1
)

REM Copy eval-monitor DLL to bin\bin_windows\plugins\
REM (eval sandbox monitor; resolved next to exe via plugins\ dir; portable with the deploy dir)
set EVAL_MONITOR_FOUND=0
if exist "target\target_windows\plugins\plugin-eval-monitor\release\eval_monitor_dll.dll" (
    copy /y "target\target_windows\plugins\plugin-eval-monitor\release\eval_monitor_dll.dll" "bin\bin_windows\plugins\" >nul 2>&1
    set /a COPIED+=1
    set EVAL_MONITOR_FOUND=1
)
if "!EVAL_MONITOR_FOUND!"=="0" if "%SKIP_PLUGIN%"=="0" (
    echo   WARN eval_monitor_dll.dll not found in build output ^(eval feature unavailable^)
)

if "!DLL_FOUND!"=="0" if "%SKIP_PLUGIN%"=="0" (
    echo   WARN plugin-ui.dll not found in build output
)

REM Copy plugin-vm DLL into plugins\ subdir (loader resolves plugins\plugin_vm.dll
REM first, exe-dir fallback for legacy layouts). Single-file deploy: image script +
REM guest ELFs are embedded in the DLL and released on first use by the host —
REM no plugin_vm\ resource dir (2026-10-22 discipline).
if exist "target\target_windows\plugins\plugin-vm\release\plugin_vm.dll" (
    copy /y "target\target_windows\plugins\plugin-vm\release\plugin_vm.dll" "bin\bin_windows\plugins\" >nul 2>&1
    set /a COPIED+=1
) else (
    if "%SKIP_PLUGIN%"=="0" echo   WARN plugin_vm.dll not found in build output ^(VM display relay unavailable^)
)

REM Copy plugin-roaming DLL into plugins\ subdir (QUIC relay server face;
REM QUIC plugin M1: absent = relay honestly degraded + CLI dynamic-hide).
if exist "target\target_windows\plugins\plugin-roaming\release\plugin_roaming.dll" (
    copy /y "target\target_windows\plugins\plugin-roaming\release\plugin_roaming.dll" "bin\bin_windows\plugins\" >nul 2>&1
    set /a COPIED+=1
) else (
    if "%SKIP_PLUGIN%"=="0" echo   WARN plugin_roaming.dll not found in build output ^(QUIC relay server unavailable^)
)

REM Copy plugin-resources DLL into plugins\ subdir (add-on resource carrier;
REM persona supermarket upstream snapshot; absent = shop pure online).
if exist "target\target_windows\plugins\plugin-resources\release\plugin_resources.dll" (
    copy /y "target\target_windows\plugins\plugin-resources\release\plugin_resources.dll" "bin\bin_windows\plugins\" >nul 2>&1
    set /a COPIED+=1
) else (
    if "%SKIP_PLUGIN%"=="0" echo   WARN plugin_resources.dll not found in build output ^(persona supermarket snapshot unavailable^)
)

REM Copy test-tools binaries to bin\bin_windows\tests\
if not exist "bin\bin_windows\tests" mkdir "bin\bin_windows\tests"
for %%e in (ai-server.exe cluster-test.exe integration-test.exe mcp-server.exe) do (
    if exist "target\target_windows\release\%%e" (
        copy /y "target\target_windows\release\%%e" "bin\bin_windows\tests\" >nul 2>&1
        set /a COPIED+=1
    )
)

echo   OK Copied !COPIED! file^(s^) to bin\bin_windows\
echo.

REM Optional: build skins (.nbskin) - only when skins\ submodule is present and Node is available.
REM Local build packs without signing (.nbskin works unsigned; the management UI marks it as unsigned).
REM Signed distribution goes through daily-release CI leaf signing.
if exist "skins" (
    where node >nul 2>&1
    if errorlevel 1 (
        echo   WARN Node.js not found in PATH - skipping .nbskin skin packaging
    ) else (
        echo   Building skins ^(.nbskin^)...
        if not exist "bin\bin_windows\skins" mkdir "bin\bin_windows\skins"
        set SKIN_COUNT=0
        for /d %%d in (skins\*) do (
            if exist "%%d\dev\pack.mjs" (
                echo     skin: %%~nxd
                pushd %%d
                if exist "ui-src\package.json" if not exist "ui-src\node_modules" (
                    call npm install --no-fund --no-audit >nul 2>&1
                )
                if exist "ui-src\package.json" (
                    call npm run build >nul 2>&1
                )
                call node dev\pack.mjs
                if errorlevel 1 (
                    popd
                    echo     WARN skin %%~nxd package failed ^(non-fatal, continuing^)
                ) else (
                    popd
                    copy /y "%%d\dist\*.nbskin" "bin\bin_windows\skins\" >nul 2>&1
                    set /a SKIN_COUNT+=1
                )
            )
        )
        if "!SKIN_COUNT!"=="0" (
            echo   SKIP no skins\*\dev\pack.mjs found
        ) else (
            echo   OK !SKIN_COUNT! skin package^(s^) copied to bin\bin_windows\skins\
        )
    )
) else (
    echo   SKIP skins\ not found - no .nbskin packaging
)
echo.

REM ============================================
REM Summary
REM ============================================
echo ============================================
echo  Build Summary (Windows)
echo ============================================
echo  Version: %VERSION%
echo  Commit:  %GIT_COMMIT%
echo.

echo  bin\bin_windows\
for %%f in (bin\bin_windows\*) do (
    set FSIZE=%%~zf
    set /a FSIZE_MB=!FSIZE! / 1048576
    echo    %%~nxf ^(!FSIZE_MB! MB^)
)

echo.
echo [SUCCESS] Build completed!
echo.
echo Run: bin\bin_windows\nemesisbot.exe gateway
echo ============================================
echo.
