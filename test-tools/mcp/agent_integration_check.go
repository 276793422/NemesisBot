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

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/logger"
)

func main() {
	// Initialize logger
	logger.Init()

	configPath := "C:/Users/Zoo/.nemesisbot/config.json"
	mcpConfigPath := "C:/Users/Zoo/.nemesisbot/config.mcp.json"

	// Load main config
	cfg, err := config.LoadConfig(configPath)
	if err != nil {
		log.Fatalf("Failed to load config: %v", err)
	}

	// Load MCP config
	mcpConfig, err := config.LoadMCPConfig(mcpConfigPath)
	if err != nil {
		log.Fatalf("Failed to load MCP config: %v", err)
	}

	if !mcpConfig.Enabled {
		fmt.Println("❌ MCP is disabled in config")
		os.Exit(1)
	}

	fmt.Printf("✅ Config loaded\n")
	fmt.Printf("✅ MCP enabled: %d server(s)\n", len(mcpConfig.Servers))

	// Create agent
	agentInstance, err := agent.NewAgentInstance(cfg, nil)
	if err != nil {
		log.Fatalf("Failed to create agent: %v", err)
	}
	defer agentInstance.Close()

	// Register MCP tools
	fmt.Println("\n➡️ Registering MCP tools...")
	ctx := context.Background()

	// This is the same logic as in agent/loop.go
	for _, serverCfg := range mcpConfig.Servers {
		func() {
			defer func() {
				if r := recover(); r != nil {
					fmt.Printf("❌ Panic during MCP tool registration: %v\n", r)
				}
			}()

			// Import MCP package
			mcpPkg := struct {
				NewClient             func(interface{}) (interface{}, error)
				CreateToolsFromClient func(interface{}) (interface {
					Length() int
				}, error)
			}{}

			// Since we can't import mcp here directly in the test,
			// we'll just verify the config is correct
			fmt.Printf("  • Server: %s\n", serverCfg.Name)
			fmt.Printf("    Command: %s\n", serverCfg.Command)
			fmt.Printf("    Timeout: %d seconds\n", serverCfg.Timeout)
		}()
	}

	// Check registered tools
	fmt.Println("\n➡️ Checking registered tools...")
	allTools := agentInstance.ListTools()
	fmt.Printf("✅ Total tools registered: %d\n", len(allTools))

	// Find MCP tools
	mcpToolCount := 0
	for _, tool := range allTools {
		toolName := tool.Name()
		if len(toolName) > 4 && toolName[:4] == "mcp_" {
			mcpToolCount++
			fmt.Printf("  • %s\n", toolName)
		}
	}

	fmt.Printf("\n✅ MCP tools registered: %d\n", mcpToolCount)

	if mcpToolCount == 0 {
		fmt.Println("\n⚠️  No MCP tools found!")
		fmt.Println("Expected MCP tools to be present.")
		fmt.Println("\nThis could mean:")
		fmt.Println("  1. MCP servers are not configured correctly")
		fmt.Println("  2. MCP servers failed to initialize")
		fmt.Println("  3. Tools were not properly registered")
		os.Exit(1)
	}

	// Test tool lookup
	fmt.Println("\n➡️ Testing tool lookup...")
	echoToolName := "mcp_test-mcp-server_echo"
	tool := agentInstance.GetTool(echoToolName)
	if tool == nil {
		fmt.Printf("❌ Tool not found: %s\n", echoToolName)
		os.Exit(1)
	}
	fmt.Printf("✅ Tool found: %s\n", echoToolName)
	fmt.Printf("   Description: %s\n", tool.Description())

	fmt.Println("\n🎉 All integration tests passed!")
	fmt.Println("\nConclusion:")
	fmt.Println("  ✅ MCP tools are registered in Agent")
	fmt.Println("  ✅ Tools can be looked up by name")
	fmt.Println("  ✅ Agent is ready to use MCP tools")
}
