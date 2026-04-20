package forge_test

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/forge"
)

func newTestMCPInstaller(t *testing.T) (*forge.MCPInstaller, string) {
	t.Helper()
	tmpDir := t.TempDir()
	workspace := tmpDir
	// Create config dir and empty MCP config
	configDir := filepath.Join(workspace, "config")
	os.MkdirAll(configDir, 0755)
	mcpConfigPath := filepath.Join(configDir, "config.mcp.json")
	cfg := &config.MCPConfig{
		Enabled: false,
		Servers: []config.MCPServerConfig{},
		Timeout: 30,
	}
	data, _ := json.MarshalIndent(cfg, "", "  ")
	os.WriteFile(mcpConfigPath, data, 0644)
	return forge.NewMCPInstaller(workspace), workspace
}

func TestMCPInstallerInstallPython(t *testing.T) {
	inst, workspace := newTestMCPInstaller(t)

	// Create MCP directory with server.py
	mcpDir := filepath.Join(workspace, "forge", "mcp", "json-validator")
	os.MkdirAll(mcpDir, 0755)
	os.WriteFile(filepath.Join(mcpDir, "server.py"), []byte("import mcp\n"), 0644)

	artifact := &forge.Artifact{
		ID:   "mcp-json-validator",
		Type: forge.ArtifactMCP,
		Name: "json-validator",
	}
	err := inst.Install(artifact, mcpDir)
	if err != nil {
		t.Fatalf("Install failed: %v", err)
	}

	// Verify config was updated
	configPath := filepath.Join(workspace, "config", "config.mcp.json")
	data, _ := os.ReadFile(configPath)
	var cfg config.MCPConfig
	json.Unmarshal(data, &cfg)

	if !cfg.Enabled {
		t.Error("MCP config should be enabled after install")
	}
	if len(cfg.Servers) != 1 {
		t.Fatalf("Expected 1 server, got %d", len(cfg.Servers))
	}
	if cfg.Servers[0].Name != "json-validator" {
		t.Errorf("Expected name 'json-validator', got '%s'", cfg.Servers[0].Name)
	}
	if cfg.Servers[0].Command != "uv" {
		t.Errorf("Expected command 'uv', got '%s'", cfg.Servers[0].Command)
	}
}

func TestMCPInstallerInstallGo(t *testing.T) {
	inst, workspace := newTestMCPInstaller(t)

	// Create MCP directory with main.go
	mcpDir := filepath.Join(workspace, "forge", "mcp", "go-server")
	os.MkdirAll(mcpDir, 0755)
	os.WriteFile(filepath.Join(mcpDir, "main.go"), []byte("package main\n"), 0644)

	artifact := &forge.Artifact{
		ID:   "mcp-go-server",
		Type: forge.ArtifactMCP,
		Name: "go-server",
	}
	err := inst.Install(artifact, mcpDir)
	if err != nil {
		t.Fatalf("Install failed: %v", err)
	}

	configPath := filepath.Join(workspace, "config", "config.mcp.json")
	data, _ := os.ReadFile(configPath)
	var cfg config.MCPConfig
	json.Unmarshal(data, &cfg)

	if cfg.Servers[0].Command != "go" {
		t.Errorf("Expected command 'go', got '%s'", cfg.Servers[0].Command)
	}
}

func TestMCPInstallerInstallUpdate(t *testing.T) {
	inst, _ := newTestMCPInstaller(t)

	artifact := &forge.Artifact{
		ID:   "mcp-test",
		Type: forge.ArtifactMCP,
		Name: "test-server",
	}

	// Install twice - should update, not duplicate
	mcpDir := t.TempDir()
	os.WriteFile(filepath.Join(mcpDir, "server.py"), []byte("import mcp\n"), 0644)

	inst.Install(artifact, mcpDir)
	inst.Install(artifact, mcpDir)

	// Verify IsInstalled still works
	if !inst.IsInstalled("test-server") {
		t.Error("Server should be installed after double install")
	}
}

func TestMCPInstallerUninstall(t *testing.T) {
	inst, workspace := newTestMCPInstaller(t)

	mcpDir := filepath.Join(workspace, "forge", "mcp", "my-mcp")
	os.MkdirAll(mcpDir, 0755)
	os.WriteFile(filepath.Join(mcpDir, "server.py"), []byte("import mcp\n"), 0644)

	artifact := &forge.Artifact{
		ID:   "mcp-my-mcp",
		Type: forge.ArtifactMCP,
		Name: "my-mcp",
	}
	inst.Install(artifact, mcpDir)

	err := inst.Uninstall("my-mcp")
	if err != nil {
		t.Fatalf("Uninstall failed: %v", err)
	}

	if inst.IsInstalled("my-mcp") {
		t.Error("Server should not be installed after uninstall")
	}
}

func TestMCPInstallerIsInstalled(t *testing.T) {
	inst, _ := newTestMCPInstaller(t)

	if inst.IsInstalled("nonexistent") {
		t.Error("Non-existent server should not be installed")
	}
}

func TestMCPInstallerIsInstalledAfterInstall(t *testing.T) {
	inst, workspace := newTestMCPInstaller(t)

	mcpDir := filepath.Join(workspace, "forge", "mcp", "check-mcp")
	os.MkdirAll(mcpDir, 0755)
	os.WriteFile(filepath.Join(mcpDir, "server.py"), []byte("import mcp\n"), 0644)

	artifact := &forge.Artifact{
		ID:   "mcp-check-mcp",
		Type: forge.ArtifactMCP,
		Name: "check-mcp",
	}
	inst.Install(artifact, mcpDir)

	if !inst.IsInstalled("check-mcp") {
		t.Error("Server should be installed after Install")
	}
}

func TestMCPInstallerUninstallNonExistent(t *testing.T) {
	inst, _ := newTestMCPInstaller(t)

	// Should not error on non-existent server
	err := inst.Uninstall("nonexistent")
	if err != nil {
		t.Errorf("Uninstall of non-existent server should not error: %v", err)
	}
}

func TestMCPInstallerNoConfigFile(t *testing.T) {
	// No config file exists
	tmpDir := t.TempDir()
	inst := forge.NewMCPInstaller(tmpDir)

	if inst.IsInstalled("anything") {
		t.Error("Should return false when no config file exists")
	}
}

func TestMCPInstallerInstallDuplicateUpdate(t *testing.T) {
	inst, workspace := newTestMCPInstaller(t)

	mcpDir := filepath.Join(workspace, "forge", "mcp", "dup-server")
	os.MkdirAll(mcpDir, 0755)
	os.WriteFile(filepath.Join(mcpDir, "server.py"), []byte("import mcp\n"), 0644)

	artifact := &forge.Artifact{
		ID:   "mcp-dup-server",
		Type: forge.ArtifactMCP,
		Name: "dup-server",
	}

	// Install twice
	inst.Install(artifact, mcpDir)
	inst.Install(artifact, mcpDir)

	// Should still have only 1 server
	configPath := filepath.Join(workspace, "config", "config.mcp.json")
	data, _ := os.ReadFile(configPath)
	var cfg config.MCPConfig
	json.Unmarshal(data, &cfg)

	if len(cfg.Servers) != 1 {
		t.Errorf("Expected 1 server after duplicate install, got %d", len(cfg.Servers))
	}
}
