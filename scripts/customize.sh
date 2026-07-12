#!/usr/bin/env bash
# NemesisBot 定制构建入口（单一命令，模式化）。
#
#   scripts/customize.bat            配置(TUI) → 退出即保存 → 直接编译 → 拷贝
#   scripts/customize.sh iot         加载 minimal-iot 预设 → 编译最小 IoT 版
#   scripts/customize.sh desktop     加载 desktop 预设 → 编译
#   scripts/customize.sh <preset>    加载任意预设 → 编译
#   scripts/customize.sh config      只配置(TUI)，不编译
#   scripts/customize.sh build       只编译（用已有 .config，无则全量默认），不配置
#
# 产物拷贝到 bin/bin_customize/nemesisbot（与 bin/bin_linux 等一致）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

# binary extension: .exe on Windows (git-bash), none on unix
BIN_EXT=""
if [ -n "${OS:-}" ] && echo "$OS" | grep -qi windows; then BIN_EXT=".exe"; fi
CFG="target/debug/nemesis-build-config$BIN_EXT"

if [ ! -x "$CFG" ]; then
    echo "[customize] building configurator (nemesis-build-config)..."
    cargo build -p nemesis-build-config
fi

do_build() {
    # Generate web/.env (frontend feature gates) from .config; clear if no
    # config (no config = full default build => default-include all views).
    if "$CFG" --root "$ROOT" has-config 2>/dev/null; then
        echo "[customize] generating frontend feature env (web/.env)..."
        "$CFG" export --frontend-env > "web/.env"
    else
        rm -f "web/.env"
    fi
    # Build Vue frontend (self-contained: clean stale assets first — orphaned
    # chunks get embedded into the binary via include_dir!, bloating it ~2MB).
    # Falls back to existing static/ if npm unavailable — WITHOUT cleaning, so
    # we never embed a broken (cleaned-but-not-rebuilt) frontend (white screen).
    if [ -f "web/package.json" ]; then
        echo "[customize] building Vue frontend..."
        if (cd web && npm install 2>&1) && [ -d web/node_modules ]; then
            echo "[customize] cleaning stale Vite assets..."
            rm -rf "crates/nemesis-web/static/assets"
            if ! (cd web && npm run build 2>&1); then
                echo "[customize] ERROR Vue build failed AFTER cleaning assets — aborting to avoid embedding a broken frontend (white screen)." >&2
                exit 1
            fi
            echo "[customize] OK Vue frontend built"
        else
            echo "[customize] WARN npm not available / install failed — keeping existing static/ (NOT cleaning, to avoid white screen). Run a full build script first for a fresh frontend."
        fi
    fi

    if "$CFG" --root "$ROOT" has-config; then
        local FEATS PROFILE
        FEATS=$("$CFG" --root "$ROOT" export --features)
        PROFILE=$("$CFG" --root "$ROOT" export --profile)
        echo "[customize] customized build — profile=$PROFILE features=[$FEATS]"
        cargo build --profile "$PROFILE" -p nemesisbot --no-default-features --features "$FEATS"
    else
        echo "[customize] no .config — full default build (release)"
        PROFILE=release
        cargo build --profile release -p nemesisbot
    fi
    mkdir -p bin/bin_customize
    cp -f "target/$PROFILE/nemesisbot$BIN_EXT" "bin/bin_customize/nemesisbot$BIN_EXT"
    chmod +x "bin/bin_customize/nemesisbot$BIN_EXT" 2>/dev/null || true
    echo "[customize] DONE -> bin/bin_customize/nemesisbot$BIN_EXT"
}

MODE="${1:-}"
case "$MODE" in
    "")
        echo "[customize] mode: configure (TUI) then build"
        echo "[customize] opening TUI — toggles, then press q to save & exit"
        "$CFG" --root "$ROOT"
        do_build
        ;;
    config)
        echo "[customize] mode: configure (TUI only)"
        "$CFG" --root "$ROOT"
        ;;
    build)
        do_build
        ;;
    iot)
        echo "[customize] mode: load minimal-iot then build"
        "$CFG" --root "$ROOT" load minimal-iot
        do_build
        ;;
    *)
        echo "[customize] mode: load preset '$MODE' then build"
        "$CFG" --root "$ROOT" load "$MODE"
        do_build
        ;;
esac
