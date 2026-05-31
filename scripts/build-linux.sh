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
#   │   └── plugin_onnx.so
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

    if [ ! -d "web/node_modules" ]; then
        echo "  Installing npm dependencies..."
        (cd web && npm install 2>&1) || {
            echo "  WARN npm install failed, skipping Vue build"
            echo ""
            # fall through to step 3
        }
    fi

    if [ -d "web/node_modules" ]; then
        echo "  Running Vite build..."
        if (cd web && npm run build 2>&1); then
            echo "  OK Vue frontend built"
        else
            echo "  WARN Vue build failed, using existing static files"
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
