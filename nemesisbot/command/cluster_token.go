// Package command implements CLI commands for NemesisBot
package command

import (
	"crypto/rand"
	"encoding/base64"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/276793422/NemesisBot/module/cluster"
	"github.com/276793422/NemesisBot/module/path"
)

// CmdClusterToken manages RPC authentication token
func CmdClusterToken() {
	if len(os.Args) < 4 {
		ClusterTokenHelp()
		return
	}

	subcommand := os.Args[3]

	switch subcommand {
	case "generate":
		cmdClusterTokenGenerate()
	case "show":
		cmdClusterTokenShow()
	case "set":
		cmdClusterTokenSet()
	case "verify":
		cmdClusterTokenVerify()
	default:
		fmt.Printf("Unknown token command: %s\n", subcommand)
		ClusterTokenHelp()
	}
}

// ClusterTokenHelp prints token command help
func ClusterTokenHelp() {
	fmt.Println("\nRPC Token Management Commands:")
	fmt.Println("  generate              Generate a secure random token")
	fmt.Println("  show                  Show current RPC token (masked)")
	fmt.Println("  set <token>           Set RPC token")
	fmt.Println("  verify <token>        Verify if a token matches the configured token")
	fmt.Println()
	fmt.Println("Options:")
	fmt.Println("  generate:")
	fmt.Println("    --length <n>        Token length in bytes (default: 32)")
	fmt.Println("    --save              Save generated token to configuration")
	fmt.Println()
	fmt.Println("  show:")
	fmt.Println("    --full              Show full token (WARNING: displays secret)")
	fmt.Println()
	fmt.Println("  set:")
	fmt.Println("    --generate          Auto-generate a random token")
	fmt.Println("    --length <n>        Token length for auto-generation (default: 32)")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  # Generate a token and display it")
	fmt.Println("  nemesisbot cluster token generate")
	fmt.Println()
	fmt.Println("  # Generate and save token")
	fmt.Println("  nemesisbot cluster token generate --save")
	fmt.Println()
	fmt.Println("  # Set a specific token")
	fmt.Println("  nemesisbot cluster token set \"my-secret-token\"")
	fmt.Println()
	fmt.Println("  # Auto-generate and set token")
	fmt.Println("  nemesisbot cluster token set --generate")
	fmt.Println()
	fmt.Println("  # Show current token (masked)")
	fmt.Println("  nemesisbot cluster token show")
	fmt.Println()
	fmt.Println("  # Verify a token")
	fmt.Println("  nemesisbot cluster token verify \"my-secret-token\"")
	fmt.Println()
}

// cmdClusterTokenGenerate generates a secure random token
func cmdClusterTokenGenerate() {
	// Parse flags
	length := 32
	save := false

	for i := 3; i < len(os.Args); i++ {
		arg := os.Args[i]
		switch arg {
		case "--length":
			if i+1 < len(os.Args) {
				fmt.Sscanf(os.Args[i+1], "%d", &length)
				i++
			}
		case "--save":
			save = true
		}
	}

	// Validate length
	if length < 16 {
		fmt.Println("❌ Error: Token length must be at least 16 bytes")
		return
	}
	if length > 128 {
		fmt.Println("❌ Error: Token length must not exceed 128 bytes")
		return
	}

	// Generate random bytes
	bytes := make([]byte, length)
	if _, err := rand.Read(bytes); err != nil {
		fmt.Printf("❌ Error generating random token: %v\n", err)
		return
	}

	// Encode to base64
	token := base64.StdEncoding.EncodeToString(bytes)

	// Display token
	fmt.Println("✅ Generated RPC Token:")
	fmt.Printf("   %s\n\n", token)

	if save {
		// Save to configuration
		if err := setClusterToken(token); err != nil {
			fmt.Printf("❌ Error saving token: %v\n", err)
			return
		}
		fmt.Println("✅ Token saved to configuration")
		fmt.Println("ℹ️  Restart cluster module to apply changes:")
		fmt.Println("   nemesisbot cluster disable")
		fmt.Println("   nemesisbot cluster enable")
	} else {
		fmt.Println("ℹ️  To save this token, run:")
		fmt.Printf("   nemesisbot cluster token set \"%s\"\n", token)
		fmt.Println("   or")
		fmt.Println("   nemesisbot cluster token generate --save")
	}
}

// cmdClusterTokenShow shows the current RPC token
func cmdClusterTokenShow() {
	// Parse flags
	showFull := false
	for i := 3; i < len(os.Args); i++ {
		if os.Args[i] == "--full" {
			showFull = true
		}
	}

	// Get workspace path
	homeDir, err := path.ResolveHomeDir()
	if err != nil {
		fmt.Printf("❌ Error resolving home directory: %v\n", err)
		return
	}

	// Construct config file path (workspace/cluster/peers.toml)
	configPath := filepath.Join(homeDir, "workspace", "cluster", "peers.toml")

	// Load cluster config
	config, err := cluster.LoadStaticConfig(configPath)
	if err != nil {
		fmt.Printf("❌ Error loading cluster config: %v\n", err)
		return
	}

	// Check if token is configured
	if config.Cluster.RPCAuthToken == "" {
		fmt.Println("ℹ️  No RPC token configured")
		fmt.Println("   RPC authentication is disabled (no token)")
		fmt.Println()
		fmt.Println("To enable RPC authentication:")
		fmt.Println("  nemesisbot cluster token generate --save")
		fmt.Println("  or")
		fmt.Println("  nemesisbot cluster token set <your-token>")
		return
	}

	// Display token
	if showFull {
		fmt.Println("⚠️  WARNING: Displaying full token (this is a secret!)")
		fmt.Printf("RPC Token: %s\n", config.Cluster.RPCAuthToken)
	} else {
		// Masked display
		token := config.Cluster.RPCAuthToken
		if len(token) <= 8 {
			// Very short token, show minimal
			masked := token[:2] + strings.Repeat("*", len(token)-4) + token[len(token)-2:]
			fmt.Printf("RPC Token: %s\n", masked)
		} else {
			// Normal token, show first and last 4 chars
			masked := token[:4] + strings.Repeat("*", len(token)-8) + token[len(token)-4:]
			fmt.Printf("RPC Token: %s\n", masked)
		}
		fmt.Println()
		fmt.Println("ℹ️  To see full token, use: nemesisbot cluster token show --full")
	}

	fmt.Println()
	fmt.Println("✅ RPC authentication is enabled")
	fmt.Printf("Config file: %s/workspace/cluster/peers.toml\n", homeDir)
}

// cmdClusterTokenSet sets the RPC token
func cmdClusterTokenSet() {
	// Parse flags
	generate := false
	length := 32
	token := ""

	for i := 3; i < len(os.Args); i++ {
		arg := os.Args[i]
		switch arg {
		case "--generate":
			generate = true
		case "--length":
			if i+1 < len(os.Args) {
				fmt.Sscanf(os.Args[i+1], "%d", &length)
				i++
			}
		default:
			if !strings.HasPrefix(arg, "--") && token == "" {
				token = arg
			}
		}
	}

	// Generate token if requested
	if generate {
		if token != "" {
			fmt.Println("❌ Error: Cannot use --generate with a token argument")
			return
		}

		// Validate length
		if length < 16 {
			fmt.Println("❌ Error: Token length must be at least 16 bytes")
			return
		}
		if length > 128 {
			fmt.Println("❌ Error: Token length must not exceed 128 bytes")
			return
		}

		// Generate random bytes
		bytes := make([]byte, length)
		if _, err := rand.Read(bytes); err != nil {
			fmt.Printf("❌ Error generating random token: %v\n", err)
			return
		}

		token = base64.StdEncoding.EncodeToString(bytes)
		fmt.Printf("✅ Generated token: %s\n", token)
	}

	// Validate token
	if token == "" {
		fmt.Println("❌ Error: No token provided")
		fmt.Println()
		fmt.Println("Usage:")
		fmt.Println("  nemesisbot cluster token set <token>")
		fmt.Println("  nemesisbot cluster token set --generate")
		return
	}

	// Save token
	if err := setClusterToken(token); err != nil {
		fmt.Printf("❌ Error setting token: %v\n", err)
		return
	}

	fmt.Println("✅ RPC token updated successfully")
	fmt.Println()
	fmt.Println("ℹ️  Configuration saved to: workspace/cluster/peers.toml")
	fmt.Println("ℹ️  Restart cluster module to apply changes:")
	fmt.Println("   nemesisbot cluster disable")
	fmt.Println("   nemesisbot cluster enable")
}

// cmdClusterTokenVerify verifies if a token matches the configured token
func cmdClusterTokenVerify() {
	if len(os.Args) < 5 {
		fmt.Println("❌ Error: No token provided to verify")
		fmt.Println()
		fmt.Println("Usage: nemesisbot cluster token verify <token>")
		return
	}

	token := os.Args[4]  // cluster token verify <token>

	// Get workspace path
	homeDir, err := path.ResolveHomeDir()
	if err != nil {
		fmt.Printf("❌ Error resolving home directory: %v\n", err)
		return
	}

	// Construct config file path (workspace/cluster/peers.toml)
	configPath := filepath.Join(homeDir, "workspace", "cluster", "peers.toml")

	// Load cluster config
	config, err := cluster.LoadStaticConfig(configPath)
	if err != nil {
		fmt.Printf("❌ Error loading cluster config: %v\n", err)
		return
	}

	// Check if token is configured
	if config.Cluster.RPCAuthToken == "" {
		fmt.Println("ℹ️  No RPC token configured")
		fmt.Println("   RPC authentication is disabled (any token will be accepted)")
		fmt.Println()
		fmt.Println("To enable authentication:")
		fmt.Println("  nemesisbot cluster token set --generate")
		return
	}

	// Verify token
	if token == config.Cluster.RPCAuthToken {
		fmt.Println("✅ Token is valid")
		fmt.Println("   The provided token matches the configured RPC token")
	} else {
		fmt.Println("❌ Token is invalid")
		fmt.Println("   The provided token does NOT match the configured RPC token")
	}
}

// setClusterToken saves the token to cluster configuration
func setClusterToken(token string) error {
	// Get workspace path
	homeDir, err := path.ResolveHomeDir()
	if err != nil {
		return fmt.Errorf("failed to resolve home directory: %w", err)
	}

	// Construct config file path (workspace/cluster/peers.toml)
	configPath := filepath.Join(homeDir, "workspace", "cluster", "peers.toml")

	// Load existing config
	config, err := cluster.LoadStaticConfig(configPath)
	if err != nil {
		// Config doesn't exist, create new one
		config = &cluster.StaticConfig{
			Cluster: cluster.ClusterMeta{
				ID:            "manual",
				AutoDiscovery: true,
				LastUpdated:   cluster.GetCurrentTime(),
				RPCAuthToken:  token,
			},
		}
	} else {
		// Update existing config
		config.Cluster.RPCAuthToken = token
		config.Cluster.LastUpdated = cluster.GetCurrentTime()
	}

	// Save config with full file path
	if err := cluster.SaveStaticConfig(configPath, config); err != nil {
		return fmt.Errorf("failed to save config: %w", err)
	}

	return nil
}
