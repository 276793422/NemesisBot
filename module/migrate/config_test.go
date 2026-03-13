// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package migrate

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
)

// TestConvertConfigComprehensive tests comprehensive config conversion scenarios
func TestConvertConfigComprehensive(t *testing.T) {
	t.Run("Convert complete config with all sections", func(t *testing.T) {
		data := map[string]interface{}{
			"agents": map[string]interface{}{
				"defaults": map[string]interface{}{
					"llm":                 "zhipu/glm-4.7-flash",
					"max_tokens":          8192.0,
					"temperature":         0.8,
					"max_tool_iterations": 10.0,
					"workspace":           "/home/user/.openclaw/workspace",
				},
			},
			"providers": map[string]interface{}{
				"zhipu": map[string]interface{}{
					"api_key":  "zhipu-test-key",
					"api_base": "https://open.bigmodel.cn/api/paas/v4",
				},
			},
			"channels": map[string]interface{}{
				"telegram": map[string]interface{}{
					"enabled":    true,
					"token":      "telegram-token",
					"allow_from": []interface{}{"user1", "user2"},
				},
				"discord": map[string]interface{}{
					"enabled": true,
					"token":   "discord-token",
				},
			},
			"gateway": map[string]interface{}{
				"host": "0.0.0.0",
				"port": 8080.0,
			},
			"tools": map[string]interface{}{
				"web": map[string]interface{}{
					"search": map[string]interface{}{
						"api_key":     "brave-test-key",
						"max_results": 10.0,
					},
				},
			},
		}

		cfg, warnings, err := ConvertConfig(data)
		if err != nil {
			t.Fatalf("ConvertConfig failed: %v", err)
		}

		// Verify agents
		if cfg.Agents.Defaults.LLM != "zhipu/glm-4.7-flash" {
			t.Errorf("Expected LLM 'zhipu/glm-4.7-flash', got %q", cfg.Agents.Defaults.LLM)
		}

		if cfg.Agents.Defaults.MaxTokens != 8192 {
			t.Errorf("Expected MaxTokens 8192, got %d", cfg.Agents.Defaults.MaxTokens)
		}

		if cfg.Agents.Defaults.Temperature != 0.8 {
			t.Errorf("Expected Temperature 0.8, got %v", cfg.Agents.Defaults.Temperature)
		}

		if cfg.Agents.Defaults.MaxToolIterations != 10 {
			t.Errorf("Expected MaxToolIterations 10, got %d", cfg.Agents.Defaults.MaxToolIterations)
		}

		// Verify workspace rewrite
		if cfg.Agents.Defaults.Workspace == "/home/user/.openclaw/workspace" {
			t.Error("Workspace path should be rewritten from .openclaw to .nemesisbot")
		}

		// Verify providers
		if len(cfg.ModelList) == 0 {
			t.Error("Expected models in ModelList")
		}

		// Verify channels
		if !cfg.Channels.Telegram.Enabled {
			t.Error("Expected Telegram to be enabled")
		}

		if cfg.Channels.Telegram.Token != "telegram-token" {
			t.Errorf("Expected token 'telegram-token', got %q", cfg.Channels.Telegram.Token)
		}

		if !cfg.Channels.Discord.Enabled {
			t.Error("Expected Discord to be enabled")
		}

		if cfg.Channels.Discord.Token != "discord-token" {
			t.Errorf("Expected token 'discord-token', got %q", cfg.Channels.Discord.Token)
		}

		// Verify gateway
		if cfg.Gateway.Host != "0.0.0.0" {
			t.Errorf("Expected host '0.0.0.0', got %q", cfg.Gateway.Host)
		}

		if cfg.Gateway.Port != 8080 {
			t.Errorf("Expected port 8080, got %d", cfg.Gateway.Port)
		}

		// Verify tools
		if !cfg.Tools.Web.Brave.Enabled {
			t.Error("Expected Brave search to be enabled")
		}

		if cfg.Tools.Web.Brave.APIKey != "brave-test-key" {
			t.Errorf("Expected API key 'brave-test-key', got %q", cfg.Tools.Web.Brave.APIKey)
		}

		if cfg.Tools.Web.Brave.MaxResults != 10 {
			t.Errorf("Expected MaxResults 10, got %d", cfg.Tools.Web.Brave.MaxResults)
		}

		if len(warnings) > 0 {
			t.Errorf("Unexpected warnings: %v", warnings)
		}
	})
}

// TestConvertConfigWithAllProviders tests conversion with all supported providers
func TestConvertConfigWithAllProviders(t *testing.T) {
	data := map[string]interface{}{
		"providers": map[string]interface{}{
			"anthropic": map[string]interface{}{
				"api_key": "sk-ant-test",
			},
			"openai": map[string]interface{}{
				"api_key": "sk-openai-test",
			},
			"openrouter": map[string]interface{}{
				"api_key": "sk-or-test",
			},
			"groq": map[string]interface{}{
				"api_key": "groq-test",
			},
			"zhipu": map[string]interface{}{
				"api_key": "zhipu-test",
			},
			"vllm": map[string]interface{}{
				"api_base": "http://localhost:8000",
			},
			"gemini": map[string]interface{}{
				"api_key": "gemini-test",
			},
		},
	}

	cfg, warnings, err := ConvertConfig(data)
	if err != nil {
		t.Fatalf("ConvertConfig failed: %v", err)
	}

	expectedModels := map[string]string{
		"claude-sonnet-4":  "anthropic",
		"gpt-4o":           "openai",
		"openrouter-model": "openrouter",
		"groq-model":       "groq",
		"glm-4.7":          "zhipu",
		"vllm-local":       "vllm",
		"gemini-2.0-flash": "gemini",
	}

	if len(cfg.ModelList) != len(expectedModels) {
		t.Errorf("Expected %d models, got %d", len(expectedModels), len(cfg.ModelList))
	}

	// Check that all expected models are present (order doesn't matter)
	foundModels := make(map[string]bool)
	for _, model := range cfg.ModelList {
		foundModels[model.ModelName] = true
		if expectedProvider, ok := expectedModels[model.ModelName]; ok {
			if !strings.Contains(model.Model, expectedProvider) {
				t.Errorf("Model %s should contain provider %s", model.ModelName, expectedProvider)
			}
		}
	}

	for expectedModel := range expectedModels {
		if !foundModels[expectedModel] {
			t.Errorf("Missing model: %s", expectedModel)
		}
	}

	if len(warnings) > 0 {
		t.Errorf("Unexpected warnings: %v", warnings)
	}
}

// TestConvertConfigWithAllChannels tests conversion with all supported channels
func TestConvertConfigWithAllChannels(t *testing.T) {
	data := map[string]interface{}{
		"channels": map[string]interface{}{
			"telegram": map[string]interface{}{
				"enabled":    true,
				"token":      "telegram-token",
				"allow_from": []interface{}{"user1"},
			},
			"discord": map[string]interface{}{
				"enabled":    true,
				"token":      "discord-token",
				"allow_from": []interface{}{"user2"},
			},
			"whatsapp": map[string]interface{}{
				"enabled":    true,
				"bridge_url": "https://bridge.example.com",
				"allow_from": []interface{}{"user3"},
			},
			"feishu": map[string]interface{}{
				"enabled":            true,
				"app_id":             "feishu-app-id",
				"app_secret":         "feishu-secret",
				"encrypt_key":        "feishu-encrypt",
				"verification_token": "feishu-verify",
				"allow_from":         []interface{}{"user4"},
			},
			"qq": map[string]interface{}{
				"enabled":    true,
				"app_id":     "qq-app-id",
				"app_secret": "qq-secret",
				"allow_from": []interface{}{"user5"},
			},
			"dingtalk": map[string]interface{}{
				"enabled":       true,
				"client_id":     "dingtalk-client-id",
				"client_secret": "dingtalk-secret",
				"allow_from":    []interface{}{"user6"},
			},
			"maixcam": map[string]interface{}{
				"enabled":    true,
				"host":       "192.168.1.100",
				"port":       8888.0,
				"allow_from": []interface{}{"user7"},
			},
		},
	}

	cfg, warnings, err := ConvertConfig(data)
	if err != nil {
		t.Fatalf("ConvertConfig failed: %v", err)
	}

	// Verify Telegram
	if !cfg.Channels.Telegram.Enabled {
		t.Error("Expected Telegram to be enabled")
	}
	if cfg.Channels.Telegram.Token != "telegram-token" {
		t.Errorf("Expected token 'telegram-token', got %q", cfg.Channels.Telegram.Token)
	}

	// Verify Discord
	if !cfg.Channels.Discord.Enabled {
		t.Error("Expected Discord to be enabled")
	}
	if cfg.Channels.Discord.Token != "discord-token" {
		t.Errorf("Expected token 'discord-token', got %q", cfg.Channels.Discord.Token)
	}

	// Verify WhatsApp
	if !cfg.Channels.WhatsApp.Enabled {
		t.Error("Expected WhatsApp to be enabled")
	}
	if cfg.Channels.WhatsApp.BridgeURL != "https://bridge.example.com" {
		t.Errorf("Expected bridge URL 'https://bridge.example.com', got %q", cfg.Channels.WhatsApp.BridgeURL)
	}

	// Verify Feishu
	if !cfg.Channels.Feishu.Enabled {
		t.Error("Expected Feishu to be enabled")
	}
	if cfg.Channels.Feishu.AppID != "feishu-app-id" {
		t.Errorf("Expected app_id 'feishu-app-id', got %q", cfg.Channels.Feishu.AppID)
	}

	// Verify QQ
	if !cfg.Channels.QQ.Enabled {
		t.Error("Expected QQ to be enabled")
	}
	if cfg.Channels.QQ.AppID != "qq-app-id" {
		t.Errorf("Expected app_id 'qq-app-id', got %q", cfg.Channels.QQ.AppID)
	}

	// Verify DingTalk
	if !cfg.Channels.DingTalk.Enabled {
		t.Error("Expected DingTalk to be enabled")
	}
	if cfg.Channels.DingTalk.ClientID != "dingtalk-client-id" {
		t.Errorf("Expected client_id 'dingtalk-client-id', got %q", cfg.Channels.DingTalk.ClientID)
	}

	// Verify MaixCam
	if !cfg.Channels.MaixCam.Enabled {
		t.Error("Expected MaixCam to be enabled")
	}
	if cfg.Channels.MaixCam.Host != "192.168.1.100" {
		t.Errorf("Expected host '192.168.1.100', got %q", cfg.Channels.MaixCam.Host)
	}
	if cfg.Channels.MaixCam.Port != 8888 {
		t.Errorf("Expected port 8888, got %d", cfg.Channels.MaixCam.Port)
	}

	if len(warnings) > 0 {
		t.Errorf("Unexpected warnings: %v", warnings)
	}
}

// TestMergeConfigComprehensive tests comprehensive merge scenarios
func TestMergeConfigComprehensive(t *testing.T) {
	t.Run("Merge all sections", func(t *testing.T) {
		existing := &config.Config{
			ModelList: []config.ModelConfig{
				{ModelName: "existing-model", Model: "provider/existing"},
			},
			Channels: config.ChannelsConfig{
				Telegram: config.TelegramConfig{Enabled: true, Token: "existing-telegram"},
			},
			Tools: config.ToolsConfig{
				Web: config.WebToolsConfig{
					Brave: config.BraveConfig{APIKey: "existing-brave"},
				},
			},
		}

		incoming := &config.Config{
			ModelList: []config.ModelConfig{
				{ModelName: "new-model", Model: "provider/new"},
			},
			Channels: config.ChannelsConfig{
				Telegram: config.TelegramConfig{Enabled: true, Token: "new-telegram"},
				Discord:  config.DiscordConfig{Enabled: true, Token: "new-discord"},
			},
			Tools: config.ToolsConfig{
				Web: config.WebToolsConfig{
					Brave: config.BraveConfig{APIKey: "new-brave"},
				},
			},
		}

		result := MergeConfig(existing, incoming)

		// Models should be merged (new model added)
		if len(result.ModelList) != 2 {
			t.Errorf("Expected 2 models, got %d", len(result.ModelList))
		}

		// Existing Telegram should not be overridden
		if result.Channels.Telegram.Token != "existing-telegram" {
			t.Error("Existing Telegram token should not be overridden")
		}

		// New Discord should be merged
		if !result.Channels.Discord.Enabled {
			t.Error("Discord should be enabled")
		}
		if result.Channels.Discord.Token != "new-discord" {
			t.Errorf("Expected token 'new-discord', got %q", result.Channels.Discord.Token)
		}

		// Existing Brave should not be overridden
		if result.Tools.Web.Brave.APIKey != "existing-brave" {
			t.Error("Existing Brave API key should not be overridden")
		}
	})
}

// TestConfigEdgeCases tests edge cases in config conversion
func TestConfigEdgeCases(t *testing.T) {
	t.Run("Empty config", func(t *testing.T) {
		data := map[string]interface{}{}

		cfg, warnings, err := ConvertConfig(data)
		if err != nil {
			t.Fatalf("ConvertConfig failed: %v", err)
		}

		if cfg == nil {
			t.Error("Expected non-nil config")
		}

		if len(warnings) > 0 {
			t.Errorf("Unexpected warnings: %v", warnings)
		}
	})

	t.Run("Config with unsupported provider", func(t *testing.T) {
		data := map[string]interface{}{
			"providers": map[string]interface{}{
				"unsupported_provider": map[string]interface{}{
					"api_key": "test",
				},
			},
		}

		cfg, warnings, err := ConvertConfig(data)
		if err != nil {
			t.Fatalf("ConvertConfig failed: %v", err)
		}

		if len(warnings) == 0 {
			t.Error("Expected warning for unsupported provider")
		}

		if len(cfg.ModelList) != 0 {
			t.Error("Unsupported provider should not add models")
		}
	})

	t.Run("Config with unsupported channel", func(t *testing.T) {
		data := map[string]interface{}{
			"channels": map[string]interface{}{
				"unsupported_channel": map[string]interface{}{
					"enabled": true,
				},
			},
		}

		_, warnings, err := ConvertConfig(data)
		if err != nil {
			t.Fatalf("ConvertConfig failed: %v", err)
		}

		if len(warnings) == 0 {
			t.Error("Expected warning for unsupported channel")
		}
	})

	t.Run("Config with invalid types", func(t *testing.T) {
		data := map[string]interface{}{
			"agents": map[string]interface{}{
				"defaults": map[string]interface{}{
					"max_tokens":  "invalid", // Should be number
					"temperature": "invalid", // Should be number
				},
			},
		}

		cfg, _, err := ConvertConfig(data)
		if err != nil {
			t.Fatalf("ConvertConfig failed: %v", err)
		}

		// Should use default values when conversion fails
		if cfg == nil {
			t.Error("Expected non-nil config")
		}
	})

	t.Run("Config with provider without api_key or api_base", func(t *testing.T) {
		data := map[string]interface{}{
			"providers": map[string]interface{}{
				"zhipu": map[string]interface{}{
					"other_field": "value",
				},
			},
		}

		cfg, warnings, err := ConvertConfig(data)
		if err != nil {
			t.Fatalf("ConvertConfig failed: %v", err)
		}

		// Provider without api_key or api_base should not be added
		if len(cfg.ModelList) != 0 {
			t.Error("Provider without api_key or api_base should not add models")
		}

		if len(warnings) > 0 {
			t.Errorf("Unexpected warnings: %v", warnings)
		}
	})
}

// TestWorkspacePathRewrite tests workspace path rewriting in various scenarios
func TestWorkspacePathRewrite(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "Standard openclaw path",
			input:    "/home/user/.openclaw/workspace",
			expected: "/home/user/.nemesisbot/workspace",
		},
		{
			name:     "Windows path",
			input:    "C:\\Users\\user\\.openclaw\\workspace",
			expected: "C:\\Users\\user\\.nemesisbot\\workspace",
		},
		{
			name:     "Multiple occurrences",
			input:    "/home/user/.openclaw/.openclaw/workspace",
			expected: "/home/user/.nemesisbot/.openclaw/workspace",
		},
		{
			name:     "No openclaw path",
			input:    "/home/user/workspace",
			expected: "/home/user/workspace",
		},
		{
			name:     "Empty path",
			input:    "",
			expected: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := rewriteWorkspacePath(tt.input)
			if result != tt.expected {
				t.Errorf("rewriteWorkspacePath(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}

// TestExecuteConfigMigration tests the executeConfigMigration function
func TestExecuteConfigMigration(t *testing.T) {
	t.Run("Migrate to new config", func(t *testing.T) {
		tmpDir := t.TempDir()
		srcConfig := filepath.Join(tmpDir, "openclaw.json")
		dstConfig := filepath.Join(tmpDir, "nemesisbot", "config.json")

		// Create source config
		configContent := `{
			"agents": {
				"defaults": {
					"llm": "zhipu/glm-4.7-flash"
				}
			}
		}`
		err := os.WriteFile(srcConfig, []byte(configContent), 0644)
		if err != nil {
			t.Fatal(err)
		}

		// Migrate
		err = executeConfigMigration(srcConfig, dstConfig, tmpDir)
		if err != nil {
			t.Fatalf("executeConfigMigration failed: %v", err)
		}

		// Verify destination exists
		if _, err := os.Stat(dstConfig); os.IsNotExist(err) {
			t.Error("Destination config should exist")
		}
	})

	t.Run("Merge with existing config", func(t *testing.T) {
		tmpDir := t.TempDir()
		srcConfig := filepath.Join(tmpDir, "openclaw.json")
		dstConfig := filepath.Join(tmpDir, "config.json")

		// Create existing config
		existingContent := `{
			"agents": {
				"defaults": {
					"llm": "existing-model"
				}
			},
			"model_list": [
				{
					"model_name": "existing-model",
					"model": "provider/existing"
				}
			]
		}`
		err := os.WriteFile(dstConfig, []byte(existingContent), 0644)
		if err != nil {
			t.Fatal(err)
		}

		// Create source config
		sourceContent := `{
			"agents": {
				"defaults": {
					"llm": "zhipu/glm-4.7-flash"
				}
			},
			"providers": {
				"zhipu": {
					"api_key": "zhipu-key"
				}
			}
		}`
		err = os.WriteFile(srcConfig, []byte(sourceContent), 0644)
		if err != nil {
			t.Fatal(err)
		}

		// Migrate
		err = executeConfigMigration(srcConfig, dstConfig, tmpDir)
		if err != nil {
			t.Fatalf("executeConfigMigration failed: %v", err)
		}

		// Load merged config and verify
		mergedCfg, err := config.LoadConfig(dstConfig)
		if err != nil {
			t.Fatalf("Failed to load merged config: %v", err)
		}

		// Should have both models
		if len(mergedCfg.ModelList) < 2 {
			t.Errorf("Expected at least 2 models in merged config, got %d", len(mergedCfg.ModelList))
		}
	})
}

// TestBackupFile tests the backupFile function
func TestBackupFile(t *testing.T) {
	t.Run("Backup existing file", func(t *testing.T) {
		tmpDir := t.TempDir()
		filePath := filepath.Join(tmpDir, "test.txt")
		content := []byte("test content")

		err := os.WriteFile(filePath, content, 0644)
		if err != nil {
			t.Fatal(err)
		}

		err = backupFile(filePath)
		if err != nil {
			t.Fatalf("backupFile failed: %v", err)
		}

		// Check backup exists
		backupPath := filePath + ".bak"
		backupContent, err := os.ReadFile(backupPath)
		if err != nil {
			t.Fatalf("Failed to read backup file: %v", err)
		}

		if string(backupContent) != string(content) {
			t.Errorf("Backup content mismatch: got %q, want %q", string(backupContent), string(content))
		}
	})

	t.Run("Backup non-existent file", func(t *testing.T) {
		err := backupFile("/nonexistent/file.txt")
		if err == nil {
			t.Error("Expected error for non-existent file")
		}
	})
}
