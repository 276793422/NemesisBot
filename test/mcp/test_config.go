// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

//go:build ignore
// +build ignore

package main

import (
	"fmt"
	"log"

	"github.com/276793422/NemesisBot/module/config"
)

func main() {
	mcpConfigPath := "C:/Users/Zoo/.nemesisbot/config.mcp.json"

	// Load MCP config
	mcpConfig, err := config.LoadMCPConfig(mcpConfigPath)
	if err != nil {
		log.Fatalf("Failed to load MCP config: %v", err)
	}

	fmt.Println("✅ MCP Config loaded successfully!")
	fmt.Printf("  Enabled: %v\n", mcpConfig.Enabled)
	fmt.Printf("  Timeout: %d seconds\n", mcpConfig.Timeout)
	fmt.Printf("  Servers: %d\n", len(mcpConfig.Servers))

	for i, server := range mcpConfig.Servers {
		fmt.Printf("\n  Server %d:\n", i+1)
		fmt.Printf("    Name: %s\n", server.Name)
		fmt.Printf("    Command: %s\n", server.Command)
		fmt.Printf("    Args: %v\n", server.Args)
		fmt.Printf("    Env: %v\n", server.Env)
		fmt.Printf("    Timeout: %d seconds\n", server.Timeout)
	}

	fmt.Println("\n✅ MCP config loading test PASSED!")
}
