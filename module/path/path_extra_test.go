// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package path

import (
	"os"
	"path/filepath"
	"testing"
)

// TestSkillsConfigPath tests SkillsConfigPath returns expected path.
func TestSkillsConfigPath(t *testing.T) {
	pm := NewPathManagerWithHome("/home/test")
	result := pm.SkillsConfigPath()
	expected := filepath.Join("/home/test", "config.skills.json")
	if result != expected {
		t.Errorf("SkillsConfigPath() = %q, want %q", result, expected)
	}
}

// TestSetSkillsConfigPath tests setting a custom skills config path.
func TestSetSkillsConfigPath(t *testing.T) {
	pm := NewPathManagerWithHome("/home/test")
	pm.SetSkillsConfigPath("/custom/skills.json")
	result := pm.SkillsConfigPath()
	if result != "/custom/skills.json" {
		t.Errorf("SkillsConfigPath() = %q, want /custom/skills.json", result)
	}
}

// TestSkillsConfigPath_EnvOverride tests SkillsConfigPath with env override.
func TestSkillsConfigPath_EnvOverride(t *testing.T) {
	origEnv := os.Getenv(EnvSkillsConfig)
	os.Unsetenv(EnvSkillsConfig)
	defer os.Setenv(EnvSkillsConfig, origEnv)

	os.Setenv(EnvSkillsConfig, "/env/skills.json")
	pm := NewPathManagerWithHome("/home/test")
	result := pm.SkillsConfigPath()
	if result != "/env/skills.json" {
		t.Errorf("SkillsConfigPath() = %q, want /env/skills.json", result)
	}
}

// TestTempDir tests TempDir returns workspace/temp.
func TestTempDir(t *testing.T) {
	pm := NewPathManagerWithHome("/home/test")
	result := pm.TempDir()
	expected := filepath.Join(pm.Workspace(), "temp")
	if result != expected {
		t.Errorf("TempDir() = %q, want %q", result, expected)
	}
}

// TestResolveSkillsConfigPath_NoEnv tests ResolveSkillsConfigPath without env.
func TestResolveSkillsConfigPath_NoEnv(t *testing.T) {
	origEnv := os.Getenv(EnvSkillsConfig)
	os.Unsetenv(EnvSkillsConfig)
	defer os.Setenv(EnvSkillsConfig, origEnv)

	result := ResolveSkillsConfigPath()
	// Should return a path containing config.skills.json
	if result == "" {
		t.Error("expected non-empty skills config path")
	}
}

// TestResolveSkillsConfigPath_EnvVar tests ResolveSkillsConfigPath with env.
func TestResolveSkillsConfigPath_EnvVar(t *testing.T) {
	origEnv := os.Getenv(EnvSkillsConfig)
	os.Unsetenv(EnvSkillsConfig)
	defer os.Setenv(EnvSkillsConfig, origEnv)

	os.Setenv(EnvSkillsConfig, "/custom/config.skills.json")
	result := ResolveSkillsConfigPath()
	if result != "/custom/config.skills.json" {
		t.Errorf("ResolveSkillsConfigPath() = %q, want /custom/config.skills.json", result)
	}
}

// TestResolveSkillsConfigPathInWorkspace tests ResolveSkillsConfigPathInWorkspace.
func TestResolveSkillsConfigPathInWorkspace_Extra(t *testing.T) {
	result := ResolveSkillsConfigPathInWorkspace("/workspace")
	expected := filepath.Join("/workspace", "config", "config.skills.json")
	if result != expected {
		t.Errorf("ResolveSkillsConfigPathInWorkspace() = %q, want %q", result, expected)
	}
}

// TestResolveScannerConfigPath_NoEnv tests ResolveScannerConfigPath without env.
func TestResolveScannerConfigPath_NoEnv(t *testing.T) {
	origEnv := os.Getenv(EnvScannerConfig)
	os.Unsetenv(EnvScannerConfig)
	defer os.Setenv(EnvScannerConfig, origEnv)

	result := ResolveScannerConfigPath()
	// Should return a path containing config.scanner.json
	if result == "" {
		t.Error("expected non-empty scanner config path")
	}
}

// TestResolveScannerConfigPath_EnvOverride tests ResolveScannerConfigPath with env.
func TestResolveScannerConfigPath_EnvOverride(t *testing.T) {
	origEnv := os.Getenv(EnvScannerConfig)
	os.Unsetenv(EnvScannerConfig)
	defer os.Setenv(EnvScannerConfig, origEnv)

	customPath := "/custom/scanner.json"
	os.Setenv(EnvScannerConfig, customPath)

	result := ResolveScannerConfigPath()
	if result != customPath {
		t.Errorf("ResolveScannerConfigPath() = %q, want %q", result, customPath)
	}
}

// TestResolveClusterConfigPathInWorkspace_Detailed tests cluster config path resolution.
func TestResolveClusterConfigPathInWorkspace_Detailed(t *testing.T) {
	result := ResolveClusterConfigPathInWorkspace("/workspace")
	expected := filepath.Join("/workspace", "config", "config.cluster.json")
	if result != expected {
		t.Errorf("ResolveClusterConfigPathInWorkspace() = %q, want %q", result, expected)
	}
}
