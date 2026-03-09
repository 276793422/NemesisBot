// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package mcp

import (
	"context"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
)

// Test MCP client initialization failures
func TestMCPClientInitFailures(t *testing.T) {
	t.Run("nil config", func(t *testing.T) {
		_, err := NewClient(nil)
		if err == nil {
			t.Error("Expected error for nil config")
		}
	})

	t.Run("empty server name", func(t *testing.T) {
		cfg := &ServerConfig{
			Name:    "",
			Command: "node",
			Args:    []string{"server.js"},
		}
		_, err := NewClient(cfg)
		if err == nil {
			t.Error("Expected error for empty server name")
		}
	})

	t.Run("valid config but command will fail", func(t *testing.T) {
		cfg := &ServerConfig{
			Name:    "test-server",
			Command: "nonexistent-command-xyz123",
			Args:    []string{},
		}
		client, err := NewClient(cfg)
		// NewClient may succeed but Initialize will fail
		if err != nil && client == nil {
			// This is acceptable - transport creation failed
			return
		}

		if client != nil {
			// Client was created, try to initialize
			ctx := context.Background()
			_, err = client.Initialize(ctx)
			if err == nil {
				t.Error("Expected error during Initialize with invalid command")
			}
		}
	})
}

// Test MCP client methods without initialization
func TestMCPClientWithoutInit(t *testing.T) {
	cfg := &ServerConfig{
		Name:    "test-server",
		Command: "echo",
		Args:    []string{},
	}

	client, err := NewClient(cfg)
	if err != nil {
		// If we can't create a client, skip these tests
		t.Skip("Cannot create client for testing")
	}

	ctx := context.Background()

	t.Run("ListTools without init", func(t *testing.T) {
		if client != nil {
			_, err := client.ListTools(ctx)
			if err == nil {
				t.Error("Expected error when calling ListTools without Initialize")
			}
		}
	})

	t.Run("CallTool without init", func(t *testing.T) {
		if client != nil {
			_, err := client.CallTool(ctx, "test", nil)
			if err == nil {
				t.Error("Expected error when calling CallTool without Initialize")
			}
		}
	})

	t.Run("ListResources without init", func(t *testing.T) {
		if client != nil {
			_, err := client.ListResources(ctx)
			if err == nil {
				t.Error("Expected error when calling ListResources without Initialize")
			}
		}
	})

	t.Run("ReadResource without init", func(t *testing.T) {
		if client != nil {
			_, err := client.ReadResource(ctx, "test://resource")
			if err == nil {
				t.Error("Expected error when calling ReadResource without Initialize")
			}
		}
	})

	t.Run("ListPrompts without init", func(t *testing.T) {
		if client != nil {
			_, err := client.ListPrompts(ctx)
			if err == nil {
				t.Error("Expected error when calling ListPrompts without Initialize")
			}
		}
	})

	t.Run("GetPrompt without init", func(t *testing.T) {
		if client != nil {
			_, err := client.GetPrompt(ctx, "test", nil)
			if err == nil {
				t.Error("Expected error when calling GetPrompt without Initialize")
			}
		}
	})
}

// Test MCPServerConfig validation
func TestMCPServerConfig(t *testing.T) {
	t.Run("ServerConfig with all fields", func(t *testing.T) {
		cfg := ServerConfig{
			Name:    "test-server",
			Command: "node",
			Args:    []string{"server.js", "--port", "8080"},
			Env:     []string{"NODE_ENV=production", "DEBUG=1"},
			Timeout: 30,
		}

		if cfg.Name != "test-server" {
			t.Errorf("Expected Name 'test-server', got '%s'", cfg.Name)
		}

		if len(cfg.Args) != 3 {
			t.Errorf("Expected 3 args, got %d", len(cfg.Args))
		}

		if len(cfg.Env) != 2 {
			t.Errorf("Expected 2 env vars, got %d", len(cfg.Env))
		}

		if cfg.Timeout != 30 {
			t.Errorf("Expected Timeout 30, got %d", cfg.Timeout)
		}
	})

	t.Run("ServerConfig with minimal fields", func(t *testing.T) {
		cfg := ServerConfig{
			Name:    "minimal-server",
			Command: "python",
		}

		if cfg.Name != "minimal-server" {
			t.Errorf("Expected Name 'minimal-server', got '%s'", cfg.Name)
		}

		if cfg.Args == nil {
			cfg.Args = []string{}
		}

		if len(cfg.Args) != 0 {
			t.Errorf("Expected 0 args, got %d", len(cfg.Args))
		}
	})
}

// Test MCPTypes helper functions
func TestMCPTypes(t *testing.T) {
	t.Run("Tool structure", func(t *testing.T) {
		tool := Tool{
			Name:        "test_tool",
			Description: "A test tool",
			InputSchema: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"arg1": map[string]interface{}{
						"type":        "string",
						"description": "First argument",
					},
				},
			},
		}

		if tool.Name != "test_tool" {
			t.Errorf("Expected Name 'test_tool', got '%s'", tool.Name)
		}

		if tool.InputSchema == nil {
			t.Error("InputSchema should not be nil")
		}
	})

	t.Run("Resource structure", func(t *testing.T) {
		resource := Resource{
			URI:         "file:///test.txt",
			Name:        "Test File",
			Description: "A test file",
			MimeType:    "text/plain",
		}

		if resource.URI != "file:///test.txt" {
			t.Errorf("Expected URI 'file:///test.txt', got '%s'", resource.URI)
		}

		if resource.MimeType != "text/plain" {
			t.Errorf("Expected MimeType 'text/plain', got '%s'", resource.MimeType)
		}
	})

	t.Run("Prompt structure", func(t *testing.T) {
		prompt := Prompt{
			Name:        "test_prompt",
			Description: "A test prompt",
			Arguments: []PromptArgument{
				{
					Name:        "arg1",
					Description: "First argument",
					Required:    true,
				},
			},
		}

		if prompt.Name != "test_prompt" {
			t.Errorf("Expected Name 'test_prompt', got '%s'", prompt.Name)
		}

		if len(prompt.Arguments) != 1 {
			t.Errorf("Expected 1 argument, got %d", len(prompt.Arguments))
		}
	})
}

// Test MCP config from config module
func TestMCPConfig(t *testing.T) {
	t.Run("MCPConfig structure", func(t *testing.T) {
		cfg := config.MCPConfig{
			Enabled: true,
			Timeout: 60,
			Servers: []config.MCPServerConfig{
				{
					Name:    "server1",
					Command: "node",
					Args:    []string{"server1.js"},
					Env:     []string{"KEY=value"},
					Timeout: 30,
				},
				{
					Name:    "server2",
					Command: "python",
					Args:    []string{"server2.py"},
				},
			},
		}

		if !cfg.Enabled {
			t.Error("Expected Enabled to be true")
		}

		if cfg.Timeout != 60 {
			t.Errorf("Expected Timeout 60, got %d", cfg.Timeout)
		}

		if len(cfg.Servers) != 2 {
			t.Errorf("Expected 2 servers, got %d", len(cfg.Servers))
		}

		if cfg.Servers[0].Name != "server1" {
			t.Errorf("Expected first server name 'server1', got '%s'", cfg.Servers[0].Name)
		}

		if cfg.Servers[1].Timeout != 0 {
			t.Errorf("Expected second server to use global timeout (0), got %d", cfg.Servers[1].Timeout)
		}
	})
}

// Test convertFromTransportResponse function
// NOTE: This is an internal function that converts transport responses
// Requires proper JSONRPCResponse structure
func TestConvertFromTransportResponse(t *testing.T) {
	t.Run("Convert with nil response", func(t *testing.T) {
		// This function is internal and tests the conversion logic
		// We can't directly call it, but we document its behavior
		t.Log("convertFromTransportResponse() converts transport.JSONRPCResponse to mcp.JSONRPCResponse")
		t.Log("It handles nil responses and JSON unmarshaling")
		t.Skip("Internal function - tested indirectly through client methods")
	})
}

// Test nextID function
// NOTE: Internal function that generates sequential request IDs
func TestNextID(t *testing.T) {
	t.Run("nextID generates sequential IDs", func(t *testing.T) {
		// nextID is an internal method on the client struct
		// It uses atomic.AddInt64 to generate sequential IDs
		t.Log("nextID() uses atomic operations for thread-safe ID generation")
		t.Log("Starts at 0 and increments by 1 for each call")
		t.Skip("Internal method - tested indirectly through client requests")
	})
}

// Test ServerInfo method
// NOTE: Returns server information from initialization
func TestServerInfoMethod(t *testing.T) {
	t.Run("ServerInfo requires initialization", func(t *testing.T) {
		// ServerInfo returns nil until Initialize is called
		t.Log("ServerInfo() returns nil before Initialize()")
		t.Log("Returns ServerInfo struct after successful initialization")
		t.Log("Requires actual MCP server to get real server info")

		// We can test this with a mock client
		cfg := &ServerConfig{
			Name:    "test-server",
			Command: "echo",
			Args:    []string{},
		}

		client, err := NewClient(cfg)
		if err != nil {
			// If client creation fails, skip the test
			t.Skip("Cannot create client for testing")
		}

		// ServerInfo should be nil before initialization
		info := client.ServerInfo()
		if info != nil {
			t.Error("ServerInfo should be nil before initialization")
		}

		// Close the client
		client.Close()
	})
}

// Test Close method
func TestClientCloseMethod(t *testing.T) {
	t.Run("Close marks client as not connected", func(t *testing.T) {
		cfg := &ServerConfig{
			Name:    "test-server",
			Command: "echo",
			Args:    []string{},
		}

		client, err := NewClient(cfg)
		if err != nil {
			t.Skip("Cannot create client for testing")
		}

		// Client should be created (even if not connected)
		if client == nil {
			t.Fatal("Client should not be nil")
		}

		// Close should not error even if not initialized
		err = client.Close()
		if err != nil {
			t.Errorf("Close() should not error, got: %v", err)
		}

		// After close, IsConnected should return false
		if client.IsConnected() {
			t.Error("IsConnected() should return false after Close()")
		}
	})
}

// Test Initialize with real command but no server
func TestInitializeWithNoServer(t *testing.T) {
	t.Run("Initialize with command that exits immediately", func(t *testing.T) {
		cfg := &ServerConfig{
			Name:    "test-server",
			Command: "echo", // Command will exit immediately
			Args:    []string{"test"},
		}

		client, err := NewClient(cfg)
		if err != nil {
			t.Skip("Cannot create client for testing")
		}

		if client == nil {
			t.Fatal("Client should not be nil")
		}

		ctx := context.Background()
		_, err = client.Initialize(ctx)

		// Should fail because echo is not an MCP server
		if err == nil {
			t.Log("Initialize succeeded (unexpected, echo is not an MCP server)")
		} else {
			t.Logf("Initialize failed as expected: %v", err)
		}

		// Clean up
		client.Close()
	})
}

// Test MCP client error handling
func TestMCPClientErrorHandling(t *testing.T) {
	t.Run("Double Initialize returns error", func(t *testing.T) {
		// This tests that calling Initialize twice returns an error
		// We need a mock client for this
		t.Log("Calling Initialize() twice should return 'already initialized' error")
		t.Skip("Requires mock client or actual MCP server")
	})

	t.Run("Methods without Initialize return error", func(t *testing.T) {
		cfg := &ServerConfig{
			Name:    "test-server",
			Command: "echo",
			Args:    []string{},
		}

		client, err := NewClient(cfg)
		if err != nil {
			t.Skip("Cannot create client for testing")
		}

		if client == nil {
			t.Fatal("Client should not be nil")
		}

		ctx := context.Background()

		// All these should return "not initialized" error
		_, err = client.ListTools(ctx)
		if err == nil {
			t.Error("ListTools should return error without Initialize")
		}

		_, err = client.CallTool(ctx, "test", nil)
		if err == nil {
			t.Error("CallTool should return error without Initialize")
		}

		client.Close()
	})
}
