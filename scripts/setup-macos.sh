#!/usr/bin/env bash
# ============================================
# NemesisBot macOS Environment Setup Script
# ============================================
# Detects and installs all system dependencies
# required to build NemesisBot on macOS (Intel / Apple Silicon).
#
# Usage:
#   ./setup-macos.sh           # Detect + install
#   ./setup-macos.sh --dry-run # Detect only, no install
#   ./setup-macos.sh --help    # Show help
#
# Dependencies installed:
#   - Homebrew packages: OpenSSL, pkg-config, etc.
#   - Rust toolchain (rustup)
#   - Node.js / npm (Homebrew or NodeSource)
#
# Prerequisites:
#   - macOS 12+ (Monterey or later recommended)
#   - Xcode Command Line Tools (xcode-select --install)
#   - Homebrew (https://brew.sh) — will prompt to install if missing

set -euo pipefail

# Switch to project root (parent of scripts/)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

# ============================================
# Colors
# ============================================
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

# ============================================
# Parse Arguments
# ============================================
DRY_RUN=false

for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=true ;;
        --help|-h)
            echo "Usage: $0 [--dry-run] [--help]"
            echo ""
            echo "Options:"
            echo "  --dry-run  Detect missing dependencies without installing"
            echo "  --help     Show this help"
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown argument: $arg${NC}"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# ============================================
# Helpers
# ============================================
ok()   { echo -e "  ${GREEN}[OK]${NC}      $1"; }
skip() { echo -e "  ${YELLOW}[SKIP]${NC}    $1"; }
miss() { echo -e "  ${CYAN}[MISS]${NC}    $1"; }
fail() { echo -e "  ${RED}[FAIL]${NC}    $1"; }
info() { echo -e "  ${BOLD}[INFO]${NC}    $1"; }

step_header() {
    echo ""
    echo -e "${BOLD}>>> $1${NC}"
    echo ""
}

# ============================================
# Banner
# ============================================

# Detect architecture
ARCH="$(uname -m)"
if [ "$ARCH" = "arm64" ]; then
    ARCH_LABEL="Apple Silicon (arm64)"
else
    ARCH_LABEL="Intel (x86_64)"
fi

# Detect macOS version
MACOS_VER="$(sw_vers -productVersion 2>/dev/null || echo "unknown")"
MACOS_NAME="$(sw_vers -productVersion 2>/dev/null | cut -d. -f1)"
case "$MACOS_NAME" in
    12)  MACOS_NAME="Monterey" ;;
    13)  MACOS_NAME="Ventura" ;;
    14)  MACOS_NAME="Sonoma" ;;
    15)  MACOS_NAME="Sequoia" ;;
    26)  MACOS_NAME="Tahoe" ;;
    *)   MACOS_NAME="macOS" ;;
esac

echo "============================================"
echo " NemesisBot macOS Environment Setup"
echo "============================================"

if [ "$DRY_RUN" = true ]; then
    echo -e " Mode: ${YELLOW}DRY RUN (detect only)${NC}"
else
    echo -e " Mode: ${GREEN}INSTALL${NC}"
fi

echo " OS:        macOS $MACOS_NAME ($MACOS_VER)"
echo " Arch:      $ARCH_LABEL"
echo "============================================"

# ============================================
# Step 1: Xcode Command Line Tools
# ============================================
step_header "Step 1/5: Xcode Command Line Tools"

# Check if Xcode CLT is installed
if xcode-select -p >/dev/null 2>&1; then
    CLT_PATH="$(xcode-select -p 2>/dev/null)"
    ok "Xcode CLT: $CLT_PATH"
else
    miss "Xcode Command Line Tools not found"
    if [ "$DRY_RUN" = true ]; then
        info "DRY RUN: would install Xcode CLT"
        echo "         Run: xcode-select --install"
    else
        info "Installing Xcode Command Line Tools..."
        echo "  A GUI prompt will appear — click 'Install' and wait."
        xcode-select --install 2>/dev/null || true

        # Wait for user to complete installation
        echo ""
        echo -e "  ${YELLOW}Press ENTER after Xcode CLT installation completes...${NC}"
        read -r

        if xcode-select -p >/dev/null 2>&1; then
            ok "Xcode CLT installed: $(xcode-select -p)"
        else
            fail "Xcode CLT installation not detected"
            echo "  Please install manually: xcode-select --install"
            exit 1
        fi
    fi
fi

# ============================================
# Step 2: Homebrew
# ============================================
step_header "Step 2/5: Homebrew"

BREW_INSTALLED=false
if command -v brew >/dev/null 2>&1; then
    BREW_VER="$(brew --version 2>/dev/null | head -1 || echo "unknown")"
    ok "brew: $BREW_VER"
    BREW_INSTALLED=true

    # Ensure Homebrew is up to date
    if [ "$DRY_RUN" = false ]; then
        info "Updating Homebrew..."
        brew update >/dev/null 2>&1 || true
        ok "Homebrew updated"
    fi
else
    miss "Homebrew not found"
    if [ "$DRY_RUN" = true ]; then
        info "DRY RUN: would install Homebrew"
        echo "         Run: /bin/bash -c \"\$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\""
    else
        info "Installing Homebrew..."
        /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

        # Add brew to PATH for current session
        if [ "$ARCH" = "arm64" ]; then
            eval "$(/opt/homebrew/bin/brew shellenv)"
        else
            eval "$(/usr/local/bin/brew shellenv)"
        fi

        if command -v brew >/dev/null 2>&1; then
            ok "Homebrew installed: $(brew --version | head -1)"
            BREW_INSTALLED=true
        else
            fail "Homebrew installation failed"
            echo "  Please install manually: https://brew.sh"
            exit 1
        fi
    fi
fi

# ============================================
# Step 3: Homebrew packages
# ============================================
step_header "Step 3/5: Homebrew packages"

# Required Homebrew packages for NemesisBot on macOS.
# Unlike Linux (GTK3 + WebKitGTK), macOS uses native Cocoa/AppKit via plugin-ui,
# so the heavy GTK/WebKit dependencies are NOT needed.
# We only need build tools and OpenSSL.
BREW_PACKAGES=(
    pkg-config
    openssl@3
)

MISSING_PACKAGES=()

info "Checking ${#BREW_PACKAGES[@]} Homebrew packages..."

for pkg in "${BREW_PACKAGES[@]}"; do
    if brew list --versions "$pkg" >/dev/null 2>&1; then
        VER="$(brew list --versions "$pkg" 2>/dev/null | head -1)"
        ok "$VER"
    else
        miss "$pkg (not installed)"
        MISSING_PACKAGES+=("$pkg")
    fi
done

# Also check for optional but recommended packages
OPTIONAL_PACKAGES=(
    cmake
    git
)

for pkg in "${OPTIONAL_PACKAGES[@]}"; do
    if brew list --versions "$pkg" >/dev/null 2>&1; then
        VER="$(brew list --versions "$pkg" 2>/dev/null | head -1)"
        ok "$VER"
    else
        skip "$pkg (optional, not installed)"
    fi
done

if [ ${#MISSING_PACKAGES[@]} -eq 0 ]; then
    ok "All required Homebrew packages installed"
elif [ "$DRY_RUN" = true ]; then
    echo ""
    info "DRY RUN: would install ${#MISSING_PACKAGES[@]} packages:"
    for pkg in "${MISSING_PACKAGES[@]}"; do
        echo "         - $pkg"
    done
else
    echo ""
    info "Installing ${#MISSING_PACKAGES[@]} missing packages..."
    brew install "${MISSING_PACKAGES[@]}"

    # Verify installation
    ALL_OK=true
    for pkg in "${MISSING_PACKAGES[@]}"; do
        if brew list --versions "$pkg" >/dev/null 2>&1; then
            ok "$pkg installed"
        else
            fail "$pkg installation failed"
            ALL_OK=false
        fi
    done

    if [ "$ALL_OK" = false ]; then
        fail "Some packages failed to install"
        echo "  Try manually: brew install ${MISSING_PACKAGES[*]}"
    fi
fi

# ============================================
# Step 4: Rust toolchain
# ============================================
step_header "Step 4/5: Rust toolchain"

RUST_INSTALLED=false
if command -v rustc >/dev/null 2>&1; then
    RUSTC_VER="$(rustc --version 2>/dev/null || echo "unknown")"
    ok "rustc: $RUSTC_VER"
    RUST_INSTALLED=true
else
    miss "rustc not found"
fi

if command -v cargo >/dev/null 2>&1; then
    CARGO_VER="$(cargo --version 2>/dev/null || echo "unknown")"
    ok "cargo: $CARGO_VER"
else
    miss "cargo not found"
fi

if [ "$RUST_INSTALLED" = false ]; then
    if [ "$DRY_RUN" = true ]; then
        info "DRY RUN: would install Rust via rustup"
    else
        info "Installing Rust toolchain via rustup..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable

        # Source the newly installed env
        # shellcheck disable=SC1090
        [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

        if command -v rustc >/dev/null 2>&1; then
            ok "rustc installed: $(rustc --version)"
            ok "cargo installed: $(cargo --version)"
        else
            fail "Rust installation failed"
            echo "  Please install manually: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        fi
    fi
fi

# wasm-pack (optional, for plugin-onnx)
if command -v wasm-pack >/dev/null 2>&1; then
    ok "wasm-pack: $(wasm-pack --version 2>/dev/null || echo "installed")"
else
    skip "wasm-pack not installed (optional, only needed for plugin-onnx WASM target)"
fi

# ============================================
# Step 5: Node.js / npm
# ============================================
step_header "Step 5/5: Node.js / npm"

NODE_INSTALLED=false
if command -v node >/dev/null 2>&1; then
    NODE_VER="$(node --version 2>/dev/null || echo "unknown")"
    ok "node: $NODE_VER"

    # Check version is >= 18
    NODE_MAJOR="$(node --version 2>/dev/null | sed 's/v\([0-9]*\).*/\1/' || echo "0")"
    if [ "$NODE_MAJOR" -lt 18 ]; then
        miss "node version $NODE_VER is < 18 (Vite requires Node 18+)"
    fi

    NODE_INSTALLED=true
else
    miss "node not found"
fi

if command -v npm >/dev/null 2>&1; then
    NPM_VER="$(npm --version 2>/dev/null || echo "unknown")"
    ok "npm: $NPM_VER"
else
    miss "npm not found"
fi

if [ "$NODE_INSTALLED" = false ]; then
    if [ "$DRY_RUN" = true ]; then
        info "DRY RUN: would install Node.js via Homebrew"
    else
        if [ "$BREW_INSTALLED" = true ]; then
            info "Installing Node.js via Homebrew..."
            brew install node

            # Verify
            if command -v node >/dev/null 2>&1; then
                ok "node installed: $(node --version)"
                ok "npm installed: $(npm --version)"
            else
                fail "Node.js installation failed"
                echo "  Please install manually: https://nodejs.org/en/download/"
            fi
        else
            info "Homebrew not available, using NodeSource..."
            curl -fsSL https://nodejs.org/dist/v20.18.0/node-v20.18.0.pkg > /tmp/node-lts.pkg
            sudo installer -pkg /tmp/node-lts.pkg -target /

            if command -v node >/dev/null 2>&1; then
                ok "node installed: $(node --version)"
                ok "npm installed: $(npm --version)"
            else
                fail "Node.js installation failed"
                echo "  Please install manually: https://nodejs.org/en/download/"
            fi
        fi
    fi
fi

# ============================================
# Environment Verification
# ============================================
step_header "Environment Verification"

VERIFY_OK=true
VERIFY_ISSUES=()

# Check commands
info "Checking commands..."
for cmd in clang make pkg-config rustc cargo node npm; do
    if command -v "$cmd" >/dev/null 2>&1; then
        ok "$cmd: $(command -v "$cmd")"
    else
        fail "$cmd: not found"
        VERIFY_OK=false
        VERIFY_ISSUES+=("$cmd")
    fi
done

# Check OpenSSL via pkg-config or brew prefix
info "Checking libraries..."

# Set OpenSSL pkg-config path if using Homebrew openssl
if [ "$ARCH" = "arm64" ]; then
    OPENSSL_PREFIX="/opt/homebrew/opt/openssl@3"
else
    OPENSSL_PREFIX="/usr/local/opt/openssl@3"
fi

if [ -d "$OPENSSL_PREFIX" ]; then
    export PKG_CONFIG_PATH="$OPENSSL_PREFIX/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
fi

if pkg-config --exists openssl 2>/dev/null; then
    ok "openssl: $(pkg-config --modversion openssl 2>/dev/null)"
else
    # Fallback: check if openssl is available via brew
    if brew list --versions openssl >/dev/null 2>&1 || \
       brew list --versions openssl@3 >/dev/null 2>&1; then
        ok "openssl: $(brew list --versions openssl 2>/dev/null || brew list --versions openssl@3 2>/dev/null)"
        info "  (detected via Homebrew; set PKG_CONFIG_PATH if cargo build fails)"
        info "  export PKG_CONFIG_PATH=\"$OPENSSL_PREFIX/lib/pkgconfig\""
    else
        fail "openssl: not found"
        VERIFY_OK=false
        VERIFY_ISSUES+=("openssl (lib)")
    fi
fi

# ============================================
# Summary
# ============================================
echo ""
echo "============================================"
echo " Setup Summary (macOS)"
echo "============================================"

if [ "$DRY_RUN" = true ]; then
    echo -e " Mode: ${YELLOW}DRY RUN${NC} (no changes made)"
fi

echo " OS:  macOS $MACOS_NAME ($MACOS_VER)"
echo " Arch: $ARCH_LABEL"
echo ""
echo " Homebrew packages checked: ${#BREW_PACKAGES[@]}"

if [ ${#MISSING_PACKAGES[@]} -gt 0 ]; then
    if [ "$DRY_RUN" = true ]; then
        echo -e " Missing: ${YELLOW}${#MISSING_PACKAGES[@]}${NC} (would install)"
    else
        echo -e " Installed: ${GREEN}${#MISSING_PACKAGES[@]}${NC}"
    fi
else
    echo -e " All installed: ${GREEN}OK${NC}"
fi

echo ""

if [ "$VERIFY_OK" = true ]; then
    echo -e " ${GREEN}All dependencies satisfied!${NC}"
    echo ""
    echo " You can now build NemesisBot:"
    echo "   scripts/build-macos.sh"
    echo ""

    # macOS-specific OpenSSL hint
    if [ -n "${OPENSSL_PREFIX:-}" ] && [ -d "${OPENSSL_PREFIX:-}" ]; then
        echo -e " ${YELLOW}Tip:${NC} If cargo build fails with OpenSSL errors, run:"
        echo "   export PKG_CONFIG_PATH=\"$OPENSSL_PREFIX/lib/pkgconfig\""
        echo ""
    fi
else
    echo -e " ${RED}Some dependencies are missing:${NC}"
    for issue in "${VERIFY_ISSUES[@]}"; do
        echo "   - $issue"
    done
    echo ""
    echo " Try installing manually or re-run without --dry-run"

    if [ ${#VERIFY_ISSUES[@]} -gt 0 ]; then
        exit 1
    fi
fi

echo "============================================"
