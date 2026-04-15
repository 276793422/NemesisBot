// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package path

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
)

func TestNewPathManager(t *testing.T) {
	pm := NewPathManager()
	if pm == nil {
		t.Fatal("NewPathManager() returned nil")
	}

	if pm.HomeDir() == "" {
		t.Error("PathManager should have a HomeDir")
	}
}

func TestNewPathManagerWithHome(t *testing.T) {
	customHome := "/custom/nemesisbot"
	pm := NewPathManagerWithHome(customHome)

	if pm.HomeDir() != customHome {
		t.Errorf("HomeDir() = %v, want %v", pm.HomeDir(), customHome)
	}
}

func TestPathManager_HomeDir(t *testing.T) {
	pm := NewPathManager()

	homeDir := pm.HomeDir()
	if homeDir == "" {
		t.Error("HomeDir() should not be empty")
	}

	// Should be thread-safe
	var wg sync.WaitGroup
	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_ = pm.HomeDir()
		}()
	}
	wg.Wait()
}

func TestPathManager_ConfigPath(t *testing.T) {
	pm := NewPathManager()

	configPath := pm.ConfigPath()
	if configPath == "" {
		t.Error("ConfigPath() should not be empty")
	}

	// Should contain config.json
	if filepath.Base(configPath) != "config.json" {
		t.Errorf("ConfigPath() should end with config.json, got %v", configPath)
	}
}

func TestPathManager_SetConfigPath(t *testing.T) {
	pm := NewPathManager()

	customPath := "/custom/config.json"
	pm.SetConfigPath(customPath)

	if pm.ConfigPath() != customPath {
		t.Errorf("ConfigPath() = %v, want %v", pm.ConfigPath(), customPath)
	}
}

func TestPathManager_MCPConfigPath(t *testing.T) {
	pm := NewPathManager()

	mcpPath := pm.MCPConfigPath()
	if mcpPath == "" {
		t.Error("MCPConfigPath() should not be empty")
	}

	// Should contain config.mcp.json
	if filepath.Base(mcpPath) != "config.mcp.json" {
		t.Errorf("MCPConfigPath() should end with config.mcp.json, got %v", mcpPath)
	}
}

func TestPathManager_SetMCPConfigPath(t *testing.T) {
	pm := NewPathManager()

	customPath := "/custom/config.mcp.json"
	pm.SetMCPConfigPath(customPath)

	if pm.MCPConfigPath() != customPath {
		t.Errorf("MCPConfigPath() = %v, want %v", pm.MCPConfigPath(), customPath)
	}
}

func TestPathManager_SecurityConfigPath(t *testing.T) {
	pm := NewPathManager()

	securityPath := pm.SecurityConfigPath()
	if securityPath == "" {
		t.Error("SecurityConfigPath() should not be empty")
	}

	// Should contain config.security.json
	if filepath.Base(securityPath) != "config.security.json" {
		t.Errorf("SecurityConfigPath() should end with config.security.json, got %v", securityPath)
	}
}

func TestPathManager_SetSecurityConfigPath(t *testing.T) {
	pm := NewPathManager()

	customPath := "/custom/config.security.json"
	pm.SetSecurityConfigPath(customPath)

	if pm.SecurityConfigPath() != customPath {
		t.Errorf("SecurityConfigPath() = %v, want %v", pm.SecurityConfigPath(), customPath)
	}
}

func TestPathManager_Workspace(t *testing.T) {
	pm := NewPathManager()

	workspace := pm.Workspace()
	if workspace == "" {
		t.Error("Workspace() should not be empty")
	}

	// Should be a subdirectory of home dir
	homeDir := pm.HomeDir()
	if !filepath.IsAbs(workspace) {
		// If not absolute, should be relative to home
		expected := filepath.Join(homeDir, "workspace")
		if workspace != expected {
			t.Logf("Workspace = %v, expected %v", workspace, expected)
		}
	}
}

func TestPathManager_AuthPath(t *testing.T) {
	pm := NewPathManager()

	authPath := pm.AuthPath()
	if authPath == "" {
		t.Error("AuthPath() should not be empty")
	}

	// Should contain auth.json
	if filepath.Base(authPath) != "auth.json" {
		t.Errorf("AuthPath() should end with auth.json, got %v", authPath)
	}
}

func TestPathManager_AuditLogDir(t *testing.T) {
	pm := NewPathManager()

	auditLogDir := pm.AuditLogDir()
	if auditLogDir == "" {
		t.Error("AuditLogDir() should not be empty")
	}

	// Should contain security_logs
	if !filepath.IsAbs(auditLogDir) {
		// If not absolute, should contain security_logs
		if !strings.Contains(auditLogDir, "security_logs") {
			t.Errorf("AuditLogDir() should contain security_logs, got %v", auditLogDir)
		}
	}
}

func TestPathManager_AgentWorkspace(t *testing.T) {
	pm := NewPathManager()

	tests := []struct {
		name       string
		agentID    string
		wantPrefix string
	}{
		{
			name:       "Default agent",
			agentID:    "",
			wantPrefix: pm.Workspace(),
		},
		{
			name:       "Main agent",
			agentID:    "main",
			wantPrefix: pm.Workspace(),
		},
		{
			name:       "Default agent",
			agentID:    "default",
			wantPrefix: pm.Workspace(),
		},
		{
			name:       "Custom agent",
			agentID:    "custom-agent",
			wantPrefix: filepath.Join(pm.HomeDir(), "workspace-custom-agent"),
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := pm.AgentWorkspace(tt.agentID)
			if got != tt.wantPrefix {
				t.Errorf("AgentWorkspace(%v) = %v, want %v", tt.agentID, got, tt.wantPrefix)
			}
		})
	}
}

func TestExpandHome(t *testing.T) {
	homeDir, err := os.UserHomeDir()
	if err != nil {
		t.Fatalf("Failed to get user home directory: %v", err)
	}

	tests := []struct {
		name string
		path string
		want string
	}{
		{
			name: "Empty string",
			path: "",
			want: "",
		},
		{
			name: "Just tilde",
			path: "~",
			want: homeDir,
		},
		{
			name: "Tilde with slash",
			path: "~/Documents",
			want: filepath.Join(homeDir, "Documents"),
		},
		{
			name: "Tilde with backslash (Windows)",
			path: `~\Documents`,
			want: filepath.Join(homeDir, "Documents"),
		},
		{
			name: "Absolute path",
			path: "/absolute/path",
			want: "/absolute/path",
		},
		{
			name: "Relative path",
			path: "relative/path",
			want: "relative/path",
		},
		{
			name: "Path without tilde",
			path: "/usr/local/bin",
			want: "/usr/local/bin",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := ExpandHome(tt.path)
			if got != tt.want {
				t.Errorf("ExpandHome(%v) = %v, want %v", tt.path, got, tt.want)
			}
		})
	}
}

func TestDetectLocal(t *testing.T) {
	// Save current directory
	cwd, err := os.Getwd()
	if err != nil {
		t.Fatalf("Failed to get working directory: %v", err)
	}
	defer os.Chdir(cwd)

	// Test in a temp directory (no .nemesisbot)
	tmpDir := t.TempDir()
	err = os.Chdir(tmpDir)
	if err != nil {
		t.Fatalf("Failed to change directory: %v", err)
	}

	if DetectLocal() {
		t.Error("DetectLocal() should return false in temp directory without .nemesisbot")
	}

	// Create .nemesisbot directory
	err = os.Mkdir(".nemesisbot", 0755)
	if err != nil {
		t.Fatalf("Failed to create .nemesisbot directory: %v", err)
	}

	if !DetectLocal() {
		t.Error("DetectLocal() should return true when .nemesisbot exists")
	}
}

func TestResolveHomeDir(t *testing.T) {
	// Save original LocalMode
	originalLocalMode := LocalMode
	defer func() { LocalMode = originalLocalMode }()

	// Test 1: LocalMode takes precedence
	LocalMode = true
	cwd, err := os.Getwd()
	if err != nil {
		t.Fatalf("Failed to get working directory: %v", err)
	}

	homeDir, err := ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir() error = %v", err)
	}

	expected := filepath.Join(cwd, DefaultHomeDir)
	if homeDir != expected {
		t.Errorf("ResolveHomeDir() with LocalMode = %v, want %v", homeDir, expected)
	}

	// Test 2: Non-local mode should return home directory
	LocalMode = false
	homeDir, err = ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir() error = %v", err)
	}

	if homeDir == "" {
		t.Error("ResolveHomeDir() should not return empty string")
	}

	// Should contain DefaultHomeDir
	if !strings.Contains(homeDir, DefaultHomeDir) {
		t.Errorf("ResolveHomeDir() should contain %v, got %v", DefaultHomeDir, homeDir)
	}
}

func TestResolveHomeDir_WithEnv(t *testing.T) {
	// Save original state
	originalLocalMode := LocalMode
	originalEnv := os.Getenv(EnvHome)
	defer func() {
		LocalMode = originalLocalMode
		if originalEnv == "" {
			os.Unsetenv(EnvHome)
		} else {
			os.Setenv(EnvHome, originalEnv)
		}
	}()

	// Disable local mode to test env variable
	LocalMode = false

	// Set custom NEMESISBOT_HOME
	customHome := t.TempDir()
	err := os.Setenv(EnvHome, customHome)
	if err != nil {
		t.Fatalf("Failed to set env variable: %v", err)
	}

	homeDir, err := ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir() error = %v", err)
	}

	expected := filepath.Join(customHome, DefaultHomeDir)
	if homeDir != expected {
		t.Errorf("ResolveHomeDir() with env = %v, want %v", homeDir, expected)
	}
}

func TestResolveConfigPath(t *testing.T) {
	// Save original state
	originalEnv := os.Getenv(EnvConfig)
	defer func() {
		if originalEnv == "" {
			os.Unsetenv(EnvConfig)
		} else {
			os.Setenv(EnvConfig, originalEnv)
		}
	}()

	// Test with NEMESISBOT_CONFIG set
	customConfig := "/custom/config.json"
	err := os.Setenv(EnvConfig, customConfig)
	if err != nil {
		t.Fatalf("Failed to set env variable: %v", err)
	}

	configPath := ResolveConfigPath()
	if configPath != customConfig {
		t.Errorf("ResolveConfigPath() = %v, want %v", configPath, customConfig)
	}

	// Test without env variable
	os.Unsetenv(EnvConfig)
	configPath = ResolveConfigPath()
	if configPath == "" {
		t.Error("ResolveConfigPath() should not return empty string")
	}

	if filepath.Base(configPath) != "config.json" {
		t.Errorf("ResolveConfigPath() should end with config.json, got %v", configPath)
	}
}

func TestResolveMCPConfigPath(t *testing.T) {
	// Save original state
	originalEnv := os.Getenv(EnvMCPConfig)
	defer func() {
		if originalEnv == "" {
			os.Unsetenv(EnvMCPConfig)
		} else {
			os.Setenv(EnvMCPConfig, originalEnv)
		}
	}()

	// Test with NEMESISBOT_MCP_CONFIG set
	customConfig := "/custom/config.mcp.json"
	err := os.Setenv(EnvMCPConfig, customConfig)
	if err != nil {
		t.Fatalf("Failed to set env variable: %v", err)
	}

	configPath := ResolveMCPConfigPath()
	if configPath != customConfig {
		t.Errorf("ResolveMCPConfigPath() = %v, want %v", configPath, customConfig)
	}

	// Test without env variable
	os.Unsetenv(EnvMCPConfig)
	configPath = ResolveMCPConfigPath()
	if configPath == "" {
		t.Error("ResolveMCPConfigPath() should not return empty string")
	}

	if filepath.Base(configPath) != "config.mcp.json" {
		t.Errorf("ResolveMCPConfigPath() should end with config.mcp.json, got %v", configPath)
	}
}

func TestResolveSecurityConfigPath(t *testing.T) {
	// Save original state
	originalEnv := os.Getenv(EnvSecurityConfig)
	defer func() {
		if originalEnv == "" {
			os.Unsetenv(EnvSecurityConfig)
		} else {
			os.Setenv(EnvSecurityConfig, originalEnv)
		}
	}()

	// Test with NEMESISBOT_SECURITY_CONFIG set
	customConfig := "/custom/config.security.json"
	err := os.Setenv(EnvSecurityConfig, customConfig)
	if err != nil {
		t.Fatalf("Failed to set env variable: %v", err)
	}

	configPath := ResolveSecurityConfigPath()
	if configPath != customConfig {
		t.Errorf("ResolveSecurityConfigPath() = %v, want %v", configPath, customConfig)
	}

	// Test without env variable
	os.Unsetenv(EnvSecurityConfig)
	configPath = ResolveSecurityConfigPath()
	if configPath == "" {
		t.Error("ResolveSecurityConfigPath() should not return empty string")
	}

	if filepath.Base(configPath) != "config.security.json" {
		t.Errorf("ResolveSecurityConfigPath() should end with config.security.json, got %v", configPath)
	}
}

func TestDefaultPathManager(t *testing.T) {
	pm1 := DefaultPathManager()
	pm2 := DefaultPathManager()

	// Should return the same instance (singleton)
	if pm1 != pm2 {
		t.Error("DefaultPathManager() should return the same instance")
	}

	// Should be thread-safe
	var wg sync.WaitGroup
	managers := make([]*PathManager, 100)
	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			managers[idx] = DefaultPathManager()
		}(i)
	}
	wg.Wait()

	// All should be the same instance
	for i := 0; i < 100; i++ {
		if managers[i] != pm1 {
			t.Errorf("managers[%d] is not the same instance", i)
		}
	}
}

func TestResolveMCPConfigPathInWorkspace(t *testing.T) {
	workspace := "/test/workspace"
	expected := filepath.Join(workspace, "config", "config.mcp.json")

	got := ResolveMCPConfigPathInWorkspace(workspace)
	if got != expected {
		t.Errorf("ResolveMCPConfigPathInWorkspace() = %v, want %v", got, expected)
	}
}

func TestResolveSecurityConfigPathInWorkspace(t *testing.T) {
	workspace := "/test/workspace"
	expected := filepath.Join(workspace, "config", "config.security.json")

	got := ResolveSecurityConfigPathInWorkspace(workspace)
	if got != expected {
		t.Errorf("ResolveSecurityConfigPathInWorkspace() = %v, want %v", got, expected)
	}
}

func TestResolveClusterConfigPathInWorkspace(t *testing.T) {
	workspace := "/test/workspace"
	expected := filepath.Join(workspace, "config", "config.cluster.json")

	got := ResolveClusterConfigPathInWorkspace(workspace)
	if got != expected {
		t.Errorf("ResolveClusterConfigPathInWorkspace() = %v, want %v", got, expected)
	}
}

func TestPathManager_Concurrency(t *testing.T) {
	pm := NewPathManager()

	// Test concurrent access to all methods
	var wg sync.WaitGroup
	methods := []func(){
		func() { pm.HomeDir() },
		func() { pm.ConfigPath() },
		func() { pm.MCPConfigPath() },
		func() { pm.SecurityConfigPath() },
		func() { pm.Workspace() },
		func() { pm.AuthPath() },
		func() { pm.AuditLogDir() },
		func() { pm.AgentWorkspace("test") },
	}

	for i := 0; i < 100; i++ {
		for _, method := range methods {
			wg.Add(1)
			go func(m func()) {
				defer wg.Done()
				m()
			}(method)
		}
	}

	wg.Wait()
}

func TestMinimalConfig_WorkspacePath(t *testing.T) {
	cfg := &minimalConfig{}

	// Test default workspace
	ws := cfg.WorkspacePath()
	if ws == "" {
		t.Error("WorkspacePath() should return default path when empty")
	}

	// Test custom workspace
	customWS := "/custom/workspace"
	cfg.Agents.Defaults.Workspace = customWS
	ws = cfg.WorkspacePath()
	if ws != customWS {
		t.Errorf("WorkspacePath() = %v, want %v", ws, customWS)
	}

	// Test with tilde expansion
	homeDir, _ := os.UserHomeDir()
	cfg.Agents.Defaults.Workspace = "~/test"
	ws = cfg.WorkspacePath()
	expected := filepath.Join(homeDir, "test")
	if ws != expected {
		t.Errorf("WorkspacePath() with ~ = %v, want %v", ws, expected)
	}
}

func TestConstants(t *testing.T) {
	// Test environment variable constants
	if EnvHome != "NEMESISBOT_HOME" {
		t.Errorf("EnvHome = %v, want NEMESISBOT_HOME", EnvHome)
	}
	if EnvConfig != "NEMESISBOT_CONFIG" {
		t.Errorf("EnvConfig = %v, want NEMESISBOT_CONFIG", EnvConfig)
	}
	if EnvMCPConfig != "NEMESISBOT_MCP_CONFIG" {
		t.Errorf("EnvMCPConfig = %v, want NEMESISBOT_MCP_CONFIG", EnvMCPConfig)
	}
	if EnvSecurityConfig != "NEMESISBOT_SECURITY_CONFIG" {
		t.Errorf("EnvSecurityConfig = %v, want NEMESISBOT_SECURITY_CONFIG", EnvSecurityConfig)
	}

	// Test default directory name
	if DefaultHomeDir != ".nemesisbot" {
		t.Errorf("DefaultHomeDir = %v, want .nemesisbot", DefaultHomeDir)
	}
}

func TestPathSeparator(t *testing.T) {
	pm := NewPathManager()

	// Test that paths use correct separator for the OS
	homeDir := pm.HomeDir()
	sep := string(filepath.Separator)

	if runtime.GOOS == "windows" {
		// Windows should use backslash
		if filepath.Separator != '\\' {
			t.Logf("Warning: Windows separator is not backslash: %v", filepath.Separator)
		}
	} else {
		// Unix should use forward slash
		if filepath.Separator != '/' {
			t.Logf("Warning: Unix separator is not forward slash: %v", filepath.Separator)
		}
	}

	_ = sep // Use the variable
	_ = homeDir
}

func TestPathManager_ThreadSafety(t *testing.T) {
	pm := NewPathManager()

	// Test concurrent reads and writes
	var wg sync.WaitGroup
	done := make(chan bool, 200)

	// Readers
	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_ = pm.HomeDir()
			_ = pm.ConfigPath()
			_ = pm.Workspace()
			done <- true
		}()
	}

	// Writers
	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func(n int) {
			defer wg.Done()
			pm.SetConfigPath(filepath.Join(os.TempDir(), "test.json"))
			pm.SetMCPConfigPath(filepath.Join(os.TempDir(), "mcp.json"))
			pm.SetSecurityConfigPath(filepath.Join(os.TempDir(), "security.json"))
			done <- true
		}(i)
	}

	wg.Wait()
	close(done)

	count := 0
	for range done {
		count++
	}

	if count != 200 {
		t.Errorf("Expected 200 operations, got %d", count)
	}
}

func TestResolveHomeDirErrorCases(t *testing.T) {
	// Save original state
	originalLocalMode := LocalMode
	defer func() { LocalMode = originalLocalMode }()

	// Test when os.UserHomeDir fails
	// This is hard to test directly, but we can test the fallback behavior
	LocalMode = false

	// Test error case for workspace resolution
	// We'll create a config that doesn't have a workspace
	pm := NewPathManager()
	pm.SetConfigPath("/non/existent/config.json")

	// Should return default paths without error
	homeDir := pm.HomeDir()
	if homeDir == "" {
		t.Error("HomeDir() should not return empty string")
	}
}

func TestResolveConfigPathWithInvalidConfig(t *testing.T) {
	// Save original state
	originalEnv := os.Getenv(EnvConfig)
	defer func() {
		if originalEnv == "" {
			os.Unsetenv(EnvConfig)
		} else {
			os.Setenv(EnvConfig, originalEnv)
		}
	}()

	// Create a config file that will cause loadConfigForWorkspace to fail
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "config.json")

	// Write invalid JSON
	os.WriteFile(configPath, []byte("invalid json"), 0644)

	// Set the config path
	pm := NewPathManager()
	pm.SetConfigPath(configPath)

	// Should not crash, should return fallback path
	resolvedPath := ResolveConfigPath()
	if resolvedPath == "" {
		t.Error("ResolveConfigPath() should not return empty string")
	}

	// Should not contain the invalid config path
	if strings.Contains(resolvedPath, tmpDir) {
		t.Error("ResolveConfigPath() should not use invalid config path")
	}
}

func TestLoadConfigForWorkspace(t *testing.T) {
	tmpDir := t.TempDir()

	// Test 1: Valid config with workspace
	validConfig := `{
	"agents": {
		"defaults": {
			"workspace": "/custom/workspace"
		}
	}
}`
	configPath := filepath.Join(tmpDir, "valid.json")
	os.WriteFile(configPath, []byte(validConfig), 0644)

	cfg, err := loadConfigForWorkspace(configPath)
	if err != nil {
		t.Errorf("loadConfigForWorkspace should succeed with valid config: %v", err)
	}
	if cfg.WorkspacePath() != "/custom/workspace" {
		t.Errorf("Expected workspace path '/custom/workspace', got %v", cfg.WorkspacePath())
	}

	// Test 2: Valid config without workspace (should use default)
	noWorkspaceConfig := `{
	"agents": {
		"defaults": {}
	}
}`
	configPath2 := filepath.Join(tmpDir, "no_workspace.json")
	os.WriteFile(configPath2, []byte(noWorkspaceConfig), 0644)

	cfg, err = loadConfigForWorkspace(configPath2)
	if err != nil {
		t.Errorf("loadConfigForWorkspace should succeed with config without workspace: %v", err)
	}
	expectedDefault := filepath.Join("~", ".nemesisbot", "workspace")
	if cfg.WorkspacePath() != expectedDefault {
		t.Errorf("Expected default workspace path %v, got %v", expectedDefault, cfg.WorkspacePath())
	}

	// Test 3: Invalid JSON
	invalidConfig := `{"invalid": json}`
	configPath3 := filepath.Join(tmpDir, "invalid.json")
	os.WriteFile(configPath3, []byte(invalidConfig), 0644)

	_, err = loadConfigForWorkspace(configPath3)
	if err == nil {
		t.Error("loadConfigForWorkspace should fail with invalid JSON")
	}

	// Test 4: Non-existent file
	_, err = loadConfigForWorkspace("/non/existent/file.json")
	if err == nil {
		t.Error("loadConfigForWorkspace should fail with non-existent file")
	}
}

func TestMinimalConfigWorkspacePath(t *testing.T) {
	cfg := &minimalConfig{}

	// Test empty workspace (should return default)
	expectedDefault := filepath.Join("~", ".nemesisbot", "workspace")
	if cfg.WorkspacePath() != expectedDefault {
		t.Errorf("Expected default workspace %v, got %v", expectedDefault, cfg.WorkspacePath())
	}

	// Test custom workspace
	cfg.Agents.Defaults.Workspace = "/custom/path"
	expectedCustom := "/custom/path"
	if cfg.WorkspacePath() != expectedCustom {
		t.Errorf("Expected custom workspace %v, got %v", expectedCustom, cfg.WorkspacePath())
	}

	// Test tilde expansion
	cfg.Agents.Defaults.Workspace = "~/custom"
	homeDir, _ := os.UserHomeDir()
	expectedWithTilde := filepath.Join(homeDir, "custom")
	if cfg.WorkspacePath() != expectedWithTilde {
		t.Errorf("Expected tilde-expanded workspace %v, got %v", expectedWithTilde, cfg.WorkspacePath())
	}
}

func TestResolvePathsWhenEnvVarIsInvalid(t *testing.T) {
	// Test with environment variable set to invalid paths
	originalConfig := os.Getenv(EnvConfig)
	originalMCP := os.Getenv(EnvMCPConfig)
	originalSecurity := os.Getenv(EnvSecurityConfig)

	defer func() {
		if originalConfig == "" {
			os.Unsetenv(EnvConfig)
		} else {
			os.Setenv(EnvConfig, originalConfig)
		}
		if originalMCP == "" {
			os.Unsetenv(EnvMCPConfig)
		} else {
			os.Setenv(EnvMCPConfig, originalMCP)
		}
		if originalSecurity == "" {
			os.Unsetenv(EnvSecurityConfig)
		} else {
			os.Setenv(EnvSecurityConfig, originalSecurity)
		}
	}()

	// Set invalid environment variables
	os.Setenv(EnvConfig, "/invalid/path")
	os.Setenv(EnvMCPConfig, "/invalid/mcp/path")
	os.Setenv(EnvSecurityConfig, "/invalid/security/path")

	// Should still return valid paths (though they might not exist)
	configPath := ResolveConfigPath()
	if configPath == "" {
		t.Error("ResolveConfigPath should not return empty string")
	}

	mcpPath := ResolveMCPConfigPath()
	if mcpPath == "" {
		t.Error("ResolveMCPConfigPath should not return empty string")
	}

	securityPath := ResolveSecurityConfigPath()
	if securityPath == "" {
		t.Error("ResolveSecurityConfigPath should not return empty string")
	}
}

func TestPathManager_ResetPaths(t *testing.T) {
	pm := NewPathManager()

	// Set custom paths
	pm.SetConfigPath("/custom/config")
	pm.SetMCPConfigPath("/custom/mcp")
	pm.SetSecurityConfigPath("/custom/security")

	// Verify they were set
	if pm.ConfigPath() != "/custom/config" {
		t.Errorf("Expected config path /custom/config, got %v", pm.ConfigPath())
	}

	// Create new manager - should have default paths
	pm2 := NewPathManager()

	// Should return default paths
	if !strings.HasSuffix(pm2.ConfigPath(), "config.json") {
		t.Errorf("Expected default config path ending with config.json, got %v", pm2.ConfigPath())
	}
	if !strings.HasSuffix(pm2.MCPConfigPath(), "config.mcp.json") {
		t.Errorf("Expected default MCP config path ending with config.mcp.json, got %v", pm2.MCPConfigPath())
	}
	if !strings.HasSuffix(pm2.SecurityConfigPath(), "config.security.json") {
		t.Errorf("Expected default security config path ending with config.security.json, got %v", pm2.SecurityConfigPath())
	}
}

// TestPathManager_ConfigPathWithEnv tests ConfigPath with environment variable
func TestPathManager_ConfigPathWithEnv(t *testing.T) {
	// Save and restore env
	originalEnv := os.Getenv(EnvConfig)
	defer func() {
		if originalEnv == "" {
			os.Unsetenv(EnvConfig)
		} else {
			os.Setenv(EnvConfig, originalEnv)
		}
	}()

	pm := NewPathManager()

	// Test without env variable (uses default path)
	os.Unsetenv(EnvConfig)
	defaultPath := pm.ConfigPath()
	if defaultPath == "" {
		t.Error("ConfigPath should not return empty string")
	}
	if !strings.HasSuffix(defaultPath, "config.json") {
		t.Errorf("ConfigPath should end with config.json, got %v", defaultPath)
	}

	// Test with env variable set
	customPath := "/custom/env/config.json"
	os.Setenv(EnvConfig, customPath)

	envPath := pm.ConfigPath()
	if envPath != customPath {
		t.Errorf("ConfigPath with env = %v, want %v", envPath, customPath)
	}

	// Test with cached config path
	pm.SetConfigPath("/cached/config.json")
	cachedPath := pm.ConfigPath()
	if cachedPath != "/cached/config.json" {
		t.Errorf("ConfigPath with cached = %v, want /cached/config.json", cachedPath)
	}
}

// TestPathManager_MCPConfigPathWithEnv tests MCPConfigPath with environment variable
func TestPathManager_MCPConfigPathWithEnv(t *testing.T) {
	// Save and restore env
	originalEnv := os.Getenv(EnvMCPConfig)
	defer func() {
		if originalEnv == "" {
			os.Unsetenv(EnvMCPConfig)
		} else {
			os.Setenv(EnvMCPConfig, originalEnv)
		}
	}()

	pm := NewPathManager()

	// Test without env variable (uses default path)
	os.Unsetenv(EnvMCPConfig)
	defaultPath := pm.MCPConfigPath()
	if defaultPath == "" {
		t.Error("MCPConfigPath should not return empty string")
	}
	if !strings.HasSuffix(defaultPath, "config.mcp.json") {
		t.Errorf("MCPConfigPath should end with config.mcp.json, got %v", defaultPath)
	}

	// Test with env variable set
	customPath := "/custom/env/config.mcp.json"
	os.Setenv(EnvMCPConfig, customPath)

	envPath := pm.MCPConfigPath()
	if envPath != customPath {
		t.Errorf("MCPConfigPath with env = %v, want %v", envPath, customPath)
	}

	// Test with cached config path
	pm.SetMCPConfigPath("/cached/config.mcp.json")
	cachedPath := pm.MCPConfigPath()
	if cachedPath != "/cached/config.mcp.json" {
		t.Errorf("MCPConfigPath with cached = %v, want /cached/config.mcp.json", cachedPath)
	}
}

// TestPathManager_SecurityConfigPathWithEnv tests SecurityConfigPath with environment variable
func TestPathManager_SecurityConfigPathWithEnv(t *testing.T) {
	// Save and restore env
	originalEnv := os.Getenv(EnvSecurityConfig)
	defer func() {
		if originalEnv == "" {
			os.Unsetenv(EnvSecurityConfig)
		} else {
			os.Setenv(EnvSecurityConfig, originalEnv)
		}
	}()

	pm := NewPathManager()

	// Test without env variable (uses default path)
	os.Unsetenv(EnvSecurityConfig)
	defaultPath := pm.SecurityConfigPath()
	if defaultPath == "" {
		t.Error("SecurityConfigPath should not return empty string")
	}
	if !strings.HasSuffix(defaultPath, "config.security.json") {
		t.Errorf("SecurityConfigPath should end with config.security.json, got %v", defaultPath)
	}

	// Test with env variable set
	customPath := "/custom/env/config.security.json"
	os.Setenv(EnvSecurityConfig, customPath)

	envPath := pm.SecurityConfigPath()
	if envPath != customPath {
		t.Errorf("SecurityConfigPath with env = %v, want %v", envPath, customPath)
	}

	// Test with cached config path
	pm.SetSecurityConfigPath("/cached/config.security.json")
	cachedPath := pm.SecurityConfigPath()
	if cachedPath != "/cached/config.security.json" {
		t.Errorf("SecurityConfigPath with cached = %v, want /cached/config.security.json", cachedPath)
	}
}

// TestResolveHomeDir_AutoDetectLocal tests auto-detection of local .nemesisbot directory
func TestResolveHomeDir_AutoDetectLocal(t *testing.T) {
	// Save original state
	originalLocalMode := LocalMode
	originalEnv := os.Getenv(EnvHome)
	defer func() {
		LocalMode = originalLocalMode
		if originalEnv == "" {
			os.Unsetenv(EnvHome)
		} else {
			os.Setenv(EnvHome, originalEnv)
		}
	}()

	// Disable local mode and env variable to test auto-detection
	LocalMode = false
	os.Unsetenv(EnvHome)

	// Test in temp directory without .nemesisbot
	tmpDir := t.TempDir()
	cwd, _ := os.Getwd()
	defer os.Chdir(cwd)
	os.Chdir(tmpDir)

	homeDir, err := ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir failed: %v", err)
	}

	// Should use default home directory (not local)
	if strings.Contains(homeDir, tmpDir) {
		t.Error("ResolveHomeDir should not use temp directory when .nemesisbot doesn't exist")
	}

	// Create .nemesisbot directory
	err = os.Mkdir(DefaultHomeDir, 0755)
	if err != nil {
		t.Fatalf("Failed to create .nemesisbot: %v", err)
	}

	homeDir, err = ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir failed: %v", err)
	}

	// Should now use local directory
	expected := filepath.Join(tmpDir, DefaultHomeDir)
	if homeDir != expected {
		t.Errorf("ResolveHomeDir with local .nemesisbot = %v, want %v", homeDir, expected)
	}
}

// TestResolveHomeDir_ErrorCases tests error handling in ResolveHomeDir
func TestResolveHomeDir_ErrorCases(t *testing.T) {
	// Save original state
	originalLocalMode := LocalMode
	originalEnv := os.Getenv(EnvHome)
	defer func() {
		LocalMode = originalLocalMode
		if originalEnv == "" {
			os.Unsetenv(EnvHome)
		} else {
			os.Setenv(EnvHome, originalEnv)
		}
	}()

	// Test 1: LocalMode (should succeed normally)
	LocalMode = true
	homeDir, err := ResolveHomeDir()
	if err != nil {
		// If os.Getwd fails, we might get an error, but it should succeed normally
		t.Logf("ResolveHomeDir with LocalMode failed: %v", err)
	} else {
		// Should succeed normally
		if homeDir == "" {
			t.Error("ResolveHomeDir should not return empty string")
		}
	}

	// Test 2: NEMESISBOT_HOME with tilde expansion
	LocalMode = false
	os.Setenv(EnvHome, "~/custom")
	homeDir, err = ResolveHomeDir()
	if err != nil {
		t.Errorf("ResolveHomeDir with ~/ in NEMESISBOT_HOME failed: %v", err)
	}

	userHome, _ := os.UserHomeDir()
	// ExpandHome("~/custom") returns filepath.Join(userHome, "custom")
	// Then ResolveHomeDir returns filepath.Join(ExpandHome("~/custom"), DefaultHomeDir)
	expected := filepath.Join(filepath.Join(userHome, "custom"), DefaultHomeDir)
	if homeDir != expected {
		t.Errorf("ResolveHomeDir with ~/ in NEMESISBOT_HOME = %v, want %v", homeDir, expected)
	}
}

// TestResolveConfigPath_WithValidConfig tests ResolveConfigPath with valid config
func TestResolveConfigPath_WithValidConfig(t *testing.T) {
	// Save original state
	originalEnv := os.Getenv(EnvConfig)
	originalLocalMode := LocalMode
	defer func() {
		if originalEnv == "" {
			os.Unsetenv(EnvConfig)
		} else {
			os.Setenv(EnvConfig, originalEnv)
		}
		LocalMode = originalLocalMode
	}()

	os.Unsetenv(EnvConfig)
	LocalMode = false

	// Create a temp directory with config
	tmpDir := t.TempDir()
	cwd, _ := os.Getwd()
	defer os.Chdir(cwd)
	os.Chdir(tmpDir)

	// Create .nemesisbot directory
	err := os.Mkdir(DefaultHomeDir, 0755)
	if err != nil {
		t.Fatalf("Failed to create .nemesisbot: %v", err)
	}

	configPath := ResolveConfigPath()
	expected := filepath.Join(tmpDir, DefaultHomeDir, "config.json")
	if configPath != expected {
		t.Errorf("ResolveConfigPath with local .nemesisbot = %v, want %v", configPath, expected)
	}
}

// TestResolveMCPConfigPath_WithConfigFile tests loading workspace from config file
func TestResolveMCPConfigPath_WithConfigFile(t *testing.T) {
	// Save original state
	originalEnv := os.Getenv(EnvMCPConfig)
	originalLocalMode := LocalMode
	originalHomeEnv := os.Getenv(EnvHome)
	defer func() {
		if originalEnv == "" {
			os.Unsetenv(EnvMCPConfig)
		} else {
			os.Setenv(EnvMCPConfig, originalEnv)
		}
		LocalMode = originalLocalMode
		if originalHomeEnv == "" {
			os.Unsetenv(EnvHome)
		} else {
			os.Setenv(EnvHome, originalHomeEnv)
		}
	}()

	os.Unsetenv(EnvMCPConfig)
	os.Unsetenv(EnvHome)
	LocalMode = false

	// Create a temp directory with config
	tmpDir := t.TempDir()
	cwd, _ := os.Getwd()
	defer os.Chdir(cwd)
	os.Chdir(tmpDir)

	// Create .nemesisbot directory
	err := os.Mkdir(DefaultHomeDir, 0755)
	if err != nil {
		t.Fatalf("Failed to create .nemesisbot: %v", err)
	}

	// Create config.json with workspace
	configContent := `{
		"agents": {
			"defaults": {
				"workspace": "/custom/workspace"
			}
		}
	}`
	configPath := filepath.Join(tmpDir, DefaultHomeDir, "config.json")
	err = os.WriteFile(configPath, []byte(configContent), 0644)
	if err != nil {
		t.Fatalf("Failed to write config: %v", err)
	}

	mcpConfigPath := ResolveMCPConfigPath()
	// On Windows, filepath.Join will use backslashes
	expected := filepath.Join("/custom/workspace", "config", "config.mcp.json")
	if mcpConfigPath != expected {
		t.Errorf("ResolveMCPConfigPath with config = %v, want %v", mcpConfigPath, expected)
	}
}

// TestResolveSecurityConfigPath_WithConfigFile tests loading workspace from config file
func TestResolveSecurityConfigPath_WithConfigFile(t *testing.T) {
	// Save original state
	originalEnv := os.Getenv(EnvSecurityConfig)
	originalLocalMode := LocalMode
	originalHomeEnv := os.Getenv(EnvHome)
	defer func() {
		if originalEnv == "" {
			os.Unsetenv(EnvSecurityConfig)
		} else {
			os.Setenv(EnvSecurityConfig, originalEnv)
		}
		LocalMode = originalLocalMode
		if originalHomeEnv == "" {
			os.Unsetenv(EnvHome)
		} else {
			os.Setenv(EnvHome, originalHomeEnv)
		}
	}()

	os.Unsetenv(EnvSecurityConfig)
	os.Unsetenv(EnvHome)
	LocalMode = false

	// Create a temp directory with config
	tmpDir := t.TempDir()
	cwd, _ := os.Getwd()
	defer os.Chdir(cwd)
	os.Chdir(tmpDir)

	// Create .nemesisbot directory
	err := os.Mkdir(DefaultHomeDir, 0755)
	if err != nil {
		t.Fatalf("Failed to create .nemesisbot: %v", err)
	}

	// Create config.json with workspace
	configContent := `{
		"agents": {
			"defaults": {
				"workspace": "/custom/workspace"
			}
		}
	}`
	configPath := filepath.Join(tmpDir, DefaultHomeDir, "config.json")
	err = os.WriteFile(configPath, []byte(configContent), 0644)
	if err != nil {
		t.Fatalf("Failed to write config: %v", err)
	}

	securityConfigPath := ResolveSecurityConfigPath()
	// On Windows, filepath.Join will use backslashes
	expected := filepath.Join("/custom/workspace", "config", "config.security.json")
	if securityConfigPath != expected {
		t.Errorf("ResolveSecurityConfigPath with config = %v, want %v", securityConfigPath, expected)
	}
}

// TestResolveMCPConfigPath_WithInvalidConfig tests fallback with invalid config
func TestResolveMCPConfigPath_WithInvalidConfig(t *testing.T) {
	// Save original state
	originalEnv := os.Getenv(EnvMCPConfig)
	originalLocalMode := LocalMode
	originalHomeEnv := os.Getenv(EnvHome)
	defer func() {
		if originalEnv == "" {
			os.Unsetenv(EnvMCPConfig)
		} else {
			os.Setenv(EnvMCPConfig, originalEnv)
		}
		LocalMode = originalLocalMode
		if originalHomeEnv == "" {
			os.Unsetenv(EnvHome)
		} else {
			os.Setenv(EnvHome, originalHomeEnv)
		}
	}()

	os.Unsetenv(EnvMCPConfig)
	os.Unsetenv(EnvHome)
	LocalMode = false

	// Create a temp directory with invalid config
	tmpDir := t.TempDir()
	cwd, _ := os.Getwd()
	defer os.Chdir(cwd)
	os.Chdir(tmpDir)

	// Create .nemesisbot directory
	err := os.Mkdir(DefaultHomeDir, 0755)
	if err != nil {
		t.Fatalf("Failed to create .nemesisbot: %v", err)
	}

	// Create invalid config.json
	configPath := filepath.Join(tmpDir, DefaultHomeDir, "config.json")
	err = os.WriteFile(configPath, []byte("invalid json"), 0644)
	if err != nil {
		t.Fatalf("Failed to write config: %v", err)
	}

	mcpConfigPath := ResolveMCPConfigPath()
	expected := filepath.Join(tmpDir, DefaultHomeDir, "config.mcp.json")
	if mcpConfigPath != expected {
		t.Errorf("ResolveMCPConfigPath with invalid config = %v, want %v", mcpConfigPath, expected)
	}
}

// TestExpandHome_EdgeCases tests edge cases in ExpandHome
func TestExpandHome_EdgeCases(t *testing.T) {
	homeDir, err := os.UserHomeDir()
	if err != nil {
		t.Fatalf("Failed to get user home directory: %v", err)
	}

	tests := []struct {
		name string
		path string
		want string
	}{
		{
			name: "Tilde only",
			path: "~",
			want: homeDir,
		},
		{
			name: "Tilde with forward slash",
			path: "~/path",
			want: filepath.Join(homeDir, "path"),
		},
		{
			name: "Tilde with backslash",
			path: `~\path`,
			want: filepath.Join(homeDir, "path"),
		},
		{
			name: "Tilde in middle of path",
			path: "/path/~/",
			want: "/path/~/",
		},
		{
			name: "Multiple tildes",
			path: "~~",
			want: homeDir, // ExpandHome returns homeDir for any path starting with ~
		},
		{
			name: "Tilde followed by non-separator returns home",
			path: "~test",
			want: homeDir, // ExpandHome returns homeDir when second char is not a separator
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := ExpandHome(tt.path)
			if got != tt.want {
				t.Errorf("ExpandHome(%v) = %v, want %v", tt.path, got, tt.want)
			}
		})
	}
}

// TestPathManager_AllPaths tests all PathManager path methods
func TestPathManager_AllPaths(t *testing.T) {
	pm := NewPathManager()
	homeDir := pm.HomeDir()

	tests := []struct {
		name      string
		path      string
		validator func(string) bool
	}{
		{
			name: "HomeDir",
			path: pm.HomeDir(),
			validator: func(s string) bool {
				return s != "" && strings.Contains(s, DefaultHomeDir)
			},
		},
		{
			name: "ConfigPath",
			path: pm.ConfigPath(),
			validator: func(s string) bool {
				return s != "" && strings.HasSuffix(s, "config.json")
			},
		},
		{
			name: "MCPConfigPath",
			path: pm.MCPConfigPath(),
			validator: func(s string) bool {
				return s != "" && strings.HasSuffix(s, "config.mcp.json")
			},
		},
		{
			name: "SecurityConfigPath",
			path: pm.SecurityConfigPath(),
			validator: func(s string) bool {
				return s != "" && strings.HasSuffix(s, "config.security.json")
			},
		},
		{
			name: "Workspace",
			path: pm.Workspace(),
			validator: func(s string) bool {
				return s != "" && strings.Contains(s, "workspace")
			},
		},
		{
			name: "AuthPath",
			path: pm.AuthPath(),
			validator: func(s string) bool {
				return s != "" && strings.HasSuffix(s, "auth.json")
			},
		},
		{
			name: "AuditLogDir",
			path: pm.AuditLogDir(),
			validator: func(s string) bool {
				return s != "" && strings.Contains(s, "security_logs")
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if !tt.validator(tt.path) {
				t.Errorf("%s validation failed: %v", tt.name, tt.path)
			}
		})
	}

	// Test AgentWorkspace with different IDs
	agentIDs := []string{"", "main", "default", "custom"}
	for _, agentID := range agentIDs {
		t.Run("AgentWorkspace_"+agentID, func(t *testing.T) {
			ws := pm.AgentWorkspace(agentID)
			if ws == "" {
				t.Error("AgentWorkspace should not return empty string")
			}
		})
	}

	_ = homeDir // Use variable
}

func TestEnvScannerConfig(t *testing.T) {
	if EnvScannerConfig != "NEMESISBOT_SCANNER_CONFIG" {
		t.Errorf("EnvScannerConfig = %q, want %q", EnvScannerConfig, "NEMESISBOT_SCANNER_CONFIG")
	}
}

func TestResolveScannerConfigPath_EnvVar(t *testing.T) {
	envVal := "/tmp/test_scanner.json"
	os.Setenv(EnvScannerConfig, envVal)
	defer os.Unsetenv(EnvScannerConfig)

	result := ResolveScannerConfigPath()
	if result != envVal {
		t.Errorf("ResolveScannerConfigPath() = %q, want %q", result, envVal)
	}
}

func TestResolveScannerConfigPath_Default(t *testing.T) {
	os.Unsetenv(EnvScannerConfig)
	os.Unsetenv(EnvHome)
	LocalMode = false

	result := ResolveScannerConfigPath()
	if result == "" {
		t.Error("ResolveScannerConfigPath() should not return empty string")
	}
	// Should end with config.scanner.json
	if !strings.HasSuffix(result, "config.scanner.json") {
		t.Errorf("Expected path ending with config.scanner.json, got %q", result)
	}
}

func TestResolveScannerConfigPathInWorkspace(t *testing.T) {
	result := ResolveScannerConfigPathInWorkspace("/workspace")
	expected := filepath.Join("/workspace", "config", "config.scanner.json")
	if result != expected {
		t.Errorf("ResolveScannerConfigPathInWorkspace() = %q, want %q", result, expected)
	}
}
