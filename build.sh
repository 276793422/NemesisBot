#!/usr/bin/env bash
# ============================================
# NemesisBot Rust Build Script
# ============================================
# Usage: ./build.sh [options]
#   No arguments  — Build release, copy to bin/
#   --clean       — Clean before building
#   --skip-plugin — Skip plugin-ui.dll build
#   --help        — Show help
#
# Output layout:
#   bin/
#   ├── nemesisbot.exe
#   ├── plugin_ui.dll
#   ├── ai-server.exe
#   ├── cluster-test.exe
#   ├── integration-test.exe
#   └── mcp-server.exe

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

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
            echo "Usage: $0 [--clean] [--skip-plugin] [--help]"
            echo ""
            echo "Options:"
            echo "  --clean        Clean target before building"
            echo "  --skip-plugin  Skip plugin-ui.dll build"
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
echo " NemesisBot Rust Build"
echo "============================================"
echo " Version:     $VERSION"
echo " Git Commit:  $GIT_COMMIT"
echo " Rustc:       $RUSTC_VERSION"
echo "============================================"
echo ""

# ============================================
# Step 1: Clean (optional)
# ============================================
if [ "$CLEAN" = true ]; then
    echo "[Step 1/4] Cleaning target..."
    cargo clean 2>/dev/null || true
    echo "  OK Cleaned"
    echo ""
else
    echo "[Step 1/4] Clean skipped (use --clean to enable)"
    echo ""
fi

# ============================================
# Step 2: Build main workspace (release)
# ============================================
echo "[Step 2/4] Building release..."
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
# Step 3: Build plugin-ui DLL (release)
# ============================================
if [ "$SKIP_PLUGIN" = true ]; then
    echo "[Step 3/4] Plugin DLL skipped (--skip-plugin)"
    echo ""
else
    echo "[Step 3/4] Building plugin-ui DLL..."
    PLUGIN_DIR="plugins/plugin-ui"

    if [ -f "$PLUGIN_DIR/Cargo.toml" ]; then
        DLL_START=$SECONDS
        if (cd "$PLUGIN_DIR" && cargo build --release 2>&1); then
            DLL_DURATION=$(( SECONDS - DLL_START ))
            echo "  OK Plugin DLL built in ${DLL_DURATION}s"
        else
            echo "  WARN Plugin DLL build failed (non-fatal, continuing without plugin)"
        fi
    else
        echo "  SKIP $PLUGIN_DIR/Cargo.toml not found"
    fi
    echo ""
fi

# ============================================
# Step 4: Copy to bin/
# ============================================
echo "[Step 4/4] Copying to bin/..."

BIN_DIR="bin"
mkdir -p "$BIN_DIR"

# Copy main binary
COPIED=()
if [ -f "target/release/nemesisbot.exe" ]; then
    cp "target/release/nemesisbot.exe" "$BIN_DIR/"
    SIZE=$(wc -c < "target/release/nemesisbot.exe")
    SIZE_MB=$(( SIZE / 1048576 ))
    COPIED+=("nemesisbot.exe (${SIZE_MB} MB)")
fi

# Copy plugin DLL to bin/plugins/
DLL_FOUND=false
mkdir -p "$BIN_DIR/plugins"
for dll_path in "plugins/plugin-ui/target/release/plugin_ui.dll" \
                "plugins/plugin-ui/target/release/plugin-ui.dll"; do
    if [ -f "$dll_path" ]; then
        cp "$dll_path" "$BIN_DIR/plugins/"
        SIZE=$(wc -c < "$dll_path")
        SIZE_MB=$(( SIZE / 1048576 ))
        COPIED+=("plugin_ui.dll (${SIZE_MB} MB)")
        DLL_FOUND=true
        break
    fi
done

if [ "$DLL_FOUND" = false ] && [ "$SKIP_PLUGIN" = false ]; then
    echo "  WARN plugin-ui.dll not found in build output"
fi

# Copy test-tools binaries to bin/tests/
mkdir -p "$BIN_DIR/tests"
for exe in ai-server.exe cluster-test.exe integration-test.exe mcp-server.exe; do
    if [ -f "target/release/$exe" ]; then
        cp "target/release/$exe" "$BIN_DIR/tests/"
        SIZE=$(wc -c < "target/release/$exe")
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
echo " Build Summary"
echo "============================================"
echo " Version: $VERSION"
echo " Commit:  $GIT_COMMIT"
echo ""
echo " bin/"
for item in "${COPIED[@]}"; do
    echo "   ├── $item"
done
echo ""
echo "[SUCCESS] Build completed!"
echo ""
echo "Run: ./bin/nemesisbot.exe gateway"
echo "============================================"
