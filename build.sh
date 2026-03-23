#!/bin/bash
# ================================================
# NemesisBot Build Script (Bash)
# ================================================
# Linux/macOS alternative to PowerShell build.ps1
#
# Usage examples:
#   ./build.sh              # Build for current platform
#   ./build.sh --rebuild    # Clean and rebuild
#   ./build.sh --clean      # Clean build files
#   ./build.sh --test       # Run tests
#   ./build.sh --all        # Build all platforms
#   ./build.sh --release    # Create release packages
# ================================================

set -e

# ================================================
# Configuration
# ================================================
PROJECT_NAME="nemesisbot"
BUILD_DIR="build"
ANDROID_MIN_API="21"

# ================================================
# Color Functions
# ================================================
print_info() {
    echo -e "\033[0;36m[INFO] $1\033[0m"
}

print_success() {
    echo -e "\033[0;32m[OK] $1\033[0m"
}

print_warning() {
    echo -e "\033[0;33m[WARN] $1\033[0m"
}

print_error() {
    echo -e "\033[0;31m[ERROR] $1\033[0m"
}

print_section() {
    echo ""
    echo -e "\033[0;34m========================================"
    echo "  $1"
    echo -e "========================================\033[0m"
    echo ""
}

# ================================================
# Helper Functions
# ================================================

get_git_info() {
    local version="0.0.0.1"
    local commit="unknown"

    # Try to get version from git tags
    if version=$(git describe --tags --abbrev=0 2>/dev/null); then
        if [ -n "$version" ]; then
            version="$version"
        fi
    fi

    # Try to get commit hash
    if commit=$(git rev-parse --short HEAD 2>/dev/null); then
        if [ -n "$commit" ]; then
            commit="$commit"
        fi
    fi

    local build_time=$(date +"%Y/%m/%d")
    local go_version=$(go version 2>/dev/null | grep -oP 'go\d+\.\d+\.\d+' || echo "unknown")

    # Return as space-separated string
    echo "$version $commit $build_time $go_version"
}

invoke_go_build() {
    local output="$1"
    local goos="$2"
    local goarch="$3"
    local tags="$4"
    local git_info="$5"

    # Parse git info
    read -r version commit build_time go_version <<< "$git_info"

    # Set platform
    if [ -n "$goos" ]; then
        export GOOS="$goos"
    fi
    if [ -n "$goarch" ]; then
        export GOARCH="$goarch"
    fi

    # Build ldflags
    local ldflags="-X main.version=${version} -X main.gitCommit=${commit} -X main.buildTime=${build_time} -X main.goVersion=${go_version} -s -w"

    # Build command
    local build_cmd="go build"
    if [ -n "$tags" ]; then
        build_cmd="$build_cmd -tags $tags"
    fi
    build_cmd="$build_cmd -ldflags \"$ldflags\" -o $output ./nemesisbot/"

    print_info "Executing: $build_cmd"

    eval "$build_cmd"
    local exit_code=$?

    if [ $exit_code -eq 0 ]; then
        if [ -f "$output" ]; then
            local size=$(wc -c < "$output" 2>/dev/null || stat -f%s "$output" 2>/dev/null)
            local size_mb=$((size / 1048576))
            if [ $size_mb -lt 1 ]; then
                size_mb="$((size / 1024)) KiB"
            else
                size_mb="$size_mb MB"
            fi

            echo ""
            echo -e "\033[0;32m========================================"
            echo "  Build Successful!"
            echo -e "\033[0;32m========================================"
            echo ""
            echo "Build Information:"
            echo -e "  Version:    \033[0;33m${version}\033[0m"
            echo -e "  Git Commit: \033[0;33m${commit}\033[0m"
            echo -e "  Build Time: \033[0;33m${build_time}\033[0m"
            echo -e "  Go Version: \033[0;33m${go_version}\033[0m"
            echo ""
            echo "Build Parameters:"
            echo -e "  Platform:   \033[0;33m${GOOS}/${GOARCH}\033[0m"
            if [ -n "$tags" ]; then
                echo -e "  Build Tags: \033[0;33m${tags}\033[0m"
            else
                echo -e "  Build Tags: \033[0;37mNone\033[0m"
            fi
            echo -e "  Output:     \033[0;33m${output}\033[0m"
            echo ""
            echo "Output File:"
            echo -e "  Path:       \033[0;33m${output}\033[0m"
            echo -e "  Size:       \033[0;33m${size_mb} (${size} bytes)\033[0m"
            echo -e "  Created:    \033[0;33m$(stat -c %y "$output" 2>/dev/null || stat -f "%Sm" "$output" 2>/dev/null)\033[0m"
            echo ""
            echo -e "\033[0;32m========================================"
        fi
    else
        print_error "Build failed with exit code $exit_code"
        exit 1
    fi

    # Clean up environment variables
    unset GOOS
    unset GOARCH
}

# ================================================
# Main Functions
# ================================================

build_project() {
    local tags="$1"
    local output_name="$2"

    print_section "Building $PROJECT_NAME"

    local git_info=$(get_git_info)
    read -r version commit build_time go_version <<< "$git_info"

    print_info "Project Information:"
    echo "  Name:       $PROJECT_NAME"
    echo "  Version:    $version"
    echo "  Commit:     $commit"
    echo "  Build Time: $build_time"
    echo "  Go Version: $go_version"
    echo "  Platform:   $(uname -s)/$(uname -m)"
    echo ""

    mkdir -p "$BUILD_DIR"

    local output="${output_name:-$BUILD_DIR/$PROJECT_NAME}"
    if [ "$(uname)" = "Linux" ]; then
        output="${output_name:-$BUILD_DIR/$PROJECT_NAME}"
    else
        output="${output_name:-$BUILD_DIR/$PROJECT_NAME}"
    fi

    print_info "Starting build..."

    invoke_go_build "$output" "" "" "$tags" "$git_info"
}

build_windows() {
    local tags="$1"

    print_section "Building Windows (amd64)"

    local git_info=$(get_git_info)
    local dir="$BUILD_DIR/windows-amd64"
    mkdir -p "$dir"

    # Build for Windows (cross-compilation from Linux)
    # Note: Wails Desktop UI requires native compilation on Windows
    # On Linux, this will build without Desktop UI
    local cross_compile_tags="production"
    if [[ "$(uname)" != "Linux" ]]; then
        # Not on Linux - use stub implementation
        cross_compile_tags="cross_compile,production"
        print_warning "Desktop UI excluded (cross-compilation from non-Windows platform)"
    fi

    invoke_go_build "$dir/$PROJECT_NAME.exe" "windows" "amd64" "$cross_compile_tags" "$git_info"
    print_success "Windows amd64 build completed"
}

build_linux() {
    local tags="$1"

    print_section "Building Linux (amd64)"

    local git_info=$(get_git_info)
    local dir="$BUILD_DIR/linux-amd64"
    mkdir -p "$dir"

    # Check if running on Linux
    if [ "$(uname)" = "Linux" ]; then
        # Native compilation - include Desktop UI
        invoke_go_build "$dir/$PROJECT_NAME" "linux" "amd64" "production" "$git_info"
    else
        # Cross-compilation from other platforms
        print_warning "Desktop UI excluded (cross-compilation)"
        invoke_go_build "$dir/$PROJECT_NAME" "linux" "amd64" "production" "$git_info"
    fi

    print_success "Linux amd64 build completed"
}

build_darwin() {
    local tags="$1"

    print_section "Building macOS (amd64)"

    local git_info=$(get_git_info)
    local dir="$BUILD_DIR/darwin-amd64"
    mkdir -p "$dir"

    # Check if running on macOS
    if [ "$(uname)" = "Darwin" ]; then
        # Native compilation - include Desktop UI
        invoke_go_build "$dir/$PROJECT_NAME" "darwin" "amd64" "production" "$git_info"
    else
        # Cross-compilation from other platforms
        print_warning "Desktop UI excluded (cross-compilation)"
        invoke_go_build "$dir/$PROJECT_NAME" "darwin" "amd64" "production" "$git_info"
    fi

    print_success "macOS amd64 build completed"
}

build_android() {
    local tags="$1"
    local arch="${2:-arm64}"

    # Check NDK path
    local ndk_path="$NDK_PATH"
    if [ -z "$ndk_path" ]; then
        ndk_path="$ANDROID_NDK_HOME"
    fi

    if [ -z "$ndk_path" ]; then
        # Try common default paths
        for path in "$HOME/Android/Sdk/ndk" "$HOME/Android/Sdk/ndk-bundle"; do
            if [ -d "$path" ]; then
                ndk_path=$(find "$path" -maxdepth 1 -type d | sort -r | head -1)
                break
            fi
        done
    fi

    if [ -z "$ndk_path" ] || [ ! -d "$ndk_path" ]; then
        print_error "Android NDK not found. Please set ANDROID_NDK_HOME or NDK_PATH environment variable"
        exit 1
    fi

    # Determine architecture and compiler
    local clang=""
    local goarch=""
    case "$arch" in
        arm64)
            clang="aarch64-linux-android${ANDROID_MIN_API}-clang"
            goarch="arm64"
            ;;
        arm)
            clang="armv7a-linux-androideabi${ANDROID_MIN_API}-clang"
            goarch="arm"
            ;;
        386)
            clang="i686-linux-android${ANDROID_MIN_API}-clang"
            goarch="386"
            ;;
        amd64)
            clang="x86_64-linux-android${ANDROID_MIN_API}-clang"
            goarch="amd64"
            ;;
        *)
            print_error "Unsupported Android architecture: $arch"
            exit 1
            ;;
    esac

    print_section "Building Android ($arch)"

    local git_info=$(get_git_info)
    local dir="$BUILD_DIR/android-$arch"
    mkdir -p "$dir"

    print_info "NDK Path: $ndk_path"
    print_info "Min API Version: android$ANDROID_MIN_API"
    print_info "Architecture: $arch"

    # Set up environment variables
    export CGO_ENABLED=1
    export GOOS=android
    export GOARCH=$goarch

    # Set up compiler paths
    local toolchain_bin="$ndk_path/toolchains/llvm/prebuilt/linux-x86_64/bin"
    if [ ! -d "$toolchain_bin" ]; then
        toolchain_bin="$ndk_path/toolchains/llvm/prebuilt/linux-x86_64/bin"
    fi
    export CC="$toolchain_bin/$clang"
    export CXX="$toolchain_bin/${clang%-clang}++"

    # Prepare ldflags
    read -r version commit build_time go_version <<< "$git_info"
    local ldflags="-X main.version=$version -X main.gitCommit=$commit -X main.buildTime=$build_time -X main.goVersion=$go_version -s -w"

    local build_cmd="go build -tags production -ldflags \"$ldflags\" -o $dir/$PROJECT_NAME ./nemesisbot/"

    print_info "Executing build..."

    eval "$build_cmd"
    local exit_code=$?

    # Clean up environment variables
    unset CGO_ENABLED
    unset GOOS
    unset GOARCH
    unset CC
    unset CXX

    if [ $exit_code -eq 0 ] && [ -f "$dir/$PROJECT_NAME" ]; then
        local size=$(wc -c < "$dir/$PROJECT_NAME" 2>/dev/null)
        local size_mb=$(echo "scale=2; $size / 1048576" | bc)

        echo ""
        echo -e "\033[0;32m========================================"
        echo "  Android Build Successful!"
        echo -e "\033[0;32m========================================"
        echo ""
        echo "Build Information:"
        echo -e "  Version:     \033[0;33m${version}\033[0m"
        echo -e "  Git Commit:  \033[0;33m${commit}\033[0m"
        echo -e "  Build Time:  \033[0;33m${build_time}\033[0m"
        echo -e "  Go Version:  \033[0;33m${go_version}\033[0m"
        echo ""
        echo "Build Parameters:"
        echo -e "  Platform:    \033[0;33mandroid/$arch\033[0m"
        echo -e "  NDK Path:    \033[0;33m${ndk_path}\033[0m"
        echo -e "  Min API:     \033[0;33mandroid${ANDROID_MIN_API}\033[0m"
        echo -e "  Build Tags:  \033[0;33mproduction\033[0m"
        echo -e "  Output:      \033[0;33m${dir}/${PROJECT_NAME}\033[0m"
        echo ""
        echo "Output File:"
        echo -e "  Path:        \033[0;33m${dir}/${PROJECT_NAME}\033[0m"
        echo -e "  Size:        \033[0;33m${size_mb} MB (${size} bytes)\033[0m"
        echo -e "  Created:     \033[0;33m$(stat -c %y "$dir/$PROJECT_NAME" 2>/dev/null)\033[0m"
        echo ""
        echo -e "\033[0;32m========================================"
    else
        print_error "Build failed with exit code $exit_code"
        exit 1
    fi
}

build_android_all() {
    print_section "Building All Android Platforms"

    build_android "$1" "arm64"
    build_android "$1" "arm"
    build_android "$1" "386"
    build_android "$1" "amd64"

    print_success "All Android platforms build completed"
}

build_all_platforms() {
    print_section "Building All Platforms"

    build_windows "$1"
    build_linux "$1"
    build_darwin "$1"

    print_success "All platforms build completed!"
    print_info "Check $BUILD_DIR/ directory"
}

build_all_with_android() {
    print_section "Building All Platforms (including Android)"

    build_windows "$1"
    build_linux "$1"
    build_darwin "$1"
    build_android_all "$1"

    print_success "All platforms build completed (including Android)!"
    print_info "Check $BUILD_DIR/ directory"
}

invoke_clean() {
    print_section "Cleaning Build Files"

    if [ -d "$BUILD_DIR" ]; then
        rm -rf "$BUILD_DIR"
        print_success "Deleted $BUILD_DIR/ directory"
    else
        print_info "No build files to clean"
    fi

    if [ -f "$PROJECT_NAME" ] || [ -f "$PROJECT_NAME.exe" ]; then
        rm -f "$PROJECT_NAME" "$PROJECT_NAME.exe" 2>/dev/null || true
    fi

    print_success "Clean completed"
}

invoke_clean_all() {
    print_section "Cleaning All Generated Files"

    if [ -d "$BUILD_DIR" ]; then
        rm -rf "$BUILD_DIR"
    fi

    if [ -f "$PROJECT_NAME" ] || [ -f "$PROJECT_NAME.exe" ]; then
        rm -f "$PROJECT_NAME" "$PROJECT_NAME.exe" 2>/dev/null || true
    fi

    find . -name "*.test" -delete 2>/dev/null || true
    find . -name "*.out" -delete 2>/dev/null || true

    print_success "All generated files cleaned"
}

invoke_test() {
    local short="$1"
    local race="$2"
    local module="$3"

    if [ -n "$module" ]; then
        print_section "Testing Module: $module"
        go test -v "$module"
    elif [ "$short" = "true" ]; then
        print_section "Running Short Tests"
        go test -short -v ./...
    elif [ "$race" = "true" ]; then
        print_section "Running Race Detection Tests"
        go test -race -v ./...
    else
        print_section "Running Tests"
        go test -v ./... 2>&1 | tee test_output.log
    fi

    if [ ${PIPESTATUS[0]} -eq 0 ]; then
        print_success "Tests completed"
    else
        print_error "Tests failed"
        exit 1
    fi
}

invoke_release() {
    print_section "Creating Release Packages"

    if [ ! -d "$BUILD_DIR" ]; then
        print_error "Please build the project first"
        exit 1
    fi

    if [ ! -d "release" ]; then
        mkdir -p "release"
    fi

    local git_info=$(get_git_info)
    read -r version _ _ _ <<< "$git_info"

    # Windows
    if [ -f "$BUILD_DIR/windows-amd64/$PROJECT_NAME.exe" ]; then
        pushd "$BUILD_DIR/windows-amd64" > /dev/null
        zip - "../../../release/$PROJECT_NAME-$version-windows-amd64.zip" "$PROJECT_NAME.exe"
        popd > /dev/null
        print_success "Windows release package created"
    fi

    # Linux
    if [ -f "$BUILD_DIR/linux-amd64/$PROJECT_NAME" ]; then
        tar -czf "release/$PROJECT_NAME-$version-linux-amd64.tar.gz" -C "$BUILD_DIR/linux-amd64" "$PROJECT_NAME"
        print_success "Linux release package created"
    fi

    # macOS
    if [ -f "$BUILD_DIR/darwin-amd64/$PROJECT_NAME" ]; then
        tar -czf "release/$PROJECT_NAME-$version-darwin-amd64.tar.gz" -C "$BUILD_DIR/darwin-amd64" "$PROJECT_NAME"
        print_success "macOS release package created"
    fi

    print_success "Release packages created: release/"

    ls -lh release/ | awk 'NR>1 {printf "  %s (%s)\n", $9, $5}'
}

show_help() {
    cat << EOF
========================================
NemesisBot Build Script (Bash)
========================================

Usage:
  ./build.sh [options]

Options:
  --rebuild          Clean and rebuild
  --clean             Clean build files
  --clean-all         Clean all generated files
  --test              Run tests
  --test-short        Run short tests
  --test-race         Run race detection
  --verbose           Verbose output
  --with-desktop     Build desktop UI version
  --full-featured     Build full-featured version
  --windows           Build Windows version
  --linux             Build Linux version
  --darwin            Build macOS version
  --android           Build Android version (ARM64, requires NDK)
  --android-all       Build all Android versions
  --all-platforms     Build all platforms (excluding Android)
  --all-with-android  Build all platforms (including Android)
  --release           Create release packages
  --help              Show this help

Parameters:
  --output <name>    Custom output filename
  --build-dir <dir>   Custom build directory
  --module <path>     Test specific module
  --ndk-path <path>   Android NDK path
  --android-min-api <n>  Android minimum API version (default: 21)

Examples:
  ./build.sh                    # Build for current platform
  ./build.sh --rebuild          # Clean and rebuild
  ./build.sh --clean            # Clean build files
  ./build.sh --test             # Run tests
  ./build.sh --with-desktop    # Build desktop UI version
  ./build.sh --all-platforms    # Build all platforms
  ./build.sh --android         # Build Android version
  ./build.sh --android --ndk-path "/path/to/ndk" --android-min-api 21
  ./build.sh --test --module ./module/agent  # Test specific module

Android Build:
  Requires Android NDK and proper toolchain.
  Set ANDROID_NDK_HOME or NDK_PATH environment variable.
  Default architecture: ARM64
  Default minimum API: 21

EOF
}

show_info() {
    local git_info=$(get_git_info)
    read -r version commit build_time go_version <<< "$git_info"

    print_section "Project Information"
    echo "Project Name:   $PROJECT_NAME"
    echo "Current Version: $version"
    echo "Git Commit:     $commit"
    echo "Build Time:     $build_time"
    echo "Go Version:     $go_version"
    echo "Build Directory: $BUILD_DIR"
    echo "Current Platform: $(uname -s)/$(uname -m)"
}

# ================================================
# Main Logic
# ================================================

# Check if Go is installed
if ! command -v go &> /dev/null; then
    print_error "Go is not installed or not in PATH"
    echo "Please install Go: https://golang.org/dl/"
    exit 1
fi

# Parse command line arguments
REBUILD=false
CLEAN=false
CLEAN_ALL=false
TEST=false
TEST_SHORT=false
TEST_RACE=false
VERBOSE=false
WITH_DESKTOP=false
FULL_FEATURED=false
WINDOWS=false
LINUX=false
DARWIN=false
ANDROID=false
ANDROID_ALL=false
ALL_PLATFORMS=false
ALL_WITH_ANDROID=false
RELEASE=false
HELP=false
OUTPUT_NAME=""
BUILD_DIR_OVERRIDE=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --rebuild)
            REBUILD=true
            shift
            ;;
        --clean)
            CLEAN=true
            shift
            ;;
        --clean-all)
            CLEAN_ALL=true
            shift
            ;;
        --test)
            TEST=true
            shift
            ;;
        --test-short)
            TEST=true
            TEST_SHORT=true
            shift
            ;;
        --test-race)
            TEST=true
            TEST_RACE=true
            shift
            ;;
        --verbose)
            VERBOSE=true
            shift
            ;;
        --with-desktop)
            WITH_DESKTOP=true
            shift
            ;;
        --full-featured)
            FULL_FEATURED=true
            shift
            ;;
        --windows)
            WINDOWS=true
            shift
            ;;
        --linux)
            LINUX=true
            shift
            ;;
        --darwin)
            DARWIN=true
            shift
            ;;
        --android)
            ANDROID=true
            shift
            ;;
        --android-all)
            ANDROID_ALL=true
            shift
            ;;
        --all-platforms)
            ALL_PLATFORMS=true
            shift
            ;;
        --all-with-android)
            ALL_WITH_ANDROID=true
            shift
            ;;
        --release)
            RELEASE=true
            shift
            ;;
        --help|-h)
            HELP=true
            shift
            ;;
        --output)
            OUTPUT_NAME="$2"
            shift 2
            ;;
        --build-dir)
            BUILD_DIR_OVERRIDE="$2"
            shift 2
            ;;
        --module)
            TEST_MODULE="$2"
            shift 2
            ;;
        --ndk-path)
            NDK_PATH="$2"
            shift 2
            ;;
        --android-min-api)
            ANDROID_MIN_API="$2"
            shift 2
            ;;
        *)
            print_error "Unknown option: $1"
            echo "Use --help to see available options"
            exit 1
            ;;
    esac
done

# Override build directory if specified
if [ -n "$BUILD_DIR_OVERRIDE" ]; then
    BUILD_DIR="$BUILD_DIR_OVERRIDE"
fi

# Show help if requested
if [ "$HELP" = true ]; then
    show_help
    exit 0
fi

# Process options
# IMPORTANT: Wails requires 'production' build tag
# Default to production mode for all builds
build_tags="production"

# Only add desktop on native builds (not cross-compilation from Linux/macOS)
if [ "$WITH_DESKTOP" = true ]; then
    if [ "$(uname)" = "Linux" ]; then
        build_tags="$build_tags,desktop"
    elif [ "$(uname)" = "Darwin" ]; then
        build_tags="$build_tags,desktop"
    else
        print_warning "Desktop UI only supported on Linux/macOS for native compilation"
    fi
fi

# Execute corresponding actions
if [ "$CLEAN" = true ]; then
    invoke_clean
elif [ "$CLEAN_ALL" = true ]; then
    invoke_clean_all
elif [ "$REBUILD" = true ]; then
    invoke_clean
    build_project "$build_tags" "$OUTPUT_NAME"
elif [ "$TEST" = true ]; then
    invoke_test "$TEST_SHORT" "$TEST_RACE" "$TEST_MODULE"
elif [ "$WINDOWS" = true ]; then
    build_windows "$build_tags"
elif [ "$LINUX" = true ]; then
    build_linux "$build_tags"
elif [ "$DARWIN" = true ]; then
    build_darwin "$build_tags"
elif [ "$ANDROID" = true ]; then
    build_android "$build_tags"
elif [ "$ANDROID_ALL" = true ]; then
    build_android_all "$build_tags"
elif [ "$ALL_PLATFORMS" = true ]; then
    build_all_platforms "$build_tags"
elif [ "$ALL_WITH_ANDROID" = true ]; then
    build_all_with_android "$build_tags"
elif [ "$RELEASE" = true ]; then
    invoke_release
elif [ -n "$OUTPUT_NAME" ] || [ -n "$TEST_MODULE" ]; then
    # Custom output or test specified
    if [ -n "$TEST" ]; then
        invoke_test "$TEST_SHORT" "$TEST_RACE" "$TEST_MODULE"
    else
        build_project "$build_tags" "$OUTPUT_NAME"
    fi
else
    # Default: build for current platform
    build_project "$build_tags" "$OUTPUT_NAME"
fi
