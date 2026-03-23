# ================================================
# NemesisBot Build Script (PowerShell)
# ================================================
# Windows PowerShell alternative to Makefile
#
# Usage examples:
#   .\build.ps1              # Build for current platform
#   .\build.ps1 -Rebuild     # Clean and rebuild
#   .\build.ps1 -Clean       # Clean build files
#   .\build.ps1 -Test        # Run tests
#   .\build.ps1 -All         # Build all platforms
#   .\build.ps1 -Release     # Create release packages
# ================================================

param(
    [switch]$Rebuild,
    [switch]$Clean,
    [switch]$CleanAll,
    [switch]$Test,
    [switch]$TestShort,
    [switch]$TestRace,
    [switch]$Verbose,
    [switch]$WithPowerShell,
    [switch]$WithDesktop,
    [switch]$FullFeatured,
    [switch]$Windows,
    [switch]$Linux,
    [switch]$Darwin,
    [switch]$Android,
    [switch]$AndroidAll,
    [switch]$AllPlatforms,
    [switch]$AllWithAndroid,
    [switch]$Release,
    [switch]$Help,
    [string]$OutputName = "",
    [string]$BuildDir = "build",
    [string]$Module = "",
    [string]$NdkPath = "",
    [string]$AndroidMinApi = "21"
)

# ================================================
# Configuration
# ================================================
$ProjectName = "nemesisbot"
$ErrorActionPreference = "Stop"

# ================================================
# Color Functions
# ================================================
function Print-Info {
    param([string]$Message)
    Write-Host "[INFO] $Message" -ForegroundColor Cyan
}

function Print-Success {
    param([string]$Message)
    Write-Host "[OK] $Message" -ForegroundColor Green
}

function Print-Warning {
    param([string]$Message)
    Write-Host "[WARN] $Message" -ForegroundColor Yellow
}

function Print-Error {
    param([string]$Message)
    Write-Host "[ERROR] $Message" -ForegroundColor Red
}

function Print-Section {
    param([string]$Title)
    Write-Host ""
    Write-Host "========================================" -ForegroundColor Blue
    Write-Host "  $Title" -ForegroundColor Blue
    Write-Host "========================================" -ForegroundColor Blue
    Write-Host ""
}

# ================================================
# Helper Functions
# ================================================

function Get-GitInfo {
    $version = "0.0.0.1"
    $commit = "unknown"

    # Try to get version from git tags (suppress errors)
    try {
        $version = & git describe --tags --abbrev=0 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($version)) {
            $version = "0.0.0.1"
        } else {
            $version = $version.Trim()
        }
    } catch {
        $version = "0.0.0.1"
    }

    # Try to get commit hash (suppress errors)
    try {
        $commit = & git rev-parse --short HEAD 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($commit)) {
            $commit = "unknown"
        } else {
            $commit = $commit.Trim()
        }
    } catch {
        $commit = "unknown"
    }

    $buildTime = Get-Date -Format "yyyy/MM/dd"

    $goVersionOutput = & go version 2>$null
    if ($goVersionOutput -match 'go version go(\d+\.\d+\.\d+)') {
        $goVersion = $matches[1]
    } else {
        $goVersion = "unknown"
    }

    return @{
        Version = $version
        Commit = $commit
        BuildTime = $buildTime
        GoVersion = $goVersion
    }
}

function Invoke-GoBuild {
    param(
        [string]$Output,
        [string]$GoOS = "",
        [string]$GoArch = "",
        [string[]]$Tags = @(),
        [hashtable]$GitInfo
    )

    $env:GOOS = $GoOS
    $env:GOARCH = $GoArch

    # Build ldflags as a single string (no spaces in buildTime)
    $ldflags = "-X main.version=$($GitInfo.Version) -X main.gitCommit=$($GitInfo.Commit) -X main.buildTime=$($GitInfo.BuildTime) -X main.goVersion=$($GitInfo.GoVersion) -s -w"

    $buildCmd = "go build"
    if ($Tags.Count -gt 0) {
        $buildCmd += " -tags " + ($Tags -join ",")
    }
    $buildCmd += " -ldflags `"$ldflags`" -o $Output ./nemesisbot/"

    Print-Info "Executing: $buildCmd"

    Invoke-Expression $buildCmd
    $exitCode = $LASTEXITCODE

    if ($exitCode -eq 0) {
        if (Test-Path $Output) {
            $fileInfo = Get-Item $Output
            $sizeMB = [math]::Round($fileInfo.Length / 1MB, 2)
            $sizeBytes = $fileInfo.Length

            Write-Host ""
            Write-Host "========================================" -ForegroundColor Green
            Write-Host "  Build Successful!" -ForegroundColor Green
            Write-Host "========================================" -ForegroundColor Green
            Write-Host ""
            Write-Host "Build Information:" -ForegroundColor Cyan
            Write-Host "  Version:     " -NoNewline; Write-Host "$($GitInfo.Version)" -ForegroundColor Yellow
            Write-Host "  Git Commit:  " -NoNewline; Write-Host "$($GitInfo.Commit)" -ForegroundColor Yellow
            Write-Host "  Build Time:  " -NoNewline; Write-Host "$($GitInfo.BuildTime)" -ForegroundColor Yellow
            Write-Host "  Go Version:  " -NoNewline; Write-Host "$($GitInfo.GoVersion)" -ForegroundColor Yellow
            Write-Host ""
            Write-Host "Build Parameters:" -ForegroundColor Cyan
            Write-Host "  Platform:    " -NoNewline; Write-Host "$GoOS/$GoArch" -ForegroundColor Yellow
            Write-Host "  Build Tags:  " -NoNewline; if ($Tags.Count -gt 0) { Write-Host "$($Tags -join ', ')" -ForegroundColor Yellow } else { Write-Host "None" -ForegroundColor Gray }
            Write-Host "  Output:      " -NoNewline; Write-Host "$Output" -ForegroundColor Yellow
            Write-Host ""
            Write-Host "Output File:" -ForegroundColor Cyan
            Write-Host "  Path:        " -NoNewline; Write-Host "$Output" -ForegroundColor Yellow
            Write-Host "  Size:        " -NoNewline; Write-Host "$sizeMB MB ($sizeBytes bytes)" -ForegroundColor Yellow
            Write-Host "  Created:     " -NoNewline; Write-Host "$($fileInfo.CreationTime)" -ForegroundColor Yellow
            Write-Host ""
            Write-Host "========================================" -ForegroundColor Green
        }
    } else {
        Write-Host "[ERROR] Build failed with exit code $exitCode" -ForegroundColor Red
        exit 1
    }

    # Clean up environment variables
    Remove-Item Env:GOOS -ErrorAction SilentlyContinue
    Remove-Item Env:GOARCH -ErrorAction SilentlyContinue
}

# ================================================
# Main Functions
# ================================================

function Build-Project {
    param(
        [string[]]$Tags = @(),
        [string]$OutputName = ""
    )

    Print-Section "Building $ProjectName"

    $gitInfo = Get-GitInfo

    Print-Info "Project Information:"
    Write-Host "  Name:       $ProjectName"
    Write-Host "  Version:    $($gitInfo.Version)"
    Write-Host "  Commit:     $($gitInfo.Commit)"
    Write-Host "  Build Time: $($gitInfo.BuildTime)"
    Write-Host "  Go Version: $($gitInfo.GoVersion)"
    Write-Host "  Platform:   $($env:GOOS)/$($env:GOARCH)"
    Write-Host ""

    if (-not (Test-Path $BuildDir)) {
        New-Item -ItemType Directory -Path $BuildDir | Out-Null
    }

    $output = if ($OutputName) { $OutputName } else { "$BuildDir\$ProjectName.exe" }

    Print-Info "Starting build..."

    Invoke-GoBuild -Output $output -Tags $Tags -GitInfo $gitInfo
}

function Build-Windows {
    param([string[]]$Tags = @())

    Print-Section "Building Windows (amd64)"
    $gitInfo = Get-GitInfo

    $dir = "$BuildDir\windows-amd64"
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }

    Invoke-GoBuild -Output "$dir\$ProjectName.exe" -GoOS "windows" -GoArch "amd64" -Tags $Tags -GitInfo $gitInfo
    Print-Success "Windows amd64 build completed"
}

function Build-Linux {
    param([string[]]$Tags = @())

    Print-Section "Building Linux (amd64)"

    # Wails Desktop UI cannot be cross-compiled (requires CGO + GTK)
    # Add cross_compile tag to use stub implementation
    $actualPlatform = $env:GOOS
    $actualCgo = $env:CGO_ENABLED

    $env:GOOS = "linux"
    $env:CGO_ENABLED = "0"  # Disable CGO for cross-compilation

    # Add cross_compile tag to use stub implementation instead of Wails
    $crossCompileTags = @("cross_compile") + $Tags

    Write-Host "[WARN] Desktop UI excluded from Linux build (cross-compilation)" -ForegroundColor Yellow
    Write-Host "[INFO] Desktop UI requires native compilation with CGO + GTK libraries" -ForegroundColor Cyan

    $gitInfo = Get-GitInfo

    $dir = "$BuildDir\linux-amd64"
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }

    Invoke-GoBuild -Output "$dir\$ProjectName" -GoOS "linux" -GoArch "amd64" -Tags $crossCompileTags -GitInfo $gitInfo
    Print-Success "Linux amd64 build completed"

    # Restore environment
    $env:GOOS = $actualPlatform
    if ($actualCgo) {
        $env:CGO_ENABLED = $actualCgo
    } else {
        Remove-Item Env:CGO_ENABLED -ErrorAction SilentlyContinue
    }
}

function Build-Darwin {
    param([string[]]$Tags = @())

    Print-Section "Building macOS (amd64)"

    # Wails Desktop UI cannot be cross-compiled (requires platform-specific frameworks)
    # Add cross_compile tag to use stub implementation
    $actualPlatform = $env:GOOS
    $actualCgo = $env:CGO_ENABLED

    $env:GOOS = "darwin"
    $env:CGO_ENABLED = "0"  # Disable CGO for cross-compilation

    # Add cross_compile tag to use stub implementation instead of Wails
    $crossCompileTags = @("cross_compile") + $Tags

    Write-Host "[WARN] Desktop UI excluded from macOS build (cross-compilation)" -ForegroundColor Yellow
    Write-Host "[INFO] Desktop UI requires native compilation on macOS" -ForegroundColor Cyan

    $gitInfo = Get-GitInfo

    $dir = "$BuildDir\darwin-amd64"
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }

    Invoke-GoBuild -Output "$dir\$ProjectName" -GoOS "darwin" -GoArch "amd64" -Tags $crossCompileTags -GitInfo $gitInfo
    Print-Success "macOS amd64 build completed"

    # Restore environment
    $env:GOOS = $actualPlatform
    if ($actualCgo) {
        $env:CGO_ENABLED = $actualCgo
    } else {
        Remove-Item Env:CGO_ENABLED -ErrorAction SilentlyContinue
    }
}

function Build-Android {
    param(
        [string[]]$Tags = @(),
        [string]$Arch = "arm64"
    )

    # Check NDK path
    if ([string]::IsNullOrEmpty($NdkPath)) {
        $NdkPath = $env:ANDROID_NDK_HOME
        if ([string]::IsNullOrEmpty($NdkPath)) {
            # Try common default paths
            $defaultPaths = @(
                "$env:LOCALAPPDATA\Android\Sdk\ndk",
                "C:\Android\Sdk\ndk"
            )

            foreach ($path in $defaultPaths) {
                if (Test-Path $path) {
                    $ndkDirs = Get-ChildItem $path -Directory | Sort-Object LastWriteTime -Descending
                    if ($ndkDirs.Count -gt 0) {
                        $NdkPath = $ndkDirs[0].FullName
                        break
                    }
                }
            }

            if ([string]::IsNullOrEmpty($NdkPath)) {
                Print-Error "Android NDK not found. Please set ANDROID_NDK_HOME environment variable or use -NdkPath parameter"
                exit 1
            }
        }
    }

    if (-not (Test-Path $NdkPath)) {
        Print-Error "NDK path does not exist: $NdkPath"
        exit 1
    }

    # Determine architecture and compiler
    $archTable = @{
        "arm64" = @{"clang" = "aarch64-linux-android$AndroidMinApi-clang"; "goarch" = "arm64"}
        "arm"   = @{"clang" = "armv7a-linux-androideabi$AndroidMinApi-clang"; "goarch" = "arm"}
        "386"   = @{"clang" = "i686-linux-android$AndroidMinApi-clang"; "goarch" = "386"}
        "amd64" = @{"clang" = "x86_64-linux-android$AndroidMinApi-clang"; "goarch" = "amd64"}
    }

    if (-not $archTable.ContainsKey($Arch)) {
        Print-Error "Unsupported Android architecture: $Arch"
        exit 1
    }

    $archInfo = $archTable[$Arch]
    $clang = $archInfo["clang"]
    $goarch = $archInfo["goarch"]

    Print-Section "Building Android ($Arch)"
    $gitInfo = Get-GitInfo

    Print-Info "NDK Path: $NdkPath"
    Print-Info "Min API Version: android$AndroidMinApi"
    Print-Info "Architecture: $Arch"

    $dir = "$BuildDir\android-$Arch"
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }

    # Set up environment variables
    $env:CGO_ENABLED = "1"
    $env:GOOS = "android"
    $env:GOARCH = $goarch

    # Set up compiler paths
    $toolchainBin = "$NdkPath\toolchains\llvm\prebuilt\windows-x86_64\bin"
    $env:CC = "$toolchainBin\$clang"
    $env:CC = "$toolchainBin\$clang.cmd"
    $env:CXX = "$toolchainBin\$clang++"
    $env:CXX = "$toolchainBin\$clang++.cmd"

    # Add cross_compile tag (Android doesn't support Wails Desktop UI)
    $crossCompileTags = @("cross_compile") + $Tags

    # Prepare ldflags (no spaces in buildTime)
    $ldflags = "-X main.version=$($gitInfo.Version) -X main.gitCommit=$($gitInfo.Commit) -X main.buildTime=$($gitInfo.BuildTime) -X main.goVersion=$($gitInfo.GoVersion) -s -w"

    $buildCmd = "go build"
    if ($crossCompileTags.Count -gt 0) {
        $buildCmd += " -tags " + ($crossCompileTags -join ",")
    }
    $buildCmd += " -ldflags `"$ldflags`" -o $dir\$ProjectName ./nemesisbot/"

    Print-Info "Executing build..."

    Invoke-Expression $buildCmd
    $exitCode = $LASTEXITCODE

    # Clean up environment variables
    Remove-Item Env:CGO_ENABLED -ErrorAction SilentlyContinue
    Remove-Item Env:GOOS -ErrorAction SilentlyContinue
    Remove-Item Env:GOARCH -ErrorAction SilentlyContinue
    Remove-Item Env:CC -ErrorAction SilentlyContinue
    Remove-Item Env:CXX -ErrorAction SilentlyContinue

    if ($exitCode -eq 0) {
        if (Test-Path "$dir\$ProjectName") {
            $fileInfo = Get-Item "$dir\$ProjectName"
            $sizeMB = [math]::Round($fileInfo.Length / 1MB, 2)
            $sizeBytes = $fileInfo.Length

            Write-Host ""
            Write-Host "========================================" -ForegroundColor Green
            Write-Host "  Android Build Successful!" -ForegroundColor Green
            Write-Host "========================================" -ForegroundColor Green
            Write-Host ""
            Write-Host "Build Information:" -ForegroundColor Cyan
            Write-Host "  Version:     " -NoNewline; Write-Host "$($gitInfo.Version)" -ForegroundColor Yellow
            Write-Host "  Git Commit:  " -NoNewline; Write-Host "$($gitInfo.Commit)" -ForegroundColor Yellow
            Write-Host "  Build Time:  " -NoNewline; Write-Host "$($gitInfo.BuildTime)" -ForegroundColor Yellow
            Write-Host "  Go Version:  " -NoNewline; Write-Host "$($gitInfo.GoVersion)" -ForegroundColor Yellow
            Write-Host ""
            Write-Host "Build Parameters:" -ForegroundColor Cyan
            Write-Host "  Platform:    " -NoNewline; Write-Host "android/$Arch" -ForegroundColor Yellow
            Write-Host "  NDK Path:    " -NoNewline; Write-Host "$NdkPath" -ForegroundColor Yellow
            Write-Host "  Min API:     " -NoNewline; Write-Host "android$AndroidMinApi" -ForegroundColor Yellow
            Write-Host "  Build Tags:  " -NoNewline; if ($crossCompileTags.Count -gt 0) { Write-Host "$($crossCompileTags -join ', ')" -ForegroundColor Yellow } else { Write-Host "None" -ForegroundColor Gray }
            Write-Host "  Output:      " -NoNewline; Write-Host "$dir\$ProjectName" -ForegroundColor Yellow
            Write-Host ""
            Write-Host "Output File:" -ForegroundColor Cyan
            Write-Host "  Path:        " -NoNewline; Write-Host "$dir\$ProjectName" -ForegroundColor Yellow
            Write-Host "  Size:        " -NoNewline; Write-Host "$sizeMB MB ($sizeBytes bytes)" -ForegroundColor Yellow
            Write-Host "  Created:     " -NoNewline; Write-Host "$($fileInfo.CreationTime)" -ForegroundColor Yellow
            Write-Host ""
            Write-Host "========================================" -ForegroundColor Green
        }
    } else {
        Write-Host "[ERROR] Build failed with exit code $exitCode" -ForegroundColor Red
        exit 1
    }
}

function Build-AndroidAll {
    param(
        [string[]]$Tags = @()
    )

    Print-Section "Building All Android Platforms"

    Build-Android -Tags $Tags -Arch "arm64"
    Build-Android -Tags $Tags -Arch "arm"
    Build-Android -Tags $Tags -Arch "386"
    Build-Android -Tags $Tags -Arch "amd64"

    Print-Success "All Android platforms build completed"
}

function Build-AllPlatforms {
    param(
        [string[]]$Tags = @()
    )

    Print-Section "Building All Platforms"

    Build-Windows -Tags $Tags
    Build-Linux -Tags $Tags
    Build-Darwin -Tags $Tags

    Print-Success "All platforms build completed!"
    Print-Info "Check $BuildDir\ directory"
}

function Build-AllWithAndroid {
    param(
        [string[]]$Tags = @()
    )

    Print-Section "Building All Platforms (including Android)"

    Build-Windows -Tags $Tags
    Build-Linux -Tags $Tags
    Build-Darwin -Tags $Tags
    Build-AndroidAll -Tags $Tags

    Print-Success "All platforms build completed (including Android)!"
    Print-Info "Check $BuildDir\ directory"
}

function Invoke-Clean {
    Print-Section "Cleaning Build Files"

    if (Test-Path $BuildDir) {
        Remove-Item -Recurse -Force $BuildDir
        Print-Success "Deleted $BuildDir\ directory"
    } else {
        Print-Info "No build files to clean"
    }

    if (Test-Path "$ProjectName.exe") {
        Remove-Item -Force "$ProjectName.exe"
    }

    Print-Success "Clean completed"
}

function Invoke-CleanAll {
    Print-Section "Cleaning All Generated Files"

    if (Test-Path $BuildDir) {
        Remove-Item -Recurse -Force $BuildDir
    }

    if (Test-Path "$ProjectName.exe") {
        Remove-Item -Force "$ProjectName.exe"
    }

    Get-ChildItem -Recurse -Filter "*.test" | Remove-Item -Force -ErrorAction SilentlyContinue
    Get-ChildItem -Recurse -Filter "*.out" | Remove-Item -Force -ErrorAction SilentlyContinue

    Print-Success "All generated files cleaned"
}

function Invoke-Test {
    param(
        [switch]$Short,
        [switch]$Race,
        [string]$Module = ""
    )

    if ($Module) {
        Print-Section "Testing Module: $Module"
        go test -v $Module
    } elseif ($Short) {
        Print-Section "Running Short Tests"
        go test -short -v ./...
    } elseif ($Race) {
        Print-Section "Running Race Detection Tests"
        go test -race -v ./...
    } else {
        Print-Section "Running Tests"
        go test -v ./... 2>&1 | Tee-Object -FilePath test_output.log
    }

    if ($LASTEXITCODE -eq 0) {
        Print-Success "Tests completed"
    } else {
        Print-Error "Tests failed"
        exit 1
    }
}

function Invoke-Release {
    Print-Section "Creating Release Packages"

    if (-not (Test-Path $BuildDir)) {
        Print-Error "Please build the project first"
        exit 1
    }

    if (-not (Test-Path "release")) {
        New-Item -ItemType Directory -Path "release" | Out-Null
    }

    $gitInfo = Get-GitInfo
    $version = $gitInfo.Version

    # Windows
    if (Test-Path "$BuildDir\windows-amd64\$ProjectName.exe") {
        Compress-Archive -Path "$BuildDir\windows-amd64\$ProjectName.exe" `
                        -DestinationPath "release\$ProjectName-$version-windows-amd64.zip"
        Print-Success "Windows release package created"
    }

    # Linux
    if (Test-Path "$BuildDir\linux-amd64\$ProjectName") {
        & tar -czf "release\$ProjectName-$version-linux-amd64.tar.gz" `
              -C "$BuildDir\linux-amd64" $ProjectName
        Print-Success "Linux release package created"
    }

    # macOS
    if (Test-Path "$BuildDir\darwin-amd64\$ProjectName") {
        & tar -czf "release\$ProjectName-$version-darwin-amd64.tar.gz" `
              -C "$BuildDir\darwin-amd64" $ProjectName
        Print-Success "macOS release package created"
    }

    Print-Success "Release packages created: release\"

    Get-ChildItem release\ | ForEach-Object {
        $size = [math]::Round($_.Length / 1KB, 2)
        Write-Host "  $($_.Name) ($size KB)" -ForegroundColor Cyan
    }
}

function Show-Help {
    Write-Host @"
========================================
NemesisBot Build Script (PowerShell)
========================================

Usage:
  .\build.ps1 [options]

Options:
  -Rebuild           Clean and rebuild
  -Clean             Clean build files
  -CleanAll          Clean all generated files
  -Test              Run tests
  -TestShort         Run short tests
  -TestRace          Run race detection
  -Verbose           Verbose output
  -WithPowerShell    Build PowerShell-enabled version
  -WithDesktop       Build desktop UI version
  -FullFeatured      Build full-featured version
  -Windows           Build Windows version
  -Linux             Build Linux version
  -Darwin            Build macOS version
  -Android           Build Android version (ARM64, requires NDK)
  -AndroidAll        Build all Android versions
  -AllPlatforms      Build all platforms (excluding Android)
  -AllWithAndroid    Build all platforms (including Android)
  -Release           Create release packages
  -Help              Show this help

Parameters:
  -OutputName <name>  Custom output filename
  -BuildDir <dir>     Custom build directory
  -Module <path>      Test specific module
  -NdkPath <path>     Android NDK path
  -AndroidMinApi <n>  Android minimum API version (default: 21)

Examples:
  .\build.ps1                    # Build for current platform
  .\build.ps1 -Rebuild            # Clean and rebuild
  .\build.ps1 -Clean              # Clean build files
  .\build.ps1 -Test               # Run tests
  .\build.ps1 -WithPowerShell     # Build PowerShell-enabled version
  .\build.ps1 -AllPlatforms       # Build all platforms
  .\build.ps1 -Android            # Build Android version
  .\build.ps1 -Android -NdkPath "C:\Android\Sdk\ndk\26.1.10909125"
  .\build.ps1 -Test -Module ./module/agent  # Test specific module

Android Build:
  Requires Android NDK and proper toolchain.
  Set ANDROID_NDK_HOME environment variable or use -NdkPath parameter.
  Default architecture: ARM64
  Default minimum API: 21

"@
}

function Show-Info {
    $gitInfo = Get-GitInfo

    Print-Section "Project Information"
    Write-Host "Project Name:   $ProjectName"
    Write-Host "Current Version: $($gitInfo.Version)"
    Write-Host "Git Commit:     $($gitInfo.Commit)"
    Write-Host "Build Time:     $($gitInfo.BuildTime)"
    Write-Host "Go Version:     $($gitInfo.GoVersion)"
    Write-Host "Build Directory:$BuildDir"
    Write-Host "Current Platform:$($env:GOOS)/$($env:GOARCH)"
}

# ================================================
# Main Logic
# ================================================

# Check if Go is installed
if (-not (Get-Command go -ErrorAction SilentlyContinue)) {
    Print-Error "Go is not installed or not in PATH"
    Write-Host "Please install Go: https://golang.org/dl/"
    exit 1
}

# Detect if user wants help (explicitly)
# Note: Default behavior is to build the project
$wantsHelp = $Help

# Only show welcome message if explicitly requested
if ($wantsHelp) {
    Show-Help
    exit 0
}

# Process options
# IMPORTANT: Wails requires 'production' build tag
# Default to production mode for all builds
$buildTags = @("production")

if ($WithPowerShell) { $buildTags += "powershell" }
if ($WithDesktop) { $buildTags += "desktop" }
if ($FullFeatured) { $buildTags += "powershell", "desktop" }

# Execute corresponding actions
if ($Clean) {
    Invoke-Clean
} elseif ($CleanAll) {
    Invoke-CleanAll
} elseif ($Rebuild) {
    Invoke-Clean
    Build-Project -Tags $buildTags -OutputName $OutputName
} elseif ($Test -or $TestShort -or $TestRace -or $Module) {
    Invoke-Test -Short:$TestShort -Race:$TestRace -Module $Module
} elseif ($Windows) {
    Build-Windows -Tags $buildTags
} elseif ($Linux) {
    Build-Linux -Tags $buildTags
} elseif ($Darwin) {
    Build-Darwin -Tags $buildTags
} elseif ($Android) {
    Build-Android -Tags $buildTags -Arch "arm64"
} elseif ($AndroidAll) {
    Build-AndroidAll -Tags $buildTags
} elseif ($AllPlatforms) {
    Build-AllPlatforms -Tags $buildTags
} elseif ($AllWithAndroid) {
    Build-AllWithAndroid -Tags $buildTags
} elseif ($Release) {
    Invoke-Release
} else {
    # Default: build for current platform
    Build-Project -Tags $buildTags -OutputName $OutputName
}
