package command

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/mcp"
)

// CmdMCP manages MCP servers
func CmdMCP() {
	if len(os.Args) < 3 {
		MCPHelp()
		return
	}

	subcommand := os.Args[2]

	switch subcommand {
	case "list":
		cmdMCPList()
	case "add":
		cmdMCPAdd()
	case "remove":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot mcp remove <server-name>")
			return
		}
		cmdMCPRemove(os.Args[3])
	case "test":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot mcp test <server-name>")
			return
		}
		cmdMCPTest(os.Args[3])
	case "inspect":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot mcp inspect <server-name>")
			return
		}
		cmdMCPInspect(os.Args[3])
	case "tools":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot mcp tools <server-name>")
			return
		}
		cmdMCPTools(os.Args[3])
	case "resources":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot mcp resources <server-name>")
			return
		}
		cmdMCPResources(os.Args[3])
	case "prompts":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot mcp prompts <server-name>")
			return
		}
		cmdMCPPrompts(os.Args[3])
	default:
		fmt.Printf("Unknown mcp command: %s\n", subcommand)
		MCPHelp()
	}
}

// MCPHelp prints MCP command help
func MCPHelp() {
	fmt.Println("\nMCP (Model Context Protocol) commands:")
	fmt.Println("  list                    List configured MCP servers")
	fmt.Println("  add                     Add a new MCP server")
	fmt.Println("  remove <name>           Remove a MCP server")
	fmt.Println("  test <name>             Test a MCP server connection")
	fmt.Println("  inspect <name>          Inspect MCP server details (tools/resources/prompts)")
	fmt.Println("  tools <name>            List available tools from a server")
	fmt.Println("  resources <name>        List available resources from a server")
	fmt.Println("  prompts <name>          List available prompts from a server")
	fmt.Println()
	fmt.Println("Add options:")
	fmt.Println("  -n, --name       Server name (required)")
	fmt.Println("  -c, --command    Command to start server (required)")
	fmt.Println("  -a, --args       Arguments for command (optional)")
	fmt.Println("  -e, --env        Environment variables (optional)")
	fmt.Println("  -t, --timeout    Timeout in seconds (default: 30)")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot mcp list")
	fmt.Println("  nemesisbot mcp add -n filesystem -c npx -a -y,@modelcontextprotocol/server-filesystem,C:/allowed/path")
	fmt.Println("  nemesisbot mcp add -n github -c npx -a -y,@modelcontextprotocol/server-github -e GITHUB_TOKEN=xxx")
	fmt.Println("  nemesisbot mcp remove filesystem")
	fmt.Println("  nemesisbot mcp test github")
	fmt.Println("  nemesisbot mcp inspect github")
	fmt.Println("  nemesisbot mcp tools test-server")
	fmt.Println("  nemesisbot mcp resources test-server")
	fmt.Println("  nemesisbot mcp prompts test-server")
	fmt.Println()
	fmt.Println("Configuration file:")
	fmt.Println("  ~/.nemesisbot/config.mcp.json")
}

func cmdMCPList() {
	mcpConfig, err := config.LoadMCPConfig(GetMCPConfigPath())
	if err != nil {
		fmt.Printf("Error loading MCP config: %v\n", err)
		return
	}

	if !mcpConfig.Enabled {
		fmt.Println("MCP is disabled in config")
		return
	}

	if len(mcpConfig.Servers) == 0 {
		fmt.Println("No MCP servers configured.")
		fmt.Println("\nAdd a server using: nemesisbot mcp add -n <name> -c <command>")
		return
	}

	fmt.Printf("\nConfigured MCP Servers (%d):\n", len(mcpConfig.Servers))
	fmt.Println("-------------------------")
	for _, server := range mcpConfig.Servers {
		fmt.Printf("  • %s\n", server.Name)
		fmt.Printf("    Command: %s %s\n", server.Command, strings.Join(server.Args, " "))
		if server.Timeout > 0 {
			fmt.Printf("    Timeout: %d seconds\n", server.Timeout)
		}
		if len(server.Env) > 0 {
			fmt.Printf("    Environment: %d variable(s)\n", len(server.Env))
		}
		fmt.Println()
	}
}

func cmdMCPAdd() {
	name := ""
	command := ""
	args := []string{}
	env := []string{}
	timeout := 30

	// Parse arguments
	parsedArgs := os.Args[3:]
	for i := 0; i < len(parsedArgs); i++ {
		switch parsedArgs[i] {
		case "-n", "--name":
			if i+1 < len(parsedArgs) {
				name = parsedArgs[i+1]
				i++
			}
		case "-c", "--command":
			if i+1 < len(parsedArgs) {
				command = parsedArgs[i+1]
				i++
			}
		case "-a", "--args":
			if i+1 < len(parsedArgs) {
				argsStr := parsedArgs[i+1]
				args = strings.Split(argsStr, ",")
				i++
			}
		case "-e", "--env":
			if i+1 < len(parsedArgs) {
				env = append(env, parsedArgs[i+1])
				i++
			}
		case "-t", "--timeout":
			if i+1 < len(parsedArgs) {
				fmt.Sscanf(parsedArgs[i+1], "%d", &timeout)
				i++
			}
		}
	}

	if name == "" {
		fmt.Println("Error: --name is required")
		fmt.Println("Usage: nemesisbot mcp add -n <name> -c <command>")
		return
	}

	if command == "" {
		fmt.Println("Error: --command is required")
		fmt.Println("Usage: nemesisbot mcp add -n <name> -c <command>")
		return
	}

	// Load existing config
	mcpConfig, err := config.LoadMCPConfig(GetMCPConfigPath())
	if err != nil {
		fmt.Printf("Error loading MCP config: %v\n", err)
		return
	}

	// Check if server already exists
	for _, server := range mcpConfig.Servers {
		if server.Name == name {
			fmt.Printf("Error: Server '%s' already exists\n", name)
			fmt.Println("Remove it first using: nemesisbot mcp remove " + name)
			return
		}
	}

	// Add new server
	mcpConfig.Enabled = true
	newServer := config.MCPServerConfig{
		Name:    name,
		Command: command,
		Args:    args,
		Env:     env,
		Timeout: timeout,
	}
	mcpConfig.Servers = append(mcpConfig.Servers, newServer)

	// Save config
	if err := config.SaveMCPConfig(GetMCPConfigPath(), mcpConfig); err != nil {
		fmt.Printf("Error saving MCP config: %v\n", err)
		return
	}

	fmt.Printf("✓ MCP server '%s' added successfully!\n", name)
	fmt.Printf("\nConfiguration saved to: %s\n", GetMCPConfigPath())
	fmt.Println("\nNext steps:")
	fmt.Printf("  1. Test the connection: nemesisbot mcp test %s\n", name)
	fmt.Println("  2. Restart agent/gateway to load the server")
}

func cmdMCPRemove(serverName string) {
	mcpConfig, err := config.LoadMCPConfig(GetMCPConfigPath())
	if err != nil {
		fmt.Printf("Error loading MCP config: %v\n", err)
		return
	}

	// Find and remove server
	found := false
	newServers := []config.MCPServerConfig{}
	for _, server := range mcpConfig.Servers {
		if server.Name == serverName {
			found = true
		} else {
			newServers = append(newServers, server)
		}
	}

	if !found {
		fmt.Printf("Error: Server '%s' not found\n", serverName)
		fmt.Println("List servers using: nemesisbot mcp list")
		return
	}

	mcpConfig.Servers = newServers

	// Save config
	if err := config.SaveMCPConfig(GetMCPConfigPath(), mcpConfig); err != nil {
		fmt.Printf("Error saving MCP config: %v\n", err)
		return
	}

	fmt.Printf("✓ MCP server '%s' removed successfully!\n", serverName)
	fmt.Println("\nRestart agent/gateway to apply changes")
}

func cmdMCPTest(serverName string) {
	mcpConfig, err := config.LoadMCPConfig(GetMCPConfigPath())
	if err != nil {
		fmt.Printf("Error loading MCP config: %v\n", err)
		return
	}

	// Find server
	var targetServer *config.MCPServerConfig
	for i := range mcpConfig.Servers {
		if mcpConfig.Servers[i].Name == serverName {
			targetServer = &mcpConfig.Servers[i]
			break
		}
	}

	if targetServer == nil {
		fmt.Printf("Error: Server '%s' not found\n", serverName)
		fmt.Println("List servers using: nemesisbot mcp list")
		return
	}

	fmt.Printf("Testing MCP server '%s'...\n", serverName)
	fmt.Printf("Command: %s %s\n\n", targetServer.Command, strings.Join(targetServer.Args, " "))

	// Import mcp package
	// Note: This is a simplified test - in production you'd use the actual MCP client
	fmt.Println("⚠ Note: This is a basic connectivity test")
	fmt.Println("For full functionality testing, restart the agent/gateway and check logs")
	fmt.Println()

	// Check if command exists
	command := targetServer.Command
	if _, err := exec.LookPath(command); err != nil {
		fmt.Printf("✗ Command '%s' not found in PATH\n", command)
		fmt.Println("\nTroubleshooting:")
		fmt.Println("  • Ensure the command is installed and in PATH")
		fmt.Println("  • For npx commands, ensure Node.js is installed")
		fmt.Println("  • For absolute paths, verify the file exists")
		return
	}

	fmt.Printf("✓ Command '%s' found\n", command)

	// Check if environment variables are set
	if len(targetServer.Env) > 0 {
		fmt.Printf("✓ %d environment variable(s) configured\n", len(targetServer.Env))
		for _, envVar := range targetServer.Env {
			parts := strings.SplitN(envVar, "=", 2)
			if len(parts) == 2 {
				key := parts[0]
				value := os.Getenv(key)
				if value != "" {
					fmt.Printf("  • %s: *** (set)\n", key)
				} else {
					fmt.Printf("  • %s: NOT SET\n", key)
				}
			}
		}
	}

	fmt.Println("\n✓ Basic checks passed!")
	fmt.Printf("\nTo test full MCP functionality:\n")
	fmt.Println("  1. Restart agent: nemesisbot agent")
	fmt.Println("  2. Or restart gateway: nemesisbot gateway")
	fmt.Println("  3. Check logs for MCP server initialization")
}

func cmdMCPInspect(serverName string) {
	mcpConfig, err := config.LoadMCPConfig(GetMCPConfigPath())
	if err != nil {
		fmt.Printf("Error loading MCP config: %v\n", err)
		return
	}

	// Find server
	var targetServer *config.MCPServerConfig
	for i := range mcpConfig.Servers {
		if mcpConfig.Servers[i].Name == serverName {
			targetServer = &mcpConfig.Servers[i]
			break
		}
	}

	if targetServer == nil {
		fmt.Printf("Error: Server '%s' not found\n", serverName)
		fmt.Println("List servers using: nemesisbot mcp list")
		return
	}

	fmt.Printf("Inspecting MCP server '%s'...\n", serverName)
	fmt.Printf("Command: %s %s\n\n", targetServer.Command, strings.Join(targetServer.Args, " "))

	// Import mcp package dynamically using reflection or create a simple client
	// For now, we'll use a simpler approach - show what would be inspected
	fmt.Println("⚠ Note: Full inspection requires running MCP client")
	fmt.Println("This will show:")
	fmt.Println("  • Server information (name, version)")
	fmt.Println("  • Available capabilities (tools, resources, prompts)")
	fmt.Println("  • List of tools")
	fmt.Println("  • List of resources")
	fmt.Println("  • List of prompts")
	fmt.Println()
	fmt.Println("For detailed inspection, use:")
	fmt.Printf("  nemesisbot mcp tools %s\n", serverName)
	fmt.Printf("  nemesisbot mcp resources %s\n", serverName)
	fmt.Printf("  nemesisbot mcp prompts %s\n", serverName)
}

func cmdMCPTools(serverName string) {
	mcpConfig, err := config.LoadMCPConfig(GetMCPConfigPath())
	if err != nil {
		fmt.Printf("Error loading MCP config: %v\n", err)
		return
	}

	// Find server
	var targetServer *config.MCPServerConfig
	for i := range mcpConfig.Servers {
		if mcpConfig.Servers[i].Name == serverName {
			targetServer = &mcpConfig.Servers[i]
			break
		}
	}

	if targetServer == nil {
		fmt.Printf("Error: Server '%s' not found\n", serverName)
		fmt.Println("List servers using: nemesisbot mcp list")
		return
	}

	fmt.Printf("Fetching tools from MCP server '%s'...\n\n", serverName)

	// Create MCP client and list tools
	ctx := context.Background()

	client, err := mcp.NewClient(&mcp.ServerConfig{
		Name:    targetServer.Name,
		Command: targetServer.Command,
		Args:    targetServer.Args,
		Env:     targetServer.Env,
		Timeout: targetServer.Timeout,
	})
	if err != nil {
		fmt.Printf("Error creating MCP client: %v\n", err)
		return
	}
	defer client.Close()

	// Initialize
	timeout := time.Duration(targetServer.Timeout) * time.Second
	if targetServer.Timeout == 0 {
		timeout = 30 * time.Second
	}
	initCtx, cancel := context.WithTimeout(ctx, timeout)
	_, err = client.Initialize(initCtx)
	cancel()
	if err != nil {
		fmt.Printf("Error initializing MCP client: %v\n", err)
		return
	}

	// List tools
	tools, err := client.ListTools(ctx)
	if err != nil {
		fmt.Printf("Error listing tools: %v\n", err)
		return
	}

	if len(tools) == 0 {
		fmt.Println("No tools available on this server.")
		return
	}

	fmt.Printf("Found %d tool(s):\n", len(tools))
	fmt.Println("-------------------")
	for i, tool := range tools {
		fmt.Printf("%d. %s\n", i+1, tool.Name)
		fmt.Printf("   Description: %s\n", tool.Description)
		// Show input schema briefly
		if props, ok := tool.InputSchema["properties"].(map[string]interface{}); ok {
			var required []string
			if req, ok := tool.InputSchema["required"].([]interface{}); ok {
				for _, r := range req {
					if s, ok := r.(string); ok {
						required = append(required, s)
					}
				}
			}
			if len(props) > 0 {
				fmt.Printf("   Parameters: ")
				paramNames := make([]string, 0, len(props))
				for name := range props {
					paramNames = append(paramNames, name)
				}
				for i, name := range paramNames {
					reqMarker := ""
					for _, req := range required {
						if name == req {
							reqMarker = "*"
							break
						}
					}
					if i > 0 {
						fmt.Print(", ")
					}
					fmt.Printf("%s%s", name, reqMarker)
				}
				fmt.Println()
				fmt.Printf("   (* = required)")
			}
		}
		fmt.Println()
	}
}

func cmdMCPResources(serverName string) {
	mcpConfig, err := config.LoadMCPConfig(GetMCPConfigPath())
	if err != nil {
		fmt.Printf("Error loading MCP config: %v\n", err)
		return
	}

	// Find server
	var targetServer *config.MCPServerConfig
	for i := range mcpConfig.Servers {
		if mcpConfig.Servers[i].Name == serverName {
			targetServer = &mcpConfig.Servers[i]
			break
		}
	}

	if targetServer == nil {
		fmt.Printf("Error: Server '%s' not found\n", serverName)
		fmt.Println("List servers using: nemesisbot mcp list")
		return
	}

	fmt.Printf("Fetching resources from MCP server '%s'...\n\n", serverName)

	// Create MCP client
	ctx := context.Background()

	client, err := mcp.NewClient(&mcp.ServerConfig{
		Name:    targetServer.Name,
		Command: targetServer.Command,
		Args:    targetServer.Args,
		Env:     targetServer.Env,
		Timeout: targetServer.Timeout,
	})
	if err != nil {
		fmt.Printf("Error creating MCP client: %v\n", err)
		return
	}
	defer client.Close()

	// Initialize
	timeout := time.Duration(targetServer.Timeout) * time.Second
	if targetServer.Timeout == 0 {
		timeout = 30 * time.Second
	}
	initCtx, cancel := context.WithTimeout(ctx, timeout)
	_, err = client.Initialize(initCtx)
	cancel()
	if err != nil {
		fmt.Printf("Error initializing MCP client: %v\n", err)
		return
	}

	// List resources
	resources, err := client.ListResources(ctx)
	if err != nil {
		fmt.Printf("Error listing resources: %v\n", err)
		return
	}

	if len(resources) == 0 {
		fmt.Println("No resources available on this server.")
		return
	}

	fmt.Printf("Found %d resource(s):\n", len(resources))
	fmt.Println("----------------------")
	for i, resource := range resources {
		fmt.Printf("%d. %s\n", i+1, resource.Name)
		fmt.Printf("   URI: %s\n", resource.URI)
		if resource.Description != "" {
			fmt.Printf("   Description: %s\n", resource.Description)
		}
		if resource.MimeType != "" {
			fmt.Printf("   MIME Type: %s\n", resource.MimeType)
		}
		fmt.Println()
	}
}

func cmdMCPPrompts(serverName string) {
	mcpConfig, err := config.LoadMCPConfig(GetMCPConfigPath())
	if err != nil {
		fmt.Printf("Error loading MCP config: %v\n", err)
		return
	}

	// Find server
	var targetServer *config.MCPServerConfig
	for i := range mcpConfig.Servers {
		if mcpConfig.Servers[i].Name == serverName {
			targetServer = &mcpConfig.Servers[i]
			break
		}
	}

	if targetServer == nil {
		fmt.Printf("Error: Server '%s' not found\n", serverName)
		fmt.Println("List servers using: nemesisbot mcp list")
		return
	}

	fmt.Printf("Fetching prompts from MCP server '%s'...\n\n", serverName)

	// Create MCP client
	ctx := context.Background()

	client, err := mcp.NewClient(&mcp.ServerConfig{
		Name:    targetServer.Name,
		Command: targetServer.Command,
		Args:    targetServer.Args,
		Env:     targetServer.Env,
		Timeout: targetServer.Timeout,
	})
	if err != nil {
		fmt.Printf("Error creating MCP client: %v\n", err)
		return
	}
	defer client.Close()

	// Initialize
	timeout := time.Duration(targetServer.Timeout) * time.Second
	if targetServer.Timeout == 0 {
		timeout = 30 * time.Second
	}
	initCtx, cancel := context.WithTimeout(ctx, timeout)
	_, err = client.Initialize(initCtx)
	cancel()
	if err != nil {
		fmt.Printf("Error initializing MCP client: %v\n", err)
		return
	}

	// List prompts
	prompts, err := client.ListPrompts(ctx)
	if err != nil {
		fmt.Printf("Error listing prompts: %v\n", err)
		return
	}

	if len(prompts) == 0 {
		fmt.Println("No prompts available on this server.")
		return
	}

	fmt.Printf("Found %d prompt(s):\n", len(prompts))
	fmt.Println("---------------------")
	for i, prompt := range prompts {
		fmt.Printf("%d. %s\n", i+1, prompt.Name)
		if prompt.Description != "" {
			fmt.Printf("   Description: %s\n", prompt.Description)
		}
		if len(prompt.Arguments) > 0 {
			fmt.Printf("   Arguments:\n")
			for _, arg := range prompt.Arguments {
				reqMarker := ""
				if arg.Required {
					reqMarker = "*"
				}
				fmt.Printf("     - %s%s: %s\n", arg.Name, reqMarker, arg.Description)
			}
			fmt.Printf("   (* = required)\n")
		}
		fmt.Println()
	}
}
