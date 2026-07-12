#!/usr/bin/env bash
# ============================================
# NemesisBot Rust Build Script (macOS)
# ============================================
# Usage: scripts/build-macos.sh [options]
#   No arguments  — Build release, copy to bin/bin_macos/
#   --clean       — Clean before building
#   --skip-plugin — Skip plugin .dylib build
#   --help        — Show help
#
# Output layout:
#   bin/bin_macos/
#   ├── nemesisbot
#   └── plugins/
#       ├── libplugin_ui.dylib
#       └── libplugin_onnx.dylib
#
# Note: Only builds the main nemesisbot binary (+ its crate dependencies).
#       Test-tools (integration-test / cluster-test / ...) are NOT compiled.

set -euo pipefail

# Switch to project root (parent of scripts/)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

# Use separate target/output dirs to avoid conflicts with Windows/Linux build
ROOT_DIR="$(pwd)"
export CARGO_TARGET_DIR="${ROOT_DIR}/target/target_macos"
BIN_DIR="bin/bin_macos"

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
            echo "Usage: scripts/build-macos.sh [--clean] [--skip-plugin] [--help]"
            echo ""
            echo "Options:"
            echo "  --clean        Clean target/target_macos before building"
            echo "  --skip-plugin  Skip plugin .dylib build"
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

# Detect architecture (Apple Silicon vs Intel)
ARCH="$(uname -m)"
if [ "$ARCH" = "arm64" ]; then
    ARCH_LABEL="Apple Silicon (arm64)"
else
    ARCH_LABEL="Intel (x86_64)"
fi

echo "============================================"
echo " NemesisBot Rust Build (macOS)"
echo "============================================"
echo " Version:     $VERSION"
echo " Git Commit:  $GIT_COMMIT"
echo " Rustc:       $RUSTC_VERSION"
echo " Arch:        $ARCH_LABEL"
echo " Target Dir:  target/target_macos"
echo " Output Dir:  $BIN_DIR/"
echo "============================================"
echo ""

# ============================================
# Step 1: Clean (optional)
# ============================================
if [ "$CLEAN" = true ]; then
    echo "[Step 1/5] Cleaning target/target_macos..."
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
    # On macOS, node_modules from Windows/Linux may have incompatible binaries.
    # Detect mismatched platform packages and reinstall.
    NEED_REINSTALL=false
    if [ -d "web/node_modules" ]; then
        if [ "$ARCH" = "arm64" ]; then
            # Check for non-arm64 rollup
            if [ -f "web/node_modules/@rollup/rollup-linux-x64-gnu/package.json" ] && \
               [ ! -d "web/node_modules/@rollup/rollup-darwin-arm64" ]; then
                NEED_REINSTALL=true
                echo "  Detected non-macOS-arm64 node_modules, reinstalling..."
            fi
        else
            # Intel: check for non-darwin-x64
            if [ -f "web/node_modules/@rollup/rollup-linux-x64-gnu/package.json" ] && \
               [ ! -d "web/node_modules/@rollup/rollup-darwin-x64" ]; then
                NEED_REINSTALL=true
                echo "  Detected non-macOS-x64 node_modules, reinstalling..."
            fi
        fi
    fi

    if [ "$NEED_REINSTALL" = true ]; then
        rm -rf web/node_modules web/package-lock.json
    fi

    # Always run npm install to ensure all declared dependencies are present.
    echo "  Checking npm dependencies..."
    (cd web && npm install 2>&1) || {
        echo "  WARN npm install failed, skipping Vue build"
        echo ""
    }

    if [ -d "web/node_modules" ]; then
        echo "  Cleaning stale Vite assets (orphaned hashed chunks get embedded into the binary via include_dir!, bloating it ~2MB)..."
        rm -rf "crates/nemesis-web/static/assets"
        rm -f "web/.env"
        echo "  Running Vite build..."
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
echo "[Step 3/5] Building release (nemesisbot only, skipping test-tools)..."
START_TIME=$SECONDS

if ! cargo build --release -p nemesisbot 2>&1; then
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
    echo "[Step 4/5] Plugin .dylib skipped (--skip-plugin)"
    echo ""
else
    echo "[Step 4/5] Building plugin shared libraries..."

    # --- plugin-ui ---
    PLUGIN_UI_DIR="plugins/plugin-ui"
    if [ -f "$PLUGIN_UI_DIR/Cargo.toml" ]; then
        echo "  Building plugin-ui..."
        DLL_START=$SECONDS
        if (cd "$PLUGIN_UI_DIR" && CARGO_TARGET_DIR="${ROOT_DIR}/target/target_macos/plugins/plugin-ui" cargo build --release 2>&1); then
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
        if (cd "$PLUGIN_ONNX_DIR" && CARGO_TARGET_DIR="${ROOT_DIR}/target/target_macos/plugins/plugin-onnx" cargo build --release 2>&1); then
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
# Step 5: Copy to bin/bin_macos/
# ============================================
echo "[Step 5/5] Copying to ${BIN_DIR}/..."

mkdir -p "$BIN_DIR"

# Copy main binary (no .exe extension on macOS)
COPIED=()
RELEASE_DIR="$CARGO_TARGET_DIR/release"
if [ -f "$RELEASE_DIR/nemesisbot" ]; then
    cp "$RELEASE_DIR/nemesisbot" "$BIN_DIR/"
    chmod +x "$BIN_DIR/nemesisbot"
    SIZE=$(wc -c < "$RELEASE_DIR/nemesisbot")
    SIZE_MB=$(( SIZE / 1048576 ))
    COPIED+=("nemesisbot (${SIZE_MB} MB)")
fi

# Copy plugin .dylib to bin/bin_macos/plugins/
mkdir -p "$BIN_DIR/plugins"

# On macOS, cdylib produces .dylib (not .so)
UI_FOUND=false
for dylib_path in "target/target_macos/plugins/plugin-ui/release/libplugin_ui.dylib" \
                  "target/target_macos/plugins/plugin-ui/release/libplugin_ui.so"; do
    if [ -f "$dylib_path" ]; then
        cp "$dylib_path" "$BIN_DIR/plugins/"
        SIZE=$(wc -c < "$dylib_path")
        SIZE_MB=$(( SIZE / 1048576 ))
        COPIED+=("libplugin_ui.dylib (${SIZE_MB} MB)")
        UI_FOUND=true
        break
    fi
done
if [ "$UI_FOUND" = false ] && [ "$SKIP_PLUGIN" = false ]; then
    echo "  WARN libplugin_ui.dylib not found in build output"
fi

ONNX_FOUND=false
for dylib_path in "target/target_macos/plugins/plugin-onnx/release/libplugin_onnx.dylib" \
                  "target/target_macos/plugins/plugin-onnx/release/libplugin_onnx.so"; do
    if [ -f "$dylib_path" ]; then
        cp "$dylib_path" "$BIN_DIR/plugins/"
        SIZE=$(wc -c < "$dylib_path")
        SIZE_MB=$(( SIZE / 1048576 ))
        COPIED+=("libplugin_onnx.dylib (${SIZE_MB} MB)")
        ONNX_FOUND=true
        break
    fi
done
if [ "$ONNX_FOUND" = false ] && [ "$SKIP_PLUGIN" = false ]; then
    echo "  WARN libplugin_onnx.dylib not found in build output"
fi

echo "  OK Copied ${#COPIED[@]} file(s) to $BIN_DIR/"
echo ""

# ============================================
# Summary
# ============================================
echo "============================================"
echo " Build Summary (macOS)"
echo "============================================"
echo " Version: $VERSION"
echo " Commit:  $GIT_COMMIT"
echo " Arch:    $ARCH_LABEL"
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
