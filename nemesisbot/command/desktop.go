package command

import (
	"fmt"
	"os"
	"strings"

	"github.com/276793422/NemesisBot/nemesisbot/desktop"
	"github.com/276793422/NemesisBot/module/services"
)

// CmdDesktop starts NemesisBot with Desktop UI
// The Desktop UI starts but the bot service does NOT start automatically
// Users can start/stop the bot through the Desktop UI
func CmdDesktop() {
	// Load configuration first (may not exist yet)
	cfg, err := LoadConfig()
	if err != nil {
		// Config doesn't exist - this is OK for desktop mode
		// The UI will guide the user through configuration
		fmt.Println("Note: No configuration found. Desktop UI will guide you through setup.")
		cfg = nil
	} else {
		// Initialize logger from config if available
		args := os.Args[2:]
		InitLoggerFromConfig(cfg, args)
	}

	// Check system requirements for Desktop UI
	if !desktop.CheckSystemRequirements() {
		fmt.Println("System requirements not met for Desktop UI")
		fmt.Println("Please install the required components:")
		fmt.Println("  - WebView2 Runtime: https://developer.microsoft.com/en-us/microsoft-edge/webview2/")
		fmt.Println("\nOr use: nemesisbot gateway")
		os.Exit(1)
	}

	// Create service manager
	svcMgr := services.NewServiceManager()

	// Start basic services (HTTP, etc.) - bot NOT started
	if err := svcMgr.StartBasicServices(); err != nil {
		fmt.Printf("Error starting basic services: %v\n", err)
		os.Exit(1)
	}

	// Create Desktop UI configuration
	desktopCfg := &desktop.Config{
		Enabled: true,
		Debug:   false,
	}

	// Channel to signal when Desktop UI window is closed
	desktopClosed := make(chan struct{})

	// Start Desktop UI in a separate goroutine (non-blocking)
	fmt.Println("🖥️  Starting Desktop UI...")
	go func() {
		defer close(desktopClosed)
		desktop.RunWithServiceManager(desktopCfg, svcMgr)
		fmt.Println("\nDesktop UI window closed")
	}()

	// Print startup banner
	printDesktopBanner(svcMgr)

	// Wait for either:
	// 1. Shutdown signal (Ctrl+C)
	// 2. Desktop UI window closed
	svcMgr.WaitForShutdownWithDesktop(desktopClosed)

	// Shutdown
	fmt.Println("\nShutting down...")
	svcMgr.Shutdown()
	fmt.Println("✓ NemesisBot Desktop stopped")
}

// printDesktopBanner prints the desktop mode banner
func printDesktopBanner(svcMgr *services.ServiceManager) {
	botState := svcMgr.GetBotState()

	fmt.Println("\n" + strings.Repeat("=", 50))
	fmt.Println("🖥️  NemesisBot Desktop Mode")
	fmt.Println(strings.Repeat("=", 50))

	if botState.IsRunning() {
		fmt.Println("Status: Bot is running")
	} else {
		fmt.Println("Status: Bot is stopped (start through Desktop UI)")
	}

	fmt.Println("\n💡 Features:")
	fmt.Println("  • Desktop UI - Configuration & Management")
	fmt.Println("  • Bot Control - Start/Stop/Restart through UI")
	fmt.Println("  • Web Chat - http://127.0.0.1:49000 (when bot is running)")

	fmt.Println("\nControls:")
	fmt.Println("  • Use the Desktop UI to start/stop the bot")
	fmt.Println("  • Press Ctrl+C to stop everything")

	fmt.Println(strings.Repeat("=", 50) + "\n")
}
