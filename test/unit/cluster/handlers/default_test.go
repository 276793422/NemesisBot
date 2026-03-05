// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package handlers_test

import (
	"sort"
	"testing"

	"github.com/276793422/NemesisBot/module/cluster/handlers"
)

// mockClusterForHandlers is a mock implementation for testing
type mockClusterForHandlers struct {
	nodeID        string
	address       string
	capabilities  []string
	peers         []interface{}
	actionsSchema []handlers.ActionSchema
	logMessages   []string
}

func (m *mockClusterForHandlers) GetNodeID() string {
	return m.nodeID
}

func (m *mockClusterForHandlers) GetCapabilities() []string {
	return m.capabilities
}

func (m *mockClusterForHandlers) GetOnlinePeers() []interface{} {
	return m.peers
}

func (m *mockClusterForHandlers) LogRPCInfo(msg string, args ...interface{}) {
	m.logMessages = append(m.logMessages, "INFO: "+msg)
}

func (m *mockClusterForHandlers) LogRPCError(msg string, args ...interface{}) {
	m.logMessages = append(m.logMessages, "ERROR: "+msg)
}

func (m *mockClusterForHandlers) LogRPCDebug(msg string, args ...interface{}) {
	m.logMessages = append(m.logMessages, "DEBUG: "+msg)
}

func (m *mockClusterForHandlers) GetActionsSchema() []interface{} {
	// Convert []handlers.ActionSchema to []map[string]interface{}
	result := make([]interface{}, len(m.actionsSchema))
	for i, schema := range m.actionsSchema {
		// Convert struct to map
		result[i] = map[string]interface{}{
			"name":        schema.Name,
			"description": schema.Description,
			"parameters":  schema.Parameters,
			"returns":     schema.Returns,
			"examples":    schema.Examples,
		}
	}
	return result
}

// mockNode is a mock implementation of rpc.Node for testing
type mockNode struct {
	id           string
	name         string
	address      string
	addresses    []string
	rpcPort      int
	capabilities []string
	status       string
	online       bool
}

func (m *mockNode) GetID() string                { return m.id }
func (m *mockNode) GetName() string              { return m.name }
func (m *mockNode) GetAddress() string           { return m.address }
func (m *mockNode) GetAddresses() []string       { return m.addresses }
func (m *mockNode) GetRPCPort() int              { return m.rpcPort }
func (m *mockNode) GetCapabilities() []string    { return m.capabilities }
func (m *mockNode) GetStatus() string            { return m.status }
func (m *mockNode) IsOnline() bool               { return m.online }

// TestRegisterDefaultHandlers tests that all default handlers are registered
func TestRegisterDefaultHandlers(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:      "test-node-1",
		address:     "127.0.0.1:21950",
		capabilities: []string{"llm_forward", "hello"},
		peers: []interface{}{
			&mockNode{
				id:           "peer-1",
				name:         "Test Peer 1",
				address:      "192.168.1.100:21950",
				addresses:    []string{"192.168.1.100:21950"},
				rpcPort:      21950,
				capabilities: []string{"llm_forward"},
				status:       "online",
				online:       true,
			},
			&mockNode{
				id:           "peer-2",
				name:         "Test Peer 2",
				address:      "192.168.1.101:21950",
				addresses:    []string{"192.168.1.101:21950"},
				rpcPort:      21950,
				capabilities: []string{"hello"},
				status:       "online",
				online:       true,
			},
		},
		logMessages: []string{},
	}

	registeredHandlers := make(map[string]bool)

	// Create a registrar that tracks registered handlers
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		registeredHandlers[action] = true
	}

	// Register default handlers
	handlers.RegisterDefaultHandlers(mockCluster, mockCluster.GetNodeID, mockCluster.GetCapabilities, mockCluster.GetOnlinePeers, mockCluster.GetActionsSchema, registrar)

	// Verify all expected handlers are registered
	expectedHandlers := []string{"ping", "get_capabilities", "get_info", "list_actions"}
	for _, handlerName := range expectedHandlers {
		if !registeredHandlers[handlerName] {
			t.Errorf("Handler '%s' was not registered", handlerName)
		}
	}

	// Verify log message was written
	if len(mockCluster.logMessages) == 0 {
		t.Error("Expected log message to be written")
	}
}

// TestPingHandler tests the ping handler response
func TestPingHandler(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID: "test-node-1",
	}

	var pingHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the ping handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "ping" {
			pingHandler = handler
		}
	}

	handlers.RegisterDefaultHandlers(mockCluster, mockCluster.GetNodeID, mockCluster.GetCapabilities, mockCluster.GetOnlinePeers, mockCluster.GetActionsSchema, registrar)

	if pingHandler == nil {
		t.Fatal("Ping handler was not registered")
	}

	// Test ping handler
	response, err := pingHandler(map[string]interface{}{})
	if err != nil {
		t.Errorf("Ping handler returned error: %v", err)
	}

	if response["status"] != "ok" {
		t.Errorf("Expected status 'ok', got '%s'", response["status"])
	}

	if response["node_id"] != "test-node-1" {
		t.Errorf("Expected node_id 'test-node-1', got '%s'", response["node_id"])
	}
}

// TestGetCapabilitiesHandler tests the get_capabilities handler
func TestGetCapabilitiesHandler(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		capabilities: []string{"llm_forward", "hello", "test_capability"},
	}

	var getCapabilitiesHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the get_capabilities handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "get_capabilities" {
			getCapabilitiesHandler = handler
		}
	}

	handlers.RegisterDefaultHandlers(mockCluster, mockCluster.GetNodeID, mockCluster.GetCapabilities, mockCluster.GetOnlinePeers, mockCluster.GetActionsSchema, registrar)

	if getCapabilitiesHandler == nil {
		t.Fatal("get_capabilities handler was not registered")
	}

	// Test get_capabilities handler
	response, err := getCapabilitiesHandler(map[string]interface{}{})
	if err != nil {
		t.Errorf("get_capabilities handler returned error: %v", err)
	}

	caps, ok := response["capabilities"].([]string)
	if !ok {
		t.Fatalf("Expected capabilities to be []string, got %T", response["capabilities"])
	}

	// Sort for comparison (order may vary)
	expectedCaps := []string{"llm_forward", "hello", "test_capability"}
	sort.Strings(caps)
	sort.Strings(expectedCaps)

	if len(caps) != len(expectedCaps) {
		t.Errorf("Expected %d capabilities, got %d", len(expectedCaps), len(caps))
	}

	for i, cap := range caps {
		if cap != expectedCaps[i] {
			t.Errorf("Expected capability '%s' at index %d, got '%s'", expectedCaps[i], i, cap)
		}
	}
}

// TestGetInfoHandler tests the get_info handler
func TestGetInfoHandler(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID: "test-node-1",
		peers: []interface{}{
			&mockNode{
				id:           "peer-1",
				name:         "Test Peer 1",
				address:      "192.168.1.100:21950",
				addresses:    []string{"192.168.1.100:21950"},
				rpcPort:      21950,
				capabilities: []string{"llm_forward"},
				status:       "online",
				online:       true,
			},
			&mockNode{
				id:           "peer-2",
				name:         "Test Peer 2",
				address:      "192.168.1.101:21950",
				addresses:    []string{"192.168.1.101:21950"},
				rpcPort:      21950,
				capabilities: []string{"hello"},
				status:       "online",
				online:       true,
			},
		},
	}

	var getInfoHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the get_info handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "get_info" {
			getInfoHandler = handler
		}
	}

	handlers.RegisterDefaultHandlers(mockCluster, mockCluster.GetNodeID, mockCluster.GetCapabilities, mockCluster.GetOnlinePeers, mockCluster.GetActionsSchema, registrar)

	if getInfoHandler == nil {
		t.Fatal("get_info handler was not registered")
	}

	// Test get_info handler
	response, err := getInfoHandler(map[string]interface{}{})
	if err != nil {
		t.Errorf("get_info handler returned error: %v", err)
	}

	if response["node_id"] != "test-node-1" {
		t.Errorf("Expected node_id 'test-node-1', got '%s'", response["node_id"])
	}

	peers, ok := response["peers"].([]map[string]interface{})
	if !ok {
		t.Fatalf("Expected peers to be []map[string]interface{}, got %T", response["peers"])
	}

	if len(peers) != 2 {
		t.Errorf("Expected 2 peers, got %d", len(peers))
	}

	// Check first peer
	if peers[0]["id"] != "peer-1" {
		t.Errorf("Expected peer id 'peer-1', got '%s'", peers[0]["id"])
	}
	if peers[0]["name"] != "Test Peer 1" {
		t.Errorf("Expected peer name 'Test Peer 1', got '%s'", peers[0]["name"])
	}
	if peers[0]["status"] != "online" {
		t.Errorf("Expected peer status 'online', got '%s'", peers[0]["status"])
	}

	// Check second peer
	if peers[1]["id"] != "peer-2" {
		t.Errorf("Expected peer id 'peer-2', got '%s'", peers[1]["id"])
	}
	if peers[1]["name"] != "Test Peer 2" {
		t.Errorf("Expected peer name 'Test Peer 2', got '%s'", peers[1]["name"])
	}
}

// TestGetInfoHandlerWithNoPeers tests get_info with no peers
func TestGetInfoHandlerWithNoPeers(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID: "test-node-1",
		peers:  []interface{}{},
	}

	var getInfoHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the get_info handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "get_info" {
			getInfoHandler = handler
		}
	}

	handlers.RegisterDefaultHandlers(mockCluster, mockCluster.GetNodeID, mockCluster.GetCapabilities, mockCluster.GetOnlinePeers, mockCluster.GetActionsSchema, registrar)

	if getInfoHandler == nil {
		t.Fatal("get_info handler was not registered")
	}

	// Test get_info handler
	response, err := getInfoHandler(map[string]interface{}{})
	if err != nil {
		t.Errorf("get_info handler returned error: %v", err)
	}

	peers, ok := response["peers"].([]map[string]interface{})
	if !ok {
		t.Fatalf("Expected peers to be []map[string]interface{}, got %T", response["peers"])
	}

	if len(peers) != 0 {
		t.Errorf("Expected 0 peers, got %d", len(peers))
	}
}

// TestGetInfoHandlerWithNonNodePeers tests get_info with non-Node peers (should be filtered)
func TestGetInfoHandlerWithNonNodePeers(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID: "test-node-1",
		peers: []interface{}{
			&mockNode{
				id:     "peer-1",
				name:   "Valid Peer",
				status: "online",
				online: true,
			},
			"invalid peer string",      // Should be filtered out
			12345,                      // Should be filtered out
			struct{ id string }{"id"}, // Should be filtered out (doesn't implement Node)
		},
	}

	var getInfoHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the get_info handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "get_info" {
			getInfoHandler = handler
		}
	}

	handlers.RegisterDefaultHandlers(mockCluster, mockCluster.GetNodeID, mockCluster.GetCapabilities, mockCluster.GetOnlinePeers, mockCluster.GetActionsSchema, registrar)

	if getInfoHandler == nil {
		t.Fatal("get_info handler was not registered")
	}

	// Test get_info handler
	response, err := getInfoHandler(map[string]interface{}{})
	if err != nil {
		t.Errorf("get_info handler returned error: %v", err)
	}

	peers, ok := response["peers"].([]map[string]interface{})
	if !ok {
		t.Fatalf("Expected peers to be []map[string]interface{}, got %T", response["peers"])
	}

	// Only the valid mockNode should be included
	if len(peers) != 1 {
		t.Errorf("Expected 1 peer (non-Node peers filtered), got %d", len(peers))
	}

	if len(peers) > 0 && peers[0]["id"] != "peer-1" {
		t.Errorf("Expected peer id 'peer-1', got '%s'", peers[0]["id"])
	}
}

// TestListActionsHandler tests the list_actions handler
func TestListActionsHandler(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID: "test-node-1",
		actionsSchema: []handlers.ActionSchema{
			{
				Name:        "ping",
				Description: "Health check, test if node is online",
				Parameters:  nil,
				Returns: map[string]interface{}{
					"properties": map[string]interface{}{
						"status": map[string]interface{}{
							"type":        "string",
							"description": "Response status",
							"enum":         []string{"ok"},
						},
						"node_id": map[string]interface{}{
							"type":        "string",
							"description": "Node ID",
						},
					},
				},
				Examples: []map[string]interface{}{
					{
						"request": map[string]interface{}{
							"action":  "ping",
							"payload": nil,
						},
						"response": map[string]interface{}{
							"status":  "ok",
							"node_id": "node-abc123",
						},
					},
				},
			},
			{
				Name:        "custom_action",
				Description: "A custom action with parameters",
				Parameters: map[string]interface{}{
					"type": "object",
					"properties": map[string]interface{}{
						"param1": map[string]interface{}{
							"type":        "string",
							"description": "First parameter",
						},
					},
					"required": []string{"param1"},
				},
				Returns: map[string]interface{}{
					"properties": map[string]interface{}{
						"result": map[string]interface{}{
							"type": "string",
						},
					},
				},
				Examples: []map[string]interface{}{},
			},
		},
	}

	var listActionsHandler func(map[string]interface{}) (map[string]interface{}, error)

	// Capture the list_actions handler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "list_actions" {
			listActionsHandler = handler
		}
	}

	handlers.RegisterDefaultHandlers(mockCluster, mockCluster.GetNodeID, mockCluster.GetCapabilities, mockCluster.GetOnlinePeers, mockCluster.GetActionsSchema, registrar)

	if listActionsHandler == nil {
		t.Fatal("list_actions handler was not registered")
	}

	// Test list_actions handler
	response, err := listActionsHandler(map[string]interface{}{})
	if err != nil {
		t.Errorf("list_actions handler returned error: %v", err)
	}

	actions, ok := response["actions"].([]interface{})
	if !ok {
		t.Fatalf("Expected actions to be []interface{}, got %T", response["actions"])
	}

	if len(actions) != 2 {
		t.Errorf("Expected 2 actions, got %d", len(actions))
	}

	// Actions are returned as map[string]interface{} after JSON serialization
	action0Map, ok := actions[0].(map[string]interface{})
	if !ok {
		t.Fatal("First action is not a map")
	}

	// Verify first action (ping)
	if action0Map["name"] != "ping" {
		t.Errorf("Expected action name 'ping', got '%v'", action0Map["name"])
	}
	if action0Map["description"] != "Health check, test if node is online" {
		t.Errorf("Expected description 'Health check, test if node is online', got '%v'", action0Map["description"])
	}

	// Parameters can be nil or an empty map (both are valid for "no parameters")
	params := action0Map["parameters"]
	if params != nil {
		if paramsMap, ok := params.(map[string]interface{}); ok && len(paramsMap) > 0 {
			t.Errorf("Expected nil or empty parameters for ping, got %v", params)
		}
	}

	// Verify returns schema for ping
	if action0Map["returns"] == nil {
		t.Error("Expected non-nil returns for ping")
	} else {
		returns, ok := action0Map["returns"].(map[string]interface{})
		if !ok {
			t.Error("Expected returns to be a map")
		} else {
			if properties, ok := returns["properties"].(map[string]interface{}); ok {
				if len(properties) != 2 {
					t.Errorf("Expected 2 return properties, got %d", len(properties))
				}
			}
		}
	}

	// Verify examples for ping
	if examples, ok := action0Map["examples"].([]interface{}); ok {
		if len(examples) != 1 {
			t.Errorf("Expected 1 example for ping, got %d", len(examples))
		}
	}

	// Verify second action (custom_action)
	action1Map, ok := actions[1].(map[string]interface{})
	if !ok {
		t.Fatal("Second action is not a map")
	}

	if action1Map["name"] != "custom_action" {
		t.Errorf("Expected action name 'custom_action', got '%v'", action1Map["name"])
	}
	if action1Map["parameters"] == nil {
		t.Error("Expected non-nil parameters for custom_action")
	}
}

// TestListActionsHandlerEmptySchema tests list_actions with empty schema
func TestListActionsHandlerEmptySchema(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID:        "test-node-1",
		actionsSchema: []handlers.ActionSchema{},
	}

	var listActionsHandler func(map[string]interface{}) (map[string]interface{}, error)

	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "list_actions" {
			listActionsHandler = handler
		}
	}

	handlers.RegisterDefaultHandlers(mockCluster, mockCluster.GetNodeID, mockCluster.GetCapabilities, mockCluster.GetOnlinePeers, mockCluster.GetActionsSchema, registrar)

	if listActionsHandler == nil {
		t.Fatal("list_actions handler was not registered")
	}

	response, err := listActionsHandler(map[string]interface{}{})
	if err != nil {
		t.Errorf("list_actions handler returned error: %v", err)
	}

	actions, ok := response["actions"].([]interface{})
	if !ok {
		t.Fatalf("Expected actions to be []interface{}, got %T", response["actions"])
	}

	if len(actions) != 0 {
		t.Errorf("Expected 0 actions, got %d", len(actions))
	}
}

// TestListActionsHandlerWithAllFields tests list_actions with complete schema
func TestListActionsHandlerWithAllFields(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID: "test-node-1",
		actionsSchema: []handlers.ActionSchema{
			{
				Name:        "complete_action",
				Description: "Action with all fields populated",
				Parameters: map[string]interface{}{
					"type": "object",
					"properties": map[string]interface{}{
						"input": map[string]interface{}{
							"type":        "string",
							"description": "Input parameter",
						},
					},
					"required": []string{"input"},
				},
				Returns: map[string]interface{}{
					"properties": map[string]interface{}{
						"output": map[string]interface{}{
							"type":        "string",
							"description": "Output result",
						},
					},
				},
				Examples: []map[string]interface{}{
					{
						"request": map[string]interface{}{
							"action": "complete_action",
							"payload": map[string]interface{}{
								"input": "test",
							},
						},
						"response": map[string]interface{}{
							"output": "result",
						},
					},
				},
			},
		},
	}

	var listActionsHandler func(map[string]interface{}) (map[string]interface{}, error)

	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "list_actions" {
			listActionsHandler = handler
		}
	}

	handlers.RegisterDefaultHandlers(mockCluster, mockCluster.GetNodeID, mockCluster.GetCapabilities, mockCluster.GetOnlinePeers, mockCluster.GetActionsSchema, registrar)

	response, err := listActionsHandler(map[string]interface{}{})
	if err != nil {
		t.Errorf("list_actions handler returned error: %v", err)
	}

	actions, ok := response["actions"].([]interface{})
	if !ok {
		t.Fatalf("Expected actions to be []interface{}, got %T", response["actions"])
	}

	if len(actions) != 1 {
		t.Fatalf("Expected 1 action, got %d", len(actions))
	}

	action, ok := actions[0].(map[string]interface{})
	if !ok {
		t.Fatal("Action is not a map")
	}

	// Verify all fields are preserved
	if action["name"] != "complete_action" {
		t.Errorf("Expected name 'complete_action', got '%v'", action["name"])
	}

	if action["description"] != "Action with all fields populated" {
		t.Errorf("Expected description 'Action with all fields populated', got '%v'", action["description"])
	}

	if action["parameters"] == nil {
		t.Error("Expected non-nil parameters")
	} else {
		// Verify parameter schema structure
		params, ok := action["parameters"].(map[string]interface{})
		if !ok {
			t.Error("Expected parameters to be a map")
		} else {
			if properties, ok := params["properties"].(map[string]interface{}); ok {
				if len(properties) != 1 {
					t.Errorf("Expected 1 parameter property, got %d", len(properties))
				}
			}
		}
	}

	if action["returns"] == nil {
		t.Error("Expected non-nil returns")
	}

	if examples, ok := action["examples"].([]interface{}); ok {
		if len(examples) != 1 {
			t.Errorf("Expected 1 example, got %d", len(examples))
		}
	}
}

// TestListActionsResponseFormat tests the response format structure
func TestListActionsResponseFormat(t *testing.T) {
	mockCluster := &mockClusterForHandlers{
		nodeID: "test-node-1",
		actionsSchema: []handlers.ActionSchema{
			{Name: "test_action", Description: "Test"},
		},
	}

	var listActionsHandler func(map[string]interface{}) (map[string]interface{}, error)

	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if action == "list_actions" {
			listActionsHandler = handler
		}
	}

	handlers.RegisterDefaultHandlers(mockCluster, mockCluster.GetNodeID, mockCluster.GetCapabilities, mockCluster.GetOnlinePeers, mockCluster.GetActionsSchema, registrar)

	response, err := listActionsHandler(map[string]interface{}{})
	if err != nil {
		t.Fatalf("list_actions handler returned error: %v", err)
	}

	// Verify response has 'actions' key
	if _, ok := response["actions"]; !ok {
		t.Error("Response missing 'actions' field")
	}

	// Verify response only has expected fields
	if len(response) != 1 {
		t.Errorf("Expected response to have 1 field, got %d", len(response))
	}
}
