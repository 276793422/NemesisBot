// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

//go:build ignore
// +build ignore

package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/mcp"
	"github.com/276793422/NemesisBot/module/tools"
)

func main() {
	mcpConfigPath := "C:/Users/Zoo/.nemesisbot/config.mcp.json"

	// Load MCP config
	mcpConfig, err := config.LoadMCPConfig(mcpConfigPath)
	if err != nil {
		log.Fatalf("Failed to load MCP config: %v", err)
	}

	if !mcpConfig.Enabled {
		fmt.Println("❌ MCP is disabled in config")
		os.Exit(1)
	}

	fmt.Printf("✅ MCP Config loaded: %d server(s)\n", len(mcpConfig.Servers))

	ctx := context.Background()
	var allTools []tools.Tool

	// Register MCP tools from each server
	for _, serverCfg := range mcpConfig.Servers {
		fmt.Printf("\n➡️ Connecting to MCP server: %s\n", serverCfg.Name)

		// Create MCP client
		client, err := mcp.NewClient(&mcp.ServerConfig{
			Name:    serverCfg.Name,
			Command: serverCfg.Command,
			Args:    serverCfg.Args,
			Env:     serverCfg.Env,
			Timeout: serverCfg.Timeout,
		})
		if err != nil {
			fmt.Printf("❌ Failed to create MCP client: %v\n", err)
			continue
		}

		// Initialize with timeout
		timeout := time.Duration(mcpConfig.Timeout) * time.Second
		if serverCfg.Timeout > 0 {
			timeout = time.Duration(serverCfg.Timeout) * time.Second
		}
		initCtx, cancel := context.WithTimeout(ctx, timeout)
		_, err = client.Initialize(initCtx)
		cancel()
		if err != nil {
			fmt.Printf("❌ Failed to initialize MCP client: %v\n", err)
			client.Close()
			continue
		}
		fmt.Printf("✅ Connected to %s\n", serverCfg.Name)

		// Create tool adapters
		serverTools, err := mcp.CreateToolsFromClient(client)
		if err != nil {
			fmt.Printf("❌ Failed to create tools: %v\n", err)
			client.Close()
			continue
		}

		allTools = append(allTools, serverTools...)
		fmt.Printf("✅ Registered %d tool(s) from %s\n", len(serverTools), serverCfg.Name)

		// Don't close the client - keep it alive for the tools
		// The tools will use the client, and we'll clean up at the end
		defer client.Close()
	}

	fmt.Printf("\n✅ Total MCP tools registered: %d\n", len(allTools))
	for _, tool := range allTools {
		fmt.Printf("   - %s: %s\n", tool.Name(), tool.Description())
	}

	// Test calling an MCP tool
	if len(allTools) > 0 {
		fmt.Println("\n➡️ Testing MCP tool call...")

		// Find the echo tool (it's named with underscores)
		var echoTool tools.Tool
		for _, tool := range allTools {
			fmt.Printf("Available tool: %s\n", tool.Name())
			if tool.Name() == "mcp_test-mcp-server_echo" {
				echoTool = tool
				break
			}
		}

		if echoTool != nil {
			fmt.Printf("Calling tool: %s\n", echoTool.Name())

			result := echoTool.Execute(ctx, map[string]interface{}{
				"text": "Hello from MCP integration test!",
			})

			if result.IsError {
				fmt.Printf("❌ Tool call failed: %s\n", result.ForLLM)
				if result.Err != nil {
					fmt.Printf("   Error: %v\n", result.Err)
				}
			} else {
				fmt.Printf("✅ Tool result: %s\n", result.ForLLM)
			}
		} else {
			fmt.Println("⚠️  Echo tool not found")
		}
	}

	fmt.Println("\n🎉 MCP integration test completed successfully!")
}
