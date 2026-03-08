// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

//go:build ignore
// +build ignore

package main

import (
	"context"
	"fmt"
	"os"
	"time"

	"github.com/276793422/NemesisBot/module/mcp"
)

func main() {
	serverExe := "C:\\AI\\NemesisBot\\NemesisBot_go\\test\\mcp\\server\\mcp-test-server.exe"

	cfg := &mcp.ServerConfig{
		Name:    "test",
		Command: serverExe,
		Args:    []string{},
		Timeout: 10,
	}

	client, err := mcp.NewClient(cfg)
	if err != nil {
		fmt.Printf("❌ NewClient failed: %v\n", err)
		os.Exit(1)
	}
	defer client.Close()

	ctx := context.Background()

	// Initialize
	fmt.Println("➡️ Step 1: Initialize...")
	initResult, err := client.Initialize(ctx)
	if err != nil {
		fmt.Printf("❌ Initialize failed: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("✅ Initialized: %s v%s\n", initResult.ServerInfo.Name, initResult.ServerInfo.Version)

	// Small delay to let server settle
	time.Sleep(100 * time.Millisecond)

	// List tools
	fmt.Println("\n➡️ Step 2: List tools...")
	tools, err := client.ListTools(ctx)
	if err != nil {
		fmt.Printf("❌ ListTools failed: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("✅ Found %d tools:\n", len(tools))
	for _, tool := range tools {
		fmt.Printf("   - %s: %s\n", tool.Name, tool.Description)
	}

	// Call echo tool
	fmt.Println("\n➡️ Step 3: Call echo tool...")
	result, err := client.CallTool(ctx, "echo", map[string]interface{}{
		"text": "Hello from simple test!",
	})
	if err != nil {
		fmt.Printf("❌ CallTool failed: %v\n", err)
		os.Exit(1)
	}

	if result.IsError {
		fmt.Printf("❌ Tool returned error: %v\n", result.Content)
		os.Exit(1)
	}

	fmt.Printf("✅ Tool result: %v\n", result.Content)

	fmt.Println("\n🎉 All tests passed!")
}
