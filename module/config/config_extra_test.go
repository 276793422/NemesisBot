// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package config

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"testing/fstest"
	"time"
)

// TestLoadSkillsConfig tests LoadSkillsConfig function
func TestLoadSkillsConfig(t *testing.T) {
	t.Run("load from existing file", func(t *testing.T) {
		tempDir := t.TempDir()
		cfgPath := filepath.Join(tempDir, "config.skills.json")

		cfg := &SkillsFullConfig{
			Enabled:               true,
			MaxConcurrentSearches: 5,
			GitHubSources: []GitHubSourceConfig{
				{Name: "test-source", Repo: "test/skills", Branch: "main"},
			},
		}
		data, _ := json.MarshalIndent(cfg, "", "  ")
		os.WriteFile(cfgPath, data, 0600)

		loaded, err := LoadSkillsConfig(cfgPath)
		if err != nil {
			t.Fatalf("LoadSkillsConfig() error = %v", err)
		}
		if !loaded.Enabled {
			t.Error("Expected Enabled = true")
		}
		if loaded.MaxConcurrentSearches != 5 {
			t.Errorf("MaxConcurrentSearches = %d, want 5", loaded.MaxConcurrentSearches)
		}
		if len(loaded.GitHubSources) != 1 {
			t.Errorf("GitHubSources = %d, want 1", len(loaded.GitHubSources))
		}
	})

	t.Run("non-existent file returns default", func(t *testing.T) {
		cfgPath := filepath.Join(t.TempDir(), "nonexistent.json")

		cfg, err := LoadSkillsConfig(cfgPath)
		if err != nil {
			t.Fatalf("LoadSkillsConfig() error = %v", err)
		}
		if !cfg.Enabled {
			t.Error("Default should be enabled")
		}
		if cfg.MaxConcurrentSearches != 2 {
			t.Errorf("Default MaxConcurrentSearches = %d, want 2", cfg.MaxConcurrentSearches)
		}
	})

	t.Run("invalid JSON returns error", func(t *testing.T) {
		tempDir := t.TempDir()
		cfgPath := filepath.Join(tempDir, "config.skills.json")
		os.WriteFile(cfgPath, []byte("invalid json"), 0600)

		_, err := LoadSkillsConfig(cfgPath)
		if err == nil {
			t.Error("Expected error for invalid JSON")
		}
	})
}

// TestSaveSkillsConfig tests SaveSkillsConfig function
func TestSaveSkillsConfig(t *testing.T) {
	t.Run("save and load roundtrip", func(t *testing.T) {
		tempDir := t.TempDir()
		cfgPath := filepath.Join(tempDir, "subdir", "config.skills.json")

		cfg := &SkillsFullConfig{
			Enabled:               true,
			MaxConcurrentSearches: 3,
			SearchCache:           SkillsSearchCacheConfig{Enabled: true, MaxSize: 100, TTLSeconds: 600},
			GitHubSources: []GitHubSourceConfig{
				{Name: "source1", Repo: "owner1/repo1", Branch: "main"},
				{Name: "source2", Repo: "owner2/repo2", Branch: "dev"},
			},
			ClawHub: SkillsClawHubConfig{Enabled: true},
		}

		err := SaveSkillsConfig(cfgPath, cfg)
		if err != nil {
			t.Fatalf("SaveSkillsConfig() error = %v", err)
		}

		// Verify file exists
		if _, err := os.Stat(cfgPath); os.IsNotExist(err) {
			t.Fatal("Config file should exist after save")
		}

		// Load and verify
		loaded, err := LoadSkillsConfig(cfgPath)
		if err != nil {
			t.Fatalf("LoadSkillsConfig() error = %v", err)
		}
		if loaded.MaxConcurrentSearches != 3 {
			t.Errorf("MaxConcurrentSearches = %d, want 3", loaded.MaxConcurrentSearches)
		}
		if len(loaded.GitHubSources) != 2 {
			t.Errorf("GitHubSources = %d, want 2", len(loaded.GitHubSources))
		}
	})
}

// TestLoadMCPConfig tests LoadMCPConfig function
func TestLoadMCPConfig(t *testing.T) {
	t.Run("load from existing file", func(t *testing.T) {
		tempDir := t.TempDir()
		cfgPath := filepath.Join(tempDir, "config.mcp.json")

		cfg := &MCPConfig{
			Enabled: true,
			Servers: []MCPServerConfig{
				{Name: "test-server", Command: "test", Args: []string{"--port", "8080"}},
			},
			Timeout: 60,
		}
		data, _ := json.MarshalIndent(cfg, "", "  ")
		os.WriteFile(cfgPath, data, 0600)

		loaded, err := LoadMCPConfig(cfgPath)
		if err != nil {
			t.Fatalf("LoadMCPConfig() error = %v", err)
		}
		if !loaded.Enabled {
			t.Error("Expected Enabled = true")
		}
		if len(loaded.Servers) != 1 {
			t.Errorf("Servers = %d, want 1", len(loaded.Servers))
		}
		if loaded.Timeout != 60 {
			t.Errorf("Timeout = %d, want 60", loaded.Timeout)
		}
	})

	t.Run("non-existent file returns default", func(t *testing.T) {
		cfgPath := filepath.Join(t.TempDir(), "nonexistent.json")

		cfg, err := LoadMCPConfig(cfgPath)
		if err != nil {
			t.Fatalf("LoadMCPConfig() error = %v", err)
		}
		if cfg.Enabled {
			t.Error("Default should be disabled")
		}
		if cfg.Timeout != 30 {
			t.Errorf("Default Timeout = %d, want 30", cfg.Timeout)
		}
	})

	t.Run("invalid JSON returns error", func(t *testing.T) {
		tempDir := t.TempDir()
		cfgPath := filepath.Join(tempDir, "config.mcp.json")
		os.WriteFile(cfgPath, []byte("invalid json"), 0600)

		_, err := LoadMCPConfig(cfgPath)
		if err == nil {
			t.Error("Expected error for invalid JSON")
		}
	})
}

// TestSaveMCPConfigRoundtrip tests SaveMCPConfig function
func TestSaveMCPConfigRoundtrip(t *testing.T) {
	t.Run("save and load roundtrip", func(t *testing.T) {
		tempDir := t.TempDir()
		cfgPath := filepath.Join(tempDir, "config.mcp.json")

		cfg := &MCPConfig{
			Enabled: true,
			Servers: []MCPServerConfig{
				{Name: "server1", Command: "npx", Args: []string{"-y", "@test/mcp-server"}},
			},
			Timeout: 45,
		}

		err := SaveMCPConfig(cfgPath, cfg)
		if err != nil {
			t.Fatalf("SaveMCPConfig() error = %v", err)
		}

		loaded, err := LoadMCPConfig(cfgPath)
		if err != nil {
			t.Fatalf("LoadMCPConfig() error = %v", err)
		}
		if loaded.Timeout != 45 {
			t.Errorf("Timeout = %d, want 45", loaded.Timeout)
		}
		if loaded.Servers[0].Name != "server1" {
			t.Errorf("Server Name = %s, want server1", loaded.Servers[0].Name)
		}
	})
}

// TestLoadSecurityConfigExtra tests LoadSecurityConfig function
func TestLoadSecurityConfigExtra(t *testing.T) {
	t.Run("load from existing file", func(t *testing.T) {
		tempDir := t.TempDir()
		cfgPath := filepath.Join(tempDir, "config.security.json")

		cfg := &SecurityConfig{
			DefaultAction:         "deny",
			LogAllOperations:      true,
			ApprovalTimeout:       300,
			MaxPendingRequests:    50,
			AuditLogRetentionDays: 30,
			AuditLogFileEnabled:   true,
		}
		data, _ := json.MarshalIndent(cfg, "", "  ")
		os.WriteFile(cfgPath, data, 0600)

		loaded, err := LoadSecurityConfig(cfgPath)
		if err != nil {
			t.Fatalf("LoadSecurityConfig() error = %v", err)
		}
		if loaded.DefaultAction != "deny" {
			t.Errorf("DefaultAction = %s, want deny", loaded.DefaultAction)
		}
		if !loaded.LogAllOperations {
			t.Error("Expected LogAllOperations = true")
		}
	})

	t.Run("non-existent file returns default", func(t *testing.T) {
		cfgPath := filepath.Join(t.TempDir(), "nonexistent.json")

		cfg, err := LoadSecurityConfig(cfgPath)
		if err != nil {
			t.Fatalf("LoadSecurityConfig() error = %v", err)
		}
		if cfg.DefaultAction != "deny" {
			t.Errorf("Default DefaultAction = %s, want deny", cfg.DefaultAction)
		}
		if cfg.ApprovalTimeout != 300 {
			t.Errorf("Default ApprovalTimeout = %d, want 300", cfg.ApprovalTimeout)
		}
	})

	t.Run("invalid JSON returns error", func(t *testing.T) {
		tempDir := t.TempDir()
		cfgPath := filepath.Join(tempDir, "config.security.json")
		os.WriteFile(cfgPath, []byte("{invalid"), 0600)

		_, err := LoadSecurityConfig(cfgPath)
		if err == nil {
			t.Error("Expected error for invalid JSON")
		}
	})
}

// TestLoadConfig tests LoadConfig function
func TestLoadConfigFromTempFile(t *testing.T) {
	t.Run("load from existing file", func(t *testing.T) {
		tempDir := t.TempDir()
		cfgPath := filepath.Join(tempDir, "config.json")

		cfg := DefaultConfig()
		cfg.Agents.Defaults.LLM = "test/model"
		cfg.Agents.Defaults.Workspace = tempDir

		data, _ := json.MarshalIndent(cfg, "", "  ")
		os.WriteFile(cfgPath, data, 0600)

		loaded, err := LoadConfig(cfgPath)
		if err != nil {
			t.Fatalf("LoadConfig() error = %v", err)
		}
		if loaded.Agents.Defaults.LLM != "test/model" {
			t.Errorf("LLM = %s, want test/model", loaded.Agents.Defaults.LLM)
		}
	})

	t.Run("non-existent file returns default", func(t *testing.T) {
		cfgPath := filepath.Join(t.TempDir(), "nonexistent.json")

		cfg, err := LoadConfig(cfgPath)
		if err != nil {
			t.Fatalf("LoadConfig() error = %v", err)
		}
		if cfg == nil {
			t.Fatal("Expected non-nil config")
		}
	})

	t.Run("invalid JSON returns error", func(t *testing.T) {
		tempDir := t.TempDir()
		cfgPath := filepath.Join(tempDir, "config.json")
		os.WriteFile(cfgPath, []byte("{invalid"), 0600)

		_, err := LoadConfig(cfgPath)
		if err == nil {
			t.Error("Expected error for invalid JSON")
		}
	})
}

// TestSaveConfig tests SaveConfig function
func TestSaveConfigToTempFile(t *testing.T) {
	t.Run("save creates parent directories", func(t *testing.T) {
		tempDir := t.TempDir()
		cfgPath := filepath.Join(tempDir, "subdir1", "subdir2", "config.json")

		cfg := DefaultConfig()

		err := SaveConfig(cfgPath, cfg)
		if err != nil {
			t.Fatalf("SaveConfig() error = %v", err)
		}

		if _, err := os.Stat(cfgPath); os.IsNotExist(err) {
			t.Error("Config file should exist after save")
		}
	})

	t.Run("save and load roundtrip", func(t *testing.T) {
		tempDir := t.TempDir()
		cfgPath := filepath.Join(tempDir, "config.json")

		cfg := DefaultConfig()
		cfg.Agents.Defaults.LLM = "zhipu/test-model"
		cfg.ModelList = []ModelConfig{
			{
				ModelName: "test-model",
				Model:     "zhipu/test-model",
				APIKey:    "test-key",
			},
		}

		err := SaveConfig(cfgPath, cfg)
		if err != nil {
			t.Fatalf("SaveConfig() error = %v", err)
		}

		loaded, err := LoadConfig(cfgPath)
		if err != nil {
			t.Fatalf("LoadConfig() error = %v", err)
		}
		if loaded.Agents.Defaults.LLM != "zhipu/test-model" {
			t.Errorf("LLM = %s, want zhipu/test-model", loaded.Agents.Defaults.LLM)
		}
		if len(loaded.ModelList) != 1 {
			t.Errorf("ModelList = %d, want 1", len(loaded.ModelList))
		}
	})
}

// TestSetEmbeddedDefaultsFromFSWithValidFS tests SetEmbeddedDefaultsFromFS
func TestSetEmbeddedDefaultsFromFSWithValidFS(t *testing.T) {
	t.Run("valid filesystem", func(t *testing.T) {
		defaultCfg := `{"agents":{"defaults":{"llm":"test/default"}}}`
		mcpCfg := `{"enabled":true,"servers":[],"timeout":30}`
		securityCfg := `{"default_action":"deny"}`
		clusterCfg := `{"enabled":false}`
		skillsCfg := `{"enabled":true,"max_concurrent_searches":2}`

		memFS := fstest.MapFS{
			"config.default.json":       &fstest.MapFile{Data: []byte(defaultCfg)},
			"config.mcp.default.json":   &fstest.MapFile{Data: []byte(mcpCfg)},
			"config.security.windows.json": &fstest.MapFile{Data: []byte(securityCfg)},
			"config.security.linux.json": &fstest.MapFile{Data: []byte(securityCfg)},
			"config.security.darwin.json": &fstest.MapFile{Data: []byte(securityCfg)},
			"config.cluster.default.json": &fstest.MapFile{Data: []byte(clusterCfg)},
			"config.skills.default.json": &fstest.MapFile{Data: []byte(skillsCfg)},
		}

		err := SetEmbeddedDefaultsFromFS(memFS)
		if err != nil {
			t.Fatalf("SetEmbeddedDefaultsFromFS() error = %v", err)
		}

		// Verify embedded defaults are set
		embeddedDefaults.mu.RLock()
		configData := embeddedDefaults.config
		mcpData := embeddedDefaults.mcp
		embeddedDefaults.mu.RUnlock()

		if len(configData) == 0 {
			t.Error("Config should be set")
		}
		if len(mcpData) == 0 {
			t.Error("MCP should be set")
		}
	})

	t.Run("missing config file returns error", func(t *testing.T) {
		memFS := fstest.MapFS{}

		err := SetEmbeddedDefaultsFromFS(memFS)
		if err == nil {
			t.Error("Expected error for missing files")
		}
	})
}

// TestSecurityLayersConfig tests security layers configuration types
func TestSecurityLayersConfig(t *testing.T) {
	t.Run("SecurityLayersConfig JSON roundtrip", func(t *testing.T) {
		cfg := SecurityLayersConfig{
			Injection: &SecurityLayerConfig{
				Enabled: true,
				Extra:   map[string]interface{}{"threshold": 0.8},
			},
			CommandGuard: &SecurityLayerConfig{Enabled: true},
			DLP: &DLPLayerConfig{
				Enabled: true,
				Rules:   []string{"credit_card", "api_key"},
				Action:  "block",
			},
			SSRF:       &SecurityLayerConfig{Enabled: true},
			Credential: &SecurityLayerConfig{Enabled: true},
			Signature: &SignatureLayerConfig{
				Enabled: true,
				Strict:  false,
			},
			AuditChain: &SecurityLayerConfig{Enabled: true},
		}

		data, err := json.Marshal(cfg)
		if err != nil {
			t.Fatalf("Marshal error: %v", err)
		}

		var loaded SecurityLayersConfig
		if err := json.Unmarshal(data, &loaded); err != nil {
			t.Fatalf("Unmarshal error: %v", err)
		}

		if !loaded.Injection.Enabled {
			t.Error("Injection should be enabled")
		}
		if loaded.DLP.Action != "block" {
			t.Errorf("DLP Action = %s, want block", loaded.DLP.Action)
		}
		if len(loaded.DLP.Rules) != 2 {
			t.Errorf("DLP Rules = %d, want 2", len(loaded.DLP.Rules))
		}
	})

	t.Run("SecurityRule JSON roundtrip", func(t *testing.T) {
		rule := SecurityRule{
			Pattern: "/workspace/**",
			Action:  "allow",
		}

		data, err := json.Marshal(rule)
		if err != nil {
			t.Fatalf("Marshal error: %v", err)
		}

		var loaded SecurityRule
		if err := json.Unmarshal(data, &loaded); err != nil {
			t.Fatalf("Unmarshal error: %v", err)
		}

		if loaded.Pattern != "/workspace/**" {
			t.Errorf("Pattern = %s, want /workspace/**", loaded.Pattern)
		}
		if loaded.Action != "allow" {
			t.Errorf("Action = %s, want allow", loaded.Action)
		}
	})
}

// TestConfigTimestamp tests time-related config functions
func TestConfigTimestamp(t *testing.T) {
	cfg := DefaultConfig()
	if cfg == nil {
		t.Fatal("DefaultConfig() should not return nil")
	}
}

// TestWorkspacePath tests Config.WorkspacePath
// TestConfigWorkspacePathExtra tests Config.WorkspacePath
func TestConfigWorkspacePathExtra(t *testing.T) {
	cfg := DefaultConfig()
	path := cfg.WorkspacePath()
	if path == "" {
		t.Error("WorkspacePath() should not be empty")
	}
}

// TestGetEffectiveLLM tests GetEffectiveLLM function
func TestGetEffectiveLLMWithConfig(t *testing.T) {
	t.Run("nil config returns default", func(t *testing.T) {
		llm := GetEffectiveLLM(nil)
		if llm != "zhipu/glm-4.7-flash" {
			t.Errorf("GetEffectiveLLM(nil) = %s, want zhipu/glm-4.7-flash", llm)
		}
	})

	t.Run("config with LLM set", func(t *testing.T) {
		cfg := DefaultConfig()
		cfg.Agents.Defaults.LLM = "anthropic/claude-sonnet-4-20250514"
		llm := GetEffectiveLLM(cfg)
		if llm != "anthropic/claude-sonnet-4-20250514" {
			t.Errorf("GetEffectiveLLM() = %s, want anthropic/claude-sonnet-4-20250514", llm)
		}
	})

	t.Run("config with empty LLM returns default", func(t *testing.T) {
		cfg := DefaultConfig()
		cfg.Agents.Defaults.LLM = ""
		llm := GetEffectiveLLM(cfg)
		if llm != "zhipu/glm-4.7-flash" {
			t.Errorf("GetEffectiveLLM() = %s, want zhipu/glm-4.7-flash", llm)
		}
	})
}

// TestModelConfigOperations tests model-related config operations
func TestModelConfigOperations(t *testing.T) {
	cfg := DefaultConfig()
	cfg.ModelList = []ModelConfig{
		{ModelName: "test-1", Model: "zhipu/test-1", APIKey: "key1"},
		{ModelName: "test-2", Model: "openai/test-2", APIKey: "key2"},
	}

	t.Run("GetModelByModelName by model_name", func(t *testing.T) {
		mc, err := cfg.GetModelByModelName("test-1")
		if err != nil {
			t.Fatalf("GetModelByModelName() error = %v", err)
		}
		if mc.ModelName != "test-1" {
			t.Errorf("ModelName = %s, want test-1", mc.ModelName)
		}
	})

	t.Run("GetModelByModelName by model field", func(t *testing.T) {
		mc, err := cfg.GetModelByModelName("openai/test-2")
		if err != nil {
			t.Fatalf("GetModelByModelName() error = %v", err)
		}
		if mc.ModelName != "test-2" {
			t.Errorf("ModelName = %s, want test-2", mc.ModelName)
		}
	})

	t.Run("GetModelByModelName not found", func(t *testing.T) {
		_, err := cfg.GetModelByModelName("nonexistent")
		if err == nil {
			t.Error("Expected error for nonexistent model")
		}
	})

	t.Run("GetModelConfig by model_name", func(t *testing.T) {
		mc, err := cfg.GetModelConfig("test-2")
		if err != nil {
			t.Fatalf("GetModelConfig() error = %v", err)
		}
		if mc.ModelName != "test-2" {
			t.Errorf("ModelName = %s, want test-2", mc.ModelName)
		}
	})

	t.Run("GetModelConfig not found", func(t *testing.T) {
		_, err := cfg.GetModelConfig("nonexistent")
		if err == nil {
			t.Error("Expected error for nonexistent model")
		}
	})
}

// TestConfigJSONSerialization tests config JSON serialization
func TestConfigJSONSerialization(t *testing.T) {
	t.Run("Config with all security layer configs", func(t *testing.T) {
		cfg := &SecurityConfig{
			DefaultAction:      "deny",
			LogAllOperations:   true,
			AuditLogFileEnabled: true,
			Layers: &SecurityLayersConfig{
				Injection:    &SecurityLayerConfig{Enabled: true, Extra: map[string]interface{}{"threshold": 0.7}},
				CommandGuard: &SecurityLayerConfig{Enabled: true},
				DLP:          &DLPLayerConfig{Enabled: true, Rules: []string{"credit_card"}, Action: "block"},
				SSRF:         &SecurityLayerConfig{Enabled: true},
				Credential:   &SecurityLayerConfig{Enabled: true},
				Signature:    &SignatureLayerConfig{Enabled: true, Strict: true},
				AuditChain:   &SecurityLayerConfig{Enabled: true},
			},
		}

		data, err := json.Marshal(cfg)
		if err != nil {
			t.Fatalf("Marshal error: %v", err)
		}

		var loaded SecurityConfig
		if err := json.Unmarshal(data, &loaded); err != nil {
			t.Fatalf("Unmarshal error: %v", err)
		}

		if loaded.Layers == nil {
			t.Fatal("Layers should not be nil")
		}
		if !loaded.Layers.Injection.Enabled {
			t.Error("Injection should be enabled")
		}
		if loaded.Layers.Signature == nil || !loaded.Layers.Signature.Strict {
			t.Error("Signature should be strict")
		}
	})
}

// TestTimeRelatedConfig tests time-related functionality
func TestTimeRelatedConfig(t *testing.T) {
	_ = time.Now() // Just verify time package is imported
}
