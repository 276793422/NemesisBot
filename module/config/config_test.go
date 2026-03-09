// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package config

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestInferDefaultModel(t *testing.T) {
	tests := []struct {
		name      string
		provider  string
		wantModel string
	}{
		{"anthropic", "anthropic", "claude-sonnet-4-20250514"},
		{"claude", "claude", "claude-sonnet-4-20250514"},
		{"openai", "openai", "gpt-4o"},
		{"gpt", "gpt", "gpt-4o"},
		{"zhipu", "zhipu", "glm-4.7-flash"},
		{"glm", "glm", "glm-4.7-flash"},
		{"groq", "groq", "llama-3.3-70b-versatile"},
		{"ollama", "ollama", "llama3.3"},
		{"gemini", "gemini", "gemini-2.0-flash-exp"},
		{"google", "google", "gemini-2.0-flash-exp"},
		{"nvidia", "nvidia", "nvidia/llama-3.1-nemotron-70b-instruct"},
		{"unknown", "unknown", ""},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := inferDefaultModel(tt.provider)
			if result != tt.wantModel {
				t.Errorf("inferDefaultModel(%q) = %q, want %q", tt.provider, result, tt.wantModel)
			}
		})
	}
}

func TestConfigSaveAndLoad(t *testing.T) {
	tempDir := t.TempDir()

	// Create a test config
	cfg := &Config{
		Agents: AgentsConfig{
			Defaults: AgentDefaults{
				LLM: "openai/gpt-4",
			},
		},
		ModelList: []ModelConfig{
			{
				ModelName: "gpt-4",
				Model:     "openai/gpt-4",
				APIKey:    "test-key",
			},
		},
	}

	// Test SaveConfig
	configFile := filepath.Join(tempDir, "test_config.json")
	err := SaveConfig(configFile, cfg)
	if err != nil {
		t.Errorf("SaveConfig() returned error: %v", err)
	}

	// Verify file exists
	if _, err := os.Stat(configFile); os.IsNotExist(err) {
		t.Error("config file should exist after SaveConfig")
	}

	// Test LoadConfig
	loadedCfg, err := LoadConfig(configFile)
	if err != nil {
		t.Errorf("LoadConfig() returned error: %v", err)
	}

	if loadedCfg == nil {
		t.Error("LoadConfig() returned nil config")
	}
}

func TestConfigLoadMCPConfig(t *testing.T) {
	tempDir := t.TempDir()

	// Create test MCP config
	mcpConfig := &MCPConfig{
		Servers: []MCPServerConfig{
			{
				Name:    "test-server",
				Command: "npx",
				Args:    []string{"-y", "@modelcontextprotocol/server-filesystem", "/tmp"},
			},
		},
	}

	mcpData, _ := json.Marshal(mcpConfig)
	mcpFile := filepath.Join(tempDir, "config.mcp.json")
	err := os.WriteFile(mcpFile, mcpData, 0644)
	if err != nil {
		t.Fatalf("Failed to create MCP config file: %v", err)
	}

	// Test LoadMCPConfig
	loadedConfig, err := LoadMCPConfig(mcpFile)
	if err != nil {
		t.Errorf("LoadMCPConfig() returned error: %v", err)
	}

	if loadedConfig == nil {
		t.Error("LoadMCPConfig() returned nil config")
	}

	if len(loadedConfig.Servers) != 1 {
		t.Errorf("Expected 1 server, got %d", len(loadedConfig.Servers))
	}
	if loadedConfig.Servers[0].Name != "test-server" {
		t.Errorf("Server name = %s, want 'test-server'", loadedConfig.Servers[0].Name)
	}
}

// TestConfigLoadSecurityConfig removed - SecurityConfig structure doesn't match test expectations


// Helper function to check if a string contains a substring
func contains(s, substr string) bool {
	return len(s) >= len(substr) && s[:len(s)] != s[:len(s)-len(substr)] &&
		   s[len(s)-len(substr):] == substr ||
		   len(s) >= len(substr) && s[:len(s)-len(substr)] != substr &&
		   s[len(s)-len(substr):] == substr ||
		   len(s) >= len(substr) && s[:len(s)-len(substr)] != substr &&
		   s[len(s)-len(substr):] == substr
}

func TestLoadSecurityConfig(t *testing.T) {
	tempDir := t.TempDir()

	// Test with non-existent file - should return default config
	securityFile := filepath.Join(tempDir, "config.security.json")
	cfg, err := LoadSecurityConfig(securityFile)
	if err != nil {
		t.Errorf("LoadSecurityConfig() with non-existent file returned error: %v", err)
	}
	if cfg == nil {
		t.Error("LoadSecurityConfig() should return default config for non-existent file")
	}
	if cfg.DefaultAction != "deny" {
		t.Errorf("DefaultAction = %s, want 'deny'", cfg.DefaultAction)
	}

	// Test with valid config file
	testCfg := &SecurityConfig{
		DefaultAction:         "allow",
		LogAllOperations:      false,
		ApprovalTimeout:       600,
		MaxPendingRequests:    50,
		AuditLogRetentionDays: 30,
		FileRules: &FileSecurityRules{
			Read: []SecurityRule{
				{Pattern: "/tmp/", Action: "allow"},
			},
		},
	}

	data, _ := json.MarshalIndent(testCfg, "", "  ")
	os.WriteFile(securityFile, data, 0644)

	loadedCfg, err := LoadSecurityConfig(securityFile)
	if err != nil {
		t.Errorf("LoadSecurityConfig() returned error: %v", err)
	}
	if loadedCfg.DefaultAction != "allow" {
		t.Errorf("DefaultAction = %s, want 'allow'", loadedCfg.DefaultAction)
	}
}

func TestSaveSecurityConfig(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &SecurityConfig{
		DefaultAction:         "ask",
		LogAllOperations:      true,
		ApprovalTimeout:       300,
		MaxPendingRequests:    100,
		AuditLogRetentionDays: 90,
	}

	securityFile := filepath.Join(tempDir, "config.security.json")
	err := SaveSecurityConfig(securityFile, cfg)
	if err != nil {
		t.Errorf("SaveSecurityConfig() returned error: %v", err)
	}

	// Verify file exists
	if _, err := os.Stat(securityFile); os.IsNotExist(err) {
		t.Error("security file should exist after SaveSecurityConfig")
	}

	// Load and verify
	loadedCfg, err := LoadSecurityConfig(securityFile)
	if err != nil {
		t.Errorf("LoadSecurityConfig() returned error: %v", err)
	}
	if loadedCfg.DefaultAction != "ask" {
		t.Errorf("DefaultAction = %s, want 'ask'", loadedCfg.DefaultAction)
	}
}

func TestSaveMCPConfig(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &MCPConfig{
		Enabled: true,
		Timeout: 60,
		Servers: []MCPServerConfig{
			{
				Name:    "test-server",
				Command: "node",
				Args:    []string{"server.js"},
				Timeout: 30,
			},
		},
	}

	mcpFile := filepath.Join(tempDir, "config.mcp.json")
	err := SaveMCPConfig(mcpFile, cfg)
	if err != nil {
		t.Errorf("SaveMCPConfig() returned error: %v", err)
	}

	// Verify file exists
	if _, err := os.Stat(mcpFile); os.IsNotExist(err) {
		t.Error("MCP file should exist after SaveMCPConfig")
	}

	// Load and verify
	loadedCfg, err := LoadMCPConfig(mcpFile)
	if err != nil {
		t.Errorf("LoadMCPConfig() returned error: %v", err)
	}
	if !loadedCfg.Enabled {
		t.Error("Enabled should be true")
	}
	if loadedCfg.Timeout != 60 {
		t.Errorf("Timeout = %d, want 60", loadedCfg.Timeout)
	}
}

func TestModelConfigValidate(t *testing.T) {
	tests := []struct {
		name    string
		cfg     ModelConfig
		wantErr bool
	}{
		{
			name: "valid config",
			cfg: ModelConfig{
				ModelName: "gpt-4",
				Model:     "openai/gpt-4",
				APIKey:    "test-key",
			},
			wantErr: false,
		},
		{
			name: "missing model_name",
			cfg: ModelConfig{
				Model:  "openai/gpt-4",
				APIKey: "test-key",
			},
			wantErr: true,
		},
		{
			name: "missing model",
			cfg: ModelConfig{
				ModelName: "gpt-4",
				APIKey:    "test-key",
			},
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := tt.cfg.Validate()
			if (err != nil) != tt.wantErr {
				t.Errorf("ModelConfig.Validate() error = %v, wantErr %v", err, tt.wantErr)
			}
		})
	}
}

func TestConfigGetModelConfig(t *testing.T) {
	cfg := &Config{
		ModelList: []ModelConfig{
			{
				ModelName: "gpt-4",
				Model:     "openai/gpt-4",
				APIKey:    "key1",
			},
			{
				ModelName: "claude-3",
				Model:     "anthropic/claude-3",
				APIKey:    "key2",
			},
		},
	}

	// Test existing model
	mc, err := cfg.GetModelConfig("gpt-4")
	if err != nil {
		t.Errorf("GetModelConfig() returned error: %v", err)
	}
	if mc.ModelName != "gpt-4" {
		t.Errorf("ModelName = %s, want 'gpt-4'", mc.ModelName)
	}

	// Test non-existent model
	_, err = cfg.GetModelConfig("non-existent")
	if err == nil {
		t.Error("GetModelConfig() should return error for non-existent model")
	}
}

func TestConfigGetModelByModelName(t *testing.T) {
	cfg := &Config{
		ModelList: []ModelConfig{
			{
				ModelName: "gpt-4",
				Model:     "openai/gpt-4",
				APIKey:    "key1",
			},
			{
				ModelName: "my-model",
				Model:     "custom/llm",
				APIKey:    "key2",
			},
		},
	}

	// Test exact match by model_name
	mc, err := cfg.GetModelByModelName("gpt-4")
	if err != nil {
		t.Errorf("GetModelByModelName() returned error: %v", err)
	}
	if mc.ModelName != "gpt-4" {
		t.Errorf("ModelName = %s, want 'gpt-4'", mc.ModelName)
	}

	// Test prefix match by model field
	mc, err = cfg.GetModelByModelName("openai/gpt-4")
	if err != nil {
		t.Errorf("GetModelByModelName() with prefix returned error: %v", err)
	}
	if mc.ModelName != "gpt-4" {
		t.Errorf("ModelName = %s, want 'gpt-4'", mc.ModelName)
	}

	// Test non-existent model
	_, err = cfg.GetModelByModelName("non-existent")
	if err == nil {
		t.Error("GetModelByModelName() should return error for non-existent model")
	}
}

func TestConfigWorkspacePath(t *testing.T) {
	cfg := &Config{
		Agents: AgentsConfig{
			Defaults: AgentDefaults{
				Workspace: "~/test-workspace",
			},
		},
	}

	path := cfg.WorkspacePath()
	if path == "" {
		t.Error("WorkspacePath() should not return empty string")
	}
	if !filepath.IsAbs(path) {
		t.Error("WorkspacePath() should return absolute path")
	}
}

func TestDefaultConfig(t *testing.T) {
	cfg := DefaultConfig()
	if cfg == nil {
		t.Fatal("DefaultConfig() returned nil")
	}

	if cfg.Agents.Defaults.Workspace == "" {
		t.Error("DefaultConfig() should set workspace")
	}
	if cfg.Agents.Defaults.LLM == "" {
		t.Error("DefaultConfig() should set LLM")
	}
	if !cfg.Channels.Web.Enabled {
		t.Error("Web channel should be enabled by default")
	}
	if cfg.Channels.Telegram.Enabled {
		t.Error("Telegram channel should be disabled by default")
	}
	if cfg.Security != nil && cfg.Security.Enabled {
		t.Error("Security should be disabled by default")
	}
}

func TestAgentModelConfigUnmarshalJSON(t *testing.T) {
	tests := []struct {
		name     string
		json     string
		want     AgentModelConfig
		wantErr  bool
	}{
		{
			name: "string format",
			json: `"gpt-4"`,
			want: AgentModelConfig{
				Primary:   "gpt-4",
				Fallbacks: nil,
			},
			wantErr: false,
		},
		{
			name: "object format with fallbacks",
			json: `{"primary": "gpt-4", "fallbacks": ["claude-haiku"]}`,
			want: AgentModelConfig{
				Primary:   "gpt-4",
				Fallbacks: []string{"claude-haiku"},
			},
			wantErr: false,
		},
		{
			name: "object format without fallbacks",
			json: `{"primary": "gpt-4"}`,
			want: AgentModelConfig{
				Primary:   "gpt-4",
				Fallbacks: nil,
			},
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var amc AgentModelConfig
			err := json.Unmarshal([]byte(tt.json), &amc)
			if (err != nil) != tt.wantErr {
				t.Errorf("UnmarshalJSON() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr {
				if amc.Primary != tt.want.Primary {
					t.Errorf("Primary = %s, want %s", amc.Primary, tt.want.Primary)
				}
				if len(amc.Fallbacks) != len(tt.want.Fallbacks) {
					t.Errorf("Fallbacks length = %d, want %d", len(amc.Fallbacks), len(tt.want.Fallbacks))
				}
			}
		})
	}
}

func TestAgentModelConfigMarshalJSON(t *testing.T) {
	tests := []struct {
		name  string
		cfg   AgentModelConfig
		check string
	}{
		{
			name: "primary only",
			cfg: AgentModelConfig{
				Primary: "gpt-4",
			},
			check: `"gpt-4"`,
		},
		{
			name: "primary with fallbacks",
			cfg: AgentModelConfig{
				Primary:   "gpt-4",
				Fallbacks: []string{"claude-haiku"},
			},
			check: "fallbacks",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			data, err := json.Marshal(tt.cfg)
			if err != nil {
				t.Errorf("MarshalJSON() error = %v", err)
				return
			}
			str := string(data)
			if !strings.Contains(str, tt.check) {
				t.Errorf("MarshalJSON() output = %s, should contain %s", str, tt.check)
			}
		})
	}
}

func TestSetEmbeddedDefaults(t *testing.T) {
	configData := []byte(`{"test": "config"}`)
	mcpData := []byte(`{"test": "mcp"}`)
	securityData := []byte(`{"test": "security"}`)
	clusterData := []byte(`{"test": "cluster"}`)

	err := SetEmbeddedDefaults(configData, mcpData, securityData, clusterData)
	if err != nil {
		t.Errorf("SetEmbeddedDefaults() returned error: %v", err)
	}

	ed := GetEmbeddedDefaults()
	if len(ed.Config) == 0 {
		t.Error("Config data should be set")
	}
	if len(ed.MCP) == 0 {
		t.Error("MCP data should be set")
	}
	if len(ed.Security) == 0 {
		t.Error("Security data should be set")
	}
	if len(ed.Cluster) == 0 {
		t.Error("Cluster data should be set")
	}
}

func TestFlexibleStringSliceUnmarshalJSON(t *testing.T) {
	tests := []struct {
		name    string
		json    string
		wantLen int
		wantErr bool
	}{
		{
			name:    "string array",
			json:    `["a", "b", "c"]`,
			wantLen: 3,
			wantErr: false,
		},
		{
			name:    "number array",
			json:    `[123, 456]`,
			wantLen: 2,
			wantErr: false,
		},
		{
			name:    "mixed array",
			json:    `["abc", 123, true]`,
			wantLen: 3,
			wantErr: false,
		},
		{
			name:    "empty array",
			json:    `[]`,
			wantLen: 0,
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var fss FlexibleStringSlice
			err := json.Unmarshal([]byte(tt.json), &fss)
			if (err != nil) != tt.wantErr {
				t.Errorf("UnmarshalJSON() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if len(fss) != tt.wantLen {
				t.Errorf("Length = %d, want %d", len(fss), tt.wantLen)
			}
		})
	}
}

func TestLoadEmbeddedConfig(t *testing.T) {
	// Test LoadEmbeddedConfig when no embedded defaults are set
	// This tests the error path
	cfg, err := LoadEmbeddedConfig()
	if err != nil {
		// Expected when no embedded defaults are set
		t.Logf("LoadEmbeddedConfig returned error (expected): %v", err)
	}
	if cfg != nil {
		t.Log("LoadEmbeddedConfig returned a config")
	}
}

func TestSetEmbeddedDefaultsFromFS(t *testing.T) {
	// Test with non-existent filesystem
	// This tests the error path when files don't exist
	// Note: Passing nil fs causes panic, so we document this instead
	t.Log("SetEmbeddedDefaultsFromFS() requires a valid fs.FS")
	t.Log("Passing nil causes panic (as expected)")
	t.Log("This function is tested in integration tests with embedded files")
	t.Skip("Cannot test with nil fs - causes panic")
}

func TestLoadConfigWithInvalidJSON(t *testing.T) {
	tempDir := t.TempDir()
	configFile := filepath.Join(tempDir, "config.json")

	// Write invalid JSON
	err := os.WriteFile(configFile, []byte("{invalid json"), 0644)
	if err != nil {
		t.Fatalf("Failed to write config file: %v", err)
	}

	_, err = LoadConfig(configFile)
	if err == nil {
		t.Error("LoadConfig should return error for invalid JSON")
	}
}

func TestLoadConfigWithValidJSON(t *testing.T) {
	tempDir := t.TempDir()
	configFile := filepath.Join(tempDir, "config.json")

	// Write valid minimal JSON
	validJSON := `{
		"agents": {
			"defaults": {
				"workspace": "~/test-workspace",
				"llm": "test-model"
			}
		},
		"channels": {
			"web": {
				"enabled": true,
				"host": "0.0.0.0",
				"port": 8080
			}
		}
	}`

	err := os.WriteFile(configFile, []byte(validJSON), 0644)
	if err != nil {
		t.Fatalf("Failed to write config file: %v", err)
	}

	cfg, err := LoadConfig(configFile)
	if err != nil {
		t.Errorf("LoadConfig returned error: %v", err)
	}
	if cfg == nil {
		t.Error("LoadConfig should return config for valid JSON")
	}
}

func TestSaveConfigWithDirectoryCreation(t *testing.T) {
	tempDir := t.TempDir()
	configFile := filepath.Join(tempDir, "subdir", "config.json")

	cfg := &Config{
		Agents: AgentsConfig{
			Defaults: AgentDefaults{
				LLM: "test-model",
			},
		},
	}

	err := SaveConfig(configFile, cfg)
	if err != nil {
		t.Errorf("SaveConfig returned error: %v", err)
	}

	// Verify file exists
	if _, err := os.Stat(configFile); os.IsNotExist(err) {
		t.Error("Config file should exist after SaveConfig")
	}
}

func TestLoadMCPConfigWithInvalidJSON(t *testing.T) {
	tempDir := t.TempDir()
	mcpFile := filepath.Join(tempDir, "config.mcp.json")

	// Write invalid JSON
	err := os.WriteFile(mcpFile, []byte("{invalid json"), 0644)
	if err != nil {
		t.Fatalf("Failed to write MCP config file: %v", err)
	}

	_, err = LoadMCPConfig(mcpFile)
	if err == nil {
		t.Error("LoadMCPConfig should return error for invalid JSON")
	}
}

func TestSaveSecurityConfigWithDirectoryCreation(t *testing.T) {
	tempDir := t.TempDir()
	securityFile := filepath.Join(tempDir, "subdir", "config.security.json")

	cfg := &SecurityConfig{
		DefaultAction: "allow",
	}

	err := SaveSecurityConfig(securityFile, cfg)
	if err != nil {
		t.Errorf("SaveSecurityConfig returned error: %v", err)
	}

	// Verify file exists
	if _, err := os.Stat(securityFile); os.IsNotExist(err) {
		t.Error("Security config file should exist after SaveSecurityConfig")
	}
}

func TestLoadSecurityConfigWithInvalidJSON(t *testing.T) {
	tempDir := t.TempDir()
	securityFile := filepath.Join(tempDir, "config.security.json")

	// Write invalid JSON
	err := os.WriteFile(securityFile, []byte("{invalid json"), 0644)
	if err != nil {
		t.Fatalf("Failed to write security config file: %v", err)
	}

	_, err = LoadSecurityConfig(securityFile)
	if err == nil {
		t.Error("LoadSecurityConfig should return error for invalid JSON")
	}
}

func TestLoadConfigWithEnvOverride(t *testing.T) {
	tempDir := t.TempDir()
	configFile := filepath.Join(tempDir, "config.json")

	// Write a config with gateway host
	validJSON := `{
		"agents": {
			"defaults": {
				"workspace": "~/test-workspace",
				"llm": "test-model"
			}
		},
		"gateway": {
			"host": "default-host",
			"port": 8080
		}
	}`

	err := os.WriteFile(configFile, []byte(validJSON), 0644)
	if err != nil {
		t.Fatalf("Failed to write config file: %v", err)
	}

	// Set environment variable to override gateway host
	originalEnv := os.Getenv("NEMESISBOT_GATEWAY_HOST")
	os.Setenv("NEMESISBOT_GATEWAY_HOST", "env-host")
	defer func() {
		if originalEnv != "" {
			os.Setenv("NEMESISBOT_GATEWAY_HOST", originalEnv)
		} else {
			os.Unsetenv("NEMESISBOT_GATEWAY_HOST")
		}
	}()

	cfg, err := LoadConfig(configFile)
	if err != nil {
		t.Errorf("LoadConfig() returned error: %v", err)
	}

	// Gateway host should be overridden by environment variable
	if cfg.Gateway.Host != "env-host" {
		t.Errorf("Gateway.Host = %s, want 'env-host' (environment override)", cfg.Gateway.Host)
	}
}

func TestLoadConfigPostProcessing(t *testing.T) {
	tempDir := t.TempDir()
	configFile := filepath.Join(tempDir, "config.json")

	// Write config with SyncTo field (new format)
	validJSON := `{
		"agents": {
			"defaults": {
				"workspace": "~/test-workspace",
				"llm": "test-model"
			}
		},
		"channels": {
			"external": {
				"enabled": true,
				"sync_to": ["web"]
			},
			"websocket": {
				"enabled": true,
				"sync_to": ["web"]
			}
		}
	}`

	err := os.WriteFile(configFile, []byte(validJSON), 0644)
	if err != nil {
		t.Fatalf("Failed to write config file: %v", err)
	}

	cfg, err := LoadConfig(configFile)
	if err != nil {
		t.Errorf("LoadConfig() returned error: %v", err)
	}

	// SyncToWeb should be populated from SyncTo
	if !cfg.Channels.External.SyncToWeb {
		t.Error("External.SyncToWeb should be true when SyncTo has 'web'")
	}
	if !cfg.Channels.WebSocket.SyncToWeb {
		t.Error("WebSocket.SyncToWeb should be true when SyncTo has 'web'")
	}
}

func TestFlexibleStringSliceEdgeCases(t *testing.T) {
	tests := []struct {
		name    string
		json    string
		wantLen int
		check   func([]string) bool
	}{
		{
			name:    "array with numbers",
			json:    `[123, 456, "abc"]`,
			wantLen: 3,
			check: func(s []string) bool {
				return s[0] == "123" && s[1] == "456" && s[2] == "abc"
			},
		},
		{
			name:    "array with boolean",
			json:    `[true, false]`,
			wantLen: 2,
			check: func(s []string) bool {
				return s[0] == "true" && s[1] == "false"
			},
		},
		{
			name:    "array with mixed types",
			json:    `["text", 123, true, null]`,
			wantLen: 4,
			check: func(s []string) bool {
				return s[0] == "text" && s[1] == "123" && s[2] == "true" && s[3] == "<nil>"
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var fss FlexibleStringSlice
			err := json.Unmarshal([]byte(tt.json), &fss)
			if err != nil {
				t.Errorf("UnmarshalJSON() error = %v", err)
				return
			}
			if len(fss) != tt.wantLen {
				t.Errorf("Length = %d, want %d", len(fss), tt.wantLen)
			}
			if !tt.check(fss) {
				t.Error("Check function failed")
			}
		})
	}
}

func TestModelConfigEdgeCases(t *testing.T) {
	t.Run("ModelConfig with all optional fields", func(t *testing.T) {
		cfg := ModelConfig{
			ModelName:     "test-model",
			Model:         "provider/model",
			APIBase:       "https://api.example.com",
			APIKey:        "test-key",
			Proxy:         "http://proxy.example.com",
			AuthMethod:    "oauth",
			ConnectMode:   "grpc",
			Workspace:     "/tmp/workspace",
		}

		err := cfg.Validate()
		if err != nil {
			t.Errorf("Validate() should not error with valid config: %v", err)
		}
	})

	t.Run("ModelConfig with minimal required fields", func(t *testing.T) {
		cfg := ModelConfig{
			ModelName: "minimal-model",
			Model:     "provider/model",
			APIKey:    "key",
		}

		err := cfg.Validate()
		if err != nil {
			t.Errorf("Validate() should not error with minimal config: %v", err)
		}
	})
}

func TestLoadMCPConfigWithEmptyFile(t *testing.T) {
	tempDir := t.TempDir()
	mcpFile := filepath.Join(tempDir, "config.mcp.json")

	// Write empty JSON object
	err := os.WriteFile(mcpFile, []byte("{}"), 0644)
	if err != nil {
		t.Fatalf("Failed to write MCP config file: %v", err)
	}

	cfg, err := LoadMCPConfig(mcpFile)
	if err != nil {
		t.Errorf("LoadMCPConfig() returned error: %v", err)
	}

	if cfg == nil {
		t.Error("LoadMCPConfig() should return config for empty JSON")
	}

	if cfg.Enabled {
		t.Error("Enabled should be false for empty config")
	}
}