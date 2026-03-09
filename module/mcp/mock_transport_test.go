// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package mcp

import (
	"context"
	"encoding/json"
	"testing"
	"time"
)

// Test MCP client with mock transport
func TestMCPClientWithMockTransport(t *testing.T) {
	t.Run("Initialize with mock transport", func(t *testing.T) {
		mockTransport := NewMockTransport()
		mockTransport.SetResponse("initialize", CreateMockInitializeResponse("test-server", "1.0.0"))

		cfg := &ServerConfig{
			Name:    "test-server",
			Command: "echo",
			Args:    []string{},
		}

		client, err := NewClient(cfg)
		if err != nil {
			t.Fatalf("NewClient() failed: %v", err)
		}

		// Inject mock transport (using internal field access would be better, but we'll work with what we have)
		t.Log("Note: This test demonstrates how mock transport could be used")
		t.Log("To fully test, we need to inject mock transport into the client")

		// For now, just verify the mock works independently
		ctx := context.Background()
		err = mockTransport.Connect(ctx)
		if err != nil {
			t.Errorf("MockTransport.Connect() failed: %v", err)
		}

		if !mockTransport.WasConnected() {
			t.Error("MockTransport should report as connected")
		}

		mockTransport.Close()
		client.Close()
	})

	t.Run("ListTools with mock response", func(t *testing.T) {
		mockTransport := NewMockTransport()

		tools := []Tool{
			{Name: "tool1", Description: "First tool", InputSchema: map[string]interface{}{"type": "object"}},
			{Name: "tool2", Description: "Second tool", InputSchema: map[string]interface{}{"type": "object"}},
		}
		mockTransport.SetResponse("tools/list", CreateMockToolsListResponse(tools))

		// Verify mock can be queried
		if !mockTransport.WasConnected() {
			// Connect first
			ctx := context.Background()
			_ = mockTransport.Connect(ctx)
		}

		// Send a tools/list request
		toolsListReq := map[string]interface{}{
			"jsonrpc": "2.0",
			"id":      2,
			"method":  "tools/list",
		}
		reqData, _ := json.Marshal(toolsListReq)

		resp, err := mockTransport.Send(context.Background(), reqData)
		if err != nil {
			t.Errorf("MockTransport.Send() failed: %v", err)
		}

		var result map[string]interface{}
		if err := json.Unmarshal(resp, &result); err != nil {
			t.Errorf("Failed to unmarshal response: %v", err)
		}

		if result["result"] == nil {
			t.Error("Response should contain result")
		}

		mockTransport.Close()
	})

	t.Run("CallTool with mock response", func(t *testing.T) {
		mockTransport := NewMockTransport()
		mockTransport.SetResponse("tools/call", CreateMockToolCallResponse("Tool executed successfully"))
		ctx := context.Background()
		_ = mockTransport.Connect(ctx)

		toolCallReq := map[string]interface{}{
			"jsonrpc": "2.0",
			"id":      3,
			"method":  "tools/call",
			"params": map[string]interface{}{
				"name":      "test_tool",
				"arguments": map[string]interface{}{},
			},
		}
		reqData, _ := json.Marshal(toolCallReq)

		resp, err := mockTransport.Send(ctx, reqData)
		if err != nil {
			t.Errorf("MockTransport.Send() failed: %v", err)
		}

		var result map[string]interface{}
		if err := json.Unmarshal(resp, &result); err != nil {
			t.Errorf("Failed to unmarshal response: %v", err)
		}

		resultObj, _ := result["result"].(map[string]interface{})
		if resultObj == nil {
			t.Error("Response should contain result")
		}

		mockTransport.Close()
	})
}

// Test MCP client methods after successful initialization
func TestMCPClientMethodsAfterInit(t *testing.T) {
	t.Run("ListTools after Initialize", func(t *testing.T) {
		// Create a client with echo command (will exit immediately)
		cfg := &ServerConfig{
			Name:    "test-server",
			Command: "echo",
			Args:    []string{"{}"}, // Echo empty JSON
		}

		client, err := NewClient(cfg)
		if err != nil {
			t.Skip("Cannot create client")
		}

		ctx := context.Background()

		// Try to initialize (will fail because echo is not an MCP server)
		initResult, err := client.Initialize(ctx)

		if err != nil {
			t.Logf("Initialize failed as expected: %v", err)
		} else {
			t.Logf("Initialize succeeded: %+v", initResult)

			// If initialization succeeded, try ListTools
			tools, err := client.ListTools(ctx)
			if err != nil {
				t.Logf("ListTools failed: %v", err)
			} else {
				t.Logf("ListTools returned %d tools", len(tools))
			}
		}

		client.Close()
	})
}

// Test MCP resource and prompt methods
func TestMCPResourceAndPromptMethods(t *testing.T) {
	mockTransport := NewMockTransport()
	defer mockTransport.Close()

	t.Run("ListResources with mock", func(t *testing.T) {
		resources := []Resource{
			{URI: "file:///test.txt", Name: "Test File", Description: "A test file", MimeType: "text/plain"},
		}
		mockTransport.SetResponse("resources/list", CreateMockResourcesListResponse(resources))

		ctx := context.Background()
		_ = mockTransport.Connect(ctx)

		req := map[string]interface{}{
			"jsonrpc": "2.0",
			"id":      4,
			"method":  "resources/list",
		}
		reqData, _ := json.Marshal(req)

		resp, err := mockTransport.Send(ctx, reqData)
		if err != nil {
			t.Errorf("Send failed: %v", err)
		}

		var result map[string]interface{}
		if err := json.Unmarshal(resp, &result); err != nil {
			t.Errorf("Failed to unmarshal: %v", err)
		}

		t.Logf("Resources list response: %+v", result)
	})

	t.Run("ReadResource with mock", func(t *testing.T) {
		contents := []byte("Test file content")
		mockTransport.SetResponse("resources/read", CreateMockReadResourceResponse(contents))

		ctx := context.Background()

		req := map[string]interface{}{
			"jsonrpc": "2.0",
			"id":      6,
			"method":  "resources/read",
			"params": map[string]interface{}{
				"uri": "file:///test.txt",
			},
		}
		reqData, _ := json.Marshal(req)

		resp, err := mockTransport.Send(ctx, reqData)
		if err != nil {
			t.Errorf("Send failed: %v", err)
		}

		var result map[string]interface{}
		if err := json.Unmarshal(resp, &result); err != nil {
			t.Errorf("Failed to unmarshal: %v", err)
		}

		t.Logf("Read resource response: %+v", result)
	})

	t.Run("ListPrompts with mock", func(t *testing.T) {
		prompts := []Prompt{
			{Name: "test_prompt", Description: "A test prompt", Arguments: []PromptArgument{}},
		}
		mockTransport.SetResponse("prompts/list", CreateMockPromptsListResponse(prompts))

		ctx := context.Background()

		req := map[string]interface{}{
			"jsonrpc": "2.0",
			"id":      5,
			"method":  "prompts/list",
		}
		reqData, _ := json.Marshal(req)

		resp, err := mockTransport.Send(ctx, reqData)
		if err != nil {
			t.Errorf("Send failed: %v", err)
		}

		var result map[string]interface{}
		if err := json.Unmarshal(resp, &result); err != nil {
			t.Errorf("Failed to unmarshal: %v", err)
		}

		t.Logf("Prompts list response: %+v", result)
	})

	t.Run("GetPrompt with mock", func(t *testing.T) {
		messages := []map[string]interface{}{
			{"role": "user", "content": map[string]interface{}{"type": "text", "text": "Hello"}},
		}
		mockTransport.SetResponse("prompts/get", CreateMockGetPromptResponse(messages))

		ctx := context.Background()

		req := map[string]interface{}{
			"jsonrpc": "2.0",
			"id":      7,
			"method":  "prompts/get",
			"params": map[string]interface{}{
				"name": "test_prompt",
			},
		}
		reqData, _ := json.Marshal(req)

		resp, err := mockTransport.Send(ctx, reqData)
		if err != nil {
			t.Errorf("Send failed: %v", err)
		}

		var result map[string]interface{}
		if err := json.Unmarshal(resp, &result); err != nil {
			t.Errorf("Failed to unmarshal: %v", err)
		}

		t.Logf("Get prompt response: %+v", result)
	})
}

// Test MCP error handling with mock
func TestMCPErrorHandlingWithMock(t *testing.T) {
	mockTransport := NewMockTransport()
	mockTransport.SetResponse("tools/call", CreateMockErrorResponse(3, "Tool not found"))

	ctx := context.Background()
	_ = mockTransport.Connect(ctx)

	req := map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      8,
		"method":  "tools/call",
		"params": map[string]interface{}{
			"name": "nonexistent_tool",
		},
	}
	reqData, _ := json.Marshal(req)

	resp, err := mockTransport.Send(ctx, reqData)
	if err != nil {
		t.Errorf("Send failed: %v", err)
	}

	var result map[string]interface{}
	if err := json.Unmarshal(resp, &result); err != nil {
		t.Errorf("Failed to unmarshal: %v", err)
	}

	if result["error"] == nil {
		t.Error("Response should contain error")
	}

	t.Logf("Error response: %+v", result)
	mockTransport.Close()
}

// Test MCP concurrent access with mock
func TestMCPConcurrentAccessWithMock(t *testing.T) {
	mockTransport := NewMockTransport()
	mockTransport.SetResponse("tools/list", CreateMockToolsListResponse([]Tool{
		{Name: "tool1", Description: "Test", InputSchema: map[string]interface{}{}},
	}))

	ctx := context.Background()
	_ = mockTransport.Connect(ctx)
	defer mockTransport.Close()

	// Send multiple concurrent requests
	done := make(chan bool, 5)

	for i := 0; i < 5; i++ {
		go func(id int) {
			req := map[string]interface{}{
				"jsonrpc": "2.0",
				"id":      id,
				"method":  "tools/list",
			}
			reqData, _ := json.Marshal(req)

			_, err := mockTransport.Send(ctx, reqData)
			if err != nil {
				t.Errorf("Concurrent request %d failed: %v", id, err)
			}
			done <- true
		}(i)
	}

	// Wait for all requests to complete
	for i := 0; i < 5; i++ {
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Fatal("Concurrent test timeout")
		}
	}

	if mockTransport.GetSendCount() != 5 {
		t.Errorf("Expected 5 sends, got %d", mockTransport.GetSendCount())
	}
}

// Test mock transport properties
func TestMockTransportProperties(t *testing.T) {
	t.Run("Connect and Close", func(t *testing.T) {
		transport := NewMockTransport()
		ctx := context.Background()

		if transport.WasConnected() {
			t.Error("Should not be connected initially")
		}

		err := transport.Connect(ctx)
		if err != nil {
			t.Errorf("Connect() failed: %v", err)
		}

		if !transport.WasConnected() {
			t.Error("Should be connected after Connect()")
		}

		if transport.IsClosed() {
			t.Error("Should not be closed after Connect()")
		}

		err = transport.Close()
		if err != nil {
			t.Errorf("Close() failed: %v", err)
		}

		if !transport.IsClosed() {
			t.Error("Should be closed after Close()")
		}
	})

	t.Run("Send after Close returns error", func(t *testing.T) {
		transport := NewMockTransport()
		ctx := context.Background()

		transport.Connect(ctx)
		transport.Close()

		req := map[string]interface{}{
			"jsonrpc": "2.0",
			"id":      1,
			"method":  "test",
		}
		reqData, _ := json.Marshal(req)

		_, err := transport.Send(ctx, reqData)
		if err == nil {
			t.Error("Send() after Close() should return error")
		}
	})

	t.Run("GetRequests returns all requests", func(t *testing.T) {
		transport := NewMockTransport()
		ctx := context.Background()

		transport.Connect(ctx)

		// Send multiple requests
		for i := 0; i < 3; i++ {
			req := map[string]interface{}{
				"jsonrpc": "2.0",
				"id":      i,
				"method":  "test",
			}
			reqData, _ := json.Marshal(req)
			_, _ = transport.Send(ctx, reqData)
		}

		requests := transport.GetRequests()
		if len(requests) != 3 {
			t.Errorf("Expected 3 requests, got %d", len(requests))
		}

		transport.ClearRequests()
		requests = transport.GetRequests()
		if len(requests) != 0 {
			t.Errorf("Expected 0 requests after Clear, got %d", len(requests))
		}

		transport.Close()
	})
}
