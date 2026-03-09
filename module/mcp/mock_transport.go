// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package mcp

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"
)

// MockTransport is a mock implementation of transport.Transport for testing
type MockTransport struct {
	mu              sync.Mutex
	responses       map[string][]byte // Pre-programmed responses for methods
	connectCalled   bool
	closed          bool
	sendCount       int
	receivedRequests [][]byte
}

// NewMockTransport creates a new mock transport
func NewMockTransport() *MockTransport {
	return &MockTransport{
		responses:        make(map[string][]byte),
		receivedRequests: make([][]byte, 0),
	}
}

// Connect simulates connecting to the MCP server
func (m *MockTransport) Connect(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.connectCalled = true
	return nil
}

// Send simulates sending a request and receiving a response
func (m *MockTransport) Send(ctx context.Context, data []byte) ([]byte, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.closed {
		return nil, fmt.Errorf("transport is closed")
	}

	// Store the request for inspection
	m.receivedRequests = append(m.receivedRequests, data)
	m.sendCount++

	// Parse the request to get the method
	var req map[string]interface{}
	if err := json.Unmarshal(data, &req); err != nil {
		return nil, fmt.Errorf("invalid request: %w", err)
	}

	method, _ := req["method"].(string)

	// Return pre-programmed response or default success response
	if resp, ok := m.responses[method]; ok {
		return resp, nil
	}

	// Default success response
	defaultResp := map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      req["id"],
		"result": map[string]interface{}{},
	}

	return json.Marshal(defaultResp)
}

// Close simulates closing the transport connection
func (m *MockTransport) Close() error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.closed = true
	return nil
}

// SetResponse sets a mock response for a specific method
func (m *MockTransport) SetResponse(method string, resp interface{}) error {
	data, err := json.Marshal(resp)
	if err != nil {
		return err
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	m.responses[method] = data
	return nil
}

// SetResponseBytes sets raw response bytes for a method
func (m *MockTransport) SetResponseBytes(method string, data []byte) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.responses[method] = data
}

// WasConnected returns true if Connect was called
func (m *MockTransport) WasConnected() bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.connectCalled
}

// IsClosed returns true if the transport is closed
func (m *MockTransport) IsClosed() bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.closed
}

// GetSendCount returns the number of Send calls made
func (m *MockTransport) GetSendCount() int {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.sendCount
}

// GetRequests returns all received requests
func (m *MockTransport) GetRequests() [][]byte {
	m.mu.Lock()
	defer m.mu.Unlock()
	result := make([][]byte, len(m.receivedRequests))
	copy(result, m.receivedRequests)
	return result
}

// ClearRequests clears the stored requests
func (m *MockTransport) ClearRequests() {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.receivedRequests = make([][]byte, 0)
}

// CreateMockInitializeResponse creates a mock initialize response
func CreateMockInitializeResponse(serverName, serverVersion string) map[string]interface{} {
	return map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      1,
		"result": map[string]interface{}{
			"protocolVersion": "2024-11-05",
			"serverInfo": map[string]interface{}{
				"name":    serverName,
				"version": serverVersion,
			},
			"capabilities": map[string]interface{}{
				"tools": map[string]bool{
					"listChanged": true,
				},
			},
		},
	}
}

// CreateMockToolsListResponse creates a mock tools/list response
func CreateMockToolsListResponse(tools []Tool) map[string]interface{} {
	toolMaps := make([]map[string]interface{}, len(tools))
	for i, tool := range tools {
		toolMaps[i] = map[string]interface{}{
			"name":        tool.Name,
			"description": tool.Description,
			"inputSchema": tool.InputSchema,
		}
	}

	return map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      2,
		"result": map[string]interface{}{
			"tools": toolMaps,
		},
	}
}

// CreateMockToolCallResponse creates a mock tool call response
func CreateMockToolCallResponse(content interface{}) map[string]interface{} {
	return map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      3,
		"result": map[string]interface{}{
			"content": []map[string]interface{}{
				{
					"type": "text",
					"text": fmt.Sprintf("%v", content),
				},
			},
			"isError": false,
		},
	}
}

// CreateMockResourcesListResponse creates a mock resources/list response
func CreateMockResourcesListResponse(resources []Resource) map[string]interface{} {
	resourceMaps := make([]map[string]interface{}, len(resources))
	for i, res := range resources {
		resourceMaps[i] = map[string]interface{}{
			"uri":         res.URI,
			"name":        res.Name,
			"description": res.Description,
			"mimeType":    res.MimeType,
		}
	}

	return map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      4,
		"result": map[string]interface{}{
			"resources": resourceMaps,
		},
	}
}

// CreateMockPromptsListResponse creates a mock prompts/list response
func CreateMockPromptsListResponse(prompts []Prompt) map[string]interface{} {
	promptMaps := make([]map[string]interface{}, len(prompts))
	for i, prompt := range prompts {
		argMaps := make([]map[string]interface{}, len(prompt.Arguments))
		for j, arg := range prompt.Arguments {
			argMaps[j] = map[string]interface{}{
				"name":        arg.Name,
				"description": arg.Description,
				"required":    arg.Required,
			}
		}

		promptMaps[i] = map[string]interface{}{
			"name":        prompt.Name,
			"description": prompt.Description,
			"arguments":   argMaps,
		}
	}

	return map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      5,
		"result": map[string]interface{}{
			"prompts": promptMaps,
		},
	}
}

// CreateMockErrorResponse creates a mock error response
func CreateMockErrorResponse(id int, message string) map[string]interface{} {
	return map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      id,
		"error": map[string]interface{}{
			"code":    -32603,
			"message": message,
		},
	}
}

// CreateMockReadResourceResponse creates a mock read resource response
func CreateMockReadResourceResponse(contents []byte) map[string]interface{} {
	return map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      6,
		"result": map[string]interface{}{
			"contents": []map[string]interface{}{
				{
					"uri":      "file:///test.txt",
					"mimeType": "text/plain",
					"text":     string(contents),
				},
			},
		},
	}
}

// CreateMockGetPromptResponse creates a mock get prompt response
func CreateMockGetPromptResponse(messages []map[string]interface{}) map[string]interface{} {
	return map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      7,
		"result": map[string]interface{}{
			"messages": messages,
		},
	}
}
