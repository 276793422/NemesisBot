#!/usr/bin/env bash
# ============================================
# NemesisBot Rust Build Script (Linux / WSL)
# ============================================
# Usage: scripts/build-linux.sh [options]
#   No arguments  — Build release, copy to bin/bin_linux/
#   --clean       — Clean before building
#   --skip-plugin — Skip plugin .so build
#   --help        — Show help
#
# Output layout:
#   bin/bin_linux/
#   ├── nemesisbot
#   ├── plugins/
#   │   ├── plugin_ui.so
#   │   ├── plugin_onnx.so
#   │   └── libplugin_vm.so   (image script + guest ELFs embedded; released on first use)
#   └── tests/
#       ├── cluster-test
#       ├── integration-test
#       └── mcp-server

set -euo pipefail

# Switch to project root (parent of scripts/)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

# Use separate target/output dirs to avoid conflicts with Windows build
ROOT_DIR="$(pwd)"
export CARGO_TARGET_DIR="${ROOT_DIR}/target/target_linux"
BIN_DIR="bin/bin_linux"

# ============================================
# Parse Arguments
# ============================================
CLEAN=false
SKIP_PLUGIN=false

for arg in "$@"; do
    case "$arg" in
        --clean)       CLEAN=true ;;
        --skip-plugin) SKIP_PLUGIN=true ;;
        --help|-h)
            echo "Usage: scripts/build-linux.sh [--clean] [--skip-plugin] [--help]"
            echo ""
            echo "Options:"
            echo "  --clean        Clean target/target_linux before building"
            echo "  --skip-plugin  Skip plugin .so build"
            echo "  --help         Show this help"
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# ============================================
# Banner
# ============================================
VERSION="0.0.0.1"
if git describe --tags --abbrev=0 >/dev/null 2>&1; then
    VERSION="$(git describe --tags --abbrev=0)"
fi

GIT_COMMIT="unknown"
if git rev-parse --short HEAD >/dev/null 2>&1; then
    GIT_COMMIT="$(git rev-parse --short HEAD)"
fi

# 编译期价目表内嵌：值随构建变化 → 绕开 target 缓存强制 build.rs 重跑
# （下载 LiteLLM 最新表内嵌；失败自动快照兜底，见 nemesis-data/build.rs）
export NEMESIS_PRICES_REFRESH="${GIT_COMMIT}_$(date +%s)"

RUSTC_VERSION="$(rustc --version 2>/dev/null || echo "unknown")"

echo "============================================"
echo " NemesisBot Rust Build (Linux)"
echo "============================================"
echo " Version:     $VERSION"
echo " Git Commit:  $GIT_COMMIT"
echo " Rustc:       $RUSTC_VERSION"
echo " Target Dir:  target/target_linux"
echo " Output Dir:  $BIN_DIR/"
echo "============================================"
echo ""

# ============================================
# Step 1: Clean (optional)
# ============================================
if [ "$CLEAN" = true ]; then
    echo "[Step 1/5] Cleaning target/target_linux..."
    cargo clean --target-dir "$CARGO_TARGET_DIR" 2>/dev/null || true
    echo "  OK Cleaned"
    echo ""
else
    echo "[Step 1/5] Clean skipped (use --clean to enable)"
    echo ""
fi

# ============================================
# Step 2: Build Vue frontend (web dashboard)
# ============================================
echo "[Step 2/5] Building Vue frontend..."

if [ -f "web/package.json" ]; then
    # On Linux/WSL, the shared Windows filesystem may have Windows-only
    # node_modules (e.g. @rollup/rollup-win32-x64-msvc).  Detect this
    # and reinstall with the correct platform binaries.
    NEED_REINSTALL=false
    if [ -d "web/node_modules" ]; then
        # Check if node_modules has Windows-only rollup but not Linux one
        if [ -f "web/node_modules/@rollup/rollup-win32-x64-msvc/package.json" ] && \
           [ ! -d "web/node_modules/@rollup/rollup-linux-x64-gnu" ]; then
            NEED_REINSTALL=true
            echo "  Detected Windows node_modules, reinstalling for Linux..."
        fi
    fi

    if [ "$NEED_REINSTALL" = true ]; then
        rm -rf web/node_modules web/package-lock.json
    fi

    # Always run npm install to ensure all declared dependencies are present.
    # npm skips already-installed packages, so this is fast when nothing is missing.
    echo "  Checking npm dependencies..."
    (cd web && npm install 2>&1) || {
        echo "  WARN npm install failed, skipping Vue build"
        echo ""
        # fall through to step 3
    }

    if [ -d "web/node_modules" ]; then
        echo "  Cleaning stale Vite assets (orphaned hashed chunks from prior builds get embedded into the binary via include_dir!, bloating it ~2MB)..."
        rm -rf "crates/nemesis-web/static/assets"
        rm -f "web/.env"
        # G7 (D1)：Linux 工具链构建时开启用户态沙盒 UI（landlock/bwrap/Seatbelt
        # 自检面板）。用环境变量而非写 web/.env —— .env 上面刚被 rm，且 env 在
        # Vite 编译期静态替换 import.meta.env，Windows/Android 构建不设此变量
        # 时该组件整体 tree-shake 出二进制。
        export VITE_USERLAND_SANDBOX=1
        echo "  Running Vite build (VITE_USERLAND_SANDBOX=1)..."
        if (cd web && npm run build 2>&1); then
            echo "  OK Vue frontend built"
        else
            echo "  ERROR Vue build failed AFTER cleaning assets — aborting to avoid embedding a broken frontend (white screen). Fix the Vue build and re-run."
            exit 1
        fi
    fi
else
    echo "  SKIP web/package.json not found"
fi
echo ""

# ============================================
# Step 3: Build main workspace (release)
# ============================================
echo "[Step 3/5] Building release..."
START_TIME=$SECONDS

if ! cargo build --release 2>&1; then
    echo ""
    echo "[ERROR] Build failed!"
    exit 1
fi

# vm-display pass（plugin-vm 宿主 loader；plugin-vm 包 = VM 能力栈唯一分发
# 依赖，打包件 exe 必须带显示 loader。增量：仅 nemesisbot crate 重编；
# 失败保留默认 exe，非致命）
echo "  Building nemesisbot with vm-display (plugin-vm loader)..."
if ! cargo build --release -p nemesisbot --features vm-display 2>&1; then
    echo "  WARN vm-display build failed, keeping default exe (VM display relay unavailable)"
fi

BUILD_DURATION=$(( SECONDS - START_TIME ))
echo "  OK Build completed in ${BUILD_DURATION}s"
echo ""

# ============================================
# Step 4: Build plugin shared libraries (release)
# ============================================
if [ "$SKIP_PLUGIN" = true ]; then
    echo "[Step 4/5] Plugin .so skipped (--skip-plugin)"
    echo ""
else
    echo "[Step 4/5] Building plugin shared libraries..."

    # --- plugin-ui ---
    PLUGIN_UI_DIR="plugins/plugin-ui"
    if [ -f "$PLUGIN_UI_DIR/Cargo.toml" ]; then
        echo "  Building plugin-ui..."
        DLL_START=$SECONDS
        if (cd "$PLUGIN_UI_DIR" && CARGO_TARGET_DIR="${ROOT_DIR}/target/target_linux/plugins/plugin-ui" cargo build --release 2>&1); then
            DLL_DURATION=$(( SECONDS - DLL_START ))
            echo "  OK Plugin-ui built in ${DLL_DURATION}s"
        else
            echo "  WARN Plugin-ui build failed (non-fatal, continuing without plugin)"
        fi
    else
        echo "  SKIP $PLUGIN_UI_DIR/Cargo.toml not found"
    fi

    # --- plugin-onnx ---
    PLUGIN_ONNX_DIR="plugins/plugin-onnx"
    if [ -f "$PLUGIN_ONNX_DIR/Cargo.toml" ]; then
        echo "  Building plugin-onnx..."
        ONNX_START=$SECONDS
        if (cd "$PLUGIN_ONNX_DIR" && CARGO_TARGET_DIR="${ROOT_DIR}/target/target_linux/plugins/plugin-onnx" cargo build --release 2>&1); then
            ONNX_DURATION=$(( SECONDS - ONNX_START ))
            echo "  OK Plugin-onnx built in ${ONNX_DURATION}s"
        else
            echo "  WARN Plugin-onnx build failed (non-fatal, continuing without plugin)"
        fi
    else
        echo "  SKIP $PLUGIN_ONNX_DIR/Cargo.toml not found"
    fi
    echo ""

    # --- plugin-wasm（WASM 插件宿主壳；缺席 = wasm 插件子系统诚实关闭） ---
    PLUGIN_WASM_DIR="plugins/plugin-wasm"
    if [ -f "$PLUGIN_WASM_DIR/Cargo.toml" ]; then
        echo "  Building plugin-wasm..."
        WASM_START=$SECONDS
        if (cd "$PLUGIN_WASM_DIR" && CARGO_TARGET_DIR="${ROOT_DIR}/target/target_linux/plugins/plugin-wasm" cargo build --release 2>&1); then
            WASM_DURATION=$(( SECONDS - WASM_START ))
            echo "  OK Plugin-wasm built in ${WASM_DURATION}s"
        else
            echo "  WARN Plugin-wasm build failed (non-fatal — wasm 插件子系统将不可用)"
        fi
    else
        echo "  SKIP $PLUGIN_WASM_DIR/Cargo.toml not found"
    fi
    echo ""

    # --- guest 产物（musl 交叉编译；必须先于 plugin-vm 构建——资源内嵌纪律：
    #     镜像脚本 + guest ELF 编译期内嵌进插件库，NMB_VM_REQUIRE_RESOURCES=1
    #     时缺席资源 fail-closed 拒绝出包，guest 必须先在场） ---
    if [ ! -f "bin/bin_guest/x86_64/nemesis-guest" ]; then
        if rustup target list --installed 2>/dev/null | grep -q x86_64-unknown-linux-musl; then
            echo "  Building guest binaries (scripts/build-guest.sh)..."
            bash scripts/build-guest.sh >/dev/null 2>&1 \
                || echo "  WARN guest build failed (plugin-vm embed will report honestly)"
        else
            echo "  WARN x86_64-unknown-linux-musl target not installed — guest resources absent (plugin-vm embed will fail honestly)"
        fi
    fi

    # --- plugin-vm（显示中继 .so 放 plugins/ 子目录；单文件部署：镜像脚本 +
    #     guest ELF 编译期内嵌，宿主首次使用时释放——没有 plugin_vm/ 资源目录。
    #     NMB_VM_REQUIRE_RESOURCES=1 = fail-closed：缺席资源拒绝出包） ---
    PLUGIN_VM_DIR="plugins/plugin-vm"
    if [ -f "$PLUGIN_VM_DIR/Cargo.toml" ]; then
        echo "  Building plugin-vm..."
        VM_START=$SECONDS
        if (cd "$PLUGIN_VM_DIR" && NMB_VM_REQUIRE_RESOURCES=1 CARGO_TARGET_DIR="${ROOT_DIR}/target/target_linux/plugins/plugin-vm" cargo build --release 2>&1); then
            VM_DURATION=$(( SECONDS - VM_START ))
            echo "  OK Plugin-vm built in ${VM_DURATION}s"
        else
            echo "  WARN Plugin-vm build failed (non-fatal — VM 显示中继将不可用)"
        fi
    else
        echo "  SKIP $PLUGIN_VM_DIR/Cargo.toml not found"
    fi
    echo ""
fi

# ============================================
# Step 5: Copy to bin/bin_linux/
# ============================================
echo "[Step 5/5] Copying to ${BIN_DIR}/..."

mkdir -p "$BIN_DIR"

# Copy main binary (no .exe extension on Linux)
COPIED=()
RELEASE_DIR="$CARGO_TARGET_DIR/release"
if [ -f "$RELEASE_DIR/nemesisbot" ]; then
    cp "$RELEASE_DIR/nemesisbot" "$BIN_DIR/"
    chmod +x "$BIN_DIR/nemesisbot"
    SIZE=$(wc -c < "$RELEASE_DIR/nemesisbot")
    SIZE_MB=$(( SIZE / 1048576 ))
    COPIED+=("nemesisbot (${SIZE_MB} MB)")
fi

# Copy plugin .so to bin/bin_linux/plugins/
mkdir -p "$BIN_DIR/plugins"

DLL_FOUND=false
for so_path in "target/target_linux/plugins/plugin-ui/release/libplugin_ui.so" \
               "target/target_linux/plugins/plugin-ui/release/libplugin_ui.so"; do
    if [ -f "$so_path" ]; then
        cp "$so_path" "$BIN_DIR/plugins/"
        SIZE=$(wc -c < "$so_path")
        SIZE_MB=$(( SIZE / 1048576 ))
        COPIED+=("libplugin_ui.so (${SIZE_MB} MB)")
        DLL_FOUND=true
        break
    fi
done
if [ "$DLL_FOUND" = false ] && [ "$SKIP_PLUGIN" = false ]; then
    echo "  WARN libplugin_ui.so not found in build output"
fi

ONNX_DLL_FOUND=false
for so_path in "target/target_linux/plugins/plugin-onnx/release/libplugin_onnx.so" \
               "target/target_linux/plugins/plugin-onnx/release/libplugin_onnx.so"; do
    if [ -f "$so_path" ]; then
        cp "$so_path" "$BIN_DIR/plugins/"
        SIZE=$(wc -c < "$so_path")
        SIZE_MB=$(( SIZE / 1048576 ))
        COPIED+=("libplugin_onnx.so (${SIZE_MB} MB)")
        ONNX_DLL_FOUND=true
        break
    fi
done
if [ "$ONNX_DLL_FOUND" = false ] && [ "$SKIP_PLUGIN" = false ]; then
    echo "  WARN libplugin_onnx.so not found in build output"
fi

# plugin-wasm 宿主壳（exe 旁 plugins/ 查找——缺席 = 子系统诚实关闭）
WASM_DLL_FOUND=false
for so_path in "target/target_linux/plugins/plugin-wasm/release/libplugin_wasm.so"; do
    if [ -f "$so_path" ]; then
        cp "$so_path" "$BIN_DIR/plugins/"
        SIZE=$(wc -c < "$so_path")
        SIZE_MB=$(( SIZE / 1048576 ))
        COPIED+=("libplugin_wasm.so (${SIZE_MB} MB)")
        WASM_DLL_FOUND=true
        break
    fi
done
if [ "$WASM_DLL_FOUND" = false ] && [ "$SKIP_PLUGIN" = false ]; then
    echo "  WARN libplugin_wasm.so not found in build output (wasm 插件子系统将不可用)"
fi

# plugin-vm 显示中继（plugins/ 子目录查找——loader 语义，exe 同目录仅旧部署回落）。
# 单文件部署：镜像脚本 + guest ELF 已内嵌 DLL，宿主首次使用时自动释放——
# 没有 plugin_vm/ 资源目录（2026-10-22 纪律）
VM_DLL_FOUND=false
for so_path in "target/target_linux/plugins/plugin-vm/release/libplugin_vm.so"; do
    if [ -f "$so_path" ]; then
        cp "$so_path" "$BIN_DIR/plugins/"
        SIZE=$(wc -c < "$so_path")
        SIZE_MB=$(( SIZE / 1048576 ))
        COPIED+=("plugins/libplugin_vm.so (${SIZE_MB} MB)")
        VM_DLL_FOUND=true
        break
    fi
done
if [ "$VM_DLL_FOUND" = false ] && [ "$SKIP_PLUGIN" = false ]; then
    echo "  WARN libplugin_vm.so not found in build output (VM 显示中继将不可用)"
fi

# Copy test-tools binaries to bin/bin_linux/tests/
mkdir -p "$BIN_DIR/tests"
for exe in cluster-test integration-test mcp-server; do
    if [ -f "$RELEASE_DIR/$exe" ]; then
        cp "$RELEASE_DIR/$exe" "$BIN_DIR/tests/"
        chmod +x "$BIN_DIR/tests/$exe"
        SIZE=$(wc -c < "$RELEASE_DIR/$exe")
        SIZE_MB=$(( SIZE / 1048576 ))
        COPIED+=("$exe (${SIZE_MB} MB)")
    fi
done

echo "  OK Copied ${#COPIED[@]} file(s) to $BIN_DIR/"
echo ""

# ============================================
# Optional: build skins (.nbskin) — skins/ 子模块在场且 Node 可用才构建。
# 本地构建只打包不签名（.nbskin 无签名照常可用，管理面标 ⚪ unsigned）；
# 签名分发走 daily-release CI 统一铸叶签署。
# ============================================
if [ -d "skins" ]; then
    if command -v node >/dev/null 2>&1; then
        echo "  Building skins (.nbskin)..."
        mkdir -p "$BIN_DIR/skins"
        SKIN_COUNT=0
        for skin_dir in skins/*/; do
            [ -f "${skin_dir}dev/pack.mjs" ] || continue
            echo "    skin: $(basename "$skin_dir")"
            (
                cd "$skin_dir"
                if [ -f "ui-src/package.json" ] && [ ! -d "ui-src/node_modules" ]; then
                    npm install --no-fund --no-audit >/dev/null 2>&1
                fi
                if [ -f "ui-src/package.json" ]; then
                    npm run build >/dev/null 2>&1
                fi
                node dev/pack.mjs
            ) || {
                echo "    WARN skin $(basename "$skin_dir") package failed (non-fatal, continuing)"
                continue
            }
            if cp "${skin_dir}dist/"*.nbskin "$BIN_DIR/skins/" 2>/dev/null; then
                SKIN_COUNT=$((SKIN_COUNT+1))
            fi
        done
        if [ "$SKIN_COUNT" -eq 0 ]; then
            echo "  SKIP no skins/*/dev/pack.mjs found"
        else
            echo "  OK $SKIN_COUNT skin package(s) copied to $BIN_DIR/skins/"
        fi
    else
        echo "  WARN Node.js not found in PATH — skipping .nbskin skin packaging"
    fi
else
    echo "  SKIP skins/ not found — no .nbskin packaging"
fi
echo ""

# ============================================
# Runtime manifest (A1 运行时完整性清单；生成失败诚实降级不阻塞打包)
# ============================================
if command -v python3 >/dev/null 2>&1; then
    if ! python3 scripts/gen_runtime_manifest.py --out-dir "$BIN_DIR" --version "$VERSION" --commit "$GIT_COMMIT"; then
        echo "  WARN runtime-manifest generation failed — gateway startup will treat deployment as unchecked (manifest absent)"
    fi
else
    echo "  WARN python3 not found in PATH — skipping runtime-manifest generation"
fi
echo ""

# ============================================
# Summary
# ============================================
echo "============================================"
echo " Build Summary (Linux)"
echo "============================================"
echo " Version: $VERSION"
echo " Commit:  $GIT_COMMIT"
echo ""
echo " $BIN_DIR/"
for item in "${COPIED[@]}"; do
    echo "   ├── $item"
done
echo ""
echo "[SUCCESS] Build completed!"
echo ""
echo "Run: ./$BIN_DIR/nemesisbot gateway"
echo "============================================"
