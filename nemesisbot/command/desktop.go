package command

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/276793422/NemesisBot/module/services"
)

// GetProjectRoot returns the project root directory
func GetProjectRoot() string {
	// Get the executable path
	execPath, err := os.Executable()
	if err != nil {
		// Fallback to current working directory
		if cwd, err := os.Getwd(); err == nil {
			return cwd
		}
		return "."
	}

	// Get the directory of the executable
	execDir := filepath.Dir(execPath)

	// If executable is in project root (not in a subdirectory), use it directly
	// Otherwise, navigate up to find project root
	// Check if we're in nemesisbot/ subdirectory
	if filepath.Base(execDir) == "nemesisbot" {
		return filepath.Dir(execDir)
	}

	// Check if we have module/ subdirectory (indicating project root)
	if _, err := os.Stat(filepath.Join(execDir, "module")); err == nil {
		return execDir
	}

	// Fallback: use current working directory
	if cwd, err := os.Getwd(); err == nil {
		// Same checks for cwd
		if filepath.Base(cwd) == "nemesisbot" {
			return filepath.Dir(cwd)
		}
		if _, err := os.Stat(filepath.Join(cwd, "module")); err == nil {
			return cwd
		}
	}

	// Final fallback
	return execDir
}

// CmdDesktop starts NemesisBot with Desktop UI (Wails v2)
// The Desktop UI starts but the bot service does NOT start automatically
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

	// Get project root directory
	projectRoot := GetProjectRoot()

	// Path to the Desktop UI executable
	desktopExePath := filepath.Join(projectRoot, "module", "desktop", "build", "bin", "desktop.exe")

	// Check if Desktop UI executable exists
	if _, err := os.Stat(desktopExePath); os.IsNotExist(err) {
		fmt.Println("⚠️  Desktop UI executable not found. Building now...")

		// Build Desktop UI
		if err := buildDesktopUI(projectRoot); err != nil {
			fmt.Printf("Error building Desktop UI: %v\n", err)
			fmt.Println("\nPlease build manually:")
			fmt.Println("  cd module/desktop")
			fmt.Println("  wails build")
			os.Exit(1)
		}

		fmt.Println("✓ Desktop UI built successfully")
	}

	// Channel to signal when Desktop UI window is closed
	desktopClosed := make(chan struct{})

	// Start Desktop UI in a separate goroutine (non-blocking)
	fmt.Println("🖥️  Starting NemesisBot Desktop UI (Wails v2)...")
	go func() {
		defer close(desktopClosed)

		cmd := exec.Command(desktopExePath)
		cmd.Stdin = os.Stdin
		cmd.Stdout = os.Stdout
		cmd.Stderr = os.Stderr

		if err := cmd.Run(); err != nil {
			fmt.Printf("Desktop UI exited with error: %v\n", err)
		} else {
			fmt.Println("\nDesktop UI window closed")
		}
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

// buildDesktopUI builds the Wails Desktop UI
func buildDesktopUI(projectRoot string) error {
	desktopDir := filepath.Join(projectRoot, "module", "desktop")

	// Check if wails is installed
	if _, err := exec.LookPath("wails"); err != nil {
		return fmt.Errorf("wails command not found. Please install Wails v2: https://wails.io/docs/getting-started/installation")
	}

	// Change to desktop directory
	if err := os.Chdir(desktopDir); err != nil {
		return fmt.Errorf("failed to change to desktop directory: %w", err)
	}

	// Run wails build
	cmd := exec.Command("wails", "build")
	cmd.Dir = desktopDir
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	fmt.Println("Building Desktop UI (this may take a few seconds)...")

	if err := cmd.Run(); err != nil {
		return fmt.Errorf("wails build failed: %w", err)
	}

	return nil
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
	fmt.Println("  • Press Ctrl+C to stop everything")

	fmt.Println(strings.Repeat("=", 50) + "\n")
}
