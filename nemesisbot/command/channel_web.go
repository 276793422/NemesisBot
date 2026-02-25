// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Channel Web Command - Secure Web Channel Configuration

package command

import (
	"bufio"
	"fmt"
	"os"
	"strings"

	"github.com/276793422/NemesisBot/module/config"
)

// CmdChannelWeb handles web channel specific commands
func CmdChannelWeb(cfg *config.Config) {
	if len(os.Args) < 4 {
		ChannelWebHelp()
		return
	}

	subcommand := os.Args[3]

	switch subcommand {
	case "auth":
		// Check if there's a sub-subcommand
		if len(os.Args) >= 5 {
			authSubcommand := os.Args[4]
			switch authSubcommand {
			case "set":
				if len(os.Args) < 6 {
					fmt.Println("Usage: nemesisbot channel web auth set <token>")
					fmt.Println()
					fmt.Println("Example:")
					fmt.Println("  nemesisbot channel web auth set my-secret-token")
					os.Exit(1)
				}
				cmdChannelWebAuthSet(cfg, os.Args[5])
			case "get":
				cmdChannelWebAuthGet(cfg)
			default:
				fmt.Printf("Unknown auth command: %s\n", authSubcommand)
				fmt.Println("Available auth commands: set, get")
				fmt.Println()
				fmt.Println("Usage:")
				fmt.Println("  nemesisbot channel web auth           # Interactive input (secure)")
				fmt.Println("  nemesisbot channel web auth set <token>  # Direct set (convenient)")
				fmt.Println("  nemesisbot channel web auth get         # Show current token")
				os.Exit(1)
			}
		} else {
			// No sub-subcommand, use interactive mode
			cmdChannelWebAuth(cfg)
		}
	case "status":
		cmdChannelWebStatus(cfg)
	case "clear":
		cmdChannelWebClear(cfg)
	case "config":
		cmdChannelWebConfig(cfg)
	default:
		fmt.Printf("Unknown web command: %s\n", subcommand)
		ChannelWebHelp()
	}
}

// ChannelWebHelp prints web channel command help
func ChannelWebHelp() {
	fmt.Println("\nWeb Channel Configuration")
	fmt.Println("=========================")
	fmt.Println()
	fmt.Println("Usage: nemesisbot channel web <command>")
	fmt.Println()
	fmt.Println("Commands:")
	fmt.Println("  auth              Set web authentication token (interactive mode)")
	fmt.Println("  auth set <token>  Set web authentication token (direct mode)")
	fmt.Println("  auth get           Show current authentication token")
	fmt.Println("  status            Show current web channel configuration")
	fmt.Println("  clear             Clear web authentication token")
	fmt.Println("  config            Show detailed web channel configuration")
	fmt.Println()
	fmt.Println("Security:")
	fmt.Println("  Interactive mode (auth):")
	fmt.Println("    - Token input is hidden from process list")
	fmt.Println("    - More secure for manual setup")
	fmt.Println()
	fmt.Println("  Direct mode (auth set <token>):")
	fmt.Println("    - Convenient for scripts and automation")
	fmt.Println("    - Token visible in process list and shell history")
	fmt.Println("    - Use with caution in production environments")
	fmt.Println()
	fmt.Println("  The token is saved to config file (ensure secure file permissions)")
	fmt.Println("  Token is never displayed in logs or status output (except 'auth get')")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  # Interactive mode (recommended for manual setup)")
	fmt.Println("  nemesisbot channel web auth")
	fmt.Println()
	fmt.Println("  # Direct mode (convenient for scripts)")
	fmt.Println("  nemesisbot channel web auth set my-secret-token")
	fmt.Println("  nemesisbot channel web auth set 276793422")
	fmt.Println()
	fmt.Println("  # View and manage")
	fmt.Println("  nemesisbot channel web auth get")
	fmt.Println("  nemesisbot channel web status")
	fmt.Println("  nemesisbot channel web clear")
}

// cmdChannelWebAuthSet directly sets the web authentication token from command line
func cmdChannelWebAuthSet(cfg *config.Config, token string) {
	token = strings.TrimSpace(token)

	// Validate token
	if token == "" {
		fmt.Println("❌ Token cannot be empty")
		os.Exit(1)
	}

	// Show warning for short token but don't require confirmation
	if len(token) < 8 {
		fmt.Println("⚠️  Warning: Short token (less than 8 characters) is not recommended")
	}

	// Save token to config
	cfg.Channels.Web.AuthToken = token
	configPath := GetConfigPath()

	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("\n❌ Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("\n✅ Web authentication token saved successfully")
	fmt.Println()
	fmt.Println("🔄 Restart gateway for changes to take effect:")
	fmt.Println("   nemesisbot gateway")
	fmt.Println()
	fmt.Println("🌐 Access URL:")
	fmt.Printf("   http://%s:%d\n", cfg.Channels.Web.Host, cfg.Channels.Web.Port)
}

// cmdChannelWebAuthGet displays the current authentication token
func cmdChannelWebAuthGet(cfg *config.Config) {
	fmt.Println("\n🔐 Current Web Authentication Token")
	fmt.Println("=================================")
	fmt.Println()

	if cfg.Channels.Web.AuthToken == "" {
		fmt.Println("Status: ❌ No token is set")
		fmt.Println()
		fmt.Println("The web interface is currently open to everyone.")
		fmt.Println()
		fmt.Println("To set a token, use:")
		fmt.Println("  nemesisbot channel web auth")
		fmt.Println("  nemesisbot channel web auth set <token>")
		return
	}

	fmt.Println("Status: ✅ Token is set")
	fmt.Printf("Length: %d characters\n", len(cfg.Channels.Web.AuthToken))
	fmt.Println()
	fmt.Println("Token value:")
	fmt.Printf("  %s\n", cfg.Channels.Web.AuthToken)
	fmt.Println()
	fmt.Println("⚠️  Security Warning:")
	fmt.Println("  - Treat this token as sensitive information")
	fmt.Println("  - Don't share it or commit it to version control")
	fmt.Println("  - Consider changing it regularly")
}

// cmdChannelWebAuth securely sets the web authentication token (interactive mode)
func cmdChannelWebAuth(cfg *config.Config) {
	fmt.Println("\n🔐 Set Web Authentication Token")
	fmt.Println("==============================")
	fmt.Println()
	fmt.Println("⚠️  Security Notice:")
	fmt.Println("   Your token will be saved to the configuration file.")
	fmt.Println("   Ensure your config file has secure permissions (0600 on Unix/Mac).")
	fmt.Println()

	// Read token from stdin
	reader := bufio.NewReader(os.Stdin)
	fmt.Print("Enter your authentication token: ")

	token, err := reader.ReadString('\n')
	if err != nil {
		fmt.Printf("\n❌ Error reading token: %v\n", err)
		os.Exit(1)
	}

	token = strings.TrimSpace(token)

	// Validate token
	if token == "" {
		fmt.Println("❌ Token cannot be empty")
		os.Exit(1)
	}

	if len(token) < 8 {
		fmt.Println("\n⚠️  Warning: Short token (less than 8 characters) is not recommended")
		fmt.Print("Continue anyway? (y/N): ")
		var confirm string
		fmt.Scanln(&confirm)
		if strings.ToLower(confirm) != "y" {
			fmt.Println("❌ Cancelled")
			return
		}
	}

	// Confirm before saving
	fmt.Println()
	fmt.Println("⚠️  The token will be saved to your configuration file:")
	fmt.Printf("   %s\n\n", GetConfigPath())
	fmt.Print("Continue? (y/N): ")

	var confirm string
	fmt.Scanln(&confirm)
	if strings.ToLower(confirm) != "y" {
		fmt.Println("❌ Cancelled")
		return
	}

	// Save token to config
	cfg.Channels.Web.AuthToken = token
	configPath := GetConfigPath()

	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("\n❌ Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("\n✅ Web authentication token saved successfully")
	fmt.Println()
	fmt.Println("🔄 Restart gateway for changes to take effect:")
	fmt.Println("   nemesisbot gateway")
	fmt.Println()
	fmt.Println("🌐 Access URL:")
	fmt.Printf("   http://%s:%d\n", cfg.Channels.Web.Host, cfg.Channels.Web.Port)
	fmt.Println()
	fmt.Println("💡 Security Tips:")
	fmt.Println("   - The token is visible in terminal history during input")
	fmt.Println("   - Consider clearing your terminal history after setting the token")
	fmt.Println("   - Use a strong, unique token (at least 16 characters recommended)")
}

// cmdChannelWebStatus shows current web channel status
func cmdChannelWebStatus(cfg *config.Config) {
	fmt.Println("\n🌐 Web Channel Status")
	fmt.Println("=====================")
	fmt.Println()

	// Enabled status
	fmt.Printf("Enabled:           ")
	if cfg.Channels.Web.Enabled {
		fmt.Println("✅ Yes")
	} else {
		fmt.Println("❌ No")
	}

	// Authentication status
	fmt.Printf("Authentication:     ")
	if cfg.Channels.Web.AuthToken != "" {
		fmt.Println("✅ Enabled")
		// Show token length but not the token itself
		tokenLength := len(cfg.Channels.Web.AuthToken)
		fmt.Printf("Token Length:      %d characters (hidden for security)\n", tokenLength)
	} else {
		fmt.Println("❌ Disabled")
		fmt.Println("                    Anyone can access the web interface!")
	}

	// Connection info
	fmt.Printf("Host:              %s\n", cfg.Channels.Web.Host)
	fmt.Printf("Port:              %d\n", cfg.Channels.Web.Port)
	fmt.Printf("WebSocket Path:   %s\n", cfg.Channels.Web.Path)
	fmt.Printf("Session Timeout:  %d seconds\n", cfg.Channels.Web.SessionTimeout)

	// Access URL
	if cfg.Channels.Web.Enabled {
		fmt.Println()
		fmt.Println("🌐 Access URL:")
		if cfg.Channels.Web.AuthToken != "" {
			fmt.Printf("   http://%s:%d\n", cfg.Channels.Web.Host, cfg.Channels.Web.Port)
			fmt.Println()
			fmt.Println("📝 Note: Authentication is required. You will need to enter the token")
			fmt.Println("   when accessing the web interface.")
		} else {
			fmt.Printf("   http://%s:%d\n", cfg.Channels.Web.Host, cfg.Channels.Web.Port)
			fmt.Println()
			fmt.Println("⚠️  Warning: Authentication is NOT enabled. Anyone can access!")
			fmt.Println()
			fmt.Println("To enable authentication, run:")
			fmt.Println("   nemesisbot channel web auth")
		}
	}

	fmt.Println()
}

// cmdChannelWebClear clears the web authentication token
func cmdChannelWebClear(cfg *config.Config) {
	// Check if token is set
	if cfg.Channels.Web.AuthToken == "" {
		fmt.Println("\nℹ️  No authentication token is currently set")
		fmt.Println("   The web interface is already open (no authentication required)")
		return
	}

	fmt.Println("\n⚠️  This will remove the web authentication token")
	fmt.Println("   After clearing, anyone can access the web interface!")
	fmt.Println()
	fmt.Print("Continue? (y/N): ")

	var confirm string
	fmt.Scanln(&confirm)
	if strings.ToLower(confirm) != "y" {
		fmt.Println("❌ Cancelled")
		return
	}

	// Clear token
	cfg.Channels.Web.AuthToken = ""
	configPath := GetConfigPath()

	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("\n❌ Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("\n✅ Web authentication token cleared")
	fmt.Println()
	fmt.Println("🔄 Restart gateway for changes to take effect:")
	fmt.Println("   nemesisbot gateway")
	fmt.Println()
	fmt.Println("🌐 Access URL:")
	fmt.Printf("   http://%s:%d\n", cfg.Channels.Web.Host, cfg.Channels.Web.Port)
	fmt.Println()
	fmt.Println("⚠️  Warning: The web interface is now open to everyone!")
}

// cmdChannelWebConfig shows detailed web channel configuration
func cmdChannelWebConfig(cfg *config.Config) {
	fmt.Println("\n🌐 Web Channel Configuration")
	fmt.Println("============================")
	fmt.Println()

	fmt.Printf("Enabled:              ")
	if cfg.Channels.Web.Enabled {
		fmt.Println("✅ Yes")
	} else {
		fmt.Println("❌ No")
	}

	fmt.Printf("Host:                 %s\n", cfg.Channels.Web.Host)
	fmt.Printf("Port:                 %d\n", cfg.Channels.Web.Port)
	fmt.Printf("WebSocket Path:       %s\n", cfg.Channels.Web.Path)
	fmt.Printf("Heartbeat Interval:   %d seconds\n", cfg.Channels.Web.HeartbeatInterval)
	fmt.Printf("Session Timeout:      %d seconds\n", cfg.Channels.Web.SessionTimeout)

	fmt.Println()
	fmt.Println("Authentication:")
	if cfg.Channels.Web.AuthToken != "" {
		fmt.Println("  Status:           ✅ Enabled")
		fmt.Printf("  Token Length:     %d characters\n", len(cfg.Channels.Web.AuthToken))
		fmt.Println("  Token Value:      [hidden for security]")
	} else {
		fmt.Println("  Status:           ❌ Disabled")
		fmt.Println("  Access Control:   ⚠️  Open to everyone")
	}

	fmt.Println()
	if len(cfg.Channels.Web.AllowFrom) > 0 {
		fmt.Println("Allow From:")
		for _, addr := range cfg.Channels.Web.AllowFrom {
			fmt.Printf("  - %s\n", addr)
		}
	} else {
		fmt.Println("Allow From:         [not restricted]")
	}

	fmt.Println()
	fmt.Println("💡 Tips:")
	fmt.Println("  - Use 'nemesisbot channel web auth' to set authentication token")
	fmt.Println("  - Use 'nemesisbot channel web clear' to remove authentication")
	fmt.Println("  - Keep your token secret and change it regularly")
}

// readSecret reads a secret (password/token) from stdin
// Note: This implementation does NOT hide input due to cross-platform limitations.
// For better security, consider:
// - Using environment variables (NEMESISBOT_CHANNELS_WEB_AUTH_TOKEN)
// - Editing config file directly with a secure text editor
func readSecret() (string, error) {
	reader := bufio.NewReader(os.Stdin)
	line, err := reader.ReadString('\n')
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(line), nil
}

// readSecretFromReader reads from a buffered reader (alternative method)
func readSecretFromReader(reader *bufio.Reader) (string, error) {
	line, err := reader.ReadString('\n')
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(line), nil
}
