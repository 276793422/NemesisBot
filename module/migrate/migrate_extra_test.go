// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package migrate

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
)

// TestResolveOpenClawHome tests resolveOpenClawHome with override and env.
func TestResolveOpenClawHome(t *testing.T) {
	tests := []struct {
		name      string
		override  string
		envValue  string
		checkFunc func(t *testing.T, result string, err error)
	}{
		{
			name:     "override takes precedence",
			override: "/custom/openclaw",
			checkFunc: func(t *testing.T, result string, err error) {
				if err != nil {
					t.Fatalf("unexpected error: %v", err)
				}
				if result != "/custom/openclaw" {
					t.Errorf("expected /custom/openclaw, got %s", result)
				}
			},
		},
		{
			name:     "env variable fallback",
			envValue: "/env/openclaw",
			checkFunc: func(t *testing.T, result string, err error) {
				if err != nil {
					t.Fatalf("unexpected error: %v", err)
				}
				if result != "/env/openclaw" {
					t.Errorf("expected /env/openclaw, got %s", result)
				}
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Clean up env
			origEnv := os.Getenv("OPENCLAW_HOME")
			os.Unsetenv("OPENCLAW_HOME")
			defer os.Setenv("OPENCLAW_HOME", origEnv)

			if tt.envValue != "" {
				os.Setenv("OPENCLAW_HOME", tt.envValue)
			}

			result, err := resolveOpenClawHome(tt.override)
			tt.checkFunc(t, result, err)
		})
	}
}

// TestResolveWorkspace tests resolveWorkspace.
func TestResolveWorkspace(t *testing.T) {
	result := resolveWorkspace("/home/user/.openclaw")
	expected := filepath.Join("/home/user/.openclaw", "workspace")
	if result != expected {
		t.Errorf("expected %s, got %s", expected, result)
	}
}

// TestResolveNemesisBotHome tests resolveNemesisBotHome with override.
func TestResolveNemesisBotHome(t *testing.T) {
	result, err := resolveNemesisBotHome("/custom/nemesisbot")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result != "/custom/nemesisbot" {
		t.Errorf("expected /custom/nemesisbot, got %s", result)
	}
}

// TestResolveNemesisBotHome_Env tests resolveNemesisBotHome with env variable.
func TestResolveNemesisBotHome_Env(t *testing.T) {
	origEnv := os.Getenv("NEMESISBOT_HOME")
	os.Unsetenv("NEMESISBOT_HOME")
	defer os.Setenv("NEMESISBOT_HOME", origEnv)

	os.Setenv("NEMESISBOT_HOME", "/env/nemesisbot")
	result, err := resolveNemesisBotHome("")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result != "/env/nemesisbot" {
		t.Errorf("expected /env/nemesisbot, got %s", result)
	}
}

// TestRun_MutuallyExclusive tests that both flags error.
func TestRun_MutuallyExclusive(t *testing.T) {
	_, err := Run(Options{ConfigOnly: true, WorkspaceOnly: true})
	if err == nil {
		t.Fatal("expected error for mutually exclusive flags")
	}
}

// TestRun_SourceNotExist tests that missing source dir errors.
func TestRun_SourceNotExist(t *testing.T) {
	_, err := Run(Options{
		Force:        true,
		OpenClawHome: "/nonexistent/path/.openclaw",
	})
	if err == nil {
		t.Fatal("expected error for nonexistent source")
	}
}

// TestPlan tests the Plan function with actual files.
func TestPlan(t *testing.T) {
	srcDir := t.TempDir()
	dstDir := t.TempDir()

	// Create a workspace directory in source
	wsDir := filepath.Join(srcDir, "workspace")
	os.MkdirAll(wsDir, 0755)

	// Create some workspace files
	os.WriteFile(filepath.Join(wsDir, "AGENT.md"), []byte("# Agent"), 0644)
	os.WriteFile(filepath.Join(wsDir, "SOUL.md"), []byte("# Soul"), 0644)

	actions, warnings, err := Plan(Options{Force: true}, srcDir, dstDir)
	if err != nil {
		t.Fatalf("Plan failed: %v", err)
	}
	if len(actions) == 0 {
		t.Error("expected some actions")
	}
	// Should have config migration warning since no config file
	if len(warnings) == 0 {
		t.Error("expected warnings about missing config")
	}
}

// TestPlan_ConfigOnly tests Plan with ConfigOnly option.
func TestPlan_ConfigOnly(t *testing.T) {
	srcDir := t.TempDir()
	dstDir := t.TempDir()

	// Create config file
	os.WriteFile(filepath.Join(srcDir, "config.json"), []byte(`{"agents":{"defaults":{"llm":"zhipu/glm-4"}}}`), 0644)

	actions, warnings, err := Plan(Options{ConfigOnly: true}, srcDir, dstDir)
	if err != nil {
		t.Fatalf("Plan failed: %v", err)
	}
	// Should have a config conversion action
	hasConfig := false
	for _, a := range actions {
		if a.Type == ActionConvertConfig {
			hasConfig = true
		}
	}
	if !hasConfig {
		t.Error("expected config conversion action")
	}
	_ = warnings
}

// TestPlan_WorkspaceOnly tests Plan with WorkspaceOnly option.
func TestPlan_WorkspaceOnly(t *testing.T) {
	srcDir := t.TempDir()
	dstDir := t.TempDir()

	// Create workspace with content
	wsDir := filepath.Join(srcDir, "workspace")
	os.MkdirAll(wsDir, 0755)
	os.WriteFile(filepath.Join(wsDir, "USER.md"), []byte("# User"), 0644)

	actions, _, err := Plan(Options{WorkspaceOnly: true, Force: true}, srcDir, dstDir)
	if err != nil {
		t.Fatalf("Plan failed: %v", err)
	}
	// Should have workspace copy actions but no config action
	for _, a := range actions {
		if a.Type == ActionConvertConfig {
			t.Error("should not have config action in workspace-only mode")
		}
	}
}

// TestExecute tests the Execute function with various actions.
func TestExecute(t *testing.T) {
	srcDir := t.TempDir()
	dstDir := t.TempDir()

	// Create source files
	os.MkdirAll(filepath.Join(srcDir, "subdir"), 0755)
	os.WriteFile(filepath.Join(srcDir, "file1.txt"), []byte("content1"), 0644)
	os.WriteFile(filepath.Join(srcDir, "subdir", "file2.txt"), []byte("content2"), 0644)

	// Create destination file that exists (for backup test)
	os.WriteFile(filepath.Join(dstDir, "file1.txt"), []byte("old content"), 0644)

	actions := []Action{
		{Type: ActionCreateDir, Destination: filepath.Join(dstDir, "newdir")},
		{Type: ActionCopy, Source: filepath.Join(srcDir, "file1.txt"), Destination: filepath.Join(dstDir, "file1.txt")},
		{Type: ActionSkip},
		{Type: ActionBackup, Source: filepath.Join(srcDir, "file1.txt"), Destination: filepath.Join(dstDir, "file1.txt")},
	}

	result := Execute(actions, srcDir, dstDir)

	if result.FilesSkipped != 1 {
		t.Errorf("expected 1 skip, got %d", result.FilesSkipped)
	}
	if result.DirsCreated == 0 {
		t.Error("expected at least 1 dir created")
	}
}

// TestExecute_ConfigMigration tests config migration execution.
func TestExecute_ConfigMigration(t *testing.T) {
	srcDir := t.TempDir()
	dstDir := t.TempDir()

	// Create source config
	configContent := `{"agents":{"defaults":{"llm":"zhipu/glm-4","max_tokens":4096}}}`
	configPath := filepath.Join(srcDir, "config.json")
	os.WriteFile(configPath, []byte(configContent), 0644)

	dstConfigPath := filepath.Join(dstDir, "config.json")

	actions := []Action{
		{
			Type:        ActionConvertConfig,
			Source:      configPath,
			Destination: dstConfigPath,
		},
	}

	result := Execute(actions, srcDir, dstDir)

	if !result.ConfigMigrated {
		t.Error("expected config to be migrated")
	}

	// Verify destination config exists
	if _, err := os.Stat(dstConfigPath); os.IsNotExist(err) {
		t.Error("expected destination config file to exist")
	}
}

// TestRelPath tests relPath helper.
func TestRelPath_WithBase(t *testing.T) {
	result := relPath("/home/user/.openclaw/workspace/AGENT.md", "/home/user/.openclaw")
	expected := filepath.Join("workspace", "AGENT.md")
	if result != expected {
		t.Errorf("expected %s, got %s", expected, result)
	}
}

// TestRelPath_InvalidBase tests relPath with non-parent base.
func TestRelPath_InvalidBase(t *testing.T) {
	result := relPath("/home/user/file.txt", "/other/path")
	// On Windows this gives "..\..\home\user\file.txt" - just verify it doesn't panic
	if result == "" {
		t.Error("expected non-empty result")
	}
}

// TestRun_DryRun tests Run with dry run option.
func TestRun_DryRun(t *testing.T) {
	srcDir := t.TempDir()
	dstDir := t.TempDir()

	// Create a fake openclaw home
	wsDir := filepath.Join(srcDir, "workspace")
	os.MkdirAll(wsDir, 0755)
	os.WriteFile(filepath.Join(wsDir, "AGENT.md"), []byte("# Agent"), 0644)

	result, err := Run(Options{
		DryRun:       true,
		Force:        true,
		OpenClawHome: srcDir,
		NemesisBotHome: dstDir,
	})
	if err != nil {
		t.Fatalf("Run dry run failed: %v", err)
	}
	if result == nil {
		t.Fatal("expected non-nil result")
	}
	// In dry run, no files should be copied
	if result.FilesCopied != 0 {
		t.Errorf("expected 0 files copied in dry run, got %d", result.FilesCopied)
	}
}

// TestConfirm tests Confirm (note: this reads from stdin).
func TestConfirm_WithInput(t *testing.T) {
	// Confirm reads from stdin which we can't easily test in unit tests.
	// Just verify the function exists.
	_ = Confirm
}

// TestPrintPlan tests PrintPlan doesn't panic.
func TestPrintPlan_NoPanic(t *testing.T) {
	actions := []Action{
		{Type: ActionConvertConfig, Source: "/src/config.json", Destination: "/dst/config.json", Description: "convert config"},
		{Type: ActionCopy, Source: "/src/file.txt", Destination: "/dst/file.txt", Description: "copy file"},
		{Type: ActionBackup, Source: "/src/file.txt", Destination: "/dst/file.txt", Description: "backup"},
		{Type: ActionSkip, Source: "/src/skip.txt", Description: "skip this"},
		{Type: ActionCreateDir, Destination: "/dst/newdir", Description: "create dir"},
	}
	warnings := []string{"warning 1", "warning 2"}

	// Should not panic
	PrintPlan(actions, warnings)
}

// TestPrintSummary tests PrintSummary doesn't panic.
func TestPrintSummary_NoPanic(t *testing.T) {
	result := &Result{
		FilesCopied:    5,
		ConfigMigrated: true,
		BackupsCreated: 2,
		FilesSkipped:   1,
		Errors:         []error{fmt.Errorf("test error")},
	}
	PrintSummary(result)

	// Test empty result
	PrintSummary(&Result{})
}

// TestRun_Refresh tests Run with Refresh option (implies WorkspaceOnly).
func TestRun_Refresh(t *testing.T) {
	srcDir := t.TempDir()
	dstDir := t.TempDir()

	// Create workspace directory in source
	wsDir := filepath.Join(srcDir, "workspace")
	os.MkdirAll(wsDir, 0755)
	os.WriteFile(filepath.Join(wsDir, "AGENT.md"), []byte("# Agent"), 0644)

	result, err := Run(Options{
		Refresh:         true,
		Force:           true,
		OpenClawHome:    srcDir,
		NemesisBotHome:  dstDir,
	})
	if err != nil {
		t.Fatalf("Run refresh failed: %v", err)
	}
	if result == nil {
		t.Fatal("expected non-nil result")
	}
}

// TestMergeConfig_MoreChannels tests merging additional channels.
func TestMergeConfig_MoreChannels(t *testing.T) {
	existing := config.DefaultConfig()
	existing.Channels.Telegram.Enabled = true
	existing.Channels.Telegram.Token = "existing-token"

	incoming := config.DefaultConfig()
	incoming.Channels.Discord.Enabled = true
	incoming.Channels.Discord.Token = "new-token"
	incoming.Channels.Telegram.Enabled = true
	incoming.Channels.Telegram.Token = "incoming-token"

	result := MergeConfig(existing, incoming)

	// Existing Telegram config should not be overwritten
	if result.Channels.Telegram.Token != "existing-token" {
		t.Error("existing enabled channel should not be overwritten")
	}
	// Discord should be added since existing has it disabled
	if !result.Channels.Discord.Enabled {
		t.Error("incoming Discord should be enabled")
	}
}

// TestMergeConfig_BraveAPI tests merging Brave API key.
func TestMergeConfig_BraveAPI(t *testing.T) {
	existing := config.DefaultConfig()
	incoming := config.DefaultConfig()
	incoming.Tools.Web.Brave.APIKey = "new-api-key"
	incoming.Tools.Web.Brave.Enabled = true

	result := MergeConfig(existing, incoming)

	if result.Tools.Web.Brave.APIKey != "new-api-key" {
		t.Error("expected Brave API key to be merged")
	}
}

// TestMergeConfig_BraveAPI_ExistingKey tests merging when existing has API key.
func TestMergeConfig_BraveAPI_ExistingKey(t *testing.T) {
	existing := config.DefaultConfig()
	existing.Tools.Web.Brave.APIKey = "old-key"
	existing.Tools.Web.Brave.Enabled = true

	incoming := config.DefaultConfig()
	incoming.Tools.Web.Brave.APIKey = "new-key"

	result := MergeConfig(existing, incoming)

	if result.Tools.Web.Brave.APIKey != "old-key" {
		t.Error("existing API key should not be overwritten")
	}
}
