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
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"github.com/276793422/NemesisBot/module/cluster"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/path"
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

// parseGlobalFlags parses global flags like --local and filters them from args.
// Returns the filtered args list (without global flags).
func parseGlobalFlags(args []string) []string {
	filtered := make([]string, 0, len(args))
	for _, arg := range args {
		if arg == "--local" {
			// Set LocalMode flag in path package
			path.LocalMode = true
			fmt.Println("📍 Local mode enabled: using ./.nemesisbot")
		} else {
			filtered = append(filtered, arg)
		}
	}
	return filtered
}

func main() {
	// Parse global flags first (before any command processing)
	// This handles --local flag which affects path resolution
	os.Args = append([]string{os.Args[0]}, parseGlobalFlags(os.Args[1:])...)

	// Pass embedded files and version info to command package
	command.SetEmbeddedFS(embeddedFiles, defaultFiles, configFiles)
	command.SetVersionInfo(version, gitCommit, buildTime, goVersion)

	if len(os.Args) < 2 {
		command.PrintHelp()
		os.Exit(1)
	}

	// Check if it's the onboard command (needs access to embedded files)
	if os.Args[1] == "onboard" {
		onboard()
	}

	// Check if it's the daemon command (independent service mode)
	if os.Args[1] == "daemon" {
		runDaemon()
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

	// Adjust paths for local mode if enabled
	// Check if we're in local mode (either explicit --local or auto-detected)
	if path.LocalMode || path.DetectLocal() {
		// Set workspace to relative path for local mode
		cfg.Agents.Defaults.Workspace = filepath.Join(".nemesisbot", "workspace")
		// Log directory is already relative in new design, no need to modify
	}

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
	cfg, err := config.LoadEmbeddedConfig()
	if err != nil {
		fmt.Printf("❌ Error loading embedded default config: %v\n", err)
		fmt.Printf("   This should not happen. Please check that config/config.default.json exists.\n")
		os.Exit(1)
	}

	// Adjust paths for local mode if enabled
	// Check if we're in local mode (either explicit --local or auto-detected)
	isLocalMode := path.LocalMode || path.DetectLocal()
	if isLocalMode {
		// Set workspace to relative path for local mode
		cfg.Agents.Defaults.Workspace = filepath.Join(".nemesisbot", "workspace")
	}

	// Save main config to .nemesisbot/config.json (root directory)
	configPath := command.GetConfigPath()
	if _, err := os.Stat(configPath); err == nil {
		fmt.Printf("⚠️  Config already exists at %s, overwriting...\n", configPath)
	}

	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("❌ Error saving config: %v\n", err)
		os.Exit(1)
	}
	fmt.Println("✓ Main config saved to .nemesisbot/config.json")

	// Create workspace config directory
	workspace := cfg.WorkspacePath()
	configDir := filepath.Join(workspace, "config")
	if err := os.MkdirAll(configDir, 0755); err != nil {
		fmt.Printf("❌ Error creating config directory: %v\n", err)
		os.Exit(1)
	}

	// Step 2: Create MCP config file in workspace/config/
	embeddedDefaults := config.GetEmbeddedDefaults()
	mcpConfigPath := filepath.Join(configDir, "config.mcp.json")
	mcpConfig, err := config.LoadMCPConfig(mcpConfigPath)
	if err != nil {
		// If MCP config doesn't exist, try to use embedded default
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

	// Step 3: Create security config file in workspace/config/
	securityConfigPath := filepath.Join(configDir, "config.security.json")
	securityCfg, err := config.LoadSecurityConfig(securityConfigPath)
	if err != nil {
		// If security config doesn't exist, try to use embedded default
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

	// Step 4: Create cluster config file in workspace/config/
	clusterConfigPath := filepath.Join(configDir, "config.cluster.json")
	if len(embeddedDefaults.Cluster) > 0 {
		// Write embedded cluster config directly
		if err := os.WriteFile(clusterConfigPath, embeddedDefaults.Cluster, 0644); err != nil {
			fmt.Printf("⚠️  Warning: Failed to create cluster config: %v\n", err)
		} else {
			fmt.Println("✓ Cluster config created")
		}
	} else {
		// Fallback to hardcoded default
		clusterCfg := map[string]interface{}{
			"enabled":            false,
			"port":               11949,
			"rpc_port":           21949,
			"broadcast_interval": 30,
		}
		data, _ := json.MarshalIndent(clusterCfg, "", "  ")
		if err := os.WriteFile(clusterConfigPath, data, 0644); err != nil {
			fmt.Printf("⚠️  Warning: Failed to create cluster config: %v\n", err)
		} else {
			fmt.Println("✓ Cluster config created")
		}
	}

	// Step 4.5: Initialize cluster peers.toml (static configuration)
	initializeClusterConfig(workspace)

	// Step 5: Enable LLM logging (optional enhancement for default mode)
	if cfg.Logging == nil {
		cfg.Logging = &config.LoggingConfig{
			LLMRequests: true,
			LogDir:      "logs/request_logs",  // Relative path
			DetailLevel: "full",
		}
		if err := config.SaveConfig(configPath, cfg); err != nil {
			fmt.Printf("⚠️  Warning: Failed to enable LLM logging: %v\n", err)
		} else {
			fmt.Println("✓ LLM logging enabled")
		}
	}

	// Step 6: Enable security module (optional enhancement for default mode)
	if cfg.Security == nil {
		cfg.Security = &config.SecurityFlagConfig{}
	}
	// Always enable security for default mode
	cfg.Security.Enabled = true
	// When security is enabled, disable restrict_to_workspace to allow file operations outside workspace
	// Security module will enforce access through rules instead
	cfg.Agents.Defaults.RestrictToWorkspace = false
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("⚠️  Warning: Failed to enable security module: %v\n", err)
	} else {
		fmt.Println("✓ Security module enabled")
	}

	// Create workspace templates
	createWorkspaceTemplates(workspace)
	fmt.Println("✓ Workspace templates created")

	// Step 7: Copy default personality files
	if err := copyDefaultFiles(workspace); err != nil {
		fmt.Printf("⚠️  Warning: Failed to copy default personality files: %v\n", err)
	} else {
		fmt.Println("✓ Default personality files installed (IDENTITY.md, SOUL.md, USER.md)")
	}

	// Step 8: Delete BOOTSTRAP.md
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

	// Step 7: Set web server host to 127.0.0.1
	cfg.Channels.Web.Host = "127.0.0.1"
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("⚠️  Warning: Failed to set web host: %v\n", err)
	} else {
		fmt.Println("✓ Web server host set to 127.0.0.1")
	}

	// Step 8: Set web server port to 49000
	cfg.Channels.Web.Port = 49000
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("⚠️  Warning: Failed to set web port: %v\n", err)
	} else {
		fmt.Println("✓ Web server port set to 49000")
	}

	// Step 9: Enable WebSocket channel for external program integration
	cfg.Channels.WebSocket.Enabled = true
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("⚠️  Warning: Failed to enable WebSocket channel: %v\n", err)
	} else {
		fmt.Println("✓ WebSocket channel enabled")
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
	fmt.Println("  2. Start the gateway:")
	fmt.Println("     nemesisbot gateway")
	fmt.Println()
	fmt.Println("  Available interfaces:")
	fmt.Println("    • Web:     http://127.0.0.1:49000 (access key: 276793422)")
	fmt.Println("    • WebSocket: ws://127.0.0.1:49001/ws")
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

// initializeClusterConfig creates the static peers.toml configuration file
func initializeClusterConfig(workspace string) {
	clusterDir := filepath.Join(workspace, "cluster")
	if err := os.MkdirAll(clusterDir, 0755); err != nil {
		fmt.Printf("⚠️  Failed to create cluster directory: %v\n", err)
		return
	}

	// Generate a node ID (use a simple format for initial setup)
	hostname, _ := os.Hostname()
	nodeID := fmt.Sprintf("node-%s-%d", hostname, time.Now().Unix())
	nodeName := "Bot " + nodeID

	// Marshal to TOML manually (since we're not importing cluster package)
	tomlData := fmt.Sprintf(
		"[cluster]\n"+
		"id = \"manual\"\n"+
		"auto_discovery = true\n"+
		"last_updated = \"%s\"\n\n"+
		"[node]\n"+
		"id = \"%s\"\n"+
		"name = \"%s\"\n"+
		"address = \"\"\n"+
		"role = \"worker\"\n"+
		"category = \"general\"\n"+
		"tags = []\n"+
		"capabilities = []\n\n"+
		"peers = []\n",
		time.Now().Format(time.RFC3339),
		nodeID,
		nodeName)

	// Save to peers.toml
	peersPath := filepath.Join(clusterDir, "peers.toml")
	if err := os.WriteFile(peersPath, []byte(tomlData), 0644); err != nil {
		fmt.Printf("⚠️  Warning: Failed to create peers.toml: %v\n", err)
	} else {
		fmt.Println("✓ Peers config created at", peersPath)
	}
}

func createWorkspaceTemplates(workspace string) {
	err := copyEmbeddedToTarget(workspace)
	if err != nil {
		fmt.Printf("Error copying workspace templates: %v\n", err)
	}
}

// runDaemon runs an independent service without LLM
// Usage: nemesisbot daemon <module> <mode>
// Example: nemesisbot daemon cluster auto
func runDaemon() {
	if len(os.Args) < 4 {
		fmt.Println("Usage: nemesisbot daemon <module> <mode>")
		fmt.Println()
		fmt.Println("Available modules:")
		fmt.Println("  cluster  - Cluster daemon service")
		fmt.Println()
		fmt.Println("Available modes:")
		fmt.Println("  auto     - Automatic mode (follows preset configuration)")
		fmt.Println()
		fmt.Println("Example:")
		fmt.Println("  nemesisbot daemon cluster auto")
		os.Exit(1)
	}

	module := os.Args[2]
	mode := os.Args[3]

	switch module {
	case "cluster":
		if mode == "auto" {
			runClusterDaemon()
		} else {
			fmt.Printf("Error: Unknown mode '%s' for module 'cluster'\n", mode)
			fmt.Println("Available modes: auto")
			os.Exit(1)
		}
	default:
		fmt.Printf("Error: Unknown module '%s'\n", module)
		fmt.Println("Available modules: cluster")
		os.Exit(1)
	}
}

// runClusterDaemon runs the cluster daemon service
// This service runs independently without LLM, only handling cluster discovery and communication
func runClusterDaemon() {
	// Debug: Print environment variable
	envHome := os.Getenv("NEMESISBOT_HOME")
	fmt.Printf("DEBUG: NEMESISBOT_HOME env var = '%s'\n", envHome)

	// Load config to get workspace path
	homeDir, err := path.ResolveHomeDir()
	if err != nil {
		fmt.Printf("Error resolving home directory: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("DEBUG: Resolved homeDir = '%s'\n", homeDir)

	configPath := filepath.Join(homeDir, "config.json")
	cfg, err := config.LoadConfig(configPath)
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Get workspace
	workspace := cfg.Agents.Defaults.Workspace
	if strings.HasPrefix(workspace, "~/") {
		homeDir, _ := os.UserHomeDir()
		workspace = filepath.Join(homeDir, workspace[2:])
	}

	// Setup logging
	logDir := filepath.Join(workspace, "logs", "cluster")
	if err := os.MkdirAll(logDir, 0755); err != nil {
		fmt.Printf("Error creating log directory: %v\n", err)
		os.Exit(1)
	}

	logFile := filepath.Join(logDir, "daemon.log")
	logF, err := os.OpenFile(logFile, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0644)
	if err != nil {
		fmt.Printf("Error opening log file: %v\n", err)
		os.Exit(1)
	}
	defer logF.Close()

	// Create logger function
	log := func(level, format string, args ...interface{}) {
		timestamp := time.Now().Format("2006-01-02 15:04:05")
		message := fmt.Sprintf(format, args...)
		logLine := fmt.Sprintf("[%s] %s %s\n", timestamp, level, message)

		// Write to file
		logF.WriteString(logLine)
		logF.Sync() // Force flush to disk

		// Print to console
		fmt.Print(logLine)
	}

	log("INFO", "🌐 Cluster Daemon Starting")
	log("INFO", "Workspace: %s", workspace)

	// Load cluster config
	clusterCfg, err := cluster.LoadAppConfig(workspace)
	if err != nil {
		log("ERROR", "Failed to load cluster config: %v", err)
		os.Exit(1)
	}

	// Daemon mode: Ignore enabled flag and always start cluster
	if !clusterCfg.Enabled {
		log("INFO", "Daemon mode: Starting cluster despite enabled=false")
		log("INFO", "Cluster will be forced enabled for daemon operation")
	}

	// Create cluster instance
	clusterInstance, err := cluster.NewCluster(workspace)
	if err != nil {
		log("ERROR", "Failed to create cluster: %v", err)
		os.Exit(1)
	}

	// Set ports from config
	clusterInstance.SetPorts(clusterCfg.Port, clusterCfg.RPCPort)

	// Start cluster
	log("INFO", "Starting cluster service...")
	if err := clusterInstance.Start(); err != nil {
		log("ERROR", "Failed to start cluster: %v", err)
		os.Exit(1)
	}

	// Register basic RPC handlers (including hello)
	log("INFO", "Registering RPC handlers...")
	if err := clusterInstance.RegisterBasicHandlers(); err != nil {
		log("ERROR", "Failed to register RPC handlers: %v", err)
		os.Exit(1)
	}

	log("INFO", "✓ Cluster started")
	log("INFO", "  Node ID: %s", clusterInstance.GetNodeID())
	log("INFO", "  UDP Port: %d", clusterCfg.Port)
	log("INFO", "  RPC Port: %d", clusterCfg.RPCPort)
	log("INFO", "  Address: %s", clusterInstance.GetAddress())
	log("INFO", "")

	// Setup signal handling for graceful shutdown
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	// RPC call ticker
	rpcTicker := time.NewTicker(60 * time.Second)
	defer rpcTicker.Stop()

	// State sync ticker (periodic full sync)
	syncTicker := time.NewTicker(5 * time.Minute)
	defer syncTicker.Stop()

	log("INFO", "Daemon running. Press Ctrl+C to stop.")
	log("INFO", "")

	// Main loop
	for {
		select {
		case <-rpcTicker.C:
			// Call all online nodes every 60 seconds
			registry := clusterInstance.GetRegistry()
			var onlineNodes []*cluster.Node

			// Type assertion for registry
			if reg, ok := registry.(*cluster.Registry); ok {
				onlineNodes = reg.GetOnline()
			}

			if len(onlineNodes) == 0 {
				log("INFO", "RPC: No nodes to call")
			} else {
				log("INFO", "RPC: Calling %d nodes...", len(onlineNodes))
				for _, node := range onlineNodes {
					go func(n *cluster.Node) {
						log("DEBUG", "RPC -> %s (%s): Starting RPC call...", n.ID, n.Address)
						log("DEBUG", "RPC -> %s: Calling clusterInstance.Call()", n.ID)
						response, err := clusterInstance.Call(n.ID, "hello", map[string]interface{}{
							"from": clusterInstance.GetNodeID(),
							"timestamp": time.Now().Format(time.RFC3339),
						})
						log("DEBUG", "RPC -> %s: Call returned, err=%v", n.ID, err)
						if err != nil {
							log("WARN", "RPC -> %s: Error: %v", n.ID, err)
						} else {
							log("INFO", "RPC -> %s: Response: %s", n.ID, string(response))
						}
					}(node)
				}
			}

		case <-syncTicker.C:
			// Periodic full sync
			log("DEBUG", "Syncing state to disk...")
			if err := clusterInstance.SyncToDisk(); err != nil {
				log("ERROR", "Failed to sync state: %v", err)
			} else {
				log("DEBUG", "State synced successfully")
			}

		case <-sigCh:
			// Shutdown signal
			log("INFO", "")
			log("INFO", "Received shutdown signal, stopping daemon...")

			// Timeout mechanism: Stop cluster with 60-second timeout
			stopDone := make(chan error, 1)
			go func() {
				stopDone <- clusterInstance.Stop()
			}()

			select {
			case err := <-stopDone:
				if err != nil {
					log("ERROR", "Error stopping cluster: %v", err)
				} else {
					log("INFO", "✓ Cluster stopped")
				}
			case <-time.After(60 * time.Second):
				log("WARN", "Shutdown timeout (60s), forcing exit...")
			}

			log("INFO", "Daemon stopped.")
			os.Exit(0)
		}
	}
}
