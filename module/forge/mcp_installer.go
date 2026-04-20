package forge

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/276793422/NemesisBot/module/config"
)

// MCPInstaller manages MCP server registration in config.mcp.json.
type MCPInstaller struct {
	workspace string
}

// NewMCPInstaller creates a new MCPInstaller for the given workspace.
func NewMCPInstaller(workspace string) *MCPInstaller {
	return &MCPInstaller{workspace: workspace}
}

// Install adds or updates an MCP server in config.mcp.json.
func (inst *MCPInstaller) Install(artifact *Artifact, mcpDir string) error {
	configPath := inst.getConfigPath()

	cfg, err := config.LoadMCPConfig(configPath)
	if err != nil {
		return fmt.Errorf("加载 MCP 配置失败: %w", err)
	}

	// Determine command based on entry file in mcpDir
	command, args := inst.buildCommand(artifact.Name, mcpDir)

	serverConfig := config.MCPServerConfig{
		Name:    artifact.Name,
		Command: command,
		Args:    args,
	}

	// Update existing or append new
	found := false
	for i, s := range cfg.Servers {
		if s.Name == artifact.Name {
			cfg.Servers[i] = serverConfig
			found = true
			break
		}
	}
	if !found {
		cfg.Servers = append(cfg.Servers, serverConfig)
	}

	// Ensure enabled
	cfg.Enabled = true

	return saveMCPConfigFile(configPath, cfg)
}

// Uninstall removes an MCP server from config.mcp.json.
func (inst *MCPInstaller) Uninstall(artifactName string) error {
	configPath := inst.getConfigPath()

	cfg, err := config.LoadMCPConfig(configPath)
	if err != nil {
		return fmt.Errorf("加载 MCP 配置失败: %w", err)
	}

	for i, s := range cfg.Servers {
		if s.Name == artifactName {
			cfg.Servers = append(cfg.Servers[:i], cfg.Servers[i+1:]...)
			break
		}
	}

	return saveMCPConfigFile(configPath, cfg)
}

// IsInstalled checks if an MCP server is already registered in config.mcp.json.
func (inst *MCPInstaller) IsInstalled(artifactName string) bool {
	configPath := inst.getConfigPath()

	cfg, err := config.LoadMCPConfig(configPath)
	if err != nil {
		return false
	}

	for _, s := range cfg.Servers {
		if s.Name == artifactName {
			return true
		}
	}
	return false
}

// getConfigPath returns the config.mcp.json path.
func (inst *MCPInstaller) getConfigPath() string {
	return filepath.Join(inst.workspace, "config", "config.mcp.json")
}

// buildCommand determines the command and args based on files in mcpDir.
func (inst *MCPInstaller) buildCommand(name, mcpDir string) (string, []string) {
	// Check for Python server.py
	if _, err := os.Stat(filepath.Join(mcpDir, "server.py")); err == nil {
		return "uv", []string{"run", "--directory", mcpDir, "server.py"}
	}

	// Check for Go main.go - use go run
	if _, err := os.Stat(filepath.Join(mcpDir, "main.go")); err == nil {
		return "go", []string{"run", filepath.Join(mcpDir, "main.go")}
	}

	// Fallback: try to detect from artifact path
	if strings.HasSuffix(mcpDir, ".go") || strings.HasSuffix(filepath.Base(mcpDir), ".go") {
		return "go", []string{"run", mcpDir}
	}

	// Default to Python
	return "uv", []string{"run", "--directory", mcpDir, "server.py"}
}

// saveMCPConfigFile writes MCPConfig to disk with proper formatting.
func saveMCPConfigFile(path string, cfg *config.MCPConfig) error {
	data, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return err
	}

	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	return os.WriteFile(path, data, 0644)
}
