// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package handlers

import (
	"fmt"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/channels"
)

// --- Mock Logger ---

type mockLogger struct {
	infos  []string
	errors []string
	debugs []string
	mu     sync.Mutex
}

func (m *mockLogger) LogRPCInfo(msg string, args ...interface{}) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.infos = append(m.infos, fmt.Sprintf(msg, args...))
}

func (m *mockLogger) LogRPCError(msg string, args ...interface{}) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.errors = append(m.errors, fmt.Sprintf(msg, args...))
}

func (m *mockLogger) LogRPCDebug(msg string, args ...interface{}) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.debugs = append(m.debugs, fmt.Sprintf(msg, args...))
}

func (m *mockLogger) getInfos() []string {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.infos
}

func (m *mockLogger) getErrors() []string {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.errors
}

// --- Mock TaskCompleter ---

type mockTaskCompleter struct {
	completions []taskCompletion
	err         error
	mu          sync.Mutex
}

type taskCompletion struct {
	taskID   string
	status   string
	response string
	errMsg   string
}

func (m *mockTaskCompleter) CompleteCallback(taskID, status, response, errMsg string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.err != nil {
		return m.err
	}
	m.completions = append(m.completions, taskCompletion{
		taskID:   taskID,
		status:   status,
		response: response,
		errMsg:   errMsg,
	})
	return nil
}

func (m *mockTaskCompleter) getCompletions() []taskCompletion {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.completions
}

// --- Mock ForgeDataProvider ---

type mockForgeDataProvider struct {
	receivedPayload map[string]interface{}
	reflectionsList map[string]interface{}
	content         string
	readErr         error
	receiveErr      error
}

func (m *mockForgeDataProvider) ReceiveReflection(payload map[string]interface{}) error {
	if m.receiveErr != nil {
		return m.receiveErr
	}
	m.receivedPayload = payload
	return nil
}

func (m *mockForgeDataProvider) GetReflectionsListPayload() map[string]interface{} {
	if m.reflectionsList == nil {
		return map[string]interface{}{
			"reflections": []string{"report1.md", "report2.md"},
			"count":       2,
		}
	}
	return m.reflectionsList
}

func (m *mockForgeDataProvider) ReadReflectionContent(filename string) (string, error) {
	if m.readErr != nil {
		return "", m.readErr
	}
	return m.content, nil
}

func (m *mockForgeDataProvider) SanitizeContent(content string) string {
	return "[SANITIZED] " + content
}

// --- Callback Handler tests ---

func TestCallbackHandler_MissingTaskID(t *testing.T) {
	logger := &mockLogger{}
	tm := &mockTaskCompleter{}

	var captured map[string]interface{}
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "peer_chat_callback" {
			result, _ := handler(map[string]interface{}{})
			captured = result
		}
	}

	RegisterCallbackHandler(logger, tm, registrar)

	if captured == nil {
		t.Fatal("Handler was not registered")
	}
	if captured["status"] != "error" {
		t.Errorf("status = %v, want error", captured["status"])
	}
	if captured["error"] != "task_id is required" {
		t.Errorf("error = %v, want 'task_id is required'", captured["error"])
	}
}

func TestCallbackHandler_MissingStatus(t *testing.T) {
	logger := &mockLogger{}
	tm := &mockTaskCompleter{}

	var captured map[string]interface{}
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "peer_chat_callback" {
			result, _ := handler(map[string]interface{}{
				"task_id": "task-1",
			})
			captured = result
		}
	}

	RegisterCallbackHandler(logger, tm, registrar)

	if captured["status"] != "received" {
		t.Errorf("status = %v, want received", captured["status"])
	}

	completions := tm.getCompletions()
	if len(completions) != 1 {
		t.Fatalf("completions = %d, want 1", len(completions))
	}
	// Missing status should default to "error"
	if completions[0].status != "error" {
		t.Errorf("status = %q, want %q", completions[0].status, "error")
	}
}

func TestCallbackHandler_Success(t *testing.T) {
	logger := &mockLogger{}
	tm := &mockTaskCompleter{}

	var captured map[string]interface{}
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "peer_chat_callback" {
			result, _ := handler(map[string]interface{}{
				"task_id":  "task-success",
				"status":   "success",
				"response": "Hello World",
			})
			captured = result
		}
	}

	RegisterCallbackHandler(logger, tm, registrar)

	if captured["status"] != "received" {
		t.Errorf("status = %v, want received", captured["status"])
	}
	if captured["task_id"] != "task-success" {
		t.Errorf("task_id = %v, want task-success", captured["task_id"])
	}

	completions := tm.getCompletions()
	if len(completions) != 1 {
		t.Fatalf("completions = %d, want 1", len(completions))
	}
	if completions[0].taskID != "task-success" {
		t.Errorf("taskID = %q, want %q", completions[0].taskID, "task-success")
	}
	if completions[0].status != "success" {
		t.Errorf("status = %q, want %q", completions[0].status, "success")
	}
	if completions[0].response != "Hello World" {
		t.Errorf("response = %q, want %q", completions[0].response, "Hello World")
	}
}

func TestCallbackHandler_WithError(t *testing.T) {
	logger := &mockLogger{}
	tm := &mockTaskCompleter{}

	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "peer_chat_callback" {
			handler(map[string]interface{}{
				"task_id": "task-err",
				"status":  "error",
				"error":   "something failed",
			})
		}
	}

	RegisterCallbackHandler(logger, tm, registrar)

	completions := tm.getCompletions()
	if len(completions) != 1 {
		t.Fatalf("completions = %d, want 1", len(completions))
	}
	if completions[0].errMsg != "something failed" {
		t.Errorf("errMsg = %q, want %q", completions[0].errMsg, "something failed")
	}
}

func TestCallbackHandler_TaskCompleterError(t *testing.T) {
	logger := &mockLogger{}
	tm := &mockTaskCompleter{err: fmt.Errorf("task not found: task-missing")}

	var captured map[string]interface{}
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "peer_chat_callback" {
			result, _ := handler(map[string]interface{}{
				"task_id": "task-missing",
				"status":  "success",
			})
			captured = result
		}
	}

	RegisterCallbackHandler(logger, tm, registrar)

	if captured["status"] != "error" {
		t.Errorf("status = %v, want error", captured["status"])
	}
	if captured["task_id"] != "task-missing" {
		t.Errorf("task_id = %v, want task-missing", captured["task_id"])
	}
}

// --- Custom Handler tests ---

func TestCustomHandler_HelloWithAllFields(t *testing.T) {
	logger := &mockLogger{}

	var captured map[string]interface{}
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "hello" {
			result, _ := handler(map[string]interface{}{
				"from":      "node-123",
				"timestamp": "2026-01-01T00:00:00Z",
			})
			captured = result
		}
	}

	RegisterCustomHandlers(logger, func() string { return "local-node" }, registrar)

	if captured == nil {
		t.Fatal("Handler was not registered")
	}
	if captured["status"] != "ok" {
		t.Errorf("status = %v, want ok", captured["status"])
	}
	if captured["node_id"] != "local-node" {
		t.Errorf("node_id = %v, want local-node", captured["node_id"])
	}
	greeting, ok := captured["greeting"].(string)
	if !ok {
		t.Fatal("greeting is not a string")
	}
	if greeting == "" {
		t.Error("greeting should not be empty")
	}
}

func TestCustomHandler_HelloMissingFields(t *testing.T) {
	logger := &mockLogger{}

	var captured map[string]interface{}
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "hello" {
			result, _ := handler(map[string]interface{}{})
			captured = result
		}
	}

	RegisterCustomHandlers(logger, func() string { return "node-1" }, registrar)

	if captured == nil {
		t.Fatal("Handler was not registered")
	}
	if captured["status"] != "ok" {
		t.Errorf("status = %v, want ok", captured["status"])
	}
}

// --- Default Handler edge case tests ---

func TestGetInfoHandler_EmptyPeers(t *testing.T) {
	logger := &mockLogger{}

	var handlersMap = make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		handlersMap[action] = handler
	}

	RegisterDefaultHandlers(
		logger,
		func() string { return "node-1" },
		func() []string { return []string{} },
		func() []interface{} { return []interface{}{} },
		func() []interface{} { return []interface{}{} },
		registrar,
	)

	handler, ok := handlersMap["get_info"]
	if !ok {
		t.Fatal("get_info handler not registered")
	}

	result, err := handler(map[string]interface{}{})
	if err != nil {
		t.Fatalf("Handler error = %v", err)
	}
	if result["node_id"] != "node-1" {
		t.Errorf("node_id = %v, want node-1", result["node_id"])
	}
	peers, ok := result["peers"].([]map[string]interface{})
	if !ok {
		t.Fatal("peers is not the right type")
	}
	if len(peers) != 0 {
		t.Errorf("peers count = %d, want 0", len(peers))
	}
}

func TestGetInfoHandler_NodeInterfacePeers(t *testing.T) {
	logger := &mockLogger{}

	// Mock node that implements Node interface
	mockNode := &testNode{
		id:           "peer-1",
		name:         "Test Peer",
		address:      "1.2.3.4:12345",
		addresses:    []string{"1.2.3.4"},
		rpcPort:      12345,
		capabilities: []string{"llm"},
		status:       "online",
		online:       true,
	}

	var handlersMap = make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		handlersMap[action] = handler
	}

	RegisterDefaultHandlers(
		logger,
		func() string { return "node-1" },
		func() []string { return []string{} },
		func() []interface{} { return []interface{}{mockNode} },
		func() []interface{} { return []interface{}{} },
		registrar,
	)

	handler := handlersMap["get_info"]
	result, _ := handler(map[string]interface{}{})

	peers := result["peers"].([]map[string]interface{})
	if len(peers) != 1 {
		t.Fatalf("peers count = %d, want 1", len(peers))
	}
	if peers[0]["id"] != "peer-1" {
		t.Errorf("peer id = %v, want peer-1", peers[0]["id"])
	}
	if peers[0]["name"] != "Test Peer" {
		t.Errorf("peer name = %v, want Test Peer", peers[0]["name"])
	}
}

func TestGetInfoHandler_NonNodePeers(t *testing.T) {
	logger := &mockLogger{}

	var handlersMap = make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		handlersMap[action] = handler
	}

	// Pass a non-Node interface
	RegisterDefaultHandlers(
		logger,
		func() string { return "node-1" },
		func() []string { return []string{} },
		func() []interface{} { return []interface{}{"just-a-string", 42} },
		func() []interface{} { return []interface{}{} },
		registrar,
	)

	handler := handlersMap["get_info"]
	result, _ := handler(map[string]interface{}{})

	peers := result["peers"].([]map[string]interface{})
	if len(peers) != 0 {
		t.Errorf("Non-Node peers should be filtered out, got %d peers", len(peers))
	}
}

// testNode implements the Node interface for testing
type testNode struct {
	id           string
	name         string
	address      string
	addresses    []string
	rpcPort      int
	capabilities []string
	status       string
	online       bool
}

func (n *testNode) GetID() string          { return n.id }
func (n *testNode) GetName() string        { return n.name }
func (n *testNode) GetAddress() string     { return n.address }
func (n *testNode) GetAddresses() []string { return n.addresses }
func (n *testNode) GetRPCPort() int        { return n.rpcPort }
func (n *testNode) GetCapabilities() []string { return n.capabilities }
func (n *testNode) GetStatus() string      { return n.status }
func (n *testNode) IsOnline() bool         { return n.online }

// --- Forge Handler tests ---

func TestRegisterForgeHandlers_ForgeShare(t *testing.T) {
	logger := &mockLogger{}
	provider := &mockForgeDataProvider{}

	var handlersMap = make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		handlersMap[action] = handler
	}

	RegisterForgeHandlers(logger, provider, func() string { return "node-forge" }, registrar)

	// Test forge_share handler
	handler, ok := handlersMap["forge_share"]
	if !ok {
		t.Fatal("forge_share handler not registered")
	}

	result, err := handler(map[string]interface{}{
		"from":    "remote-node",
		"report":  "test-report",
		"content": "reflections data",
	})
	if err != nil {
		t.Fatalf("forge_share error = %v", err)
	}
	if result["status"] != "ok" {
		t.Errorf("status = %v, want ok", result["status"])
	}
	if result["node_id"] != "node-forge" {
		t.Errorf("node_id = %v, want node-forge", result["node_id"])
	}
	// Check timestamp is valid
	ts, ok := result["timestamp"].(string)
	if !ok || ts == "" {
		t.Error("timestamp should be a non-empty string")
	}
	_, parseErr := time.Parse(time.RFC3339, ts)
	if parseErr != nil {
		t.Errorf("timestamp format invalid: %v", parseErr)
	}
}

func TestRegisterForgeHandlers_ForgeShare_Error(t *testing.T) {
	logger := &mockLogger{}
	provider := &mockForgeDataProvider{
		receiveErr: fmt.Errorf("disk full"),
	}

	var handlersMap = make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		handlersMap[action] = handler
	}

	RegisterForgeHandlers(logger, provider, func() string { return "node-forge" }, registrar)

	handler := handlersMap["forge_share"]
	result, err := handler(map[string]interface{}{
		"from": "remote-node",
	})

	if err == nil {
		t.Fatal("Expected error when provider fails")
	}
	if result["status"] != "error" {
		t.Errorf("status = %v, want error", result["status"])
	}
}

func TestRegisterForgeHandlers_ForgeShare_NoFrom(t *testing.T) {
	logger := &mockLogger{}
	provider := &mockForgeDataProvider{}

	var handlersMap = make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		handlersMap[action] = handler
	}

	RegisterForgeHandlers(logger, provider, func() string { return "node-forge" }, registrar)

	handler := handlersMap["forge_share"]
	result, _ := handler(map[string]interface{}{})
	if result["status"] != "ok" {
		t.Errorf("Missing 'from' should still succeed, got status = %v", result["status"])
	}
}

func TestRegisterForgeHandlers_GetReflections(t *testing.T) {
	logger := &mockLogger{}
	provider := &mockForgeDataProvider{}

	var handlersMap = make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		handlersMap[action] = handler
	}

	RegisterForgeHandlers(logger, provider, func() string { return "node-forge" }, registrar)

	// Test forge_get_reflections handler without filename
	handler, ok := handlersMap["forge_get_reflections"]
	if !ok {
		t.Fatal("forge_get_reflections handler not registered")
	}

	result, err := handler(map[string]interface{}{
		"from": "remote-node",
	})
	if err != nil {
		t.Fatalf("forge_get_reflections error = %v", err)
	}
	if result["node_id"] != "node-forge" {
		t.Errorf("node_id = %v, want node-forge", result["node_id"])
	}
	// Should contain reflections list
	if _, ok := result["reflections"]; !ok {
		t.Error("Result should contain reflections")
	}
}

func TestRegisterForgeHandlers_GetReflectionsWithFilename(t *testing.T) {
	logger := &mockLogger{}
	provider := &mockForgeDataProvider{
		content: "reflection content here",
	}

	var handlersMap = make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		handlersMap[action] = handler
	}

	RegisterForgeHandlers(logger, provider, func() string { return "node-forge" }, registrar)

	handler := handlersMap["forge_get_reflections"]
	result, err := handler(map[string]interface{}{
		"from":     "remote-node",
		"filename": "report1.md",
	})
	if err != nil {
		t.Fatalf("forge_get_reflections error = %v", err)
	}

	if result["filename"] != "report1.md" {
		t.Errorf("filename = %v, want report1.md", result["filename"])
	}
	content, ok := result["content"].(string)
	if !ok {
		t.Fatal("content should be a string")
	}
	if content != "[SANITIZED] reflection content here" {
		t.Errorf("content = %q, want sanitized content", content)
	}
}

func TestRegisterForgeHandlers_GetReflectionsReadError(t *testing.T) {
	logger := &mockLogger{}
	provider := &mockForgeDataProvider{
		readErr: fmt.Errorf("file not found"),
	}

	var handlersMap = make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		handlersMap[action] = handler
	}

	RegisterForgeHandlers(logger, provider, func() string { return "node-forge" }, registrar)

	handler := handlersMap["forge_get_reflections"]
	result, _ := handler(map[string]interface{}{
		"filename": "missing.md",
	})

	if result["status"] != "error" {
		t.Errorf("status = %v, want error", result["status"])
	}
}

func TestRegisterForgeHandlers_GetReflectionsNoFrom(t *testing.T) {
	logger := &mockLogger{}
	provider := &mockForgeDataProvider{}

	var handlersMap = make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		handlersMap[action] = handler
	}

	RegisterForgeHandlers(logger, provider, func() string { return "node-forge" }, registrar)

	handler := handlersMap["forge_get_reflections"]
	result, _ := handler(map[string]interface{}{})
	if result["node_id"] != "node-forge" {
		t.Errorf("node_id = %v, want node-forge", result["node_id"])
	}
}

// --- LLM Handler test ---

func TestRegisterLLMHandlers_RegistersPeerChat(t *testing.T) {
	logger := &mockLogger{}

	var registeredAction string
	var registeredHandler func(map[string]interface{}) (map[string]interface{}, error)

	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredAction = action
		registeredHandler = handler
	}

	factoryCalled := false
	handlerFactory := func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		factoryCalled = true
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			return map[string]interface{}{"status": "test"}, nil
		}
	}

	RegisterLLMHandlers(logger, nil, handlerFactory, registrar)

	if registeredAction != "peer_chat" {
		t.Errorf("registered action = %q, want %q", registeredAction, "peer_chat")
	}
	if !factoryCalled {
		t.Error("Handler factory was not called")
	}
	if registeredHandler == nil {
		t.Fatal("Handler should not be nil")
	}

	// Test the handler works
	result, err := registeredHandler(map[string]interface{}{})
	if err != nil {
		t.Fatalf("Handler error = %v", err)
	}
	if result["status"] != "test" {
		t.Errorf("status = %v, want test", result["status"])
	}
}

// --- Logger interface test ---

func TestMockLogger_ImplementsLogger(t *testing.T) {
	// Verify mockLogger implements Logger interface
	var _ Logger = &mockLogger{}
}

// --- ActionSchema test ---

func TestActionSchema_Fields(t *testing.T) {
	schema := ActionSchema{
		Name:        "test_action",
		Description: "A test action",
		Parameters: map[string]interface{}{
			"param1": "string",
		},
		Returns: map[string]interface{}{
			"result": "string",
		},
		Examples: []map[string]interface{}{
			{"input": "test", "output": "result"},
		},
	}

	if schema.Name != "test_action" {
		t.Errorf("Name = %q, want %q", schema.Name, "test_action")
	}
	if len(schema.Parameters) != 1 {
		t.Errorf("Parameters len = %d, want 1", len(schema.Parameters))
	}
	if len(schema.Returns) != 1 {
		t.Errorf("Returns len = %d, want 1", len(schema.Returns))
	}
	if len(schema.Examples) != 1 {
		t.Errorf("Examples len = %d, want 1", len(schema.Examples))
	}
}

// --- Peer Chat Handler test ---

func TestRegisterPeerChatHandlers_WithNilRPCChannel(t *testing.T) {
	logger := &mockLogger{}

	var registeredAction string
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredAction = action
	}

	handlerFactory := func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			return nil, nil
		}
	}

	RegisterPeerChatHandlers(logger, nil, handlerFactory, registrar)

	if registeredAction != "peer_chat" {
		t.Errorf("registered action = %q, want %q", registeredAction, "peer_chat")
	}

	// Verify logging occurred
	infos := logger.getInfos()
	if len(infos) == 0 {
		t.Error("Expected info log after registration")
	}
}
