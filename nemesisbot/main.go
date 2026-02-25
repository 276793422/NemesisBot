// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package main

import (
	"embed"
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/nemesisbot/command"
)

//go:embed workspace
var embeddedFiles embed.FS

//go:embed default
var defaultFiles embed.FS

//go:embed config
var configFiles embed.FS

var (
	version   = "dev"
	gitCommit string
	buildTime string
	goVersion string
)

const logo = "🤖"

func main() {
	// Pass embedded files and version info to command package
	command.SetEmbeddedFS(embeddedFiles, defaultFiles)
	command.SetVersionInfo(version, gitCommit, buildTime, goVersion)

	// Initialize embedded default configurations
	if err := config.SetEmbeddedDefaults(configFiles); err != nil {
		fmt.Fprintf(os.Stderr, "Warning: Failed to load embedded default configs: %v\n", err)
	}

	if len(os.Args) < 2 {
		command.PrintHelp()
		os.Exit(1)
	}

	// Check if it's the onboard command (needs access to embedded files)
	if os.Args[1] == "onboard" {
		onboard()
	}

	// Route to command dispatcher
	command.Dispatch()
}

func onboard() {
	// Check for "default" parameter
	if len(os.Args) >= 3 && os.Args[2] == "default" {
		onboardDefault()
		return
	}

	// Standard onboard flow
	configPath := command.GetConfigPath()

	if _, err := os.Stat(configPath); err == nil {
		fmt.Printf("Config already exists at %s\n", configPath)
		fmt.Print("Overwrite? (y/n): ")
		var response string
		fmt.Scanln(& response)
		if response != "y" {
			fmt.Println("Aborted.")
			return
		}
		fmt.Println("Overwriting existing configuration...")
	}

	cfg := config.DefaultConfig()
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("✓ Config saved to: %s\n", configPath)

	// Create MCP config file
	mcpConfigPath := command.GetMCPConfigPath()
	mcpConfig := &config.MCPConfig{
		Enabled: true,
		Servers: []config.MCPServerConfig{},
		Timeout: 30,
	}
	if err := config.SaveMCPConfig(mcpConfigPath, mcpConfig); err != nil {
		fmt.Printf("Warning: Failed to create MCP config: %v\n", err)
		// Don't exit on MCP config failure, it's optional
	} else {
		fmt.Printf("✓ MCP config created at: %s\n", mcpConfigPath)
	}

	// Create security config file
	securityConfigPath := command.GetSecurityConfigPath()
	securityCfg, err := config.LoadSecurityConfig(securityConfigPath)
	if err != nil {
		fmt.Printf("Warning: Failed to create security config: %v\n", err)
	} else {
		if err := config.SaveSecurityConfig(securityConfigPath, securityCfg); err != nil {
			fmt.Printf("Warning: Failed to save security config: %v\n", err)
		} else {
			fmt.Printf("✓ Security config created at: %s\n", securityConfigPath)
		}
	}

	workspace := cfg.WorkspacePath()
	createWorkspaceTemplates(workspace)

	fmt.Printf("%s nemesisbot is ready!\n", logo)
	fmt.Println("\nNext steps:")
	fmt.Println("  1. Add your API key to", configPath)
	fmt.Println("     Get one at: https://openrouter.ai/keys")
	fmt.Println("  2. Chat: nemesisbot agent -m \"Hello!\"")
	fmt.Println("\nMCP servers:")
	fmt.Println("  Add MCP servers using: nemesisbot mcp add -n <name> -c <command>")
	fmt.Println("  List MCP servers: nemesisbot mcp list")
}

func copyEmbeddedToTarget(targetDir string) error {
	// Ensure target directory exists
	if err := os.MkdirAll(targetDir, 0755); err != nil {
		return fmt.Errorf("failed to create target directory: %w", err)
	}

	// Walk through all files in embed.FS
	err := fs.WalkDir(embeddedFiles, "workspace", func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}

		// Skip directories
		if d.IsDir() {
			return nil
		}

		// Read embedded file
		data, err := embeddedFiles.ReadFile(path)
		if err != nil {
			return fmt.Errorf("failed to read embedded file %s: %w", path, err)
		}

		new_path, err := filepath.Rel("workspace", path)
		if err != nil {
			return fmt.Errorf("failed to get relative path for %s: %v\n", path, err)
		}

		// Build target file path
		targetPath := filepath.Join(targetDir, new_path)

		// Ensure target file's directory exists
		if err := os.MkdirAll(filepath.Dir(targetPath), 0755); err != nil {
			return fmt.Errorf("failed to create directory %s: %w", filepath.Dir(targetPath), err)
		}

		// Write file
		if err := os.WriteFile(targetPath, data, 0644); err != nil {
			return fmt.Errorf("failed to write file %s: %w", targetPath, err)
		}

		return nil
	})

	return err
}

// onboardDefault initializes NemesisBot with default settings for quick start
func onboardDefault() {
	fmt.Println("🚀 Initializing NemesisBot with default settings...")
	fmt.Println()

	// Step 1: Load embedded default configuration
	// This uses config/config.default.json as the single source of truth
	configPath := command.GetConfigPath()

	// Overwrite existing config without prompting for 'default' command
	if _, err := os.Stat(configPath); err == nil {
		fmt.Printf("⚠️  Config already exists at %s, overwriting...\n", configPath)
	}

	// Load from embedded config (config/config.default.json)
	cfg, err := config.LoadEmbeddedConfig()
	if err != nil {
		fmt.Printf("❌ Error loading embedded default config: %v\n", err)
		fmt.Printf("   This should not happen. Please check that config/config.default.json exists.\n")
		os.Exit(1)
	}

	// Save base config
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("❌ Error saving config: %v\n", err)
		os.Exit(1)
	}
	fmt.Println("✓ Config saved (from embedded config/config.default.json)")

	// Create MCP config file
	mcpConfigPath := command.GetMCPConfigPath()
	mcpConfig, err := config.LoadMCPConfig(mcpConfigPath)
	if err != nil {
		// If MCP config doesn't exist, try to use embedded default
		embeddedDefaults := config.GetEmbeddedDefaults()
		if len(embeddedDefaults.MCP) > 0 {
			var mcpCfg config.MCPConfig
			if err := json.Unmarshal(embeddedDefaults.MCP, &mcpCfg); err != nil {
				fmt.Printf("⚠️  Warning: Failed to parse embedded MCP config: %v\n", err)
			} else {
				mcpConfig = &mcpCfg
			}
		}
	}
	if mcpConfig == nil {
		// Fallback to hardcoded default
		mcpConfig = &config.MCPConfig{
			Enabled: true,
			Servers: []config.MCPServerConfig{},
			Timeout: 30,
		}
	}
	if err := config.SaveMCPConfig(mcpConfigPath, mcpConfig); err != nil {
		fmt.Printf("⚠️  Warning: Failed to create MCP config: %v\n", err)
	} else {
		fmt.Println("✓ MCP config created")
	}

	// Create security config file
	securityConfigPath := command.GetSecurityConfigPath()
	securityCfg, err := config.LoadSecurityConfig(securityConfigPath)
	if err != nil {
		// If security config doesn't exist, try to use embedded default
		embeddedDefaults := config.GetEmbeddedDefaults()
		if len(embeddedDefaults.Security) > 0 {
			var secCfg config.SecurityConfig
			if err := json.Unmarshal(embeddedDefaults.Security, &secCfg); err != nil {
				fmt.Printf("⚠️  Warning: Failed to parse embedded security config: %v\n", err)
			} else {
				securityCfg = &secCfg
			}
		}
	}
	if securityCfg == nil {
		// Fallback to hardcoded default
		securityCfg = &config.SecurityConfig{
			DefaultAction:         "ask",
			LogAllOperations:      false,
			LogDenialsOnly:        true,
			ApprovalTimeout:       300,
			MaxPendingRequests:    10,
			AuditLogRetentionDays: 30,
			AuditLogFileEnabled:   true,
			SynchronousMode:       false,
		}
	}
	if err := config.SaveSecurityConfig(securityConfigPath, securityCfg); err != nil {
		fmt.Printf("⚠️  Warning: Failed to save security config: %v\n", err)
	} else {
		fmt.Println("✓ Security config created")
	}

	// Step 2: Enable LLM logging (optional enhancement for default mode)
	if cfg.Logging == nil {
		cfg.Logging = &config.LoggingConfig{
			LLMRequests: true,
			LogDir:      "~/.nemesisbot/workspace/logs/request_logs",
			DetailLevel: "full",
		}
		if err := config.SaveConfig(configPath, cfg); err != nil {
			fmt.Printf("⚠️  Warning: Failed to enable LLM logging: %v\n", err)
		} else {
			fmt.Println("✓ LLM logging enabled")
		}
	}

	// Step 3: Enable security module (optional enhancement for default mode)
	if cfg.Security == nil {
		cfg.Security = &config.SecurityFlagConfig{}
	}
	// Always enable security for default mode
	cfg.Security.Enabled = true
	// When security is enabled, keep restrict_to_workspace as-is from config
	// Security module will enforce access through rules instead
	// Don't override the config default settings
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("⚠️  Warning: Failed to enable security module: %v\n", err)
	} else {
		fmt.Println("✓ Security module enabled")
	}

	// Create workspace
	workspace := cfg.WorkspacePath()
	createWorkspaceTemplates(workspace)
	fmt.Println("✓ Workspace templates created")

	// Step 4: Copy default personality files
	if err := copyDefaultFiles(workspace); err != nil {
		fmt.Printf("⚠️  Warning: Failed to copy default personality files: %v\n", err)
	} else {
		fmt.Println("✓ Default personality files installed (IDENTITY.md, SOUL.md, USER.md)")
	}

	// Step 5: Delete BOOTSTRAP.md
	if err := deleteBootstrapFile(workspace); err != nil {
		// Don't show warning if file doesn't exist
		if !os.IsNotExist(err) {
			fmt.Printf("⚠️  Warning: Failed to delete BOOTSTRAP.md: %v\n", err)
		}
	} else {
		fmt.Println("✓ BOOTSTRAP.md removed")
	}

	// Step 6: Set default web authentication token
	cfg.Channels.Web.AuthToken = "276793422"
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("⚠️  Warning: Failed to set web auth token: %v\n", err)
	} else {
		fmt.Println("✓ Web authentication token set")
	}

	fmt.Println()
	fmt.Printf("%s Initialization complete!\n", logo)
	fmt.Println()
	fmt.Println("📝 Next step:")
	fmt.Println("  Configure your LLM API key and start chatting:")
	fmt.Println()
	fmt.Println("  1. Add API key:")
	fmt.Println("     nemesisbot model add --model zhipu/glm-4.7-flash --key YOUR_API_KEY --default")
	fmt.Println()
	fmt.Println("  2. Start chatting:")
	fmt.Println("     nemesisbot agent")
	fmt.Println()
	fmt.Println("  Or start the web gateway:")
	fmt.Println("     nemesisbot gateway")
	fmt.Println("     # Open http://localhost:18790 in your browser")
	fmt.Println("     # Default access key: 276793422")
	fmt.Println()
	fmt.Println("For more information:")
	fmt.Println("  nemesisbot --help")
}

// copyDefaultFiles copies default personality files from embedded FS to workspace
func copyDefaultFiles(workspace string) error {
	// List of files to copy
	files := []string{"IDENTITY.md", "SOUL.md", "USER.md"}

	for _, filename := range files {
		// Read from embedded FS
		// Note: embed.FS always uses forward slashes, even on Windows
		srcPath := "default/" + filename
		data, err := defaultFiles.ReadFile(srcPath)
		if err != nil {
			return fmt.Errorf("failed to read embedded file %s: %w", filename, err)
		}

		// Write to workspace
		dstPath := filepath.Join(workspace, filename)
		if err := os.WriteFile(dstPath, data, 0644); err != nil {
			return fmt.Errorf("failed to write file %s: %w", filename, err)
		}
	}

	return nil
}

// deleteBootstrapFile removes BOOTSTRAP.md from workspace
func deleteBootstrapFile(workspace string) error {
	bootstrapPath := filepath.Join(workspace, "BOOTSTRAP.md")
	return os.Remove(bootstrapPath)
}

func createWorkspaceTemplates(workspace string) {
	err := copyEmbeddedToTarget(workspace)
	if err != nil {
		fmt.Printf("Error copying workspace templates: %v\n", err)
	}
}
