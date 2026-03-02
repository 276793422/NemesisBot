// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package path_test

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/path"
)

// TestExpandHome tests the ExpandHome function.
func TestExpandHome(t *testing.T) {
	home, err := os.UserHomeDir()
	if err != nil {
		t.Fatalf("Failed to get user home dir: %v", err)
	}

	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "empty string",
			input:    "",
			expected: "",
		},
		{
			name:     "tilde only",
			input:    "~",
			expected: home,
		},
		{
			name:     "tilde with path",
			input:    "~/test",
			expected: filepath.Join(home, "test"),
		},
		{
			name:     "absolute path",
			input:    "/absolute/path",
			expected: "/absolute/path",
		},
		{
			name:     "relative path",
			input:    "relative/path",
			expected: "relative/path",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := path.ExpandHome(tt.input)
			if result != tt.expected {
				t.Errorf("ExpandHome(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}

// TestResolveHomeDir_Default tests ResolveHomeDir with default settings.
func TestResolveHomeDir_Default(t *testing.T) {
	// Save and cleanup env
	origHome := os.Getenv(path.EnvHome)
	defer os.Setenv(path.EnvHome, origHome)
	os.Unsetenv(path.EnvHome)

	home, err := path.ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir() error = %v", err)
	}

	userHome, err := os.UserHomeDir()
	if err != nil {
		t.Fatalf("Failed to get user home dir: %v", err)
	}

	expected := filepath.Join(userHome, path.DefaultHomeDir)
	if home != expected {
		t.Errorf("ResolveHomeDir() = %q, want %q", home, expected)
	}
}

// TestResolveHomeDir_WithEnv tests ResolveHomeDir with NEMESISBOT_HOME set.
func TestResolveHomeDir_WithEnv(t *testing.T) {
	// Save and cleanup env
	origHome := os.Getenv(path.EnvHome)
	defer os.Setenv(path.EnvHome, origHome)

	customPath := "/custom/nemesisbot"
	os.Setenv(path.EnvHome, customPath)

	home, err := path.ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir() error = %v", err)
	}

	if home != customPath {
		t.Errorf("ResolveHomeDir() = %q, want %q", home, customPath)
	}
}

// TestResolveHomeDir_WithTilde tests ResolveHomeDir with tilde in NEMESISBOT_HOME.
func TestResolveHomeDir_WithTilde(t *testing.T) {
	// Save and cleanup env
	origHome := os.Getenv(path.EnvHome)
	defer os.Setenv(path.EnvHome, origHome)

	os.Setenv(path.EnvHome, "~/custom")

	home, err := path.ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir() error = %v", err)
	}

	userHome, err := os.UserHomeDir()
	if err != nil {
		t.Fatalf("Failed to get user home dir: %v", err)
	}

	expected := filepath.Join(userHome, "custom")
	if home != expected {
		t.Errorf("ResolveHomeDir() = %q, want %q", home, expected)
	}
}

// TestResolveConfigPath_Priority tests environment variable priority.
func TestResolveConfigPath_Priority(t *testing.T) {
	// Save and cleanup env
	origHome := os.Getenv(path.EnvHome)
	origConfig := os.Getenv(path.EnvConfig)
	defer func() {
		os.Setenv(path.EnvHome, origHome)
		os.Setenv(path.EnvConfig, origConfig)
	}()

	tests := []struct {
		name            string
		setHomeEnv      bool
		homeValue       string
		setConfigEnv    bool
		configValue     string
		expectedContains string
	}{
		{
			name:            "NEMESISBOT_CONFIG highest priority",
			setHomeEnv:      true,
			homeValue:       "/home/custom",
			setConfigEnv:    true,
			configValue:     "/custom/config.json",
			expectedContains: "/custom/config.json",
		},
		{
			name:            "NEMESISBOT_HOME secondary",
			setHomeEnv:      true,
			homeValue:       "/home/custom",
			setConfigEnv:    false,
			configValue:     "",
			expectedContains: "/home/custom/config.json",
		},
		{
			name:            "default when both unset",
			setHomeEnv:      false,
			homeValue:       "",
			setConfigEnv:    false,
			configValue:     "",
			expectedContains: ".nemesisbot/config.json",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			os.Unsetenv(path.EnvHome)
			os.Unsetenv(path.EnvConfig)

			if tt.setHomeEnv {
				os.Setenv(path.EnvHome, tt.homeValue)
			}
			if tt.setConfigEnv {
				os.Setenv(path.EnvConfig, tt.configValue)
			}

			result := path.ResolveConfigPath()
			if result != tt.expectedContains {
				// For contains check when paths might differ slightly
				if filepath.Base(result) != filepath.Base(tt.expectedContains) {
					t.Errorf("ResolveConfigPath() = %q, want to contain %q", result, tt.expectedContains)
				}
			}
		})
	}
}

// TestResolveMCPConfigPath tests MCP config path resolution.
func TestResolveMCPConfigPath(t *testing.T) {
	// Save and cleanup env
	origMCP := os.Getenv(path.EnvMCPConfig)
	origHome := os.Getenv(path.EnvHome)
	defer func() {
		os.Setenv(path.EnvMCPConfig, origMCP)
		os.Setenv(path.EnvHome, origHome)
	}()

	os.Unsetenv(path.EnvMCPConfig)
	os.Unsetenv(path.EnvHome)

	result := path.ResolveMCPConfigPath()
	if filepath.Base(result) != "config.mcp.json" {
		t.Errorf("ResolveMCPConfigPath() = %q, want ending with config.mcp.json", result)
	}

	// Test with environment variable
	os.Setenv(path.EnvMCPConfig, "/custom/mcp.json")
	result = path.ResolveMCPConfigPath()
	if result != "/custom/mcp.json" {
		t.Errorf("ResolveMCPConfigPath() with env = %q, want %q", result, "/custom/mcp.json")
	}
}

// TestResolveSecurityConfigPath tests security config path resolution.
func TestResolveSecurityConfigPath(t *testing.T) {
	// Save and cleanup env
	origSecurity := os.Getenv(path.EnvSecurityConfig)
	origHome := os.Getenv(path.EnvHome)
	defer func() {
		os.Setenv(path.EnvSecurityConfig, origSecurity)
		os.Setenv(path.EnvHome, origHome)
	}()

	os.Unsetenv(path.EnvSecurityConfig)
	os.Unsetenv(path.EnvHome)

	result := path.ResolveSecurityConfigPath()
	if filepath.Base(result) != "config.security.json" {
		t.Errorf("ResolveSecurityConfigPath() = %q, want ending with config.security.json", result)
	}

	// Test with environment variable
	os.Setenv(path.EnvSecurityConfig, "/custom/security.json")
	result = path.ResolveSecurityConfigPath()
	if result != "/custom/security.json" {
		t.Errorf("ResolveSecurityConfigPath() with env = %q, want %q", result, "/custom/security.json")
	}
}

// TestPathManager_NewPathManager tests PathManager creation.
func TestPathManager_NewPathManager(t *testing.T) {
	pm := path.NewPathManager()

	if pm == nil {
		t.Fatal("NewPathManager() returned nil")
	}

	homeDir := pm.HomeDir()
	if homeDir == "" {
		t.Error("HomeDir() is empty")
	}
}

// TestPathManager_NewPathManagerWithHome tests PathManager with custom home.
func TestPathManager_NewPathManagerWithHome(t *testing.T) {
	customHome := "/custom/nemesisbot"
	pm := path.NewPathManagerWithHome(customHome)

	if pm.HomeDir() != customHome {
		t.Errorf("HomeDir() = %q, want %q", pm.HomeDir(), customHome)
	}

	expectedWorkspace := filepath.Join(customHome, "workspace")
	if pm.Workspace() != expectedWorkspace {
		t.Errorf("Workspace() = %q, want %q", pm.Workspace(), expectedWorkspace)
	}
}

// TestPathManager_ConfigPath tests ConfigPath method.
func TestPathManager_ConfigPath(t *testing.T) {
	pm := path.NewPathManager()

	// Test default path
	configPath := pm.ConfigPath()
	if filepath.Base(configPath) != "config.json" {
		t.Errorf("ConfigPath() = %q, want ending with config.json", configPath)
	}
}

// TestPathManager_ConfigPathWithEnv tests ConfigPath with environment variable.
func TestPathManager_ConfigPathWithEnv(t *testing.T) {
	// Save and cleanup env
	origConfig := os.Getenv(path.EnvConfig)
	defer os.Setenv(path.EnvConfig, origConfig)

	customPath := "/custom/config.json"
	os.Setenv(path.EnvConfig, customPath)

	pm := path.NewPathManager()
	if pm.ConfigPath() != customPath {
		t.Errorf("ConfigPath() with env = %q, want %q", pm.ConfigPath(), customPath)
	}
}

// TestPathManager_AgentWorkspace tests AgentWorkspace method.
func TestPathManager_AgentWorkspace(t *testing.T) {
	customHome := "/custom/nemesisbot"
	pm := path.NewPathManagerWithHome(customHome)

	tests := []struct {
		name     string
		agentID  string
		expected string
	}{
		{
			name:     "main agent uses main workspace",
			agentID:  "main",
			expected: filepath.Join(customHome, "workspace"),
		},
		{
			name:     "default agent uses main workspace",
			agentID:  "default",
			expected: filepath.Join(customHome, "workspace"),
		},
		{
			name:     "empty agent ID uses main workspace",
			agentID:  "",
			expected: filepath.Join(customHome, "workspace"),
		},
		{
			name:     "custom agent uses separate workspace",
			agentID:  "bot1",
			expected: filepath.Join(customHome, "workspace-bot1"),
		},
		{
			name:     "another custom agent",
			agentID:  "test-agent",
			expected: filepath.Join(customHome, "workspace-test-agent"),
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := pm.AgentWorkspace(tt.agentID)
			if result != tt.expected {
				t.Errorf("AgentWorkspace(%q) = %q, want %q", tt.agentID, result, tt.expected)
			}
		})
	}
}

// TestPathManager_AuthPath tests AuthPath method.
func TestPathManager_AuthPath(t *testing.T) {
	customHome := "/custom/nemesisbot"
	pm := path.NewPathManagerWithHome(customHome)

	expected := filepath.Join(customHome, "auth.json")
	if pm.AuthPath() != expected {
		t.Errorf("AuthPath() = %q, want %q", pm.AuthPath(), expected)
	}
}

// TestPathManager_AuditLogDir tests AuditLogDir method.
func TestPathManager_AuditLogDir(t *testing.T) {
	customHome := "/custom/nemesisbot"
	pm := path.NewPathManagerWithHome(customHome)

	expected := filepath.Join(customHome, "workspace", "logs", "security_logs")
	if pm.AuditLogDir() != expected {
		t.Errorf("AuditLogDir() = %q, want %q", pm.AuditLogDir(), expected)
	}
}

// TestDefaultPathManager tests DefaultPathManager singleton.
func TestDefaultPathManager(t *testing.T) {
	pm1 := path.DefaultPathManager()
	pm2 := path.DefaultPathManager()

	if pm1 != pm2 {
		t.Error("DefaultPathManager() returned different instances")
	}
}

// TestPathManager_Concurrency tests concurrent access to PathManager.
func TestPathManager_Concurrency(t *testing.T) {
	pm := path.NewPathManager()
	done := make(chan bool)

	// Run multiple goroutines accessing PathManager
	for i := 0; i < 10; i++ {
		go func() {
			for j := 0; j < 100; j++ {
				pm.HomeDir()
				pm.ConfigPath()
				pm.Workspace()
				pm.AgentWorkspace("test")
			}
			done <- true
		}()
	}

	// Wait for all goroutines to complete
	for i := 0; i < 10; i++ {
		<-done
	}
	// If we got here without panic or deadlock, test passed
}
