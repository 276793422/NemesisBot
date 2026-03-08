// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package main

import (
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
)

func TestPlatformSpecificSecurityConfigLoading(t *testing.T) {
	// Load platform-specific security config directly
	configPath := filepath.Join("../../nemesisbot/config", config.GetPlatformSecurityConfigFilename())

	secCfg, err := config.LoadSecurityConfig(configPath)
	if err != nil {
		t.Fatalf("Failed to load security config: %v", err)
	}

	// Verify basic structure
	if secCfg == nil {
		t.Fatal("Security config should not be nil")
	}

	if secCfg.DefaultAction == "" {
		t.Error("Default action should be set")
	}

	// Verify platform-specific rules are loaded
	switch runtime.GOOS {
	case "windows":
		// Windows should have registry rules
		if secCfg.RegistryRules == nil {
			t.Error("Windows security config should have registry rules")
		}
		if len(secCfg.FileRules.Read) == 0 {
			t.Error("Windows security config should have file read rules")
		}

	case "linux", "darwin":
		// Linux and macOS should have file rules
		if len(secCfg.FileRules.Read) == 0 {
			t.Error("Security config should have file read rules")
		}
		// Should have process rules
		if len(secCfg.ProcessRules.Exec) == 0 {
			t.Error("Security config should have process exec rules")
		}
	}

	t.Logf("✓ Platform-specific security config loaded successfully for %s", runtime.GOOS)
}

func TestAllPlatformConfigFilesAreValid(t *testing.T) {
	platforms := []string{"windows", "linux", "darwin", "other"}

	for _, platform := range platforms {
		t.Run(platform, func(t *testing.T) {
			configPath := filepath.Join("../../nemesisbot/config", "config.security."+platform+".json")

			cfg, err := config.LoadSecurityConfig(configPath)
			if err != nil {
				t.Fatalf("Failed to load config for %s: %v", platform, err)
			}

			if cfg == nil {
				t.Fatal("Config should not be nil")
			}

			// Validate required fields
			if cfg.DefaultAction == "" {
				t.Error("Missing default_action")
			}

			if cfg.FileRules == nil {
				t.Error("Missing file_rules")
			}

			if cfg.ProcessRules == nil {
				t.Error("Missing process_rules")
			}

			if cfg.NetworkRules == nil {
				t.Error("Missing network_rules")
			}

			if cfg.HardwareRules == nil {
				t.Error("Missing hardware_rules")
			}

			// Platform-specific validation
			switch platform {
			case "windows":
				if cfg.RegistryRules == nil {
					t.Error("Windows config must have registry_rules")
				}
				// Check for Windows-specific paths
				hasWindowsPath := false
				for _, rule := range cfg.FileRules.Read {
					pattern := rule.Pattern
					if contains(pattern, "C:/") || contains(pattern, "Program Files") {
						hasWindowsPath = true
						break
					}
				}
				if !hasWindowsPath {
					t.Error("Windows config should protect Windows paths")
				}
			case "linux":
				// Check for Linux-specific paths
				hasLinuxPath := false
				for _, rule := range cfg.FileRules.Read {
					pattern := rule.Pattern
					if contains(pattern, "/etc/") || contains(pattern, "/usr/") || contains(pattern, "/bin/") {
						hasLinuxPath = true
						break
					}
				}
				if !hasLinuxPath {
					t.Error("Linux config should protect Linux paths")
				}
			case "darwin":
				// Check for macOS-specific paths
				hasMacOSPath := false
				for _, rule := range cfg.FileRules.Read {
					pattern := rule.Pattern
					if contains(pattern, "/System/") || contains(pattern, "/Library/") {
						hasMacOSPath = true
						break
					}
				}
				if !hasMacOSPath {
					t.Error("macOS config should protect macOS paths")
				}
			}

			t.Logf("✓ %s config is valid", platform)
		})
	}
}

func TestPlatformDetection(t *testing.T) {
	filename := config.GetPlatformSecurityConfigFilename()
	displayName := config.GetPlatformDisplayName()
	platformInfo := config.GetPlatformInfo()

	if filename == "" {
		t.Error("Filename should not be empty")
	}

	if displayName == "" {
		t.Error("Display name should not be empty")
	}

	if platformInfo == "" {
		t.Error("Platform info should not be empty")
	}

	// Verify filename matches expected pattern
	if !contains(filename, "config.security.") || !contains(filename, ".json") {
		t.Errorf("Unexpected filename format: %s", filename)
	}

	t.Logf("✓ Platform detection working correctly")
	t.Logf("  Filename: %s", filename)
	t.Logf("  Display Name: %s", displayName)
	t.Logf("  Platform Info: %s", platformInfo)
}

func TestPlatformConfigFileExists(t *testing.T) {
	platform := runtime.GOOS
	filename := "config.security." + platform + ".json"

	configPath := filepath.Join("../../nemesisbot/config", filename)
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		t.Errorf("Platform config file does not exist: %s", filename)
	}

	t.Logf("✓ Platform config file exists: %s", filename)
}

// Helper function
func contains(s, substr string) bool {
	if len(s) < len(substr) {
		return false
	}
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
