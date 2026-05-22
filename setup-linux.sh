#!/usr/bin/env bash
# ============================================
# NemesisBot Linux Environment Setup Script
# ============================================
# Detects and installs all system dependencies
# required to build NemesisBot on Linux / WSL.
#
# Usage:
#   ./setup-linux.sh           # Detect + install
#   ./setup-linux.sh --dry-run # Detect only, no install
#   ./setup-linux.sh --help    # Show help
#
# Dependencies installed:
#   - System packages (apt): GTK3, WebKitGTK, OpenSSL, etc.
#   - Rust toolchain (rustup)
#   - Node.js / npm (NodeSource LTS)

set -euo pipefail

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
echo "============================================"
echo " NemesisBot Linux Environment Setup"
echo "============================================"

if [ "$DRY_RUN" = true ]; then
    echo -e " Mode: ${YELLOW}DRY RUN (detect only)${NC}"
else
    echo -e " Mode: ${GREEN}INSTALL${NC}"
fi

# Detect distro
if [ -f /etc/os-release ]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    echo " OS:        ${PRETTY_NAME:-unknown}"
    echo " ID:        ${ID:-unknown}"
else
    echo " OS:        unknown (no /etc/os-release)"
fi

echo "============================================"

# ============================================
# Root / sudo check
# ============================================
step_header "Step 1/4: Permission check"

if [ "$(id -u)" -eq 0 ]; then
    ok "Running as root"
    SUDO=""
else
    if command -v sudo >/dev/null 2>&1; then
        ok "sudo available"
        SUDO="sudo"
    else
        fail "Not running as root and sudo not available"
        echo "  Please run as root or install sudo first:"
        echo "    su -c 'apt install -y sudo'"
        exit 1
    fi
fi

# ============================================
# Step 2: System packages (apt)
# ============================================
step_header "Step 2/4: System packages (apt)"

# Required apt packages
APT_PACKAGES=(
    build-essential
    pkg-config
    libssl-dev
    libgtk-3-dev
    libayatana-appindicator3-dev
    libjavascriptcoregtk-4.1-dev
    libsoup-3.0-dev
    libwebkit2gtk-4.1-dev
    libxdo-dev
    libx11-dev
)

MISSING_PACKAGES=()

info "Checking ${#APT_PACKAGES[@]} system packages..."

for pkg in "${APT_PACKAGES[@]}"; do
    if dpkg -s "$pkg" >/dev/null 2>&1; then
        ok "$pkg"
    else
        miss "$pkg (not installed)"
        MISSING_PACKAGES+=("$pkg")
    fi
done

if [ ${#MISSING_PACKAGES[@]} -eq 0 ]; then
    ok "All system packages installed"
elif [ "$DRY_RUN" = true ]; then
    echo ""
    info "DRY RUN: would install ${#MISSING_PACKAGES[@]} packages:"
    for pkg in "${MISSING_PACKAGES[@]}"; do
        echo "         - $pkg"
    done
else
    echo ""
    info "Installing ${#MISSING_PACKAGES[@]} missing packages..."

    $SUDO DEBIAN_FRONTEND=noninteractive apt-get update -qq

    $SUDO DEBIAN_FRONTEND=noninteractive apt-get install -y "${MISSING_PACKAGES[@]}"

    # Verify installation
    ALL_OK=true
    for pkg in "${MISSING_PACKAGES[@]}"; do
        if dpkg -s "$pkg" >/dev/null 2>&1; then
            ok "$pkg installed"
        else
            fail "$pkg installation failed"
            ALL_OK=false
        fi
    done

    if [ "$ALL_OK" = false ]; then
        fail "Some packages failed to install"
        echo "  Try manually: sudo apt-get install -y ${MISS_PACKAGES[*]}"
    fi
fi

# ============================================
# Step 3: Rust toolchain
# ============================================
step_header "Step 3/4: Rust toolchain"

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
# Step 4: Node.js / npm
# ============================================
step_header "Step 4/4: Node.js / npm"

NODE_INSTALLED=false
if command -v node >/dev/null 2>&1; then
    NODE_VER="$(node --version 2>/dev/null || echo "unknown")"
    ok "node: $NODE_VER"
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
        info "DRY RUN: would install Node.js LTS via NodeSource"
    else
        info "Installing Node.js LTS via NodeSource..."

        # Detect distro family
        if [ -f /etc/os-release ]; then
            # shellcheck disable=SC1091
            . /etc/os-release
        fi

        DISTRO="${ID:-ubuntu}"

        case "$DISTRO" in
            ubuntu|debian|linuxmint|pop)
                # Install NodeSource setup script for Node 20.x LTS
                $SUDO DEBIAN_FRONTEND=noninteractive apt-get update -qq
                $SUDO DEBIAN_FRONTEND=noninteractive apt-get install -y ca-certificates curl
                curl -fsSL https://deb.nodesource.com/setup_20.x | $SUDO -E bash -
                $SUDO DEBIAN_FRONTEND=noninteractive apt-get install -y nodejs
                ;;
            *)
                # Fallback: try nvm-style install
                info "Distro '$DISTRO' not directly supported for NodeSource"
                info "Attempting nvm install..."
                curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
                # shellcheck disable=SC1090
                [ -f "$HOME/.nvm/nvm.sh" ] && . "$HOME/.nvm/nvm.sh"
                nvm install --lts
                ;;
        esac

        # Verify
        if command -v node >/dev/null 2>&1; then
            ok "node installed: $(node --version)"
            ok "npm installed: $(npm --version)"
        else
            fail "Node.js installation failed"
            echo "  Please install manually: https://nodejs.org/en/download/"
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
for cmd in gcc make pkg-config rustc cargo node npm; do
    if command -v "$cmd" >/dev/null 2>&1; then
        ok "$cmd: $(command -v "$cmd")"
    else
        fail "$cmd: not found"
        VERIFY_OK=false
        VERIFY_ISSUES+=("$cmd")
    fi
done

# Check libraries via pkg-config
info "Checking libraries..."
for lib in openssl gtk+-3.0 javascriptcoregtk-4.1 libsoup-3.0 webkit2gtk-4.1 x11; do
    if pkg-config --exists "$lib" 2>/dev/null; then
        ok "$lib: $(pkg-config --modversion "$lib" 2>/dev/null)"
    else
        fail "$lib: not found"
        VERIFY_OK=false
        VERIFY_ISSUES+=("$lib (lib)")
    fi
done

# Check optional libraries
for lib in ayatana-appindicator3-0.1 xdo; do
    if pkg-config --exists "$lib" 2>/dev/null; then
        ok "$lib: $(pkg-config --modversion "$lib" 2>/dev/null)"
    else
        skip "$lib: not found (optional)"
    fi
done

# ============================================
# Summary
# ============================================
echo ""
echo "============================================"
echo " Setup Summary"
echo "============================================"

if [ "$DRY_RUN" = true ]; then
    echo -e " Mode: ${YELLOW}DRY RUN${NC} (no changes made)"
fi

echo ""
echo " System packages checked: ${#APT_PACKAGES[@]}"

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
    echo "   ./build.sh"
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
