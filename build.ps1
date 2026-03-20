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
    $version = & git describe --tags --abbrev=0 2>$null
    if (-not $version) { $version = "0.0.0.1" }

    $commit = & git rev-parse --short HEAD 2>$null
    if (-not $commit) { $commit = "unknown" }

    $buildTime = Get-Date -Format "yyyy/MM/dd HH:mm:ss"

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

    $ldflags = "-X main.version=$($GitInfo.Version) " +
               "-X main.gitCommit=$($GitInfo.Commit) " +
               "-X main.buildTime='$($GitInfo.BuildTime)' " +
               "-X main.goVersion=$($GitInfo.GoVersion) " +
               "-s -w"

    $tagsStr = if ($Tags.Count -gt 0) { "-tags " + ($Tags -join ",") } else { "" }

    $buildCmd = "go build $tagsStr -ldflags `"$ldflags`" -o $Output ./nemesisbot/"

    Print-Info "Executing: $buildCmd"

    Invoke-Expression $buildCmd

    if ($LASTEXITCODE -eq 0) {
        if (Test-Path $Output) {
            $size = (Get-Item $Output).Length / 1MB
            $sizeStr = [math]::Round($size, 2)
            Write-Host "[OK] Build successful: $Output ($sizeStr MB)" -ForegroundColor Green
        }
    } else {
        Write-Host "[ERROR] Build failed" -ForegroundColor Red
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
    $gitInfo = Get-GitInfo

    $dir = "$BuildDir\linux-amd64"
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }

    Invoke-GoBuild -Output "$dir\$ProjectName" -GoOS "linux" -GoArch "amd64" -Tags $Tags -GitInfo $gitInfo
    Print-Success "Linux amd64 build completed"
}

function Build-Darwin {
    param([string[]]$Tags = @())

    Print-Section "Building macOS (amd64)"
    $gitInfo = Get-GitInfo

    $dir = "$BuildDir\darwin-amd64"
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }

    Invoke-GoBuild -Output "$dir\$ProjectName" -GoOS "darwin" -GoArch "amd64" -Tags $Tags -GitInfo $gitInfo
    Print-Success "macOS amd64 build completed"
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

    $ldflags = "-X main.version=$($gitInfo.Version) " +
               "-X main.gitCommit=$($gitInfo.Commit) " +
               "-X main.buildTime='$($gitInfo.BuildTime)' " +
               "-X main.goVersion=$($gitInfo.GoVersion) " +
               "-s -w"

    $tagsStr = if ($Tags.Count -gt 0) { "-tags " + ($Tags -join ",") } else { "" }

    $buildCmd = "go build $tagsStr -ldflags `"$ldflags`" -o $dir\$ProjectName ./nemesisbot/"

    Print-Info "Executing build..."
    Invoke-Expression $buildCmd

    # Clean up environment variables
    Remove-Item Env:CGO_ENABLED -ErrorAction SilentlyContinue
    Remove-Item Env:GOOS -ErrorAction SilentlyContinue
    Remove-Item Env:GOARCH -ErrorAction SilentlyContinue
    Remove-Item Env:CC -ErrorAction SilentlyContinue
    Remove-Item Env:CXX -ErrorAction SilentlyContinue

    if ($LASTEXITCODE -eq 0) {
        if (Test-Path "$dir\$ProjectName") {
            $size = (Get-Item "$dir\$ProjectName").Length / 1MB
            $sizeStr = [math]::Round($size, 2)
            Write-Host "[OK] Android $Arch build completed ($sizeStr MB)" -ForegroundColor Green
        }
    } else {
        Write-Host "[ERROR] Build failed" -ForegroundColor Red
        exit 1
    }
}

function Build-AndroidAll {
    Print-Section "Building All Android Platforms"

    Build-Android -Arch "arm64"
    Build-Android -Arch "arm"
    Build-Android -Arch "386"
    Build-Android -Arch "amd64"

    Print-Success "All Android platforms build completed"
}

function Build-AllPlatforms {
    Print-Section "Building All Platforms"

    Build-Windows
    Build-Linux
    Build-Darwin

    Print-Success "All platforms build completed!"
    Print-Info "Check $BuildDir\ directory"
}

function Build-AllWithAndroid {
    Print-Section "Building All Platforms (including Android)"

    Build-Windows
    Build-Linux
    Build-Darwin
    Build-AndroidAll

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

# Detect if no parameters specified (show help)
$anyAction = $Rebuild -or $Clean -or $CleanAll -or $Test -or $TestShort -or $TestRace -or
              $Windows -or $Linux -or $Darwin -or $Android -or $AndroidAll -or
              $AllPlatforms -or $AllWithAndroid -or $Release -or
              ($Module -ne "") -or ($OutputName -ne "")

# If no action specified, show welcome message and help
if (-not $anyAction -and -not $WithPowerShell -and -not $WithDesktop -and -not $FullFeatured) {
    Write-Host ""
    Write-Host "========================================" -ForegroundColor Cyan
    Write-Host "  Welcome to NemesisBot Build System!" -ForegroundColor Cyan
    Write-Host "========================================" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Quick Start:" -ForegroundColor Green
    Write-Host "  .\build.ps1              - Build for current platform"
    Write-Host "  .\build.ps1 -Help       - View all available commands"
    Write-Host ""
    Write-Host "Get Help:"
    Write-Host "  Run " -NoNewline
    Write-Host ".\build.ps1 -Help" -ForegroundColor Yellow
    Write-Host " to see complete usage instructions"
    Write-Host ""
    Write-Host "Common Commands:" -ForegroundColor Green
    Write-Host "  .\build.ps1 -Windows    - Build Windows"
    Write-Host "  .\build.ps1 -Linux      - Build Linux"
    Write-Host "  .\build.ps1 -Android    - Build Android"
    Write-Host "  .\build.ps1 -AllPlatforms - Build all platforms"
    Write-Host "  .\build.ps1 -Test       - Run tests"
    Write-Host "  .\build.ps1 -Clean      - Clean build files"
    Write-Host ""
    Write-Host "[?] Tip: Use " -NoNewline
    Write-Host ".\build.ps1 -Help" -ForegroundColor Yellow
    Write-Host " to see all commands"
    Write-Host ""
    Show-Help
    exit 0
}

if ($Help) {
    Show-Help
    exit 0
}

# Process options
$buildTags = @()

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
    Build-AndroidAll
} elseif ($AllPlatforms) {
    Build-AllPlatforms
} elseif ($AllWithAndroid) {
    Build-AllWithAndroid
} elseif ($Release) {
    Invoke-Release
} else {
    # Default: build for current platform
    Build-Project -Tags $buildTags -OutputName $OutputName
}
