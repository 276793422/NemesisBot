package command

import (
	"fmt"
	"os"
	"strings"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/desktop/process"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/path"
	"github.com/276793422/NemesisBot/module/security/approval"
	"github.com/276793422/NemesisBot/module/services"
)

// CmdGateway starts the NemesisBot gateway server
// This command starts the bot service immediately (traditional behavior)
func CmdGateway() {
	// Check configuration file exists first (Gateway mode requires config)
	configPath := GetConfigPath()
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		fmt.Printf("❌ Error: Configuration file not found: %s\n", configPath)
		fmt.Println("\n💡 Gateway mode requires a configuration file.")
		fmt.Println("   Run 'nemesisbot onboard default' to create one.")
		fmt.Println("\n   Or specify a custom path:")
		fmt.Println("   export NEMESISBOT_CONFIG=/path/to/config.json  (Linux/Mac)")
		fmt.Println("   set NEMESISBOT_CONFIG=C:\\path\\to\\config.json  (Windows)")
		os.Exit(1)
	}

	// Check home directory exists
	homeDir, err := path.ResolveHomeDir()
	if err != nil {
		fmt.Printf("❌ Error: Cannot resolve home directory: %v\n", err)
		os.Exit(1)
	}

	if _, err := os.Stat(homeDir); os.IsNotExist(err) {
		fmt.Printf("❌ Error: Configuration directory not found: %s\n", homeDir)
		fmt.Println("\n💡 Please initialize your configuration first.")
		fmt.Println("   Run 'nemesisbot onboard default' to create configuration.")
		os.Exit(1)
	}

	// Load configuration first
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("❌ Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Initialize logger from config
	args := os.Args[2:]
	InitLoggerFromConfig(cfg, args)

	// Initialize ProcessManager
	procMgr := process.NewProcessManager()
	if err := procMgr.Start(); err != nil {
		fmt.Printf("⚠️  ProcessManager initialization failed: %v\n", err)
		fmt.Println("   Multi-process window features will not be available.")
		fmt.Println("   This is expected in cross-compile builds.")
	} else {
		fmt.Println("✓ ProcessManager started")
		if process.PopupSupported {
			approval.SetChildProcessFactory(procMgr)
			fmt.Println("✓ Approval handler registered")
		} else {
			fmt.Println("⚠️  Popup not supported in this build")
			fmt.Println("   All approval requests will be auto-rejected.")
		}
	}
	defer func() {
		// Cleanup ProcessManager
		if err := procMgr.Stop(); err != nil {
			fmt.Printf("⚠️  ProcessManager cleanup warning: %v\n", err)
		}
	}()

	// Create service manager
	svcMgr := services.NewServiceManager()

	// Start basic services (HTTP, etc.)
	if err := svcMgr.StartBasicServices(); err != nil {
		fmt.Printf("Error starting basic services: %v\n", err)
		os.Exit(1)
	}

	// Print agent info (using temp components for display)
	printAgentStartupInfo(cfg)

	// Start bot service immediately (gateway behavior)
	if err := svcMgr.StartBot(); err != nil {
		fmt.Printf("Error starting bot service: %v\n", err)
		svcMgr.Shutdown()
		os.Exit(1)
	}

	// Print startup banner
	printGatewayBanner(cfg)

	// Wire system tray callbacks to service manager
	// Replace 0.0.0.0 with 127.0.0.1 for browser compatibility
	webHost := cfg.Channels.Web.Host
	if webHost == "0.0.0.0" || webHost == "" {
		webHost = "127.0.0.1"
	}
	webURL := fmt.Sprintf("http://%s:%d", webHost, cfg.Channels.Web.Port)
	chatURL := fmt.Sprintf("http://%s:%d/chat/", webHost, cfg.Channels.Web.Port)

	// Pass ProcessManager and auth info to SystemTray for Dashboard child process
	if globalSystemTray != nil {
		globalSystemTray.SetProcessManager(procMgr)
		globalSystemTray.SetAuthToken(cfg.Channels.Web.AuthToken)
		globalSystemTray.SetWebPort(cfg.Channels.Web.Port)
		globalSystemTray.SetWebHost(webHost)
	}

	ConfigureSystemTray(webURL, chatURL, svcMgr.StartBot, svcMgr.StopBot)

	// Wait for shutdown signal
	// Supports: Ctrl+C, system tray quit, desktop UI close, WebSocket close, etc.
	svcMgr.WaitForShutdownWithDesktop(GetGlobalShutdownChan())

	// Shutdown
	fmt.Println("\nShutting down...")
	svcMgr.Shutdown()
	fmt.Println("✓ Gateway stopped")
}

// printAgentStartupInfo prints agent startup information
func printAgentStartupInfo(cfg *config.Config) {
	// Use lightweight AgentRegistry (no cluster, no RPC, no message bus)
	// to get tool/skill counts without the overhead of a full AgentLoop
	registry := agent.NewAgentRegistry(cfg, nil)

	defaultAgent := registry.GetDefaultAgent()
	if defaultAgent == nil {
		return
	}

	toolsList := defaultAgent.Tools.List()
	skillsInfo := defaultAgent.ContextBuilder.GetSkillsInfo()

	fmt.Println("\n📦 Agent Status:")
	fmt.Printf("  • Tools: %d loaded\n", len(toolsList))
	fmt.Printf("  • Skills: %d/%d available\n",
		skillsInfo["available"],
		skillsInfo["total"])

	logger.InfoCF("agent", "Agent initialized",
		map[string]interface{}{
			"tools_count":      len(toolsList),
			"skills_total":     skillsInfo["total"],
			"skills_available": skillsInfo["available"],
		})
}

// printGatewayBanner prints the gateway startup banner
func printGatewayBanner(cfg *config.Config) {
	fmt.Println("\n" + strings.Repeat("=", 50))
	fmt.Println("🚀 NemesisBot Gateway")
	fmt.Println(strings.Repeat("=", 50))
	fmt.Printf("🌐 Web Interface: http://%s:%d\n", cfg.Channels.Web.Host, cfg.Channels.Web.Port)
	fmt.Printf("🔑 Auth Token: %s\n", cfg.Channels.Web.AuthToken)

	// Count enabled channels
	enabledCount := 0
	if cfg.Channels.Web.Enabled {
		enabledCount++
	}
	if cfg.Channels.Telegram.Enabled {
		enabledCount++
	}
	if cfg.Channels.Discord.Enabled {
		enabledCount++
	}
	if cfg.Channels.Feishu.Enabled {
		enabledCount++
	}
	if cfg.Channels.Slack.Enabled {
		enabledCount++
	}

	if enabledCount > 0 {
		fmt.Printf("✓ %d channel(s) enabled\n", enabledCount)
	} else {
		fmt.Println("⚠ Warning: No channels enabled")
	}

	fmt.Printf("✓ Gateway started on %s:%d\n", cfg.Gateway.Host, cfg.Gateway.Port)
	fmt.Println("\n💡 Press Ctrl+C to stop")
	fmt.Println(strings.Repeat("=", 50) + "\n")
}

// GatewayHelp prints gateway command help
func GatewayHelp() {
	fmt.Println("\nGateway - Start NemesisBot gateway server")
	fmt.Println()
	fmt.Println("Usage: nemesisbot gateway [options]")
	fmt.Println()
	fmt.Println("Options:")
	fmt.Println("  -d, --debug    Enable debug logging")
	fmt.Println("  -q, --quiet    Disable all logging")
	fmt.Println("      --no-console  Disable console output (file only)")
	fmt.Println()
	fmt.Println("The gateway starts all enabled channels and services:")
	fmt.Println("  • Web chat interface")
	fmt.Println("  • Telegram, Discord, Slack, and other bots")
	fmt.Println("  • Cron scheduler")
	fmt.Println("  • Heartbeat service")
	fmt.Println("  • Device monitoring")
	fmt.Println()
	fmt.Println("Press Ctrl+C to stop the gateway")
}
