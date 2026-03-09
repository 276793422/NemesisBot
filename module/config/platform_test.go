// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package config

import (
	"runtime"
	"testing"
)

func TestGetPlatformSecurityConfigFilename(t *testing.T) {
	filename := GetPlatformSecurityConfigFilename()

	if filename == "" {
		t.Error("GetPlatformSecurityConfigFilename() should not return empty string")
	}

	// Check that filename matches expected pattern
	expectedFiles := map[string]string{
		"windows": "config.security.windows.json",
		"linux":   "config.security.linux.json",
		"darwin":  "config.security.darwin.json",
	}

	expected, ok := expectedFiles[runtime.GOOS]
	if !ok {
		expected = "config.security.other.json"
	}

	if filename != expected {
		t.Errorf("GetPlatformSecurityConfigFilename() = %v, want %v", filename, expected)
	}
}

func TestGetPlatformDisplayName(t *testing.T) {
	displayName := GetPlatformDisplayName()

	if displayName == "" {
		t.Error("GetPlatformDisplayName() should not return empty string")
	}

	// Check that display name is one of the expected values
	expectedNames := map[string]string{
		"windows": "Windows",
		"linux":   "Linux",
		"darwin":  "macOS",
	}

	expected, ok := expectedNames[runtime.GOOS]
	if !ok {
		// For unknown platforms, should contain the GOOS
		if displayName == "Unknown" {
			t.Error("GetPlatformDisplayName() should include GOOS for unknown platforms")
		}
		return
	}

	if displayName != expected {
		t.Errorf("GetPlatformDisplayName() = %v, want %v", displayName, expected)
	}
}

func TestGetPlatformInfo(t *testing.T) {
	info := GetPlatformInfo()

	if info == "" {
		t.Error("GetPlatformInfo() should not return empty string")
	}

	// Check that info contains both OS and architecture
	if runtime.GOOS != "" && runtime.GOARCH != "" {
		// Just verify it returns something without being too specific
		if len(info) < len(runtime.GOOS)+len(runtime.GOARCH)+3 {
			t.Errorf("GetPlatformInfo() returned too short string: %v", info)
		}
	}
}

func TestPlatformConsistency(t *testing.T) {
	// Test that platform functions return consistent results
	displayName := GetPlatformDisplayName()
	securityFile := GetPlatformSecurityConfigFilename()
	platformInfo := GetPlatformInfo()

	// All should return non-empty values
	if displayName == "" {
		t.Error("DisplayName is empty")
	}
	if securityFile == "" {
		t.Error("SecurityConfigFilename is empty")
	}
	if platformInfo == "" {
		t.Error("PlatformInfo is empty")
	}

	// Security filename should contain the platform name
	if runtime.GOOS == "windows" {
		if securityFile != "config.security.windows.json" {
			t.Errorf("Windows security filename mismatch: %v", securityFile)
		}
		if displayName != "Windows" {
			t.Errorf("Windows display name mismatch: %v", displayName)
		}
	}
}

func TestGetPlatformDisplayName_AllPlatforms(t *testing.T) {
	// Test that all known platforms have display names
	knownPlatforms := []string{"windows", "linux", "darwin"}

	for _, platform := range knownPlatforms {
		// We can't actually change runtime.GOOS, but we can verify
		// the function works for the current platform
		displayName := GetPlatformDisplayName()
		if displayName == "" {
			t.Errorf("Platform %s should have a display name", platform)
		}
	}
}

func TestGetPlatformSecurityConfigFilename_AllPlatforms(t *testing.T) {
	// Test that all known platforms have security config filenames
	knownPlatforms := []string{"windows", "linux", "darwin"}

	for _, platform := range knownPlatforms {
		filename := GetPlatformSecurityConfigFilename()
		if filename == "" {
			t.Errorf("Platform %s should have a security config filename", platform)
		}

		// Verify filename starts with expected prefix
		if len(filename) < len("config.security.") {
			t.Errorf("Security config filename too short: %v", filename)
		}
	}
}
