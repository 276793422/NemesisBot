package command

import (
	"bufio"
	"fmt"
	"os"
	"strings"

	"github.com/276793422/NemesisBot/module/config"
)

// CmdChannelWebSocket handles websocket channel specific commands
func CmdChannelWebSocket(cfg *config.Config) {
	if len(os.Args) < 4 {
		WebSocketHelp()
		return
	}

	subcommand := os.Args[3]

	switch subcommand {
	case "setup":
		cmdWebSocketSetup(cfg)
	case "config":
		cmdWebSocketConfig(cfg)
	case "set":
		if len(os.Args) < 5 {
			fmt.Println("Usage: nemesisbot channel websocket set <parameter> <value>")
			fmt.Println()
			fmt.Println("Parameters:")
			fmt.Println("  host      Set WebSocket server host")
			fmt.Println("  port      Set WebSocket server port")
			fmt.Println("  path      Set WebSocket path")
			fmt.Println("  token     Set authentication token")
			fmt.Println("  sync      Enable/disable web sync (true/false)")
			fmt.Println("  session   Set web session ID for sync")
			fmt.Println()
			fmt.Println("Examples:")
			fmt.Println("  nemesisbot channel websocket set host 127.0.0.1")
			fmt.Println("  nemesisbot channel websocket set port 49001")
			fmt.Println("  nemesisbot channel websocket set sync true")
			fmt.Println("  nemesisbot channel websocket set session abc123")
			os.Exit(1)
		}
		cmdWebSocketSet(cfg, os.Args[4], getValue(os.Args, 5))
	case "get":
		if len(os.Args) < 5 {
			fmt.Println("Usage: nemesisbot channel websocket get <parameter>")
			fmt.Println()
			fmt.Println("Parameters:")
			fmt.Println("  host      Get WebSocket server host")
			fmt.Println("  port      Get WebSocket server port")
			fmt.Println("  path      Get WebSocket path")
			fmt.Println("  token     Get authentication token")
			fmt.Println("  sync      Get web sync setting")
			fmt.Println("  session   Get web session ID")
			fmt.Println()
			fmt.Println("Examples:")
			fmt.Println("  nemesisbot channel websocket get host")
			fmt.Println("  nemesisbot channel websocket get sync")
			os.Exit(1)
		}
		cmdWebSocketGet(cfg, os.Args[4])
	default:
		fmt.Printf("Unknown websocket command: %s\n", subcommand)
		WebSocketHelp()
	}
}

// WebSocketHelp prints websocket channel help
func WebSocketHelp() {
	fmt.Println("WebSocket Channel Commands")
	fmt.Println("===========================")
	fmt.Println()
	fmt.Println("The WebSocket channel provides a standalone WebSocket server for")
	fmt.Println("external programs to communicate with NemesisBot.")
	fmt.Println()
	fmt.Println("Usage: nemesisbot channel websocket <command>")
	fmt.Println()
	fmt.Println("Commands:")
	fmt.Println("  setup    Interactive setup for WebSocket channel")
	fmt.Println("  config   Show current WebSocket channel configuration")
	fmt.Println("  set      Set a specific configuration parameter")
	fmt.Println("  get      Get a specific configuration parameter")
	fmt.Println()
	fmt.Println("Set command usage:")
	fmt.Println("  nemesisbot channel websocket set <parameter> <value>")
	fmt.Println()
	fmt.Println("  Parameters:")
	fmt.Println("    host      - WebSocket server host (default: 127.0.0.1)")
	fmt.Println("    port      - WebSocket server port (default: 49001)")
	fmt.Println("    path      - WebSocket path (default: /ws)")
	fmt.Println("    token     - Authentication token (optional)")
	fmt.Println("    sync      - Enable/disable web sync (default: false)")
	fmt.Println("    session   - Web session ID for sync (empty = broadcast)")
	fmt.Println()
	fmt.Println("Get command usage:")
	fmt.Println("  nemesisbot channel websocket get <parameter>")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  # Interactive setup")
	fmt.Println("  nemesisbot channel websocket setup")
	fmt.Println()
	fmt.Println("  # Set parameters directly")
	fmt.Println("  nemesisbot channel websocket set host 127.0.0.1")
	fmt.Println("  nemesisbot channel websocket set port 49001")
	fmt.Println("  nemesisbot channel websocket set sync true")
	fmt.Println("  nemesisbot channel websocket set session autospeak")
	fmt.Println()
	fmt.Println("  # Get parameters")
	fmt.Println("  nemesisbot channel websocket get host")
	fmt.Println("  nemesisbot channel websocket get sync")
	fmt.Println()
	fmt.Println("Workflow:")
	fmt.Println("  1. nemesisbot channel websocket setup")
	fmt.Println("  2. nemesisbot channel enable websocket")
	fmt.Println("  3. nemesisbot gateway")
	fmt.Println()
	fmt.Println("WebSocket Connection:")
	fmt.Println("  ws://<host>:<port><path>?token=<token>")
	fmt.Println("  Example: ws://127.0.0.1:49001/ws")
	fmt.Println()
	fmt.Println("Message Format:")
	fmt.Println("  Send: {\"type\":\"message\",\"content\":\"Hello\"}")
	fmt.Println("  Recv: {\"type\":\"message\",\"role\":\"assistant\",\"content\":\"Hi!\"}")
	fmt.Println()
	fmt.Println("Features:")
	fmt.Println("  - Single client connection only")
	fmt.Println("  - Bidirectional message communication")
	fmt.Println("  - Optional authentication via token")
	fmt.Println("  - Optional message synchronization to web interface")
}

// cmdWebSocketSetup interactively sets up WebSocket channel
func cmdWebSocketSetup(cfg *config.Config) {
	configPath := GetConfigPath()

	fmt.Println("======================================")
	fmt.Println("  WebSocket Channel Setup")
	fmt.Println("======================================")
	fmt.Println()
	fmt.Println("This will help you configure the WebSocket channel for")
	fmt.Println("external program integration.")
	fmt.Println()
	fmt.Println("WebSocket server will listen on the specified address and")
	fmt.Println("accept connections from external programs.")
	fmt.Println()

	reader := bufio.NewReader(os.Stdin)

	// Get host
	fmt.Printf("Current host: %s\n", cfg.Channels.WebSocket.Host)
	fmt.Print("Enter new host (or press Enter to keep current): ")
	host, _ := reader.ReadString('\n')
	host = strings.TrimSpace(host)

	if host != "" {
		cfg.Channels.WebSocket.Host = host
		fmt.Printf("✅ Host set to: %s\n", host)
	}

	// Get port
	fmt.Printf("\nCurrent port: %d\n", cfg.Channels.WebSocket.Port)
	fmt.Print("Enter new port (or press Enter to keep current): ")
	portStr, _ := reader.ReadString('\n')
	portStr = strings.TrimSpace(portStr)

	if portStr != "" {
		var port int
		if _, err := fmt.Sscanf(portStr, "%d", &port); err == nil {
			cfg.Channels.WebSocket.Port = port
			fmt.Printf("✅ Port set to: %d\n", port)
		} else {
			fmt.Printf("❌ Invalid port number: %s\n", portStr)
		}
	}

	// Get path
	fmt.Printf("\nCurrent path: %s\n", cfg.Channels.WebSocket.Path)
	fmt.Print("Enter new path (or press Enter to keep current): ")
	path, _ := reader.ReadString('\n')
	path = strings.TrimSpace(path)

	if path != "" {
		cfg.Channels.WebSocket.Path = path
		fmt.Printf("✅ Path set to: %s\n", path)
	}

	// Get auth token
	fmt.Println("\nAuthentication")
	fmt.Println("-------------")
	fmt.Printf("Current token: %s\n", formatToken(cfg.Channels.WebSocket.AuthToken))
	fmt.Print("Enter authentication token (or press Enter to keep current/clear): ")
	token, _ := reader.ReadString('\n')
	token = strings.TrimSpace(token)

	if token != "" {
		cfg.Channels.WebSocket.AuthToken = token
		if token == "(none)" || token == "(not set)" {
			cfg.Channels.WebSocket.AuthToken = ""
			fmt.Println("✅ Token cleared")
		} else {
			fmt.Printf("✅ Token set to: %s\n", formatToken(token))
		}
	}

	// Ask about web sync
	fmt.Println("\nWeb Synchronization")
	fmt.Println("---------------------")
	fmt.Println("When enabled, all messages will also appear in the Web interface.")
	fmt.Printf("Current setting: %v\n", cfg.Channels.WebSocket.SyncToWeb)
	fmt.Print("Enable web sync? (Y/n): ")
	syncResp, _ := reader.ReadString('\n')
	syncResp = strings.ToLower(strings.TrimSpace(syncResp))

	if syncResp == "" || syncResp == "y" || syncResp == "yes" {
		cfg.Channels.WebSocket.SyncToWeb = true
		fmt.Println("✅ Web sync enabled")
	} else {
		cfg.Channels.WebSocket.SyncToWeb = false
		fmt.Println("❌ Web sync disabled")
	}

	// Ask about web session ID
	fmt.Println("\nWeb Session ID")
	fmt.Println("---------------")
	fmt.Println("If web sync is enabled, you can specify which web session to sync to.")
	fmt.Println("Leave empty to broadcast to all web sessions.")
	fmt.Printf("Current session ID: %s\n", formatSessionID(cfg.Channels.WebSocket.WebSessionID))
	fmt.Print("Enter web session ID (or press Enter to keep current): ")
	sessionID, _ := reader.ReadString('\n')
	sessionID = strings.TrimSpace(sessionID)

	if sessionID != "" {
		cfg.Channels.WebSocket.WebSessionID = sessionID
		if sessionID == "(not set)" {
			cfg.Channels.WebSocket.WebSessionID = ""
			fmt.Println("✅ Session ID cleared (will broadcast)")
		} else {
			fmt.Printf("✅ Session ID set to: %s\n", sessionID)
		}
	}

	// Save configuration
	fmt.Println("\nSaving configuration...")
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("❌ Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("\n======================================")
	fmt.Println("✅ WebSocket channel configured successfully!")
	fmt.Println("======================================")
	fmt.Println()
	fmt.Println("Configuration Summary:")
	fmt.Printf("  Server: ws://%s:%d%s\n", cfg.Channels.WebSocket.Host, cfg.Channels.WebSocket.Port, cfg.Channels.WebSocket.Path)
	if cfg.Channels.WebSocket.AuthToken != "" {
		fmt.Printf("  Token:  %s\n", cfg.Channels.WebSocket.AuthToken)
	} else {
		fmt.Println("  Token:  (none - open connection)")
	}
	fmt.Printf("  Sync:   %v\n", cfg.Channels.WebSocket.SyncToWeb)
	if cfg.Channels.WebSocket.SyncToWeb && cfg.Channels.WebSocket.WebSessionID != "" {
		fmt.Printf("  Session: %s\n", cfg.Channels.WebSocket.WebSessionID)
	}
	fmt.Println()
	fmt.Println("Next steps:")
	fmt.Println("  1. Enable the channel:")
	fmt.Println("     nemesisbot channel enable websocket")
	fmt.Println()
	fmt.Println("  2. Start the gateway:")
	fmt.Println("     nemesisbot gateway")
	fmt.Println()
	fmt.Println("  3. Connect your WebSocket client:")
	fmt.Printf("     ws://%s:%d%s\n", cfg.Channels.WebSocket.Host, cfg.Channels.WebSocket.Port, cfg.Channels.WebSocket.Path)
	if cfg.Channels.WebSocket.AuthToken != "" {
		fmt.Printf("     ?token=%s\n", cfg.Channels.WebSocket.AuthToken)
	}
}

// cmdWebSocketConfig shows current WebSocket channel configuration
func cmdWebSocketConfig(cfg *config.Config) {
	fmt.Println("WebSocket Channel Configuration")
	fmt.Println("===============================")
	fmt.Println()

	fmt.Printf("Enabled:         ")
	if cfg.Channels.WebSocket.Enabled {
		fmt.Println("✅ Yes")
	} else {
		fmt.Println("❌ No")
	}

	fmt.Printf("Host:            %s\n", cfg.Channels.WebSocket.Host)
	fmt.Printf("Port:            %d\n", cfg.Channels.WebSocket.Port)
	fmt.Printf("Path:            %s\n", cfg.Channels.WebSocket.Path)
	fmt.Printf("Auth Token:      %s\n", formatToken(cfg.Channels.WebSocket.AuthToken))
	fmt.Printf("Allow From:      %v\n", cfg.Channels.WebSocket.AllowFrom)
	fmt.Printf("Sync to Web:     ")
	if cfg.Channels.WebSocket.SyncToWeb {
		fmt.Println("✅ Yes")
	} else {
		fmt.Println("❌ No")
	}
	fmt.Printf("Web Session ID:  %s\n", formatSessionID(cfg.Channels.WebSocket.WebSessionID))

	fmt.Println()
	fmt.Println("Connection URL:")
	url := fmt.Sprintf("ws://%s:%d%s", cfg.Channels.WebSocket.Host, cfg.Channels.WebSocket.Port, cfg.Channels.WebSocket.Path)
	fmt.Printf("  %s\n", url)
	if cfg.Channels.WebSocket.AuthToken != "" {
		fmt.Printf("  ?token=%s\n", cfg.Channels.WebSocket.AuthToken)
	}

	fmt.Println()
	if cfg.Channels.WebSocket.Enabled {
		fmt.Println("✅ WebSocket channel is enabled")
		fmt.Println()
		fmt.Println("To disable, run:")
		fmt.Println("  nemesisbot channel disable websocket")
	} else {
		fmt.Println("⚠️  WebSocket channel is currently disabled")
		fmt.Println()
		fmt.Println("To enable, run:")
		fmt.Println("  nemesisbot channel enable websocket")
	}
}

// cmdWebSocketSet sets a specific WebSocket channel parameter
func cmdWebSocketSet(cfg *config.Config, param, value string) {
	configPath := GetConfigPath()

	var updated bool
	var requiresRestart bool

	switch param {
	case "host":
		cfg.Channels.WebSocket.Host = value
		updated = true
		requiresRestart = true
		fmt.Printf("✅ Host set to: %s\n", value)

	case "port":
		var port int
		if _, err := fmt.Sscanf(value, "%d", &port); err == nil && port > 0 && port < 65536 {
			cfg.Channels.WebSocket.Port = port
			updated = true
			requiresRestart = true
			fmt.Printf("✅ Port set to: %d\n", port)
		} else {
			fmt.Printf("❌ Invalid port number: %s (must be 1-65535)\n", value)
			return
		}

	case "path":
		if !strings.HasPrefix(value, "/") {
			value = "/" + value
		}
		cfg.Channels.WebSocket.Path = value
		updated = true
		requiresRestart = true
		fmt.Printf("✅ Path set to: %s\n", value)

	case "token":
		if value == "" || value == "(none)" || value == "(clear)" {
			cfg.Channels.WebSocket.AuthToken = ""
			updated = true
			requiresRestart = true
			fmt.Println("✅ Authentication token cleared")
		} else {
			cfg.Channels.WebSocket.AuthToken = value
			updated = true
			requiresRestart = true
			fmt.Printf("✅ Token set to: %s\n", formatToken(value))
		}

	case "sync":
		// Parse boolean value
		boolVal := strings.ToLower(value)
		if boolVal == "true" || boolVal == "yes" || boolVal == "y" || boolVal == "1" || boolVal == "on" {
			cfg.Channels.WebSocket.SyncToWeb = true
			updated = true
			requiresRestart = false
			fmt.Println("✅ Web sync enabled")
		} else if boolVal == "false" || boolVal == "no" || boolVal == "n" || boolVal == "0" || boolVal == "off" {
			cfg.Channels.WebSocket.SyncToWeb = false
			updated = true
			requiresRestart = false
			fmt.Println("❌ Web sync disabled")
		} else {
			fmt.Printf("❌ Invalid value for sync: %s\n", value)
			fmt.Println("Valid values: true, false, yes, no, y, n, 1, 0, on, off")
			return
		}

	case "session":
		if value == "" || value == "(none)" || value == "(clear)" {
			cfg.Channels.WebSocket.WebSessionID = ""
			updated = true
			requiresRestart = false
			fmt.Println("✅ Session ID cleared (will broadcast to all web sessions)")
		} else {
			cfg.Channels.WebSocket.WebSessionID = value
			updated = true
			requiresRestart = false
			fmt.Printf("✅ Session ID set to: %s\n", value)
		}

	default:
		fmt.Printf("❌ Unknown parameter: %s\n", param)
		fmt.Println()
		fmt.Println("Valid parameters:")
		fmt.Println("  host      - Set WebSocket server host")
		fmt.Println("  port      - Set WebSocket server port")
		fmt.Println("  path      - Set WebSocket path")
		fmt.Println("  token     - Set authentication token")
		fmt.Println("  sync      - Enable/disable web sync")
		fmt.Println("  session   - Set web session ID")
		return
	}

	if !updated {
		return
	}

	// Save configuration
	fmt.Println("\nSaving configuration...")
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("❌ Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("✅ Configuration saved successfully")

	if requiresRestart {
		fmt.Println()
		fmt.Println("⚠️  Restart gateway for changes to take effect:")
		fmt.Println("   nemesisbot gateway")
	}
}

// cmdWebSocketGet gets a specific WebSocket channel parameter
func cmdWebSocketGet(cfg *config.Config, param string) {
	switch param {
	case "host":
		fmt.Printf("Host: %s\n", cfg.Channels.WebSocket.Host)

	case "port":
		fmt.Printf("Port: %d\n", cfg.Channels.WebSocket.Port)

	case "path":
		fmt.Printf("Path: %s\n", cfg.Channels.WebSocket.Path)

	case "token":
		fmt.Printf("Auth Token: %s\n", formatToken(cfg.Channels.WebSocket.AuthToken))

	case "sync":
		if cfg.Channels.WebSocket.SyncToWeb {
			fmt.Println("Web sync: enabled (true)")
		} else {
			fmt.Println("Web sync: disabled (false)")
		}

	case "session":
		fmt.Printf("Web Session ID: %s\n", formatSessionID(cfg.Channels.WebSocket.WebSessionID))

	default:
		fmt.Printf("❌ Unknown parameter: %s\n", param)
		fmt.Println()
		fmt.Println("Valid parameters:")
		fmt.Println("  host      - Get WebSocket server host")
		fmt.Println("  port      - Get WebSocket server port")
		fmt.Println("  path      - Get WebSocket path")
		fmt.Println("  token     - Get authentication token")
		fmt.Println("  sync      - Get web sync setting")
		fmt.Println("  session   - Get web session ID")
	}
}

// formatToken formats token for display
func formatToken(token string) string {
	if token == "" {
		return "(none - open connection)"
	}
	return token
}

// formatSessionID formats session ID for display
func formatSessionID(sessionID string) string {
	if sessionID == "" {
		return "(not set - will broadcast to all sessions)"
	}
	return sessionID
}

// getValue gets value from args array safely
func getValue(args []string, index int) string {
	if index < len(args) {
		return args[index]
	}
	return ""
}
