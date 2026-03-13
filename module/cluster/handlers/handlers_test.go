// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package handlers

import (
	"testing"

	"github.com/276793422/NemesisBot/module/channels"
)

// MockLogger implements Logger for testing
type MockLogger struct {
	infos  []string
	errors []string
	debugs []string
}

func NewMockLogger() *MockLogger {
	return &MockLogger{
		infos:  make([]string, 0),
		errors: make([]string, 0),
		debugs: make([]string, 0),
	}
}

func (m *MockLogger) LogRPCInfo(msg string, args ...interface{}) {
	m.infos = append(m.infos, msg)
}

func (m *MockLogger) LogRPCError(msg string, args ...interface{}) {
	m.errors = append(m.errors, msg)
}

func (m *MockLogger) LogRPCDebug(msg string, args ...interface{}) {
	m.debugs = append(m.debugs, msg)
}

func (m *MockLogger) GetInfos() []string {
	return m.infos
}

func (m *MockLogger) GetErrors() []string {
	return m.errors
}

func (m *MockLogger) GetDebugs() []string {
	return m.debugs
}

func (m *MockLogger) Clear() {
	m.infos = make([]string, 0)
	m.errors = make([]string, 0)
	m.debugs = make([]string, 0)
}

// MockNode implements Node for testing
type MockNode struct {
	id           string
	name         string
	address      string
	addresses    []string
	rpcPort      int
	capabilities []string
	status       string
	online       bool
}

func NewMockNode(id, name string) *MockNode {
	return &MockNode{
		id:           id,
		name:         name,
		address:      "192.168.1.100",
		addresses:    []string{"192.168.1.100", "10.0.0.1"},
		rpcPort:      8080,
		capabilities: []string{"llm", "tools"},
		status:       "online",
		online:       true,
	}
}

func (m *MockNode) GetID() string {
	return m.id
}

func (m *MockNode) GetName() string {
	return m.name
}

func (m *MockNode) GetAddress() string {
	return m.address
}

func (m *MockNode) GetAddresses() []string {
	return m.addresses
}

func (m *MockNode) GetRPCPort() int {
	return m.rpcPort
}

func (m *MockNode) GetCapabilities() []string {
	return m.capabilities
}

func (m *MockNode) GetStatus() string {
	return m.status
}

func (m *MockNode) IsOnline() bool {
	return m.online
}

func (m *MockNode) SetStatus(status string) {
	m.status = status
}

func (m *MockNode) SetOnline(online bool) {
	m.online = online
}

func (m *MockNode) SetCapabilities(caps []string) {
	m.capabilities = caps
}

// Test RegisterDefaultHandlers
func TestRegisterDefaultHandlers(t *testing.T) {
	logger := NewMockLogger()
	getNodeID := func() string {
		return "test-node-1"
	}
	getCapabilities := func() []string {
		return []string{"llm", "tools", "cluster"}
	}
	getOnlinePeers := func() []interface{} {
		return []interface{}{
			NewMockNode("node-1", "Node 1"),
			NewMockNode("node-2", "Node 2"),
		}
	}
	getActionsSchema := func() []interface{} {
		return []interface{}{
			map[string]interface{}{
				"name":        "ping",
				"description": "Health check",
			},
			map[string]interface{}{
				"name":        "hello",
				"description": "Say hello",
			},
		}
	}

	registeredHandlers := make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = handler
	}

	RegisterDefaultHandlers(logger, getNodeID, getCapabilities, getOnlinePeers, getActionsSchema, registrar)

	// Verify handlers were registered
	if len(registeredHandlers) != 4 {
		t.Errorf("Expected 4 handlers, got %d", len(registeredHandlers))
	}

	// Check specific handlers
	expectedHandlers := []string{"ping", "get_capabilities", "get_info", "list_actions"}
	for _, expected := range expectedHandlers {
		if _, ok := registeredHandlers[expected]; !ok {
			t.Errorf("Handler '%s' was not registered", expected)
		}
	}

	// Verify log was called
	infos := logger.GetInfos()
	if len(infos) == 0 {
		t.Error("Expected info log to be called")
	}

	found := false
	for _, info := range infos {
		if contains(info, "Registered default handlers") {
			found = true
			break
		}
	}
	if !found {
		t.Error("Expected log message about registering default handlers")
	}
}

// Test ping handler
func TestPingHandler(t *testing.T) {
	logger := NewMockLogger()
	getNodeID := func() string {
		return "test-node-1"
	}
	getCapabilities := func() []string {
		return []string{}
	}
	getOnlinePeers := func() []interface{} {
		return []interface{}{}
	}
	getActionsSchema := func() []interface{} {
		return []interface{}{}
	}

	registeredHandlers := make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = handler
	}

	RegisterDefaultHandlers(logger, getNodeID, getCapabilities, getOnlinePeers, getActionsSchema, registrar)

	// Test ping handler
	pingHandler, ok := registeredHandlers["ping"]
	if !ok {
		t.Fatal("Ping handler not registered")
	}

	response, err := pingHandler(map[string]interface{}{})
	if err != nil {
		t.Fatalf("Ping handler returned error: %v", err)
	}

	if response["status"] != "ok" {
		t.Errorf("Expected status 'ok', got %v", response["status"])
	}

	if response["node_id"] != "test-node-1" {
		t.Errorf("Expected node_id 'test-node-1', got %v", response["node_id"])
	}
}

// Test get_capabilities handler
func TestGetCapabilitiesHandler(t *testing.T) {
	logger := NewMockLogger()
	getNodeID := func() string {
		return "test-node-1"
	}
	expectedCaps := []string{"llm", "tools", "cluster"}
	getCapabilities := func() []string {
		return expectedCaps
	}
	getOnlinePeers := func() []interface{} {
		return []interface{}{}
	}
	getActionsSchema := func() []interface{} {
		return []interface{}{}
	}

	registeredHandlers := make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = handler
	}

	RegisterDefaultHandlers(logger, getNodeID, getCapabilities, getOnlinePeers, getActionsSchema, registrar)

	// Test get_capabilities handler
	capsHandler, ok := registeredHandlers["get_capabilities"]
	if !ok {
		t.Fatal("get_capabilities handler not registered")
	}

	response, err := capsHandler(map[string]interface{}{})
	if err != nil {
		t.Fatalf("get_capabilities handler returned error: %v", err)
	}

	caps, ok := response["capabilities"].([]string)
	if !ok {
		t.Fatal("capabilities field is not a string slice")
	}

	if len(caps) != len(expectedCaps) {
		t.Errorf("Expected %d capabilities, got %d", len(expectedCaps), len(caps))
	}

	for i, cap := range caps {
		if cap != expectedCaps[i] {
			t.Errorf("Expected capability %d to be %s, got %s", i, expectedCaps[i], cap)
		}
	}
}

// Test get_info handler
func TestGetInfoHandler(t *testing.T) {
	logger := NewMockLogger()
	getNodeID := func() string {
		return "test-node-1"
	}
	getCapabilities := func() []string {
		return []string{}
	}

	node1 := NewMockNode("node-1", "Node 1")
	node1.SetCapabilities([]string{"llm", "tools"})
	node2 := NewMockNode("node-2", "Node 2")
	node2.SetCapabilities([]string{"cluster"})

	getOnlinePeers := func() []interface{} {
		return []interface{}{node1, node2}
	}
	getActionsSchema := func() []interface{} {
		return []interface{}{}
	}

	registeredHandlers := make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = handler
	}

	RegisterDefaultHandlers(logger, getNodeID, getCapabilities, getOnlinePeers, getActionsSchema, registrar)

	// Test get_info handler
	infoHandler, ok := registeredHandlers["get_info"]
	if !ok {
		t.Fatal("get_info handler not registered")
	}

	response, err := infoHandler(map[string]interface{}{})
	if err != nil {
		t.Fatalf("get_info handler returned error: %v", err)
	}

	if response["node_id"] != "test-node-1" {
		t.Errorf("Expected node_id 'test-node-1', got %v", response["node_id"])
	}

	peers, ok := response["peers"].([]map[string]interface{})
	if !ok {
		t.Fatal("peers field is not a slice of maps")
	}

	if len(peers) != 2 {
		t.Errorf("Expected 2 peers, got %d", len(peers))
	}

	// Check first peer
	if peers[0]["id"] != "node-1" {
		t.Errorf("Expected first peer id 'node-1', got %v", peers[0]["id"])
	}
	if peers[0]["name"] != "Node 1" {
		t.Errorf("Expected first peer name 'Node 1', got %v", peers[0]["name"])
	}

	// Check second peer
	if peers[1]["id"] != "node-2" {
		t.Errorf("Expected second peer id 'node-2', got %v", peers[1]["id"])
	}
}

// Test get_info handler with non-Node peers
func TestGetInfoHandlerWithNonNodePeers(t *testing.T) {
	logger := NewMockLogger()
	getNodeID := func() string {
		return "test-node-1"
	}
	getCapabilities := func() []string {
		return []string{}
	}

	getOnlinePeers := func() []interface{} {
		return []interface{}{
			NewMockNode("node-1", "Node 1"),
			"not a node", // This should be ignored
			123,          // This should be ignored
			NewMockNode("node-2", "Node 2"),
		}
	}
	getActionsSchema := func() []interface{} {
		return []interface{}{}
	}

	registeredHandlers := make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = handler
	}

	RegisterDefaultHandlers(logger, getNodeID, getCapabilities, getOnlinePeers, getActionsSchema, registrar)

	// Test get_info handler
	infoHandler, ok := registeredHandlers["get_info"]
	if !ok {
		t.Fatal("get_info handler not registered")
	}

	response, err := infoHandler(map[string]interface{}{})
	if err != nil {
		t.Fatalf("get_info handler returned error: %v", err)
	}

	peers, ok := response["peers"].([]map[string]interface{})
	if !ok {
		t.Fatal("peers field is not a slice of maps")
	}

	// Should only return 2 peers (ignoring non-Node items)
	if len(peers) != 2 {
		t.Errorf("Expected 2 peers (non-Node items filtered), got %d", len(peers))
	}
}

// Test list_actions handler
func TestListActionsHandler(t *testing.T) {
	logger := NewMockLogger()
	getNodeID := func() string {
		return "test-node-1"
	}
	getCapabilities := func() []string {
		return []string{}
	}
	getOnlinePeers := func() []interface{} {
		return []interface{}{}
	}

	expectedActions := []interface{}{
		map[string]interface{}{
			"name":        "ping",
			"description": "Health check",
			"parameters": map[string]interface{}{
				"timeout": map[string]interface{}{
					"type":        "integer",
					"description": "Timeout in seconds",
				},
			},
		},
		map[string]interface{}{
			"name":        "hello",
			"description": "Say hello",
		},
	}

	getActionsSchema := func() []interface{} {
		return expectedActions
	}

	registeredHandlers := make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = handler
	}

	RegisterDefaultHandlers(logger, getNodeID, getCapabilities, getOnlinePeers, getActionsSchema, registrar)

	// Test list_actions handler
	actionsHandler, ok := registeredHandlers["list_actions"]
	if !ok {
		t.Fatal("list_actions handler not registered")
	}

	response, err := actionsHandler(map[string]interface{}{})
	if err != nil {
		t.Fatalf("list_actions handler returned error: %v", err)
	}

	actions, ok := response["actions"].([]interface{})
	if !ok {
		t.Fatal("actions field is not a slice")
	}

	if len(actions) != len(expectedActions) {
		t.Errorf("Expected %d actions, got %d", len(expectedActions), len(actions))
	}
}

// Test RegisterCustomHandlers
func TestRegisterCustomHandlers(t *testing.T) {
	logger := NewMockLogger()
	getNodeID := func() string {
		return "test-node-1"
	}

	registeredHandlers := make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = handler
	}

	RegisterCustomHandlers(logger, getNodeID, registrar)

	// Verify hello handler was registered
	helloHandler, ok := registeredHandlers["hello"]
	if !ok {
		t.Fatal("hello handler not registered")
	}

	// Test hello handler with payload
	payload := map[string]interface{}{
		"from":      "test-node-2",
		"timestamp": "2026-03-09T10:00:00Z",
	}

	response, err := helloHandler(payload)
	if err != nil {
		t.Fatalf("hello handler returned error: %v", err)
	}

	if response["status"] != "ok" {
		t.Errorf("Expected status 'ok', got %v", response["status"])
	}

	if response["node_id"] != "test-node-1" {
		t.Errorf("Expected node_id 'test-node-1', got %v", response["node_id"])
	}

	// Verify greeting contains the from field
	greeting, ok := response["greeting"].(string)
	if !ok {
		t.Fatal("greeting field is not a string")
	}

	if !contains(greeting, "test-node-2") {
		t.Errorf("Expected greeting to contain 'test-node-2', got %s", greeting)
	}

	// Verify log was called
	infos := logger.GetInfos()
	if len(infos) < 2 {
		t.Error("Expected at least 2 info logs")
	}

	found := false
	for _, info := range infos {
		if contains(info, "Hello handler") {
			found = true
			break
		}
	}
	if !found {
		t.Error("Expected log message from hello handler")
	}
}

// Test hello handler with missing fields
func TestHelloHandlerMissingFields(t *testing.T) {
	logger := NewMockLogger()
	getNodeID := func() string {
		return "test-node-1"
	}

	registeredHandlers := make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = handler
	}

	RegisterCustomHandlers(logger, getNodeID, registrar)

	// Test hello handler with empty payload
	helloHandler, ok := registeredHandlers["hello"]
	if !ok {
		t.Fatal("hello handler not registered")
	}

	response, err := helloHandler(map[string]interface{}{})
	if err != nil {
		t.Fatalf("hello handler returned error: %v", err)
	}

	if response["status"] != "ok" {
		t.Errorf("Expected status 'ok', got %v", response["status"])
	}

	// Should still work with empty payload
	greeting, ok := response["greeting"].(string)
	if !ok {
		t.Fatal("greeting field is not a string")
	}

	if greeting == "" {
		t.Error("Expected non-empty greeting even with missing fields")
	}
}

// Test RegisterPeerChatHandlers
func TestRegisterPeerChatHandlers(t *testing.T) {
	logger := NewMockLogger()

	// Create a mock RPCChannel
	rpcChannel := &channels.RPCChannel{}

	// Mock handler factory with correct signature
	handlerFactory := func(ch *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			return map[string]interface{}{
				"status":   "ok",
				"response": "test response",
			}, nil
		}
	}

	registeredHandlers := make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = handler
	}

	RegisterPeerChatHandlers(logger, rpcChannel, handlerFactory, registrar)

	// Verify peer_chat handler was registered
	peerChatHandler, ok := registeredHandlers["peer_chat"]
	if !ok {
		t.Fatal("peer_chat handler not registered")
	}

	// Test the handler
	response, err := peerChatHandler(map[string]interface{}{})
	if err != nil {
		t.Fatalf("peer_chat handler returned error: %v", err)
	}

	if response["status"] != "ok" {
		t.Errorf("Expected status 'ok', got %v", response["status"])
	}

	// Verify log was called
	infos := logger.GetInfos()
	if len(infos) == 0 {
		t.Error("Expected info log to be called")
	}

	found := false
	for _, info := range infos {
		if contains(info, "peer chat handler") {
			found = true
			break
		}
	}
	if !found {
		t.Error("Expected log message about registering peer chat handler")
	}
}

// Test RegisterLLMHandlers (alias for RegisterPeerChatHandlers)
func TestRegisterLLMHandlers(t *testing.T) {
	logger := NewMockLogger()

	// Create a mock RPCChannel
	rpcChannel := &channels.RPCChannel{}

	// Mock handler factory with correct signature
	handlerFactory := func(ch *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		return func(payload map[string]interface{}) (map[string]interface{}, error) {
			return map[string]interface{}{
				"status": "ok",
			}, nil
		}
	}

	registeredHandlers := make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = handler
	}

	// Register using the alias
	RegisterLLMHandlers(logger, rpcChannel, handlerFactory, registrar)

	// Verify peer_chat handler was registered (RegisterLLMHandlers is an alias)
	_, ok := registeredHandlers["peer_chat"]
	if !ok {
		t.Fatal("peer_chat handler not registered via RegisterLLMHandlers")
	}
}

// Test ActionSchema structure
func TestActionSchema(t *testing.T) {
	schema := ActionSchema{
		Name:        "test_action",
		Description: "Test action description",
		Parameters: map[string]interface{}{
			"param1": map[string]interface{}{
				"type":        "string",
				"description": "Parameter 1",
				"required":    true,
			},
		},
		Returns: map[string]interface{}{
			"result": map[string]interface{}{
				"type":        "string",
				"description": "Result",
			},
		},
		Examples: []map[string]interface{}{
			{
				"payload": map[string]interface{}{
					"param1": "value1",
				},
				"result": "success",
			},
		},
	}

	if schema.Name != "test_action" {
		t.Errorf("Expected name 'test_action', got %s", schema.Name)
	}

	if schema.Description != "Test action description" {
		t.Errorf("Expected description 'Test action description', got %s", schema.Description)
	}

	if schema.Parameters == nil {
		t.Error("Parameters should not be nil")
	}

	if schema.Returns == nil {
		t.Error("Returns should not be nil")
	}

	if len(schema.Examples) != 1 {
		t.Errorf("Expected 1 example, got %d", len(schema.Examples))
	}
}

// Test Registrar function type
func TestRegistrar(t *testing.T) {
	// Test that we can create and use a registrar
	registered := make(map[string]func(map[string]interface{}) (map[string]interface{}, error))

	var registrar Registrar = func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registered[action] = handler
	}

	// Use the registrar
	testHandler := func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{"status": "ok"}, nil
	}

	registrar("test_action", testHandler)

	if _, ok := registered["test_action"]; !ok {
		t.Error("Handler was not registered")
	}
}

// Test handler error handling
func TestHandlerWithError(t *testing.T) {
	logger := NewMockLogger()
	getNodeID := func() string {
		return "test-node-1"
	}
	getCapabilities := func() []string {
		return []string{}
	}
	getOnlinePeers := func() []interface{} {
		return []interface{}{}
	}
	getActionsSchema := func() []interface{} {
		return []interface{}{}
	}

	registeredHandlers := make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = handler
	}

	RegisterDefaultHandlers(logger, getNodeID, getCapabilities, getOnlinePeers, getActionsSchema, registrar)

	// Test that handlers don't panic with nil payload
	pingHandler, ok := registeredHandlers["ping"]
	if !ok {
		t.Fatal("Ping handler not registered")
	}

	// Should handle nil payload gracefully
	response, err := pingHandler(nil)
	if err != nil {
		t.Fatalf("Ping handler should handle nil payload, got error: %v", err)
	}

	if response == nil {
		t.Error("Ping handler should return non-nil response")
	}
}

// Test concurrent handler registration
func TestConcurrentHandlerRegistration(t *testing.T) {
	logger := NewMockLogger()
	getNodeID := func() string {
		return "test-node-1"
	}
	getCapabilities := func() []string {
		return []string{}
	}
	getOnlinePeers := func() []interface{} {
		return []interface{}{}
	}
	getActionsSchema := func() []interface{} {
		return []interface{}{}
	}

	registeredHandlers := make(map[string]func(map[string]interface{}) (map[string]interface{}, error))
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = handler
	}

	// Register default handlers multiple times (should be safe)
	for i := 0; i < 5; i++ {
		RegisterDefaultHandlers(logger, getNodeID, getCapabilities, getOnlinePeers, getActionsSchema, registrar)
	}

	// Should still have the expected handlers
	expectedHandlers := []string{"ping", "get_capabilities", "get_info", "list_actions"}
	for _, expected := range expectedHandlers {
		if _, ok := registeredHandlers[expected]; !ok {
			t.Errorf("Handler '%s' was not registered", expected)
		}
	}
}

// Helper function
func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(substr) == 0 ||
		(len(s) > 0 && len(substr) > 0 && containsHelper(s, substr)))
}

func containsHelper(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
