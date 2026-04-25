package mcp

import (
	"context"
	"encoding/json"
	"sync"
	"testing"

	"github.com/276793422/NemesisBot/module/mcp/transport"
)

// fullMockTransport implements the full transport.Transport interface for testing.
type fullMockTransport struct {
	mu         sync.Mutex
	responses  map[string]*transport.JSONRPCResponse
	closed     bool
	connected  bool
	sendCount  int
}

func newFullMockTransport() *fullMockTransport {
	return &fullMockTransport{
		responses: make(map[string]*transport.JSONRPCResponse),
	}
}

func (m *fullMockTransport) Connect(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.connected = true
	return nil
}

func (m *fullMockTransport) Send(ctx context.Context, req *transport.JSONRPCRequest) (*transport.JSONRPCResponse, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.sendCount++

	if m.closed {
		return nil, errTransport("transport is closed")
	}

	if resp, ok := m.responses[req.Method]; ok {
		return resp, nil
	}

	// Default success response
	return &transport.JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      req.ID,
		Result:  json.RawMessage(`{}`),
	}, nil
}

func (m *fullMockTransport) Close() error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.closed = true
	return nil
}

func (m *fullMockTransport) IsConnected() bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.connected && !m.closed
}

func (m *fullMockTransport) Name() string {
	return "mock"
}

func (m *fullMockTransport) setResponse(method string, result interface{}) {
	data, _ := json.Marshal(result)
	m.mu.Lock()
	defer m.mu.Unlock()
	m.responses[method] = &transport.JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      1,
		Result:  data,
	}
}

func (m *fullMockTransport) setError(method string, code int, message string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.responses[method] = &transport.JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      1,
		Error: &transport.RPCError{
			Code:    code,
			Message: message,
		},
	}
}

type errTransport string

func (e errTransport) Error() string { return string(e) }

// newClientWithFullMock creates a client with a full mock transport.
func newClientWithFullMock() (*client, *fullMockTransport) {
	mock := newFullMockTransport()
	return &client{
		config:    &ServerConfig{Name: "test-server", Command: "mock"},
		transport: mock,
		reqID:     0,
	}, mock
}

// --- Initialize tests ---

func TestClient_Initialize_FullMock(t *testing.T) {
	c, mock := newClientWithFullMock()

	mock.setResponse("initialize", map[string]interface{}{
		"protocolVersion": ProtocolVersion,
		"serverInfo":      map[string]interface{}{"name": "test-server", "version": "1.0.0"},
		"capabilities":    map[string]interface{}{},
	})

	result, err := c.Initialize(context.Background())
	if err != nil {
		t.Fatalf("Initialize failed: %v", err)
	}
	if result.ServerInfo.Name != "test-server" {
		t.Errorf("Expected server name 'test-server', got '%s'", result.ServerInfo.Name)
	}
}

func TestClient_Initialize_AlreadyInit(t *testing.T) {
	c, _ := newClientWithFullMock()
	c.initialized = true

	_, err := c.Initialize(context.Background())
	if err == nil {
		t.Error("Should error when already initialized")
	}
}

func TestClient_Initialize_TransportError(t *testing.T) {
	c, mock := newClientWithFullMock()
	mock.setError("initialize", -32603, "connection failed")

	_, err := c.Initialize(context.Background())
	if err == nil {
		t.Error("Should error when transport returns error")
	}
}

// --- ListTools tests ---

func TestClient_ListTools_FullMock(t *testing.T) {
	c, mock := newClientWithFullMock()
	c.initialized = true

	mock.setResponse("tools/list", map[string]interface{}{
		"tools": []interface{}{
			map[string]interface{}{"name": "read_file", "description": "Read a file"},
			map[string]interface{}{"name": "write_file", "description": "Write a file"},
		},
	})

	result, err := c.ListTools(context.Background())
	if err != nil {
		t.Fatalf("ListTools failed: %v", err)
	}
	if len(result) != 2 {
		t.Fatalf("Expected 2 tools, got %d", len(result))
	}
	if result[0].Name != "read_file" {
		t.Errorf("Expected tool 'read_file', got '%s'", result[0].Name)
	}
}

func TestClient_ListTools_ServerError(t *testing.T) {
	c, mock := newClientWithFullMock()
	c.initialized = true

	mock.setError("tools/list", -32603, "internal error")

	_, err := c.ListTools(context.Background())
	if err == nil {
		t.Error("Should error when server returns error")
	}
}

// --- CallTool tests ---

func TestClient_CallTool_FullMock(t *testing.T) {
	c, mock := newClientWithFullMock()
	c.initialized = true

	mock.setResponse("tools/call", map[string]interface{}{
		"content": []interface{}{
			map[string]interface{}{"type": "text", "text": "result data"},
		},
		"isError": false,
	})

	result, err := c.CallTool(context.Background(), "read_file", map[string]interface{}{"path": "/test.txt"})
	if err != nil {
		t.Fatalf("CallTool failed: %v", err)
	}
	if result.IsError {
		t.Error("Result should not be error")
	}
}

func TestClient_CallTool_ServerError(t *testing.T) {
	c, mock := newClientWithFullMock()
	c.initialized = true

	mock.setError("tools/call", -32603, "tool not found")

	_, err := c.CallTool(context.Background(), "nonexistent", nil)
	if err == nil {
		t.Error("Should error when tool not found")
	}
}

// --- ListResources tests ---

func TestClient_ListResources_FullMock(t *testing.T) {
	c, mock := newClientWithFullMock()
	c.initialized = true

	mock.setResponse("resources/list", map[string]interface{}{
		"resources": []interface{}{
			map[string]interface{}{"uri": "file:///test.txt", "name": "Test", "mimeType": "text/plain"},
		},
	})

	result, err := c.ListResources(context.Background())
	if err != nil {
		t.Fatalf("ListResources failed: %v", err)
	}
	if len(result) != 1 {
		t.Fatalf("Expected 1 resource, got %d", len(result))
	}
	if result[0].URI != "file:///test.txt" {
		t.Errorf("Expected URI 'file:///test.txt', got '%s'", result[0].URI)
	}
}

func TestClient_ListResources_ServerError(t *testing.T) {
	c, mock := newClientWithFullMock()
	c.initialized = true

	mock.setError("resources/list", -32603, "not supported")

	_, err := c.ListResources(context.Background())
	if err == nil {
		t.Error("Should error when server returns error")
	}
}

// --- ReadResource tests ---

func TestClient_ReadResource_FullMock(t *testing.T) {
	c, mock := newClientWithFullMock()
	c.initialized = true

	mock.setResponse("resources/read", map[string]interface{}{
		"contents": []interface{}{
			map[string]interface{}{"uri": "file:///test.txt", "mimeType": "text/plain", "text": "file content"},
		},
	})

	result, err := c.ReadResource(context.Background(), "file:///test.txt")
	if err != nil {
		t.Fatalf("ReadResource failed: %v", err)
	}
	_ = result
}

func TestClient_ReadResource_ServerError(t *testing.T) {
	c, mock := newClientWithFullMock()
	c.initialized = true

	mock.setError("resources/read", -32603, "resource not found")

	_, err := c.ReadResource(context.Background(), "file:///nonexistent")
	if err == nil {
		t.Error("Should error when resource not found")
	}
}

// --- ListPrompts tests ---

func TestClient_ListPrompts_FullMock(t *testing.T) {
	c, mock := newClientWithFullMock()
	c.initialized = true

	mock.setResponse("prompts/list", map[string]interface{}{
		"prompts": []interface{}{
			map[string]interface{}{
				"name":        "greeting",
				"description": "A greeting prompt",
				"arguments": []interface{}{
					map[string]interface{}{"name": "name", "description": "Person name", "required": true},
				},
			},
		},
	})

	result, err := c.ListPrompts(context.Background())
	if err != nil {
		t.Fatalf("ListPrompts failed: %v", err)
	}
	if len(result) != 1 {
		t.Fatalf("Expected 1 prompt, got %d", len(result))
	}
	if result[0].Name != "greeting" {
		t.Errorf("Expected prompt 'greeting', got '%s'", result[0].Name)
	}
}

func TestClient_ListPrompts_ServerError(t *testing.T) {
	c, mock := newClientWithFullMock()
	c.initialized = true

	mock.setError("prompts/list", -32603, "not supported")

	_, err := c.ListPrompts(context.Background())
	if err == nil {
		t.Error("Should error when server returns error")
	}
}

// --- GetPrompt tests ---

func TestClient_GetPrompt_FullMock(t *testing.T) {
	c, mock := newClientWithFullMock()
	c.initialized = true

	mock.setResponse("prompts/get", map[string]interface{}{
		"messages": []interface{}{
			map[string]interface{}{
				"role": "user",
				"content": map[string]interface{}{
					"type": "text",
					"text": "Hello",
				},
			},
		},
	})

	result, err := c.GetPrompt(context.Background(), "greeting", map[string]interface{}{"name": "World"})
	if err != nil {
		t.Fatalf("GetPrompt failed: %v", err)
	}
	if len(result.Messages) != 1 {
		t.Errorf("Expected 1 message, got %d", len(result.Messages))
	}
}

func TestClient_GetPrompt_ServerError(t *testing.T) {
	c, mock := newClientWithFullMock()
	c.initialized = true

	mock.setError("prompts/get", -32603, "prompt not found")

	_, err := c.GetPrompt(context.Background(), "nonexistent", nil)
	if err == nil {
		t.Error("Should error when prompt not found")
	}
}

// --- Double close test ---

func TestClient_DoubleClose_FullMock(t *testing.T) {
	c, _ := newClientWithFullMock()

	err := c.Close()
	if err != nil {
		t.Errorf("First Close failed: %v", err)
	}
	err = c.Close()
	if err != nil {
		t.Errorf("Second Close should not error: %v", err)
	}
}

// --- decodeResult tests ---

func TestDecodeResult_FullMock(t *testing.T) {
	resp := &JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      1,
		Result:  json.RawMessage(`{"protocolVersion": "2024-11-05"}`),
	}

	var result struct {
		ProtocolVersion string `json:"protocolVersion"`
	}
	err := decodeResult(resp, &result)
	if err != nil {
		t.Fatalf("decodeResult failed: %v", err)
	}
	if result.ProtocolVersion != "2024-11-05" {
		t.Errorf("Expected '2024-11-05', got '%s'", result.ProtocolVersion)
	}
}

func TestDecodeResult_ErrorResp(t *testing.T) {
	resp := &JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      1,
		Error: &JSONRPCError{
			Code:    -32603,
			Message: "Internal error",
		},
	}

	var result struct{}
	err := decodeResult(resp, &result)
	if err == nil {
		t.Error("Should error when response has error")
	}
}

func TestDecodeResult_NilResult2(t *testing.T) {
	resp := &JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      1,
	}

	var result struct{}
	err := decodeResult(resp, &result)
	_ = err
}

// --- convert helpers ---

func TestConvertToTransportReq(t *testing.T) {
	req := &JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      int64(1),
		Method:  "test",
		Params:  map[string]interface{}{"key": "value"},
	}

	transportReq, err := convertToTransportRequest(req)
	if err != nil {
		t.Fatalf("convertToTransportRequest failed: %v", err)
	}
	if transportReq.Method != "test" {
		t.Errorf("Expected method 'test', got '%s'", transportReq.Method)
	}
}

func TestConvertFromTransportResp(t *testing.T) {
	resp := &transport.JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      1,
		Result:  json.RawMessage(`{"test": true}`),
	}

	mcpResp := convertFromTransportResponse(resp)
	if mcpResp.JSONRPC != "2.0" {
		t.Errorf("Expected JSONRPC '2.0', got '%s'", mcpResp.JSONRPC)
	}
}

func TestConvertFromTransportResp_WithError(t *testing.T) {
	resp := &transport.JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      1,
		Error: &transport.RPCError{
			Code:    -32603,
			Message: "test error",
		},
	}

	mcpResp := convertFromTransportResponse(resp)
	if mcpResp.Error == nil {
		t.Fatal("Error should not be nil")
	}
	if mcpResp.Error.Code != -32603 {
		t.Errorf("Expected error code -32603, got %d", mcpResp.Error.Code)
	}
}
