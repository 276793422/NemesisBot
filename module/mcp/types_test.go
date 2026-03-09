// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package mcp

import (
	"encoding/json"
	"testing"
)

// TestJSONRPCError tests the JSONRPCError Error() method
func TestJSONRPCError(t *testing.T) {
	t.Run("Error with data", func(t *testing.T) {
		err := &JSONRPCError{
			Code:    -32600,
			Message: "Invalid Request",
			Data:    "expected id",
		}

		result := err.Error()
		expected := "MCP error -32600: Invalid Request (data: expected id)"
		if result != expected {
			t.Errorf("Expected %q, got %q", expected, result)
		}
	})

	t.Run("Error without data", func(t *testing.T) {
		err := &JSONRPCError{
			Code:    -32601,
			Message: "Method not found",
		}

		result := err.Error()
		expected := "MCP error -32601: Method not found"
		if result != expected {
			t.Errorf("Expected %q, got %q", expected, result)
		}
	})
}

// TestDecodeResult tests the decodeResult function
func TestDecodeResult(t *testing.T) {
	t.Run("Decode successful result", func(t *testing.T) {
		rawJSON := json.RawMessage(`{"name":"test","value":123}`)
		resp := &JSONRPCResponse{
			JSONRPC: JSONRPCVersion,
			ID:      1,
			Result:  rawJSON,
		}

		var result struct {
			Name  string `json:"name"`
			Value int    `json:"value"`
		}

		err := decodeResult(resp, &result)
		if err != nil {
			t.Fatalf("decodeResult failed: %v", err)
		}

		if result.Name != "test" {
			t.Errorf("Expected name 'test', got %q", result.Name)
		}

		if result.Value != 123 {
			t.Errorf("Expected value 123, got %d", result.Value)
		}
	})

	t.Run("Decode result with error response", func(t *testing.T) {
		resp := &JSONRPCResponse{
			JSONRPC: JSONRPCVersion,
			ID:      1,
			Error: &JSONRPCError{
				Code:    -32600,
				Message: "Invalid Request",
			},
		}

		var result struct {
			Name string `json:"name"`
		}

		err := decodeResult(resp, &result)
		if err == nil {
			t.Error("Expected error for response with error field")
		}
	})

	t.Run("Decode empty result", func(t *testing.T) {
		resp := &JSONRPCResponse{
			JSONRPC: JSONRPCVersion,
			ID:      1,
			Result:  json.RawMessage{},
		}

		var result struct {
			Name string `json:"name"`
		}

		err := decodeResult(resp, &result)
		if err == nil {
			t.Error("Expected error for empty result")
		}
	})

	t.Run("Decode invalid JSON", func(t *testing.T) {
		rawJSON := json.RawMessage(`{invalid json}`)
		resp := &JSONRPCResponse{
			JSONRPC: JSONRPCVersion,
			ID:      1,
			Result:  rawJSON,
		}

		var result struct {
			Name string `json:"name"`
		}

		err := decodeResult(resp, &result)
		if err == nil {
			t.Error("Expected error for invalid JSON")
		}
	})
}

// TestProtocolVersionConstants tests protocol version constants
func TestProtocolVersionConstants(t *testing.T) {
	if ProtocolVersion != "2025-06-18" {
		t.Errorf("Expected ProtocolVersion '2025-06-18', got %q", ProtocolVersion)
	}

	if JSONRPCVersion != "2.0" {
		t.Errorf("Expected JSONRPCVersion '2.0', got %q", JSONRPCVersion)
	}
}

// TestJSONRPCRequest tests JSONRPCRequest structure
func TestJSONRPCRequest(t *testing.T) {
	t.Run("Create request with params", func(t *testing.T) {
		req := &JSONRPCRequest{
			JSONRPC: JSONRPCVersion,
			ID:      1,
			Method:  "test/method",
			Params: map[string]interface{}{
				"arg1": "value1",
				"arg2": 123,
			},
		}

		if req.JSONRPC != JSONRPCVersion {
			t.Error("JSONRPC version not set correctly")
		}

		if req.Method != "test/method" {
			t.Error("Method not set correctly")
		}

		if req.Params == nil {
			t.Error("Params should not be nil")
		}
	})

	t.Run("Create request without params", func(t *testing.T) {
		req := &JSONRPCRequest{
			JSONRPC: JSONRPCVersion,
			ID:      2,
			Method:  "test/no-params",
		}

		if req.Params != nil {
			t.Error("Params should be nil when not provided")
		}
	})

	t.Run("Create request without ID (notification)", func(t *testing.T) {
		req := &JSONRPCRequest{
			JSONRPC: JSONRPCVersion,
			Method:  "test/notification",
		}

		if req.ID != nil {
			t.Error("ID should be nil for notifications")
		}
	})
}

// TestTool tests Tool structure
func TestTool(t *testing.T) {
	t.Run("Create tool", func(t *testing.T) {
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
			t.Errorf("Expected name 'test_tool', got %q", tool.Name)
		}

		if tool.Description != "A test tool" {
			t.Errorf("Expected description 'A test tool', got %q", tool.Description)
		}

		if tool.InputSchema == nil {
			t.Error("InputSchema should not be nil")
		}
	})
}

// TestToolContent tests ToolContent structure
func TestToolContent(t *testing.T) {
	t.Run("Text content", func(t *testing.T) {
		content := ToolContent{
			Type: "text",
			Text: "Test content",
		}

		if content.Type != "text" {
			t.Errorf("Expected type 'text', got %q", content.Type)
		}

		if content.Text != "Test content" {
			t.Errorf("Expected text 'Test content', got %q", content.Text)
		}
	})

	t.Run("Image content", func(t *testing.T) {
		content := ToolContent{
			Type:     "image",
			Data:     "base64data",
			MimeType: "image/png",
		}

		if content.Type != "image" {
			t.Errorf("Expected type 'image', got %q", content.Type)
		}

		if content.Data != "base64data" {
			t.Errorf("Expected data 'base64data', got %q", content.Data)
		}

		if content.MimeType != "image/png" {
			t.Errorf("Expected mime type 'image/png', got %q", content.MimeType)
		}
	})

	t.Run("Resource content", func(t *testing.T) {
		content := ToolContent{
			Type: "resource",
			Data: "resource_uri",
		}

		if content.Type != "resource" {
			t.Errorf("Expected type 'resource', got %q", content.Type)
		}

		if content.Data != "resource_uri" {
			t.Errorf("Expected data 'resource_uri', got %q", content.Data)
		}
	})
}

// TestToolCallResult tests ToolCallResult structure
func TestToolCallResult(t *testing.T) {
	t.Run("Successful result", func(t *testing.T) {
		result := ToolCallResult{
			Content: []ToolContent{
				{Type: "text", Text: "Success"},
			},
			IsError: false,
		}

		if result.IsError {
			t.Error("Expected IsError to be false")
		}

		if len(result.Content) != 1 {
			t.Errorf("Expected 1 content item, got %d", len(result.Content))
		}
	})

	t.Run("Error result", func(t *testing.T) {
		result := ToolCallResult{
			Content: []ToolContent{
				{Type: "text", Text: "Error occurred"},
			},
			IsError: true,
		}

		if !result.IsError {
			t.Error("Expected IsError to be true")
		}
	})
}

// TestResource tests Resource structure
func TestResource(t *testing.T) {
	resource := Resource{
		URI:         "file:///test.txt",
		Name:        "test.txt",
		Description: "A test file",
		MimeType:    "text/plain",
	}

	if resource.URI != "file:///test.txt" {
		t.Errorf("Expected URI 'file:///test.txt', got %q", resource.URI)
	}

	if resource.Name != "test.txt" {
		t.Errorf("Expected name 'test.txt', got %q", resource.Name)
	}

	if resource.Description != "A test file" {
		t.Errorf("Expected description 'A test file', got %q", resource.Description)
	}

	if resource.MimeType != "text/plain" {
		t.Errorf("Expected mime type 'text/plain', got %q", resource.MimeType)
	}
}

// TestResourceContent tests ResourceContent structure
func TestResourceContent(t *testing.T) {
	t.Run("Text resource", func(t *testing.T) {
		content := ResourceContent{
			URI:      "file:///test.txt",
			MimeType: "text/plain",
			Text:     "Hello, World!",
		}

		if content.Text != "Hello, World!" {
			t.Errorf("Expected text 'Hello, World!', got %q", content.Text)
		}
	})

	t.Run("Blob resource", func(t *testing.T) {
		blob := []byte{0x01, 0x02, 0x03}
		content := ResourceContent{
			URI:      "file:///binary.dat",
			MimeType: "application/octet-stream",
			Blob:     blob,
		}

		if len(content.Blob) != 3 {
			t.Errorf("Expected blob length 3, got %d", len(content.Blob))
		}

		if content.Blob[0] != 0x01 {
			t.Errorf("Expected first byte 0x01, got 0x%02x", content.Blob[0])
		}
	})
}

// TestPrompt tests Prompt structure
func TestPrompt(t *testing.T) {
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
		t.Errorf("Expected name 'test_prompt', got %q", prompt.Name)
	}

	if len(prompt.Arguments) != 1 {
		t.Errorf("Expected 1 argument, got %d", len(prompt.Arguments))
	}

	if !prompt.Arguments[0].Required {
		t.Error("Expected argument to be required")
	}
}

// TestPromptMessage tests PromptMessage structure
func TestPromptMessage(t *testing.T) {
	message := PromptMessage{
		Role: "user",
		Content: PromptMessageContent{
			Type: "text",
			Text: "Hello",
		},
	}

	if message.Role != "user" {
		t.Errorf("Expected role 'user', got %q", message.Role)
	}

	if message.Content.Type != "text" {
		t.Errorf("Expected content type 'text', got %q", message.Content.Type)
	}

	if message.Content.Text != "Hello" {
		t.Errorf("Expected text 'Hello', got %q", message.Content.Text)
	}
}

// TestPromptResult tests PromptResult structure
func TestPromptResult(t *testing.T) {
	result := PromptResult{
		Messages: []PromptMessage{
			{
				Role: "user",
				Content: PromptMessageContent{
					Type: "text",
					Text: "Test message",
				},
			},
		},
		Description: "A test prompt result",
	}

	if len(result.Messages) != 1 {
		t.Errorf("Expected 1 message, got %d", len(result.Messages))
	}

	if result.Description != "A test prompt result" {
		t.Errorf("Expected description 'A test prompt result', got %q", result.Description)
	}
}

// TestServerCapabilities tests ServerCapabilities structure
func TestServerCapabilities(t *testing.T) {
	capabilities := ServerCapabilities{
		Tools: &ToolCapabilities{
			ListChanged: true,
		},
		Resources: &ResourceCapabilities{
			Subscribe:   true,
			ListChanged: false,
		},
		Prompts: &PromptCapabilities{
			ListChanged: true,
		},
	}

	if capabilities.Tools == nil {
		t.Error("Tools capabilities should not be nil")
	}

	if !capabilities.Tools.ListChanged {
		t.Error("Expected Tools.ListChanged to be true")
	}

	if capabilities.Resources == nil {
		t.Error("Resources capabilities should not be nil")
	}

	if !capabilities.Resources.Subscribe {
		t.Error("Expected Resources.Subscribe to be true")
	}
}

// TestServerInfo tests ServerInfo structure
func TestServerInfo(t *testing.T) {
	info := ServerInfo{
		Name:    "Test Server",
		Version: "1.0.0",
	}

	if info.Name != "Test Server" {
		t.Errorf("Expected name 'Test Server', got %q", info.Name)
	}

	if info.Version != "1.0.0" {
		t.Errorf("Expected version '1.0.0', got %q", info.Version)
	}
}

// TestClientInfo tests ClientInfo structure
func TestClientInfo(t *testing.T) {
	info := ClientInfo{
		Name:    "Test Client",
		Version: "2.0.0",
	}

	if info.Name != "Test Client" {
		t.Errorf("Expected name 'Test Client', got %q", info.Name)
	}

	if info.Version != "2.0.0" {
		t.Errorf("Expected version '2.0.0', got %q", info.Version)
	}
}

// TestInitializeParams tests InitializeParams structure
func TestInitializeParams(t *testing.T) {
	params := InitializeParams{
		ProtocolVersion: ProtocolVersion,
		Capabilities: ClientCapabilities{
			Tools:     map[string]bool{},
			Resources: map[string]bool{},
			Prompts:   map[string]bool{},
		},
		ClientInfo: ClientInfo{
			Name:    "Test Client",
			Version: "1.0.0",
		},
	}

	if params.ProtocolVersion != ProtocolVersion {
		t.Error("Protocol version not set correctly")
	}

	if params.ClientInfo.Name != "Test Client" {
		t.Error("Client info not set correctly")
	}
}

// TestInitializeResult tests InitializeResult structure
func TestInitializeResult(t *testing.T) {
	result := InitializeResult{
		ProtocolVersion: ProtocolVersion,
		Capabilities: ServerCapabilities{
			Tools: &ToolCapabilities{},
		},
		ServerInfo: ServerInfo{
			Name:    "Test Server",
			Version: "1.0.0",
		},
	}

	if result.ProtocolVersion != ProtocolVersion {
		t.Error("Protocol version not set correctly")
	}

	if result.ServerInfo.Name != "Test Server" {
		t.Error("Server info not set correctly")
	}
}

// TestClientCapabilities tests ClientCapabilities structure
func TestClientCapabilities(t *testing.T) {
	capabilities := ClientCapabilities{
		Tools: map[string]bool{
			"list": true,
		},
		Resources: map[string]bool{},
		Prompts: map[string]bool{
			"list": true,
		},
	}

	if capabilities.Tools == nil {
		t.Error("Tools map should not be nil")
	}

	if capabilities.Resources == nil {
		t.Error("Resources map should not be nil")
	}

	if capabilities.Prompts == nil {
		t.Error("Prompts map should not be nil")
	}
}

// TestServerConfig tests ServerConfig structure
func TestServerConfig(t *testing.T) {
	config := ServerConfig{
		Name:    "test-server",
		Command: "node",
		Args:    []string{"server.js"},
		Env:     []string{"NODE_ENV=test"},
		Timeout: 30,
	}

	if config.Name != "test-server" {
		t.Errorf("Expected name 'test-server', got %q", config.Name)
	}

	if config.Command != "node" {
		t.Errorf("Expected command 'node', got %q", config.Command)
	}

	if len(config.Args) != 1 {
		t.Errorf("Expected 1 arg, got %d", len(config.Args))
	}

	if config.Args[0] != "server.js" {
		t.Errorf("Expected arg 'server.js', got %q", config.Args[0])
	}

	if config.Timeout != 30 {
		t.Errorf("Expected timeout 30, got %d", config.Timeout)
	}

	t.Run("Config with empty env", func(t *testing.T) {
		config := ServerConfig{
			Name:    "test-server",
			Command: "python",
			Args:    []string{"-m", "server"},
		}

		if len(config.Env) != 0 {
			t.Errorf("Expected empty env, got %d items", len(config.Env))
		}
	})
}

// TestToolCapabilities tests ToolCapabilities structure
func TestToolCapabilities(t *testing.T) {
	capabilities := ToolCapabilities{
		ListChanged: true,
	}

	if !capabilities.ListChanged {
		t.Error("Expected ListChanged to be true")
	}
}

// TestResourceCapabilities tests ResourceCapabilities structure
func TestResourceCapabilities(t *testing.T) {
	capabilities := ResourceCapabilities{
		Subscribe:   true,
		ListChanged: false,
	}

	if !capabilities.Subscribe {
		t.Error("Expected Subscribe to be true")
	}

	if capabilities.ListChanged {
		t.Error("Expected ListChanged to be false")
	}
}

// TestPromptCapabilities tests PromptCapabilities structure
func TestPromptCapabilities(t *testing.T) {
	capabilities := PromptCapabilities{
		ListChanged: true,
	}

	if !capabilities.ListChanged {
		t.Error("Expected ListChanged to be true")
	}
}

// TestPromptArgument tests PromptArgument structure
func TestPromptArgument(t *testing.T) {
	arg := PromptArgument{
		Name:        "test_arg",
		Description: "A test argument",
		Required:    true,
	}

	if arg.Name != "test_arg" {
		t.Errorf("Expected name 'test_arg', got %q", arg.Name)
	}

	if !arg.Required {
		t.Error("Expected Required to be true")
	}
}

// TestPromptMessageContent tests PromptMessageContent structure
func TestPromptMessageContent(t *testing.T) {
	t.Run("Text content", func(t *testing.T) {
		content := PromptMessageContent{
			Type: "text",
			Text: "Test text",
		}

		if content.Type != "text" {
			t.Errorf("Expected type 'text', got %q", content.Type)
		}

		if content.Text != "Test text" {
			t.Errorf("Expected text 'Test text', got %q", content.Text)
		}
	})

	t.Run("Image content", func(t *testing.T) {
		content := PromptMessageContent{
			Type: "image",
			Data: "base64data",
		}

		if content.Type != "image" {
			t.Errorf("Expected type 'image', got %q", content.Type)
		}

		if content.Data != "base64data" {
			t.Errorf("Expected data 'base64data', got %q", content.Data)
		}
	})

	t.Run("Resource content", func(t *testing.T) {
		content := PromptMessageContent{
			Type: "resource",
			Data: "file:///test.txt",
		}

		if content.Type != "resource" {
			t.Errorf("Expected type 'resource', got %q", content.Type)
		}

		if content.Data != "file:///test.txt" {
			t.Errorf("Expected data 'file:///test.txt', got %q", content.Data)
		}
	})
}
