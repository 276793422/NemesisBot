package command

import (
	"fmt"
	"os"
	"strings"

	"github.com/276793422/NemesisBot/module/desktop"
	"github.com/276793422/NemesisBot/module/services"
)

// CmdDesktop starts NemesisBot with Desktop UI (Wails v2)
// The Desktop UI starts in the nemesisbot.exe process (not as a separate process)
// The bot service does NOT start automatically
// Users can start/stop the bot through the Desktop UI
func CmdDesktop() {
	// Load configuration first (may not exist yet)
	cfg, err := LoadConfig()
	if err != nil {
		// Config doesn't exist - this is OK for desktop mode
		// The UI will guide the user through setup
		fmt.Println("Note: No configuration found. Desktop UI will guide you through setup.")
		cfg = nil
	} else {
		// Initialize logger from config if available
		args := os.Args[2:]
		InitLoggerFromConfig(cfg, args)
	}

	// Create service manager
	svcMgr := services.NewServiceManager()

	// Start basic services (HTTP, etc.) - bot NOT started
	if err := svcMgr.StartBasicServices(); err != nil {
		fmt.Printf("Error starting basic services: %v\n", err)
		os.Exit(1)
	}

	// Print startup banner
	printDesktopBanner(svcMgr)

	// Create desktop config
	desktopCfg := &desktop.Config{
		Enabled: true,
		Debug:   false,
	}

	// Start Wails Desktop UI in the current process (not as a separate process)
	// This will block until the Desktop UI window is closed
	fmt.Println("🖥️  Starting NemesisBot Desktop UI (Wails v2)...")
	if err := desktop.RunWithServiceManager(desktopCfg, svcMgr); err != nil {
		fmt.Printf("Error starting Desktop UI: %v\n", err)
		svcMgr.Shutdown()
		os.Exit(1)
	}

	// Shutdown
	fmt.Println("\nShutting down...")
	svcMgr.Shutdown()
	fmt.Println("✓ NemesisBot Desktop stopped")
}

// printDesktopBanner prints the desktop mode banner
func printDesktopBanner(svcMgr *services.ServiceManager) {
	botState := svcMgr.GetBotState()

	fmt.Println("\n" + strings.Repeat("=", 50))
	fmt.Println("🖥️  NemesisBot Desktop Mode (Wails v2)")
	fmt.Println(strings.Repeat("=", 50))

	if botState.IsRunning() {
		fmt.Println("Status: Bot is running")
	} else {
		fmt.Println("Status: Bot is stopped (start through Desktop UI)")
	}

	fmt.Println("\n💡 Features:")
	fmt.Println("  • Desktop UI - Configuration & Management (Wails v2)")
	fmt.Println("  • Bot Control - Start/Stop/Restart through UI")
	fmt.Println("  • Web Chat - http://127.0.0.1:49000 (when bot is running)")

	fmt.Println("\nControls:")
	fmt.Println("  • Use the Desktop UI to start/stop the bot")
	fmt.Println("  • Press Ctrl+C or close the window to stop")

	fmt.Println(strings.Repeat("=", 50) + "\n")
}
