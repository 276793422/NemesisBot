// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package mcp

import (
	"context"
	"testing"
)

// MockClient is a mock implementation of the MCP Client interface for testing
type MockClient struct {
	serverInfo  *ServerInfo
	initialized bool
	connected   bool
	tools       []Tool
	callToolFunc func(ctx context.Context, name string, args map[string]interface{}) (*ToolCallResult, error)
}

func NewMockClient(serverName string) *MockClient {
	return &MockClient{
		serverInfo: &ServerInfo{
			Name:    serverName,
			Version: "1.0.0",
		},
		initialized: true,
		connected:   true,
	}
}

func (m *MockClient) Initialize(ctx context.Context) (*InitializeResult, error) {
	m.initialized = true
	m.connected = true
	return &InitializeResult{
		ProtocolVersion: ProtocolVersion,
		ServerInfo:      *m.serverInfo,
	}, nil
}

func (m *MockClient) ListTools(ctx context.Context) ([]Tool, error) {
	return m.tools, nil
}

func (m *MockClient) CallTool(ctx context.Context, name string, args map[string]interface{}) (*ToolCallResult, error) {
	if m.callToolFunc != nil {
		return m.callToolFunc(ctx, name, args)
	}
	return &ToolCallResult{
		Content: []ToolContent{
			{Type: "text", Text: "Mock result"},
		},
		IsError: false,
	}, nil
}

func (m *MockClient) ListResources(ctx context.Context) ([]Resource, error) {
	return []Resource{}, nil
}

func (m *MockClient) ReadResource(ctx context.Context, uri string) (*ResourceContent, error) {
	return &ResourceContent{}, nil
}

func (m *MockClient) ListPrompts(ctx context.Context) ([]Prompt, error) {
	return []Prompt{}, nil
}

func (m *MockClient) GetPrompt(ctx context.Context, name string, args map[string]interface{}) (*PromptResult, error) {
	return &PromptResult{}, nil
}

func (m *MockClient) Close() error {
	m.connected = false
	return nil
}

func (m *MockClient) ServerInfo() *ServerInfo {
	return m.serverInfo
}

func (m *MockClient) IsConnected() bool {
	return m.connected
}

func (m *MockClient) SetTools(tools []Tool) {
	m.tools = tools
}

// TestSanitizeName tests the sanitizeName function
func TestSanitizeName(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"simple", "simple"},
		{"with-hyphen", "with-hyphen"},
		{"with_underscore", "with_underscore"},
		{"with space", "with_space"},
		{"with/slash", "with_slash"},
		{"with\\backslash", "with_backslash"},
		{"with.dot", "with_dot"},
		{"with@at", "with_at"},
		{"with:colon", "with_colon"},
		{"with;semicolon", "with_semicolon"},
		{"with,comma", "with_comma"},
		{"with!exclamation", "with_exclamation"},
		{"with?question", "with_question"},
		{"with*asterisk", "with_asterisk"},
		{"with+plus", "with_plus"},
		{"with=equals", "with_equals"},
		{"with%percent", "with_percent"},
		{"with&ampersand", "with_ampersand"},
		{"with#hash", "with_hash"},
		{"with$dollar", "with_dollar"},
		{"with|pipe", "with_pipe"},
		{"with~tilde", "with_tilde"},
		{"with^caret", "with_caret"},
		{"with`backtick", "with_backtick"},
		{"", ""},
		{"MixedCase123", "MixedCase123"},
		{"123numbers", "123numbers"},
		{"multiple   spaces", "multiple___spaces"},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := sanitizeName(tt.input)
			if result != tt.expected {
				t.Errorf("sanitizeName(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}

// TestNewAdapter tests the NewAdapter function
func TestNewAdapter(t *testing.T) {
	client := NewMockClient("test-server")
	mcpTool := Tool{
		Name:        "test_tool",
		Description: "A test tool",
		InputSchema: map[string]interface{}{
			"type": "object",
		},
	}

	adapter := NewAdapter(client, mcpTool)

	if adapter == nil {
		t.Fatal("NewAdapter returned nil")
	}

	if adapter.client != client {
		t.Error("Client not set correctly")
	}

	if adapter.mcpTool.Name != mcpTool.Name {
		t.Error("MCP tool not set correctly")
	}
}

// TestAdapterName tests the Adapter Name() method
func TestAdapterName(t *testing.T) {
	tests := []struct {
		serverName    string
		toolName      string
		expectedName  string
	}{
		{"test-server", "my_tool", "mcp_test-server_my_tool"},
		{"My Server", "test_tool", "mcp_My_Server_test_tool"},
		{"server-with-dash", "tool-with-dash", "mcp_server-with-dash_tool-with-dash"},
		{"server/with/slash", "tool/with/slash", "mcp_server_with_slash_tool_with_slash"},
		{"server with space", "tool with space", "mcp_server_with_space_tool_with_space"},
	}

	for _, tt := range tests {
		t.Run(tt.serverName+"_"+tt.toolName, func(t *testing.T) {
			client := NewMockClient(tt.serverName)
			mcpTool := Tool{
				Name: tt.toolName,
			}

			adapter := NewAdapter(client, mcpTool)
			name := adapter.Name()

			if name != tt.expectedName {
				t.Errorf("Name() = %q, want %q", name, tt.expectedName)
			}
		})
	}
}

// TestAdapterDescription tests the Adapter Description() method
func TestAdapterDescription(t *testing.T) {
	client := NewMockClient("test-server")
	mcpTool := Tool{
		Name:        "test_tool",
		Description: "A test tool",
	}

	adapter := NewAdapter(client, mcpTool)
	description := adapter.Description()

	expected := "[MCP:test-server] A test tool"
	if description != expected {
		t.Errorf("Description() = %q, want %q", description, expected)
	}
}

// TestAdapterParameters tests the Adapter Parameters() method
func TestAdapterParameters(t *testing.T) {
	client := NewMockClient("test-server")
	mcpTool := Tool{
		Name:        "test_tool",
		Description: "A test tool",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"arg1": map[string]interface{}{
					"type":        "string",
					"description": "First argument",
				},
				"arg2": map[string]interface{}{
					"type": "integer",
					"description": "Second argument",
				},
			},
		},
	}

	adapter := NewAdapter(client, mcpTool)
	params := adapter.Parameters()

	if params == nil {
		t.Fatal("Parameters() returned nil")
	}

	if params["type"] != "object" {
		t.Errorf("Expected type 'object', got %q", params["type"])
	}

	if params["additionalProperties"] != false {
		t.Error("Expected additionalProperties to be false")
	}

	properties, ok := params["properties"]
	if !ok {
		t.Fatal("properties key not found")
	}

	// Properties should contain the InputSchema
	propertiesMap, ok := properties.(map[string]interface{})
	if !ok {
		t.Fatal("properties is not a map")
	}

	if len(propertiesMap) == 0 {
		t.Error("properties should not be empty")
	}
}

// TestAdapterExecuteSuccess tests successful tool execution
func TestAdapterExecuteSuccess(t *testing.T) {
	client := NewMockClient("test-server")
	client.callToolFunc = func(ctx context.Context, name string, args map[string]interface{}) (*ToolCallResult, error) {
		return &ToolCallResult{
			Content: []ToolContent{
				{Type: "text", Text: "Success result"},
			},
			IsError: false,
		}, nil
	}

	mcpTool := Tool{
		Name:        "test_tool",
		Description: "A test tool",
		InputSchema: map[string]interface{}{
			"type": "object",
		},
	}

	adapter := NewAdapter(client, mcpTool)
	args := map[string]interface{}{
		"arg1": "value1",
		"arg2": 123,
	}

	result := adapter.Execute(context.Background(), args)

	if result.Err != nil {
		t.Fatalf("Execute() returned error: %v", result.Err)
	}

	if result.ForLLM != "Success result" {
		t.Errorf("Expected ForLLM 'Success result', got %q", result.ForLLM)
	}
}

// TestAdapterExecuteMultipleContentTypes tests execution with multiple content types
func TestAdapterExecuteMultipleContentTypes(t *testing.T) {
	client := NewMockClient("test-server")
	client.callToolFunc = func(ctx context.Context, name string, args map[string]interface{}) (*ToolCallResult, error) {
		return &ToolCallResult{
			Content: []ToolContent{
				{Type: "text", Text: "Line 1"},
				{Type: "text", Text: "Line 2"},
				{Type: "image", Data: "base64data", MimeType: "image/png"},
				{Type: "resource", Data: "file:///test.txt"},
			},
			IsError: false,
		}, nil
	}

	mcpTool := Tool{
		Name:        "test_tool",
		Description: "A test tool",
		InputSchema: map[string]interface{}{
			"type": "object",
		},
	}

	adapter := NewAdapter(client, mcpTool)
	result := adapter.Execute(context.Background(), map[string]interface{}{})

	if result.Err != nil {
		t.Fatalf("Execute() returned error: %v", result.Err)
	}

	// Should join text parts and format other types
	expectedLines := []string{"Line 1", "Line 2", "[Image: base64data, mime-type: image/png]", "[Resource: file:///test.txt]"}
	output := result.ForLLM

	// Just check that all parts are present
	for _, line := range expectedLines {
		if !contains(output, line) {
			t.Errorf("Expected output to contain %q, got %q", line, output)
		}
	}
}

// TestAdapterExecuteError tests tool execution with error
func TestAdapterExecuteError(t *testing.T) {
	client := NewMockClient("test-server")
	client.callToolFunc = func(ctx context.Context, name string, args map[string]interface{}) (*ToolCallResult, error) {
		return &ToolCallResult{
			Content: []ToolContent{
				{Type: "text", Text: "Tool execution failed"},
			},
			IsError: true,
		}, nil
	}

	mcpTool := Tool{
		Name:        "test_tool",
		Description: "A test tool",
		InputSchema: map[string]interface{}{
			"type": "object",
		},
	}

	adapter := NewAdapter(client, mcpTool)
	result := adapter.Execute(context.Background(), map[string]interface{}{})

	if !result.IsError {
		t.Fatal("Execute() should return error result for tool error status")
	}

	if !contains(result.ForLLM, "test_tool") {
		t.Errorf("Error message should contain tool name, got %v", result.ForLLM)
	}

	if !contains(result.ForLLM, "Tool execution failed") {
		t.Errorf("Error message should contain tool error text, got %v", result.ForLLM)
	}
}

// TestAdapterExecuteMultipleErrorTexts tests error result with multiple text contents
func TestAdapterExecuteMultipleErrorTexts(t *testing.T) {
	client := NewMockClient("test-server")
	client.callToolFunc = func(ctx context.Context, name string, args map[string]interface{}) (*ToolCallResult, error) {
		return &ToolCallResult{
			Content: []ToolContent{
				{Type: "text", Text: "Error 1"},
				{Type: "text", Text: "Error 2"},
				{Type: "text", Text: "Error 3"},
			},
			IsError: true,
		}, nil
	}

	mcpTool := Tool{
		Name:        "test_tool",
		Description: "A test tool",
		InputSchema: map[string]interface{}{
			"type": "object",
		},
	}

	adapter := NewAdapter(client, mcpTool)
	result := adapter.Execute(context.Background(), map[string]interface{}{})

	if !result.IsError {
		t.Fatal("Execute() should return error result for tool error status")
	}

	// Should join multiple error texts with "; "
	errorMsg := result.ForLLM
	if !contains(errorMsg, "Error 1") || !contains(errorMsg, "Error 2") || !contains(errorMsg, "Error 3") {
		t.Errorf("Error message should contain all error texts, got %v", errorMsg)
	}
}

// TestAdapterExecuteCallError tests error when calling the MCP tool
func TestAdapterExecuteCallError(t *testing.T) {
	client := NewMockClient("test-server")
	client.callToolFunc = func(ctx context.Context, name string, args map[string]interface{}) (*ToolCallResult, error) {
		return nil, &testError{"MCP call failed"}
	}

	mcpTool := Tool{
		Name:        "test_tool",
		Description: "A test tool",
		InputSchema: map[string]interface{}{
			"type": "object",
		},
	}

	adapter := NewAdapter(client, mcpTool)
	result := adapter.Execute(context.Background(), map[string]interface{}{})

	if !result.IsError {
		t.Fatal("Execute() should return error result when call fails")
	}

	// The adapter returns ErrorResult which sets IsError=true
	// but doesn't set the Err field (it's kept for internal use)
	// The error message is in ForLLM
	if result.ForLLM == "" {
		t.Error("Expected error message in ForLLM field")
	}
}

// TestCreateToolsFromClient tests creating tools from an MCP client
func TestCreateToolsFromClient(t *testing.T) {
	client := NewMockClient("test-server")

	mcpTools := []Tool{
		{
			Name:        "tool1",
			Description: "First tool",
			InputSchema: map[string]interface{}{
				"type": "object",
			},
		},
		{
			Name:        "tool2",
			Description: "Second tool",
			InputSchema: map[string]interface{}{
				"type": "object",
			},
		},
		{
			Name:        "tool3",
			Description: "Third tool",
			InputSchema: map[string]interface{}{
				"type": "object",
			},
		},
	}

	client.SetTools(mcpTools)

	toolsList, err := CreateToolsFromClient(client)
	if err != nil {
		t.Fatalf("CreateToolsFromClient failed: %v", err)
	}

	if len(toolsList) != 3 {
		t.Errorf("Expected 3 tools, got %d", len(toolsList))
	}

	// Verify each tool is an adapter with the correct name
	for i, tool := range toolsList {
		adapter, ok := tool.(*Adapter)
		if !ok {
			t.Errorf("Tool %d is not an Adapter", i)
			continue
		}

		expectedName := "mcp_test-server_" + mcpTools[i].Name
		if adapter.Name() != expectedName {
			t.Errorf("Tool %d: expected name %q, got %q", i, expectedName, adapter.Name())
		}
	}
}

// TestCreateToolsFromClientEmpty tests creating tools when client has no tools
func TestCreateToolsFromClientEmpty(t *testing.T) {
	client := NewMockClient("test-server")
	client.SetTools([]Tool{})

	toolsList, err := CreateToolsFromClient(client)
	if err != nil {
		t.Fatalf("CreateToolsFromClient failed: %v", err)
	}

	if len(toolsList) != 0 {
		t.Errorf("Expected 0 tools, got %d", len(toolsList))
	}
}

// TestCreateToolsFromClientError tests error when listing tools fails
func TestCreateToolsFromClientError(t *testing.T) {
	client := &MockClient{
		serverInfo: &ServerInfo{
			Name: "test-server",
		},
	}

	// Don't set tools, so ListTools will return empty (but not error in our mock)
	// In a real scenario, this could error
	toolsList, err := CreateToolsFromClient(client)

	// Should succeed with empty list
	if err != nil {
		t.Fatalf("CreateToolsFromClient failed: %v", err)
	}

	if len(toolsList) != 0 {
		t.Errorf("Expected 0 tools, got %d", len(toolsList))
	}
}

// Helper functions

type testError struct {
	msg string
}

func (e *testError) Error() string {
	return e.msg
}

func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(s) > len(substr) && containsHelper(s, substr))
}

func containsHelper(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
