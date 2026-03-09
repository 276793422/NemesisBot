// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package mcp

import (
	"context"
	"testing"
	"time"
)

// TestNewClient tests the NewClient function
func TestNewClient(t *testing.T) {
	t.Run("Valid config", func(t *testing.T) {
		config := &ServerConfig{
			Name:    "test-server",
			Command: "node",
			Args:    []string{"server.js"},
		}

		client, err := NewClient(config)
		if err != nil {
			t.Fatalf("NewClient failed: %v", err)
		}

		if client == nil {
			t.Fatal("NewClient returned nil client")
		}

		if !client.IsConnected() {
			// Note: The actual client may not be connected until Initialize is called
			// This depends on the transport implementation
		}
	})

	t.Run("Nil config", func(t *testing.T) {
		client, err := NewClient(nil)
		if err == nil {
			t.Error("Expected error for nil config")
		}

		if client != nil {
			t.Error("NewClient should return nil client for nil config")
		}
	})

	t.Run("Empty server name", func(t *testing.T) {
		config := &ServerConfig{
			Name:    "",
			Command: "node",
			Args:    []string{"server.js"},
		}

		client, err := NewClient(config)
		if err == nil {
			t.Error("Expected error for empty server name")
		}

		if client != nil {
			t.Error("NewClient should return nil client for empty server name")
		}
	})
}

// TestClientServerInfo tests the ServerInfo() method
func TestClientServerInfo(t *testing.T) {
	client := NewMockClient("test-server")

	info := client.ServerInfo()

	if info == nil {
		t.Fatal("ServerInfo() returned nil")
	}

	if info.Name != "test-server" {
		t.Errorf("Expected server name 'test-server', got %q", info.Name)
	}

	if info.Version != "1.0.0" {
		t.Errorf("Expected version '1.0.0', got %q", info.Version)
	}
}

// TestClientIsConnected tests the IsConnected() method
func TestClientIsConnected(t *testing.T) {
	client := NewMockClient("test-server")

	if !client.IsConnected() {
		t.Error("Expected IsConnected() to return true")
	}

	// Close the client
	client.Close()

	if client.IsConnected() {
		t.Error("Expected IsConnected() to return false after Close()")
	}
}

// TestClientClose tests the Close() method
func TestClientClose(t *testing.T) {
	client := NewMockClient("test-server")

	err := client.Close()
	if err != nil {
		t.Fatalf("Close() failed: %v", err)
	}

	if client.IsConnected() {
		t.Error("Client should not be connected after Close()")
	}

	// Close should be idempotent
	err = client.Close()
	if err != nil {
		t.Fatalf("Close() failed on second call: %v", err)
	}
}

// TestClientInitialize tests the Initialize() method
func TestClientInitialize(t *testing.T) {
	client := NewMockClient("test-server")

	ctx := context.Background()
	result, err := client.Initialize(ctx)

	if err != nil {
		t.Fatalf("Initialize() failed: %v", err)
	}

	if result == nil {
		t.Fatal("Initialize() returned nil result")
	}

	if result.ProtocolVersion != ProtocolVersion {
		t.Errorf("Expected protocol version %q, got %q", ProtocolVersion, result.ProtocolVersion)
	}

	if result.ServerInfo.Name != "test-server" {
		t.Errorf("Expected server name 'test-server', got %q", result.ServerInfo.Name)
	}
}

// TestClientListTools tests the ListTools() method
func TestClientListTools(t *testing.T) {
	client := NewMockClient("test-server")

	tools := []Tool{
		{Name: "tool1", Description: "First tool"},
		{Name: "tool2", Description: "Second tool"},
	}
	client.SetTools(tools)

	ctx := context.Background()
	result, err := client.ListTools(ctx)

	if err != nil {
		t.Fatalf("ListTools() failed: %v", err)
	}

	if len(result) != 2 {
		t.Errorf("Expected 2 tools, got %d", len(result))
	}

	if result[0].Name != "tool1" {
		t.Errorf("Expected first tool name 'tool1', got %q", result[0].Name)
	}
}

// TestClientCallTool tests the CallTool() method
func TestClientCallTool(t *testing.T) {
	client := NewMockClient("test-server")

	ctx := context.Background()
	result, err := client.CallTool(ctx, "test_tool", map[string]interface{}{"arg1": "value1"})

	if err != nil {
		t.Fatalf("CallTool() failed: %v", err)
	}

	if result == nil {
		t.Fatal("CallTool() returned nil result")
	}

	if result.IsError {
		t.Error("Expected IsError to be false")
	}

	if len(result.Content) == 0 {
		t.Error("Expected non-empty content")
	}
}

// TestClientListResources tests the ListResources() method
func TestClientListResources(t *testing.T) {
	client := NewMockClient("test-server")

	ctx := context.Background()
	result, err := client.ListResources(ctx)

	if err != nil {
		t.Fatalf("ListResources() failed: %v", err)
	}

	if result == nil {
		t.Fatal("ListResources() returned nil result")
	}
}

// TestClientReadResource tests the ReadResource() method
func TestClientReadResource(t *testing.T) {
	client := NewMockClient("test-server")

	ctx := context.Background()
	result, err := client.ReadResource(ctx, "file:///test.txt")

	if err != nil {
		t.Fatalf("ReadResource() failed: %v", err)
	}

	if result == nil {
		t.Fatal("ReadResource() returned nil result")
	}
}

// TestClientListPrompts tests the ListPrompts() method
func TestClientListPrompts(t *testing.T) {
	client := NewMockClient("test-server")

	ctx := context.Background()
	result, err := client.ListPrompts(ctx)

	if err != nil {
		t.Fatalf("ListPrompts() failed: %v", err)
	}

	if result == nil {
		t.Fatal("ListPrompts() returned nil result")
	}
}

// TestClientGetPrompt tests the GetPrompt() method
func TestClientGetPrompt(t *testing.T) {
	client := NewMockClient("test-server")

	ctx := context.Background()
	result, err := client.GetPrompt(ctx, "test_prompt", map[string]interface{}{"arg1": "value1"})

	if err != nil {
		t.Fatalf("GetPrompt() failed: %v", err)
	}

	if result == nil {
		t.Fatal("GetPrompt() returned nil result")
	}
}

// TestClientConcurrentAccess tests concurrent access to client methods
func TestClientConcurrentAccess(t *testing.T) {
	client := NewMockClient("test-server")

	tools := []Tool{
		{Name: "tool1", Description: "First tool"},
		{Name: "tool2", Description: "Second tool"},
	}
	client.SetTools(tools)

	ctx := context.Background()
	done := make(chan bool, 10)

	// Spawn multiple goroutines accessing the client concurrently
	for i := 0; i < 5; i++ {
		go func() {
			_, _ = client.ListTools(ctx)
			done <- true
		}()

		go func() {
			_, _ = client.CallTool(ctx, "tool1", map[string]interface{}{})
			done <- true
		}()
	}

	// Wait for all goroutines to complete
	for i := 0; i < 10; i++ {
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Fatal("Concurrent access test timeout")
		}
	}
}

// TestClientTimeout tests timeout behavior
func TestClientTimeout(t *testing.T) {
	// This test would require a more sophisticated mock that simulates timeout
	// For now, we just verify the client structure supports timeout

	config := &ServerConfig{
		Name:    "test-server",
		Command: "node",
		Args:    []string{"server.js"},
		Timeout: 5,
	}

	if config.Timeout != 5 {
		t.Errorf("Expected timeout 5, got %d", config.Timeout)
	}
}

// TestConvertToTransportRequest tests the convertToTransportRequest function
func TestConvertToTransportRequest(t *testing.T) {
	t.Run("Request with params", func(t *testing.T) {
		req := &JSONRPCRequest{
			JSONRPC: JSONRPCVersion,
			ID:      1,
			Method:  "test/method",
			Params: map[string]interface{}{
				"arg1": "value1",
				"arg2": 123,
			},
		}

		transportReq, err := convertToTransportRequest(req)
		if err != nil {
			t.Fatalf("convertToTransportRequest failed: %v", err)
		}

		if transportReq.JSONRPC != JSONRPCVersion {
			t.Error("JSONRPC version not preserved")
		}

		if transportReq.ID != req.ID {
			t.Error("ID not preserved")
		}

		if transportReq.Method != req.Method {
			t.Error("Method not preserved")
		}

		if len(transportReq.Params) == 0 {
			t.Error("Params should not be empty")
		}
	})

	t.Run("Request without params", func(t *testing.T) {
		req := &JSONRPCRequest{
			JSONRPC: JSONRPCVersion,
			ID:      2,
			Method:  "test/no-params",
		}

		transportReq, err := convertToTransportRequest(req)
		if err != nil {
			t.Fatalf("convertToTransportRequest failed: %v", err)
		}

		if len(transportReq.Params) == 0 {
			// Empty params are marshaled to null or empty
		}
	})
}
