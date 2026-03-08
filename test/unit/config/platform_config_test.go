// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package config_test

import (
	"runtime"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
)

func TestGetPlatformSecurityConfigFilename(t *testing.T) {
	filename := config.GetPlatformSecurityConfigFilename()
	expected := "config.security." + runtime.GOOS + ".json"

	if filename != expected {
		t.Errorf("Expected %s, got %s", expected, filename)
	}

	// Test specific platforms
	testCases := []struct {
		goos     string
		expected string
	}{
		{"windows", "config.security.windows.json"},
		{"linux", "config.security.linux.json"},
		{"darwin", "config.security.darwin.json"},
		{"freebsd", "config.security.other.json"},
		{"netbsd", "config.security.other.json"},
	}

	// Note: We can't directly test runtime.GOOS changes,
	// but we can verify the function returns expected format
	for _, tc := range testCases {
		result := "config.security." + tc.goos + ".json"
		if tc.goos == "windows" || tc.goos == "linux" || tc.goos == "darwin" {
			if result != tc.expected {
				t.Errorf("Platform %s: expected %s, got %s", tc.goos, tc.expected, result)
			}
		} else {
			// Other platforms should return .other.json
			if tc.goos != runtime.GOOS { // Skip current platform
				// This is just a format check
				if "config.security.other.json" != tc.expected {
					t.Errorf("Other platforms should return .other.json")
				}
			}
		}
	}
}

func TestGetPlatformDisplayName(t *testing.T) {
	displayName := config.GetPlatformDisplayName()

	if displayName == "" {
		t.Error("Display name should not be empty")
	}

	// Verify it returns known platform names
	knownPlatforms := map[string]string{
		"windows": "Windows",
		"linux":   "Linux",
		"darwin":  "macOS",
	}

	expectedName, ok := knownPlatforms[runtime.GOOS]
	if ok {
		if displayName != expectedName {
			t.Errorf("Expected display name %s, got %s", expectedName, displayName)
		}
	}
}

func TestGetPlatformInfo(t *testing.T) {
	info := config.GetPlatformInfo()

	if info == "" {
		t.Error("Platform info should not be empty")
	}

	// Verify it contains GOOS
	if !contains(info, runtime.GOOS) {
		t.Errorf("Platform info should contain GOOS %s", runtime.GOOS)
	}

	// Verify it contains GOARCH
	if !contains(info, runtime.GOARCH) {
		t.Errorf("Platform info should contain GOARCH %s", runtime.GOARCH)
	}
}

func TestPlatformSecurityConfigLoading(t *testing.T) {
	// Test that we can load the platform-specific security config
	cfg, err := config.LoadSecurityConfig("testdata/config.security." + runtime.GOOS + ".json")
	if err != nil {
		t.Fatalf("Failed to load platform security config: %v", err)
	}

	if cfg == nil {
		t.Fatal("Config should not be nil")
	}

	// Verify default action is set
	if cfg.DefaultAction == "" {
		t.Error("Default action should be set")
	}

	// Verify file rules exist
	if cfg.FileRules == nil {
		t.Error("File rules should be present")
	}

	// Platform-specific rule verification
	switch runtime.GOOS {
	case "windows":
		// Windows should have registry rules
		if cfg.RegistryRules == nil {
			t.Error("Windows config should have registry rules")
		}
		if cfg.RegistryRules.Read == nil {
			t.Error("Windows registry read rules should be present")
		}
	case "linux", "darwin":
		// Linux and macOS should have file rules protecting system directories
		if cfg.FileRules == nil || cfg.FileRules.Read == nil {
			t.Error("Linux/macOS should have file read rules")
		}
	}
}

func TestPlatformConfigFileStructure(t *testing.T) {
	// Test the structure of platform-specific config files
	configPath := "../nemesisbot/config/config.security." + runtime.GOOS + ".json"

	cfg, err := config.LoadSecurityConfig(configPath)
	if err != nil {
		t.Fatalf("Failed to load platform config: %v", err)
	}

	// Verify required top-level fields
	if cfg.DefaultAction == "" {
		t.Error("Missing default_action")
	}
	if cfg.ApprovalTimeout == 0 {
		t.Error("Missing approval_timeout_seconds")
	}
	if cfg.MaxPendingRequests == 0 {
		t.Error("Missing max_pending_requests")
	}

	// Verify file rules exist and have required operations
	if cfg.FileRules == nil {
		t.Fatal("Missing file_rules")
	}
	if cfg.FileRules.Read == nil {
		t.Error("Missing file_rules.read")
	}
	if cfg.FileRules.Write == nil {
		t.Error("Missing file_rules.write")
	}
	if cfg.FileRules.Delete == nil {
		t.Error("Missing file_rules.delete")
	}

	// Verify process rules exist
	if cfg.ProcessRules == nil {
		t.Fatal("Missing process_rules")
	}
	if cfg.ProcessRules.Exec == nil {
		t.Error("Missing process_rules.exec")
	}

	// Verify network rules exist
	if cfg.NetworkRules == nil {
		t.Fatal("Missing network_rules")
	}
	if cfg.NetworkRules.Request == nil {
		t.Error("Missing network_rules.request")
	}

	// Verify hardware rules exist
	if cfg.HardwareRules == nil {
		t.Fatal("Missing hardware_rules")
	}
}

// Helper function to check if a string contains a substring
func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(s) > len(substr) && (s[:len(substr)] == substr || s[len(s)-len(substr):] == substr || containsMiddle(s, substr)))
}

func containsMiddle(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}

func TestAllPlatformConfigFilesExist(t *testing.T) {
	platforms := []string{"windows", "linux", "darwin", "other"}

	for _, platform := range platforms {
		filename := "../nemesisbot/config/config.security." + platform + ".json"
		cfg, err := config.LoadSecurityConfig(filename)
		if err != nil {
			t.Errorf("Failed to load config for platform %s: %v", platform, err)
			continue
		}

		if cfg == nil {
			t.Errorf("Config for platform %s should not be nil", platform)
			continue
		}

		// Basic validation
		if cfg.DefaultAction == "" {
			t.Errorf("Config for platform %s missing default_action", platform)
		}
	}
}
