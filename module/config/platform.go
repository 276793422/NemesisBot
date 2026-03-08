// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package config

import (
	"fmt"
	"runtime"
)

// GetPlatformSecurityConfigFilename returns the platform-specific security config filename
func GetPlatformSecurityConfigFilename() string {
	switch runtime.GOOS {
	case "windows":
		return "config.security.windows.json"
	case "linux":
		return "config.security.linux.json"
	case "darwin":
		return "config.security.darwin.json"
	default:
		return "config.security.other.json"
	}
}

// GetPlatformDisplayName returns a human-readable platform name
func GetPlatformDisplayName() string {
	switch runtime.GOOS {
	case "windows":
		return "Windows"
	case "linux":
		return "Linux"
	case "darwin":
		return "macOS"
	default:
		return fmt.Sprintf("Unknown (%s)", runtime.GOOS)
	}
}

// GetPlatformInfo returns detailed platform information
func GetPlatformInfo() string {
	return fmt.Sprintf("%s (%s/%s)", GetPlatformDisplayName(), runtime.GOOS, runtime.GOARCH)
}
