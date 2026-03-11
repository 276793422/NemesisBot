package command

import (
	"fmt"
	"os"
	"strings"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/services"
)

// CmdGateway starts the NemesisBot gateway server
// This command starts the bot service immediately (traditional behavior)
func CmdGateway() {
	// Load configuration first
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Initialize logger from config
	args := os.Args[2:]
	InitLoggerFromConfig(cfg, args)

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

	// Wait for shutdown signal
	svcMgr.WaitForShutdown()

	// Shutdown
	fmt.Println("\nShutting down...")
	svcMgr.Shutdown()
	fmt.Println("✓ Gateway stopped")
}

// printAgentStartupInfo prints agent startup information
func printAgentStartupInfo(cfg *config.Config) {
	// Create temporary components for startup info display
	provider, err := providers.CreateProvider(cfg)
	if err != nil {
		fmt.Printf("Error creating provider: %v\n", err)
		return
	}

	msgBus := bus.NewMessageBus()
	agentLoop := agent.NewAgentLoop(cfg, msgBus, provider)

	startupInfo := agentLoop.GetStartupInfo()
	toolsInfo := startupInfo["tools"].(map[string]interface{})
	skillsInfo := startupInfo["skills"].(map[string]interface{})

	fmt.Println("\n📦 Agent Status:")
	fmt.Printf("  • Tools: %d loaded\n", toolsInfo["count"])
	fmt.Printf("  • Skills: %d/%d available\n",
		skillsInfo["available"],
		skillsInfo["total"])

	logger.InfoCF("agent", "Agent initialized",
		map[string]interface{}{
			"tools_count":      toolsInfo["count"],
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
