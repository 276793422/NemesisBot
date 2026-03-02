---
name: build
description: Provides comprehensive information about the NemesisBot project build process, including build.bat usage, embedded files, and compilation steps. Use this skill whenever you need to compile the project or understand the build system.
---

# NemesisBot Build System Skill

This skill contains all knowledge about building the NemesisBot project from source code, including the build.bat script, embedded file system, and troubleshooting build issues.

## Overview

NemesisBot uses Go 1.25.7 and a custom build script (`build.bat`) that:
1. Copies necessary directories (config, default, workspace) to the nemesisbot package
2. Embeds these directories into the binary using `go:embed`
3. Compiles with version information injected via ldflags
4. Cleans up temporary directories after compilation

## Build Process

### Step 1: Build Script Execution

**Windows Batch Script:**
```bat
cd C:\AI\NemesisBot\NemesisBot
build.bat
```

**What build.bat does:**
1. **Gathers build information** (version, git commit, build time, Go version)
2. **Step 1.5: Copy default/** → `nemesisbot/default/`**
   - Contains default personality files (IDENTITY.md, SOUL.md, USER.md)
3. **Step 1.6: Copy config/** → `nemesisbot/config/`**
   - Contains default configuration files
   - **Critical:** These are embedded defaults (config.default.json, config.mcp.default.json, etc.)
4. **Step 2: Build** with dynamic ldflags
   ```
   go build -ldflags "-X main.version=%VERSION% -X main.gitCommit=%GIT_COMMIT% -X main.buildTime=%BUILD_TIME% -X main.goVersion=%GO_VERSION% -s -w" -o nemesisbot.exe .\nemesisbot\
   ```
5. **Step 3: Clean up** temporary directories
   - Removes `nemesisbot/workspace`, `nemesisbot/default`, `nemesisbot/config`

### Step 2: Embedded File System

**In `nemesisbot/main.go`:**
```go
//go:embed workspace
var embeddedFiles embed.FS

//go:embed default
var defaultFiles embed.FS

//go:embed config
var configFiles embed.FS
```

**Directory Structure:**
```
nemesisbot/
├── main.go              ← Embeds the three directories
├── command/            ← Command implementations
├── workspace/          ← Temporary, copied during build, cleaned after
├── default/             ← Temporary, copied during build, cleaned after
└── config/             ← Temporary, copied during build, cleaned after
```

**Why this approach:**
- `config/` and `default/` are in the project root for easy editing
- Build script copies them to `nemesisbot/` for Go embed
- Prevents hardcoded paths and allows developers to edit configs in root directory

### Step 3: Configuration Initialization

**Build-time flow:**
```
1. main() → SetEmbeddedFS(embeddedFiles, defaultFiles, configFiles)
2. command.initializeConfigDefaults(configFS)
3. Reads all *.default.json files from embedded configFS
4. Stores in config.embeddedDefaults (in-memory)
5. Available via config.LoadEmbeddedConfig()
```

**Runtime flow:**
```
1. LoadEmbeddedConfig() → reads from embeddedDefaults
2. Generates actual config files during onboard
3. Saves to workspace/config/ directory
```

## Build Commands

### Standard Build (Windows)
```batch
build.bat
```

Output: `nemesisbot.exe` in project root

### Manual Build (Cross-platform)

**Note:** The project uses embedded files, so manual builds require preparing directories first.

```bash
# Step 1: Prepare embedded directories (Windows/Linux/macOS)
cp -r config nemesisbot/
cp -r default nemesisbot/
cp -r workspace nemesisbot/

# Step 2: Build
go build -ldflags "-X main.version=dev -X main.gitCommit=$(git rev-parse --short HEAD) -X main.buildTime=$(date -u +%Y/%m/%d) -X main.goVersion=$(go version)" -o nemesisbot.exe ./nemesisbot

# Step 3: Clean up (optional, but recommended)
rm -rf nemesisbot/config
rm -rf nemesisbot/default
rm -rf nemesisbot/workspace
```

### Build with Version Tags

When you have a version tag:
```batch
git tag v1.0.0
build.bat
```

The build script will automatically detect the tag and use it.

## Embedded Configuration Files

### Location
- **Source:** `config/` directory in project root
- **Build-time:** Copied to `nemesisbot/config/` and embedded
- **Default config files:**
  - `config.default.json` - Main configuration template
  - `config.mcp.default.json` - MCP configuration template
  - `config.security.default.json` - Security configuration template
  - `config.cluster.default.json` - Cluster configuration template

### Loading Embedded Defaults
```go
// In config package
cfg, err := config.LoadEmbeddedConfig()
```

This reads from the embedded config files, not from disk.

## Common Build Issues

### Issue 1: "pattern config: no matching files found"

**Cause:** `nemesisbot/config/` directory doesn't exist during build

**Solution:**
1. Always run `build.bat` instead of `go build` directly
2. Or manually prepare directories first (see manual build above)

### Issue 2: Changes to config/ not reflected in binary

**Cause:** Go caches embedded files

**Solution:**
1. Touch any file in `nemesisbot/main.go` (e.g., add a comment)
2. Rebuild with `build.bat`
3. Or use `go build -a` to force rebuild

### Issue 3: "embedded default config not available"

**Cause:** `initializeConfigDefaults()` failed to read from embedded FS

**Solution:**
- Ensure all `*.default.json` files exist in `config/` directory
- Check build output for file copy errors
- Verify `configFiles` is passed to `SetEmbeddedFS()`

## Build Script Details

**File:** `build.bat`

**Key Features:**
- **Automatic version detection** from git tags
- **Dynamic ldflags** for version injection
- **Safe directory handling** with error checking
- **Automatic cleanup** of temporary directories

**Version Information Injection:**
```go
main.version      = "0.0.0.1"  // from git tag or default
main.gitCommit   = "abc1234"   // short git hash
main.buildTime   = "2025/03/03" // current date
main.goVersion    = "go1.25.7"   // Go version
```

## Configuration File Locations

### Source (Editable)
```
project-root/
├── config/
│   ├── config.default.json
│   ├── config.mcp.default.json
│   ├── config.security.default.json
│   └── config.cluster.default.json
└── default/
    ├── IDENTITY.md
    ├── SOUL.md
    └── USER.md
```

### Generated (Runtime)
```
~/.nemesisbot/
├── config.json                    ← Main config
└── workspace/
    └── config/
        ├── config.json           ← Generated from embedded
        ├── config.mcp.json
        ├── config.security.json
        └── config.cluster.json
```

## Development Workflow

### 1. Making Configuration Changes
```bash
# Edit files in config/ directory (root directory)
vim config/config.default.json
# Then rebuild
build.bat
```

### 2. Testing Changes
```bash
# Clean previous installation
rm -rf .nemesisbot

# Initialize with new config
./nemesisbot.exe onboard default --local

# Test the application
./nemesisbot.exe gateway
```

### 3. Development Build Cycle
```
1. Edit code
2. Edit config files (in config/)
3. Run build.bat
4. Test with ./nemesisbot.exe
5. Repeat
```

## Quick Reference

### Build Commands
```bash
# Standard build
build.bat

# Manual build (prepare directories first)
cp -r config nemesisbot/ && cp -r default nemesisbot/ && cp -r workspace nemesisbot/
go build -o nemesisbot.exe ./nemesisbot
```

### Clean Build
```bash
# Clean all build artifacts
del nemesisbot.exe
del .nemesisbot\workspace\* /s /q
rmdir .nemesisbot\workspace /s /q
```

## Troubleshooting Checklist

When build fails, check:

- [ ] Go 1.25.7 is in PATH (check: `go version`)
- [ ] Working directory is project root
- [ ] `config/`, `default/`, `workspace/` directories exist in project root
- [ ] No file locks on `nemesisbot.exe` (close running instances)
- [ ] Sufficient disk space for compilation
- [ ] All dependencies installed (`go mod download`)

## Build Script Exit Codes

- **0:** Success
- **1:** Failed to copy workspace/default/config directory
- **Build error:** Check Go compiler output for specific errors

## Advanced: Custom Build Flags

To modify build behavior, edit `build.bat`:

**Change output filename:**
```batch
go build -o mybot.exe .\nemesisbot\
```

**Disable stripping (for debugging):**
```batch
go build -ldflags "-X main.version=%VERSION% ..." -s -w
# Remove -s -w flags to keep symbols
```

**Add race detector:**
```batch
go build -race -ldflags "..."
```

## Related Files

- `nemesisbot/main.go` - Entry point with go:embed directives
- `build.bat` - Windows build script
- `config/config.default.json` - Main configuration template
- `nemesisbot/command/common.go` - Config initialization
- `module/config/config.go` - Config loading functions

## Version Information

To check version info in compiled binary:
```bash
./nemesisbot.exe version
```

Or with git tags:
```bash
git describe --tags --abbrev=0  # Get version
git rev-parse --short HEAD   # Get commit hash
```

---

**Last Updated:** 2025-03-03
**Maintained By:** Development Team
