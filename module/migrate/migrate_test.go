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

// TestCamelToSnake tests the camelToSnake function
func TestCamelToSnake(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"camelCase", "camel_case"},
		{"CamelCase", "camel_case"},
		{"APIKey", "api_key"},
		{"API_BASE", "api_base"},
		{"getHTTPResponse", "get_http_response"},
		{"XMLHttpRequest", "xml_http_request"},
		{"userID", "user_id"},
		{"already_snake", "already_snake"},
		{"UPPER", "upper"},
		{"lower", "lower"},
		{"get2HTTPResponse", "get2_http_response"},
		{"HTTP2Protocol", "http2_protocol"},
		{"", ""},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := camelToSnake(tt.input)
			if result != tt.expected {
				t.Errorf("camelToSnake(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}

// TestConvertKeysToSnake tests the convertKeysToSnake function
func TestConvertKeysToSnake(t *testing.T) {
	t.Run("Convert map with camelCase keys", func(t *testing.T) {
		input := map[string]interface{}{
			"camelCase":   "value1",
			"AnotherKey":  "value2",
			"APIResponse": map[string]interface{}{
				"statusCode": 200,
				"dataBody":   "content",
			},
		}

		result := convertKeysToSnake(input)

		resultMap, ok := result.(map[string]interface{})
		if !ok {
			t.Fatal("Result should be a map")
		}

		if val, ok := resultMap["camel_case"]; !ok || val != "value1" {
			t.Error("camel_case key not found or incorrect")
		}

		if val, ok := resultMap["another_key"]; !ok || val != "value2" {
			t.Error("another_key key not found or incorrect")
		}

		if _, ok := resultMap["api_response"]; !ok {
			t.Error("api_response key not found")
		}
	})

	t.Run("Convert array of maps", func(t *testing.T) {
		input := []interface{}{
			map[string]interface{}{"firstName": "John", "lastName": "Doe"},
			map[string]interface{}{"firstName": "Jane", "lastName": "Smith"},
		}

		result := convertKeysToSnake(input)

		resultArray, ok := result.([]interface{})
		if !ok {
			t.Fatal("Result should be an array")
		}

		if len(resultArray) != 2 {
			t.Errorf("Expected 2 items, got %d", len(resultArray))
		}

		firstMap, ok := resultArray[0].(map[string]interface{})
		if !ok {
			t.Fatal("Array item should be a map")
		}

		if val, ok := firstMap["first_name"]; !ok || val != "John" {
			t.Error("first_name key not found or incorrect")
		}
	})

	t.Run("Leave non-map, non-array values unchanged", func(t *testing.T) {
		input := "string value"
		result := convertKeysToSnake(input)
		if result != "string value" {
			t.Errorf("String value should be unchanged, got %v", result)
		}

		num := 42
		result = convertKeysToSnake(num)
		if result != 42 {
			t.Errorf("Number value should be unchanged, got %v", result)
		}
	})
}

// TestRewriteWorkspacePath tests the rewriteWorkspacePath function
func TestRewriteWorkspacePath(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"/home/user/.openclaw/workspace", "/home/user/.nemesisbot/workspace"},
		{"/path/to/.openclaw/files", "/path/to/.nemesisbot/files"},
		{"/home/user/openclaw/workspace", "/home/user/openclaw/workspace"},
		{"/path/.openclaw/.openclaw/test", "/path/.nemesisbot/.openclaw/test"},
		{"", ""},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := rewriteWorkspacePath(tt.input)
			if result != tt.expected {
				t.Errorf("rewriteWorkspacePath(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}

// TestGetMap tests the getMap helper function
func TestGetMap(t *testing.T) {
	data := map[string]interface{}{
		"valid": map[string]interface{}{
			"key": "value",
		},
		"invalid": "string",
		"missing": nil,
	}

	t.Run("Get valid map", func(t *testing.T) {
		result, ok := getMap(data, "valid")
		if !ok {
			t.Error("Expected ok=true for valid map")
		}
		if result == nil {
			t.Error("Expected non-nil map")
		}
	})

	t.Run("Get invalid map", func(t *testing.T) {
		_, ok := getMap(data, "invalid")
		if ok {
			t.Error("Expected ok=false for non-map value")
		}
	})

	t.Run("Get missing key", func(t *testing.T) {
		_, ok := getMap(data, "missing")
		if ok {
			t.Error("Expected ok=false for missing key")
		}
	})

	t.Run("Get non-existent key", func(t *testing.T) {
		_, ok := getMap(data, "nonexistent")
		if ok {
			t.Error("Expected ok=false for non-existent key")
		}
	})
}

// TestGetString tests the getString helper function
func TestGetString(t *testing.T) {
	data := map[string]interface{}{
		"valid":   "string value",
		"invalid": 123,
		"missing": nil,
	}

	t.Run("Get valid string", func(t *testing.T) {
		result, ok := getString(data, "valid")
		if !ok {
			t.Error("Expected ok=true for valid string")
		}
		if result != "string value" {
			t.Errorf("Expected 'string value', got %q", result)
		}
	})

	t.Run("Get invalid string", func(t *testing.T) {
		_, ok := getString(data, "invalid")
		if ok {
			t.Error("Expected ok=false for non-string value")
		}
	})

	t.Run("Get missing key", func(t *testing.T) {
		_, ok := getString(data, "missing")
		if ok {
			t.Error("Expected ok=false for missing key")
		}
	})
}

// TestGetFloat tests the getFloat helper function
func TestGetFloat(t *testing.T) {
	data := map[string]interface{}{
		"valid":   123.45,
		"invalid": "string",
		"missing": nil,
	}

	t.Run("Get valid float", func(t *testing.T) {
		result, ok := getFloat(data, "valid")
		if !ok {
			t.Error("Expected ok=true for valid float")
		}
		if result != 123.45 {
			t.Errorf("Expected 123.45, got %v", result)
		}
	})

	t.Run("Get invalid float", func(t *testing.T) {
		_, ok := getFloat(data, "invalid")
		if ok {
			t.Error("Expected ok=false for non-float value")
		}
	})

	t.Run("Get missing key", func(t *testing.T) {
		_, ok := getFloat(data, "missing")
		if ok {
			t.Error("Expected ok=false for missing key")
		}
	})
}

// TestGetBool tests the getBool helper function
func TestGetBool(t *testing.T) {
	data := map[string]interface{}{
		"true":     true,
		"false":    false,
		"invalid":  "string",
		"missing":  nil,
	}

	t.Run("Get true value", func(t *testing.T) {
		result, ok := getBool(data, "true")
		if !ok {
			t.Error("Expected ok=true for valid bool")
		}
		if result != true {
			t.Errorf("Expected true, got %v", result)
		}
	})

	t.Run("Get false value", func(t *testing.T) {
		result, ok := getBool(data, "false")
		if !ok {
			t.Error("Expected ok=true for valid bool")
		}
		if result != false {
			t.Errorf("Expected false, got %v", result)
		}
	})

	t.Run("Get invalid bool", func(t *testing.T) {
		_, ok := getBool(data, "invalid")
		if ok {
			t.Error("Expected ok=false for non-bool value")
		}
	})

	t.Run("Get missing key", func(t *testing.T) {
		_, ok := getBool(data, "missing")
		if ok {
			t.Error("Expected ok=false for missing key")
		}
	})
}

// TestGetBoolOrDefault tests the getBoolOrDefault helper function
func TestGetBoolOrDefault(t *testing.T) {
	data := map[string]interface{}{
		"true":    true,
		"false":   false,
		"invalid": "string",
	}

	t.Run("Get true value", func(t *testing.T) {
		result := getBoolOrDefault(data, "true", false)
		if result != true {
			t.Errorf("Expected true, got %v", result)
		}
	})

	t.Run("Get false value", func(t *testing.T) {
		result := getBoolOrDefault(data, "false", true)
		if result != false {
			t.Errorf("Expected false, got %v", result)
		}
	})

	t.Run("Get default for missing key", func(t *testing.T) {
		result := getBoolOrDefault(data, "missing", true)
		if result != true {
			t.Errorf("Expected default true, got %v", result)
		}
	})

	t.Run("Get default for invalid value", func(t *testing.T) {
		result := getBoolOrDefault(data, "invalid", false)
		if result != false {
			t.Errorf("Expected default false, got %v", result)
		}
	})
}

// TestGetStringSlice tests the getStringSlice helper function
func TestGetStringSlice(t *testing.T) {
	data := map[string]interface{}{
		"valid":   []interface{}{"a", "b", "c"},
		"mixed":   []interface{}{"string", 123, true},
		"invalid": "string",
		"empty":   []interface{}{},
		"missing": nil,
	}

	t.Run("Get valid string slice", func(t *testing.T) {
		result := getStringSlice(data, "valid")
		if len(result) != 3 {
			t.Errorf("Expected 3 items, got %d", len(result))
		}
		if result[0] != "a" || result[1] != "b" || result[2] != "c" {
			t.Error("Incorrect values in result")
		}
	})

	t.Run("Get mixed array (only strings extracted)", func(t *testing.T) {
		result := getStringSlice(data, "mixed")
		if len(result) != 1 {
			t.Errorf("Expected 1 item, got %d", len(result))
		}
		if result[0] != "string" {
			t.Errorf("Expected 'string', got %q", result[0])
		}
	})

	t.Run("Get invalid slice", func(t *testing.T) {
		result := getStringSlice(data, "invalid")
		if len(result) != 0 {
			t.Errorf("Expected empty slice, got %d items", len(result))
		}
	})

	t.Run("Get empty slice", func(t *testing.T) {
		result := getStringSlice(data, "empty")
		if len(result) != 0 {
			t.Errorf("Expected empty slice, got %d items", len(result))
		}
	})

	t.Run("Get missing key", func(t *testing.T) {
		result := getStringSlice(data, "missing")
		if len(result) != 0 {
			t.Errorf("Expected empty slice, got %d items", len(result))
		}
	})
}

// TestInferProviderFromModel tests the inferProviderFromModel function
func TestInferProviderFromModel(t *testing.T) {
	tests := []struct {
		model    string
		expected string
	}{
		{"claude-3-opus", "anthropic"},
		{"gpt-4", "openai"},
		{"llama-3-70b", "groq"},
		{"gemini-pro", "gemini"},
		{"glm-4", "zhipu"},
		{"unknown-model", ""},
		{"", ""},
		{"CLAUDE-SONNET", "anthropic"},
		{"GPT-3.5-TURBO", "openai"},
	}

	for _, tt := range tests {
		t.Run(tt.model, func(t *testing.T) {
			result := inferProviderFromModel(tt.model)
			if result != tt.expected {
				t.Errorf("inferProviderFromModel(%q) = %q, want %q", tt.model, result, tt.expected)
			}
		})
	}
}

// TestLoadOpenClawConfig tests the LoadOpenClawConfig function
func TestLoadOpenClawConfig(t *testing.T) {
	t.Run("Load valid JSON config", func(t *testing.T) {
		tmpDir := t.TempDir()
		configPath := filepath.Join(tmpDir, "config.json")
		content := `{"camelCase": "value", "anotherKey": 123}`
		err := os.WriteFile(configPath, []byte(content), 0644)
		if err != nil {
			t.Fatal(err)
		}

		result, err := LoadOpenClawConfig(configPath)
		if err != nil {
			t.Fatalf("LoadOpenClawConfig failed: %v", err)
		}

		if val, ok := result["camel_case"]; !ok || val != "value" {
			t.Error("camel_case key not found or incorrect")
		}
	})

	t.Run("Load non-existent file", func(t *testing.T) {
		_, err := LoadOpenClawConfig("/nonexistent/path/config.json")
		if err == nil {
			t.Error("Expected error for non-existent file")
		}
	})

	t.Run("Load invalid JSON", func(t *testing.T) {
		tmpDir := t.TempDir()
		configPath := filepath.Join(tmpDir, "config.json")
		err := os.WriteFile(configPath, []byte("{invalid json"), 0644)
		if err != nil {
			t.Fatal(err)
		}

		_, err = LoadOpenClawConfig(configPath)
		if err == nil {
			t.Error("Expected error for invalid JSON")
		}
	})
}

// TestConvertConfig tests the ConvertConfig function
func TestConvertConfig(t *testing.T) {
	t.Run("Convert basic config", func(t *testing.T) {
		data := map[string]interface{}{
			"agents": map[string]interface{}{
				"defaults": map[string]interface{}{
					"llm":           "anthropic/claude-3-opus",
					"max_tokens":    4096.0,
					"temperature":   0.7,
					"workspace":     "/home/user/.openclaw/workspace",
				},
			},
		}

		cfg, warnings, err := ConvertConfig(data)
		if err != nil {
			t.Fatalf("ConvertConfig failed: %v", err)
		}

		if cfg.Agents.Defaults.LLM != "anthropic/claude-3-opus" {
			t.Errorf("Expected LLM 'anthropic/claude-3-opus', got %q", cfg.Agents.Defaults.LLM)
		}

		if cfg.Agents.Defaults.MaxTokens != 4096 {
			t.Errorf("Expected MaxTokens 4096, got %d", cfg.Agents.Defaults.MaxTokens)
		}

		if cfg.Agents.Defaults.Temperature != 0.7 {
			t.Errorf("Expected Temperature 0.7, got %v", cfg.Agents.Defaults.Temperature)
		}

		if !strings.Contains(cfg.Agents.Defaults.Workspace, ".nemesisbot") {
			t.Error("Workspace path should be rewritten to .nemesisbot")
		}

		if len(warnings) > 0 {
			t.Errorf("Unexpected warnings: %v", warnings)
		}
	})

	t.Run("Convert config with providers", func(t *testing.T) {
		data := map[string]interface{}{
			"providers": map[string]interface{}{
				"anthropic": map[string]interface{}{
					"api_key":  "sk-ant-test",
					"api_base": "https://api.anthropic.com",
				},
				"unknown_provider": map[string]interface{}{
					"api_key": "test",
				},
			},
		}

		cfg, warnings, err := ConvertConfig(data)
		if err != nil {
			t.Fatalf("ConvertConfig failed: %v", err)
		}

		if len(cfg.ModelList) == 0 {
			t.Error("Expected models to be added to ModelList")
		}

		if len(warnings) == 0 {
			t.Error("Expected warning for unsupported provider")
		}
	})

	t.Run("Convert config with channels", func(t *testing.T) {
		data := map[string]interface{}{
			"channels": map[string]interface{}{
				"telegram": map[string]interface{}{
					"enabled":     true,
					"token":       "test-token",
					"allow_from":  []interface{}{"user1", "user2"},
				},
				"unsupported": map[string]interface{}{
					"enabled": true,
				},
			},
		}

		cfg, warnings, err := ConvertConfig(data)
		if err != nil {
			t.Fatalf("ConvertConfig failed: %v", err)
		}

		if !cfg.Channels.Telegram.Enabled {
			t.Error("Expected Telegram to be enabled")
		}

		if cfg.Channels.Telegram.Token != "test-token" {
			t.Errorf("Expected token 'test-token', got %q", cfg.Channels.Telegram.Token)
		}

		if len(warnings) == 0 {
			t.Error("Expected warning for unsupported channel")
		}
	})

	t.Run("Convert config with old model format", func(t *testing.T) {
		data := map[string]interface{}{
			"agents": map[string]interface{}{
				"defaults": map[string]interface{}{
					"model": "claude-3-opus",
				},
			},
		}

		cfg, warnings, err := ConvertConfig(data)
		if err != nil {
			t.Fatalf("ConvertConfig failed: %v", err)
		}

		if !strings.Contains(cfg.Agents.Defaults.LLM, "claude-3-opus") {
			t.Errorf("Expected LLM to contain claude-3-opus, got %q", cfg.Agents.Defaults.LLM)
		}

		if len(warnings) > 0 {
			t.Errorf("Unexpected warnings: %v", warnings)
		}
	})
}

// TestMergeConfig tests the MergeConfig function
func TestMergeConfig(t *testing.T) {
	t.Run("Merge models", func(t *testing.T) {
		existing := &config.Config{
			ModelList: []config.ModelConfig{
				{ModelName: "model1"},
			},
		}
		incoming := &config.Config{
			ModelList: []config.ModelConfig{
				{ModelName: "model2"},
			},
		}

		result := MergeConfig(existing, incoming)

		if len(result.ModelList) != 2 {
			t.Errorf("Expected 2 models, got %d", len(result.ModelList))
		}
	})

	t.Run("Don't duplicate existing models", func(t *testing.T) {
		existing := &config.Config{
			ModelList: []config.ModelConfig{
				{ModelName: "model1", Model: "provider/model1"},
			},
		}
		incoming := &config.Config{
			ModelList: []config.ModelConfig{
				{ModelName: "model1", Model: "provider/model1-new"},
			},
		}

		result := MergeConfig(existing, incoming)

		if len(result.ModelList) != 1 {
			t.Errorf("Expected 1 model (no duplicate), got %d", len(result.ModelList))
		}
	})

	t.Run("Merge channels", func(t *testing.T) {
		existing := &config.Config{}
		existing.Channels.Telegram.Enabled = false

		incoming := &config.Config{}
		incoming.Channels.Telegram.Enabled = true
		incoming.Channels.Telegram.Token = "test-token"

		result := MergeConfig(existing, incoming)

		if !result.Channels.Telegram.Enabled {
			t.Error("Expected Telegram to be enabled")
		}

		if result.Channels.Telegram.Token != "test-token" {
			t.Errorf("Expected token 'test-token', got %q", result.Channels.Telegram.Token)
		}
	})

	t.Run("Don't override enabled channels", func(t *testing.T) {
		existing := &config.Config{}
		existing.Channels.Telegram.Enabled = true
		existing.Channels.Telegram.Token = "existing-token"

		incoming := &config.Config{}
		incoming.Channels.Telegram.Enabled = true
		incoming.Channels.Telegram.Token = "new-token"

		result := MergeConfig(existing, incoming)

		if result.Channels.Telegram.Token != "existing-token" {
			t.Error("Should not override existing channel configuration")
		}
	})
}

// TestFindOpenClawConfig tests the findOpenClawConfig function
func TestFindOpenClawConfig(t *testing.T) {
	t.Run("Find openclaw.json", func(t *testing.T) {
		tmpDir := t.TempDir()
		configPath := filepath.Join(tmpDir, "openclaw.json")
		err := os.WriteFile(configPath, []byte("{}"), 0644)
		if err != nil {
			t.Fatal(err)
		}

		result, err := findOpenClawConfig(tmpDir)
		if err != nil {
			t.Fatalf("findOpenClawConfig failed: %v", err)
		}

		if result != configPath {
			t.Errorf("Expected %q, got %q", configPath, result)
		}
	})

	t.Run("Find config.json", func(t *testing.T) {
		tmpDir := t.TempDir()
		configPath := filepath.Join(tmpDir, "config.json")
		err := os.WriteFile(configPath, []byte("{}"), 0644)
		if err != nil {
			t.Fatal(err)
		}

		result, err := findOpenClawConfig(tmpDir)
		if err != nil {
			t.Fatalf("findOpenClawConfig failed: %v", err)
		}

		if result != configPath {
			t.Errorf("Expected %q, got %q", configPath, result)
		}
	})

	t.Run("Prefer openclaw.json over config.json", func(t *testing.T) {
		tmpDir := t.TempDir()
		openclawPath := filepath.Join(tmpDir, "openclaw.json")
		configPath := filepath.Join(tmpDir, "config.json")

		err := os.WriteFile(openclawPath, []byte("{}"), 0644)
		if err != nil {
			t.Fatal(err)
		}
		err = os.WriteFile(configPath, []byte("{}"), 0644)
		if err != nil {
			t.Fatal(err)
		}

		result, err := findOpenClawConfig(tmpDir)
		if err != nil {
			t.Fatalf("findOpenClawConfig failed: %v", err)
		}

		if result != openclawPath {
			t.Errorf("Expected %q (openclaw.json), got %q", openclawPath, result)
		}
	})

	t.Run("No config found", func(t *testing.T) {
		tmpDir := t.TempDir()

		_, err := findOpenClawConfig(tmpDir)
		if err == nil {
			t.Error("Expected error when no config found")
		}
	})
}

// TestPlanFileCopy tests the planFileCopy function
func TestPlanFileCopy(t *testing.T) {
	t.Run("Source doesn't exist", func(t *testing.T) {
		action := planFileCopy("/nonexistent/src", "/dst", false)
		if action.Type != ActionSkip {
			t.Errorf("Expected ActionSkip, got %v", action.Type)
		}
	})

	t.Run("Destination doesn't exist", func(t *testing.T) {
		tmpDir := t.TempDir()
		srcFile := filepath.Join(tmpDir, "src.txt")
		err := os.WriteFile(srcFile, []byte("content"), 0644)
		if err != nil {
			t.Fatal(err)
		}

		action := planFileCopy(srcFile, "/dst.txt", false)
		if action.Type != ActionCopy {
			t.Errorf("Expected ActionCopy, got %v", action.Type)
		}
	})

	t.Run("Destination exists without force", func(t *testing.T) {
		tmpDir := t.TempDir()
		srcFile := filepath.Join(tmpDir, "src.txt")
		dstFile := filepath.Join(tmpDir, "dst.txt")

		err := os.WriteFile(srcFile, []byte("content"), 0644)
		if err != nil {
			t.Fatal(err)
		}
		err = os.WriteFile(dstFile, []byte("existing"), 0644)
		if err != nil {
			t.Fatal(err)
		}

		action := planFileCopy(srcFile, dstFile, false)
		if action.Type != ActionBackup {
			t.Errorf("Expected ActionBackup, got %v", action.Type)
		}
	})

	t.Run("Destination exists with force", func(t *testing.T) {
		tmpDir := t.TempDir()
		srcFile := filepath.Join(tmpDir, "src.txt")
		dstFile := filepath.Join(tmpDir, "dst.txt")

		err := os.WriteFile(srcFile, []byte("content"), 0644)
		if err != nil {
			t.Fatal(err)
		}
		err = os.WriteFile(dstFile, []byte("existing"), 0644)
		if err != nil {
			t.Fatal(err)
		}

		action := planFileCopy(srcFile, dstFile, true)
		if action.Type != ActionCopy {
			t.Errorf("Expected ActionCopy with force=true, got %v", action.Type)
		}
	})
}

// TestPlanDirCopy tests the planDirCopy function
func TestPlanDirCopy(t *testing.T) {
	t.Run("Plan directory copy", func(t *testing.T) {
		tmpDir := t.TempDir()
		srcDir := filepath.Join(tmpDir, "src")
		dstDir := filepath.Join(tmpDir, "dst")

		err := os.MkdirAll(filepath.Join(srcDir, "subdir"), 0755)
		if err != nil {
			t.Fatal(err)
		}
		err = os.WriteFile(filepath.Join(srcDir, "file1.txt"), []byte("content1"), 0644)
		if err != nil {
			t.Fatal(err)
		}
		err = os.WriteFile(filepath.Join(srcDir, "subdir", "file2.txt"), []byte("content2"), 0644)
		if err != nil {
			t.Fatal(err)
		}

		actions, err := planDirCopy(srcDir, dstDir, false)
		if err != nil {
			t.Fatalf("planDirCopy failed: %v", err)
		}

		if len(actions) < 3 {
			t.Errorf("Expected at least 3 actions (2 files + 1 dir), got %d", len(actions))
		}
	})

	t.Run("Source directory doesn't exist", func(t *testing.T) {
		_, err := planDirCopy("/nonexistent/src", "/dst", false)
		if err == nil {
			t.Error("Expected error for non-existent source directory")
		}
	})
}

// TestPlanWorkspaceMigration tests the PlanWorkspaceMigration function
func TestPlanWorkspaceMigration(t *testing.T) {
	t.Run("Plan workspace migration", func(t *testing.T) {
		tmpDir := t.TempDir()
		srcWorkspace := filepath.Join(tmpDir, "src_workspace")
		dstWorkspace := filepath.Join(tmpDir, "dst_workspace")

		err := os.MkdirAll(srcWorkspace, 0755)
		if err != nil {
			t.Fatal(err)
		}

		// Create migrateable files
		err = os.WriteFile(filepath.Join(srcWorkspace, "AGENT.md"), []byte("agent"), 0644)
		if err != nil {
			t.Fatal(err)
		}
		err = os.WriteFile(filepath.Join(srcWorkspace, "SOUL.md"), []byte("soul"), 0644)
		if err != nil {
			t.Fatal(err)
		}

		// Create memory directory
		err = os.MkdirAll(filepath.Join(srcWorkspace, "memory"), 0755)
		if err != nil {
			t.Fatal(err)
		}
		err = os.WriteFile(filepath.Join(srcWorkspace, "memory", "chat1.json"), []byte("{}"), 0644)
		if err != nil {
			t.Fatal(err)
		}

		actions, err := PlanWorkspaceMigration(srcWorkspace, dstWorkspace, false)
		if err != nil {
			t.Fatalf("PlanWorkspaceMigration failed: %v", err)
		}

		if len(actions) == 0 {
			t.Error("Expected at least some actions")
		}
	})

	t.Run("Source workspace doesn't exist", func(t *testing.T) {
		tmpDir := t.TempDir()
		srcWorkspace := filepath.Join(tmpDir, "nonexistent")
		dstWorkspace := filepath.Join(tmpDir, "dst")

		actions, err := PlanWorkspaceMigration(srcWorkspace, dstWorkspace, false)
		if err != nil {
			t.Fatalf("PlanWorkspaceMigration failed: %v", err)
		}

		// When source doesn't exist, we should get actions for migrateable files
		// that don't exist (will be skipped)
		// The number of actions equals migrateable files
		if len(actions) > len(migrateableFiles) {
			t.Errorf("Expected at most %d actions (skip actions for non-existent files), got %d", len(migrateableFiles), len(actions))
		}

		// All actions should be SKIP type
		for _, action := range actions {
			if action.Type != ActionSkip {
				t.Errorf("Expected only SKIP actions for non-existent workspace, got %v", action.Type)
			}
		}
	})
}

// TestCopyFile tests the copyFile function
func TestCopyFile(t *testing.T) {
	t.Run("Copy file successfully", func(t *testing.T) {
		tmpDir := t.TempDir()
		srcFile := filepath.Join(tmpDir, "src.txt")
		dstFile := filepath.Join(tmpDir, "dst.txt")

		err := os.WriteFile(srcFile, []byte("test content"), 0644)
		if err != nil {
			t.Fatal(err)
		}

		err = copyFile(srcFile, dstFile)
		if err != nil {
			t.Fatalf("copyFile failed: %v", err)
		}

		content, err := os.ReadFile(dstFile)
		if err != nil {
			t.Fatalf("Failed to read destination file: %v", err)
		}

		if string(content) != "test content" {
			t.Errorf("Expected 'test content', got %q", string(content))
		}
	})

	t.Run("Source file doesn't exist", func(t *testing.T) {
		err := copyFile("/nonexistent/src", "/dst")
		if err == nil {
			t.Error("Expected error for non-existent source file")
		}
	})
}

// TestRelPath tests the relPath function
func TestRelPath(t *testing.T) {
	t.Run("Get relative path", func(t *testing.T) {
		path := "/home/user/project/src/file.txt"
		base := "/home/user/project"

		result := relPath(path, base)
		// Just verify we get something reasonable
		if result == "" {
			t.Error("Expected non-empty result")
		}
	})

	t.Run("Path is not relative to base", func(t *testing.T) {
		path := "/other/path/file.txt"
		base := "/home/user/project"

		result := relPath(path, base)
		// Should return base name as fallback
		// On different platforms, the result may vary
		if result == "" {
			t.Error("Expected non-empty result")
		}
		// The important thing is it doesn't crash and returns something
	})
}
