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
	"github.com/276793422/NemesisBot/module/desktop"
	"github.com/276793422/NemesisBot/module/desktop/systray"
	"github.com/276793422/NemesisBot/module/logger"
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

// isChildMode checks if the process is running in child mode
// Child processes are started with --multiple flag by ProcessManager
func isChildMode() bool {
	for _, arg := range os.Args {
		if arg == "--multiple" {
			return true
		}
	}
	return false
}

// shouldStartSystemTray checks if the process should start system tray
// System tray is only needed for non-window processes (gateway, daemon)
func shouldStartSystemTray() bool {
	// Child process mode → has window, no need for tray
	if isChildMode() {
		return false
	}

	// Need at least 2 args
	if len(os.Args) < 2 {
		return false
	}

	// Check command type
	cmd := os.Args[1]
	// Gateway and Daemon modes need system tray (no window)
	return cmd == "gateway" || cmd == "daemon"
}

// runChildMode runs the process in child mode
// This mode is used when the process is a child process created by ProcessManager
// It runs a window (e.g., approval window) instead of normal commands
func runChildMode() {
	if err := desktop.RunChildMode(); err != nil {
		fmt.Printf("❌ Child mode failed: %v\n", err)
		os.Exit(1)
	}
	os.Exit(0)
}

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

	// Check if it's child process mode (must be checked before other logic)
	// Child process is started by ProcessManager with --multiple flag
	if isChildMode() {
		runChildMode()
		return
	}

	// Start system tray for non-window processes (gateway, daemon)
	var systemTray *systray.SystemTray
	if shouldStartSystemTray() {
		fmt.Println("🔔 System tray enabled")
		systemTray = systray.NewSystemTray()

		// Set quit handler to trigger global shutdown
		systemTray.SetOnQuit(func() {
		fmt.Println("\n🛑 Shutdown requested from system tray")
		command.TriggerShutdown()
		systemTray.Stop()
	})

		// Run in goroutine (non-blocking)
		go func() {
		if err := systemTray.Run(); err != nil {
			fmt.Printf("⚠️  System tray error: %v\n", err)
		}
		}()
		// Cleanup on exit
		defer func() {
			if systemTray != nil {
				systemTray.Stop()
			}
		}()
	}

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
	fmt.Printf("📟 Detected platform: %s\n", config.GetPlatformInfo())
	fmt.Println("🔒 Applying platform-specific security rules...")

	configPath := command.GetConfigPath()

	if _, err := os.Stat(configPath); err == nil {
		fmt.Printf("Config already exists at %s\n", configPath)
		fmt.Print("Overwrite? (y/n): ")
		var response string
		fmt.Scanln(&response)
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
			fmt.Printf("✓ Security config created at: %s (%s)\n", securityConfigPath, config.GetPlatformDisplayName())
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
	fmt.Printf("📟 Detected platform: %s\n", config.GetPlatformInfo())
	fmt.Println("🔒 Applying platform-specific security rules...")
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
		fmt.Printf("✓ Security config created (%s rules)\n", config.GetPlatformDisplayName())
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

	// Step 4.6: Create skills config file in workspace/config/
	skillsConfigPath := filepath.Join(configDir, "config.skills.json")
	skillsCfg, err := config.LoadSkillsConfig(skillsConfigPath)
	if err != nil {
		// If skills config doesn't exist, try to use embedded default
		if len(embeddedDefaults.Skills) > 0 {
			var sCfg config.SkillsFullConfig
			if err := json.Unmarshal(embeddedDefaults.Skills, &sCfg); err != nil {
				fmt.Printf("⚠️  Warning: Failed to parse embedded skills config: %v\n", err)
			} else {
				skillsCfg = &sCfg
			}
		}
	}
	if skillsCfg == nil {
		// Fallback to hardcoded default
		skillsCfg = &config.SkillsFullConfig{
			Enabled:               true,
			SearchCache:           config.SkillsSearchCacheConfig{Enabled: true, MaxSize: 50, TTLSeconds: 300},
			MaxConcurrentSearches: 2,
			GitHubSources:         []config.GitHubSourceConfig{},
			ClawHub:               config.SkillsClawHubConfig{Enabled: false},
		}
	}
	if err := config.SaveSkillsConfig(skillsConfigPath, skillsCfg); err != nil {
		fmt.Printf("⚠️  Warning: Failed to create skills config: %v\n", err)
	} else {
		fmt.Println("✓ Skills config created")
	}

	// Step 5: Enable LLM logging (optional enhancement for default mode)
	if cfg.Logging == nil {
		cfg.Logging = &config.LoggingConfig{
			LLM: &config.LLMLogConfig{
				Enabled:     true,
				LogDir:      "logs/request_logs", // Relative path
				DetailLevel: "full",
			},
		}
		fmt.Println("✓ LLM logging enabled")
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
	fmt.Println("✓ Security module enabled")

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

	// Step 9: Set web and WebSocket configuration
	cfg.Channels.Web.AuthToken = "276793422"
	cfg.Channels.Web.Host = "127.0.0.1"
	cfg.Channels.Web.Port = 49000
	cfg.Channels.WebSocket.Enabled = true
	fmt.Println("✓ Web and WebSocket configuration set")

	// Save all config changes in one write
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("❌ Error saving final config: %v\n", err)
		os.Exit(1)
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
	// Load config to get workspace path
	homeDir, err := path.ResolveHomeDir()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error resolving home directory: %v\n", err)
		os.Exit(1)
	}

	configPath := filepath.Join(homeDir, "config.json")
	cfg, err := config.LoadConfig(configPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Get workspace
	workspace := cfg.Agents.Defaults.Workspace
	if strings.HasPrefix(workspace, "~/") {
		hd, _ := os.UserHomeDir()
		workspace = filepath.Join(hd, workspace[2:])
	}

	// Setup logging directory
	logDir := filepath.Join(workspace, "logs", "cluster")
	if err := os.MkdirAll(logDir, 0755); err != nil {
		fmt.Fprintf(os.Stderr, "Error creating log directory: %v\n", err)
		os.Exit(1)
	}

	// Initialize unified logger with file output
	logFile := filepath.Join(logDir, "daemon.log")
	if err := logger.EnableFileLogging(logFile); err != nil {
		fmt.Fprintf(os.Stderr, "Error opening log file: %v\n", err)
		os.Exit(1)
	}

	logger.InfoC("daemon", "Cluster Daemon Starting")
	logger.InfoCF("daemon", "Workspace", map[string]interface{}{"path": workspace})

	// Load cluster config
	clusterCfg, err := cluster.LoadAppConfig(workspace)
	if err != nil {
		logger.ErrorCF("daemon", "Failed to load cluster config", map[string]interface{}{"error": err.Error()})
		os.Exit(1)
	}

	// Daemon mode: Ignore enabled flag and always start cluster
	if !clusterCfg.Enabled {
		logger.InfoC("daemon", "Daemon mode: Starting cluster despite enabled=false")
	}

	// Create cluster instance
	clusterInstance, err := cluster.NewCluster(workspace)
	if err != nil {
		logger.ErrorCF("daemon", "Failed to create cluster", map[string]interface{}{"error": err.Error()})
		os.Exit(1)
	}

	// Set ports from config
	clusterInstance.SetPorts(clusterCfg.Port, clusterCfg.RPCPort)

	// Start cluster
	logger.InfoC("daemon", "Starting cluster service...")
	if err := clusterInstance.Start(); err != nil {
		logger.ErrorCF("daemon", "Failed to start cluster", map[string]interface{}{"error": err.Error()})
		os.Exit(1)
	}

	// Register basic RPC handlers (including hello)
	logger.InfoC("daemon", "Registering RPC handlers...")
	if err := clusterInstance.RegisterBasicHandlers(); err != nil {
		logger.ErrorCF("daemon", "Failed to register RPC handlers", map[string]interface{}{"error": err.Error()})
		os.Exit(1)
	}

	logger.InfoCF("daemon", "Cluster started", map[string]interface{}{
		"node_id":   clusterInstance.GetNodeID(),
		"udp_port":  clusterCfg.Port,
		"rpc_port":  clusterCfg.RPCPort,
		"address":   clusterInstance.GetAddress(),
	})

	// Setup signal handling for graceful shutdown
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	// RPC call ticker
	rpcTicker := time.NewTicker(60 * time.Second)
	defer rpcTicker.Stop()

	// State sync ticker (periodic full sync)
	syncTicker := time.NewTicker(5 * time.Minute)
	defer syncTicker.Stop()

	logger.InfoC("daemon", "Daemon running. Press Ctrl+C to stop.")

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
				logger.DebugC("daemon", "RPC: No nodes to call")
			} else {
				logger.InfoCF("daemon", "RPC: Calling nodes", map[string]interface{}{"count": len(onlineNodes)})
				for _, node := range onlineNodes {
					go func(n *cluster.Node) {
						logger.DebugCF("daemon", "RPC call starting", map[string]interface{}{"node": n.ID, "address": n.Address})
						response, err := clusterInstance.Call(n.ID, "hello", map[string]interface{}{
							"from":      clusterInstance.GetNodeID(),
							"timestamp": time.Now().Format(time.RFC3339),
						})
						if err != nil {
							logger.WarnCF("daemon", "RPC call failed", map[string]interface{}{"node": n.ID, "error": err.Error()})
						} else {
							logger.InfoCF("daemon", "RPC response", map[string]interface{}{"node": n.ID, "response": string(response)})
						}
					}(node)
				}
			}

		case <-syncTicker.C:
			// Periodic full sync
			logger.DebugC("daemon", "Syncing state to disk...")
			if err := clusterInstance.SyncToDisk(); err != nil {
				logger.ErrorCF("daemon", "Failed to sync state", map[string]interface{}{"error": err.Error()})
			} else {
				logger.DebugC("daemon", "State synced successfully")
			}

		case <-sigCh:
			// Shutdown signal
			logger.InfoC("daemon", "Received shutdown signal, stopping daemon...")

			// Timeout mechanism: Stop cluster with 60-second timeout
			stopDone := make(chan error, 1)
			go func() {
				stopDone <- clusterInstance.Stop()
			}()

			select {
			case err := <-stopDone:
				if err != nil {
					logger.ErrorCF("daemon", "Error stopping cluster", map[string]interface{}{"error": err.Error()})
				} else {
					logger.InfoC("daemon", "Cluster stopped")
				}
			case <-time.After(60 * time.Second):
				logger.WarnC("daemon", "Shutdown timeout (60s), forcing exit...")
			}

			logger.InfoC("daemon", "Daemon stopped.")
			os.Exit(0)
		}
	}
}
