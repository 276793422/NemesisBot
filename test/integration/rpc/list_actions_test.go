// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

// This test file is in the same package as the RPC integration tests
// to share the testBot helper and createTestBot function.
package main

import (
	"encoding/json"
	"fmt"
	"testing"
	"time"
)

// TestListActionsRPCFlow tests the complete RPC flow for list_actions
// This demonstrates:
// 1. Client bot sends list_actions RPC request to server bot
// 2. Server bot's RPC server receives and processes the request
// 3. Server bot returns all available actions with their schemas
// 4. Client bot receives and validates the response
func TestListActionsRPCFlow(t *testing.T) {
	t.Log("=== List Actions RPC Flow Integration Test ===")

	// Setup: Create two bot instances
	serverBot := createTestBot("Server-Bot", 21960) // Server
	clientBot := createTestBot("Client-Bot", 21959) // Client

	// Start server bot
	if err := serverBot.Start(); err != nil {
		t.Fatalf("Failed to start server bot: %v", err)
	}
	defer serverBot.Stop()

	t.Logf("Server bot started on port %d", serverBot.RPCPort)

	// Wait for server to be ready
	time.Sleep(500 * time.Millisecond)

	// Client sends list_actions request
	t.Log("\n[Test] Client -> Server: list_actions request")

	response, err := clientBot.SendRPCRequest("127.0.0.1:21960", "list_actions", nil)
	if err != nil {
		t.Fatalf("Failed to send list_actions request: %v", err)
	}

	t.Logf("Client received response: %s", response)

	// Parse and verify response
	var result map[string]interface{}
	if err := json.Unmarshal([]byte(response), &result); err != nil {
		t.Fatalf("Failed to parse response: %v", err)
	}

	// Verify actions field exists
	actionsRaw, ok := result["actions"]
	if !ok {
		t.Fatal("Response missing 'actions' field")
	}

	// Convert to []interface{}
	actions, ok := actionsRaw.([]interface{})
	if !ok {
		t.Fatalf("Expected actions to be []interface{}, got %T", actionsRaw)
	}

	if len(actions) == 0 {
		t.Error("Expected at least one action, got empty list")
	}

	t.Logf("Received %d actions", len(actions))

	// Verify expected actions exist
	expectedActions := map[string]bool{
		"ping":             false,
		"get_capabilities": false,
		"get_info":         false,
		"list_actions":     false,
	}

	for _, actionRaw := range actions {
		action, ok := actionRaw.(map[string]interface{})
		if !ok {
			t.Logf("Warning: action is not a map: %T", actionRaw)
			continue
		}

		name, ok := action["name"].(string)
		if !ok {
			t.Logf("Warning: action name is not string: %T", action["name"])
			continue
		}

		if _, exists := expectedActions[name]; exists {
			expectedActions[name] = true
			t.Logf("Found action: %s", name)

			// Verify action has required fields
			if desc, ok := action["description"].(string); ok {
				t.Logf("  Description: %s", desc)
			} else {
				t.Errorf("Action %s missing description", name)
			}

			// Verify parameters field exists (may be nil)
			if _, hasParams := action["parameters"]; hasParams {
				t.Logf("  Has parameters: yes")
			} else {
				t.Logf("  Has parameters: no")
			}

			// Verify returns field exists (may be nil)
			if _, hasReturns := action["returns"]; hasReturns {
				t.Logf("  Has returns: yes")
			} else {
				t.Logf("  Has returns: no")
			}

			// Verify examples field exists (may be empty or nil)
			if examples, hasExamples := action["examples"]; hasExamples {
				if exList, ok := examples.([]interface{}); ok {
					t.Logf("  Examples count: %d", len(exList))
				}
			}
		}
	}

	// Check all expected actions were found
	for actionName, found := range expectedActions {
		if !found {
			t.Errorf("Expected action '%s' not found in response", actionName)
		}
	}

	t.Log("\n=== Test PASSED ===")
}

// TestListActionsAcrossNodes tests list_actions across multiple nodes
func TestListActionsAcrossNodes(t *testing.T) {
	t.Log("=== List Actions Across Multiple Nodes Test ===")

	// Create three bot instances
	bot1 := createTestBot("Bot-1", 21961)
	bot2 := createTestBot("Bot-2", 21962)
	bot3 := createTestBot("Bot-3", 21963)

	// Start all bots
	bots := []*testBot{bot1, bot2, bot3}
	for _, bot := range bots {
		if err := bot.Start(); err != nil {
			t.Fatalf("Failed to start bot %s: %v", bot.Name, err)
		}
		defer bot.Stop()
		t.Logf("%s started on port %d", bot.Name, bot.RPCPort)
	}

	// Wait for all bots to be ready
	time.Sleep(500 * time.Millisecond)

	// Each bot queries list_actions from every other bot
	testCases := []struct {
		client    *testBot
		server    *testBot
		serverURL string
	}{
		{bot1, bot2, "127.0.0.1:21962"},
		{bot1, bot3, "127.0.0.1:21963"},
		{bot2, bot1, "127.0.0.1:21961"},
		{bot2, bot3, "127.0.0.1:21963"},
		{bot3, bot1, "127.0.0.1:21961"},
		{bot3, bot2, "127.0.0.1:21962"},
	}

	for _, tc := range testCases {
		t.Logf("\n[Test] %s -> %s: list_actions request", tc.client.Name, tc.server.Name)

		response, err := tc.client.SendRPCRequest(tc.serverURL, "list_actions", nil)
		if err != nil {
			t.Errorf("Failed to send request from %s to %s: %v", tc.client.Name, tc.server.Name, err)
			continue
		}

		var result map[string]interface{}
		if err := json.Unmarshal([]byte(response), &result); err != nil {
			t.Errorf("Failed to parse response from %s: %v", tc.server.Name, err)
			continue
		}

		actionsRaw, ok := result["actions"]
		if !ok {
			t.Errorf("Response from %s missing 'actions' field", tc.server.Name)
			continue
		}

		actions, ok := actionsRaw.([]interface{})
		if !ok {
			t.Errorf("Invalid actions type from %s: %T", tc.server.Name, actionsRaw)
			continue
		}

		t.Logf("%s -> %s: Received %d actions", tc.client.Name, tc.server.Name, len(actions))
	}

	t.Log("\n=== Test PASSED ===")
}

// TestListActionsSchemaCompleteness tests that schema contains all required fields
func TestListActionsSchemaCompleteness(t *testing.T) {
	t.Log("=== List Actions Schema Completeness Test ===")

	// Create server and client bots
	serverBot := createTestBot("Schema-Server", 21964)
	clientBot := createTestBot("Schema-Client", 21965)

	// Start server
	if err := serverBot.Start(); err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer serverBot.Stop()

	t.Logf("Schema server started on port %d", serverBot.RPCPort)
	time.Sleep(500 * time.Millisecond)

	// Query list_actions
	response, err := clientBot.SendRPCRequest("127.0.0.1:21964", "list_actions", nil)
	if err != nil {
		t.Fatalf("Failed to send request: %v", err)
	}

	var result map[string]interface{}
	if err := json.Unmarshal([]byte(response), &result); err != nil {
		t.Fatalf("Failed to parse response: %v", err)
	}

	actionsRaw, ok := result["actions"].([]interface{})
	if !ok {
		t.Fatalf("Invalid actions type: %T", result["actions"])
	}

	// Verify each action has complete schema
	for i, actionRaw := range actionsRaw {
		action, ok := actionRaw.(map[string]interface{})
		if !ok {
			t.Errorf("Action %d: not a map", i)
			continue
		}

		name, _ := action["name"].(string)

		// Required fields
		if _, hasName := action["name"]; !hasName {
			t.Errorf("Action %d: missing 'name' field", i)
		}
		if _, hasDesc := action["description"]; !hasDesc {
			t.Errorf("Action '%s': missing 'description' field", name)
		}

		// Optional fields (should exist even if null/nil)
		if _, hasParams := action["parameters"]; !hasParams {
			t.Errorf("Action '%s': missing 'parameters' field", name)
		}
		if _, hasReturns := action["returns"]; !hasReturns {
			t.Errorf("Action '%s': missing 'returns' field", name)
		}
		if _, hasExamples := action["examples"]; !hasExamples {
			t.Errorf("Action '%s': missing 'examples' field", name)
		}

		t.Logf("Action '%s': schema complete", name)
	}

	t.Log("\n=== Test PASSED ===")
}

// TestListActionsWithPeers tests list_actions in a cluster with peers
func TestListActionsWithPeers(t *testing.T) {
	t.Log("=== List Actions with Peers Test ===")

	// Create a cluster of 3 bots
	bots := []*testBot{
		createTestBot("Cluster-Node-1", 21966),
		createTestBot("Cluster-Node-2", 21967),
		createTestBot("Cluster-Node-3", 21968),
	}

	// Start all bots
	for _, bot := range bots {
		if err := bot.Start(); err != nil {
			t.Fatalf("Failed to start %s: %v", bot.Name, err)
		}
		defer bot.Stop()
		t.Logf("%s started on port %d", bot.Name, bot.RPCPort)
	}

	time.Sleep(500 * time.Millisecond)

	// First, use get_info to discover peers
	t.Log("\n[Test] Querying node 1 for cluster info")

	infoResponse, err := bots[0].SendRPCRequest("127.0.0.1:21966", "get_info", nil)
	if err != nil {
		t.Fatalf("Failed to send get_info request: %v", err)
	}

	t.Logf("Cluster info: %s", infoResponse)

	// Then query list_actions from each node
	for i, bot := range bots {
		serverURL := fmt.Sprintf("127.0.0.1:%d", 21966+i)

		t.Logf("\n[Test] Querying %s for actions", bot.Name)

		actionsResponse, err := bots[0].SendRPCRequest(serverURL, "list_actions", nil)
		if err != nil {
			t.Errorf("Failed to query %s: %v", bot.Name, err)
			continue
		}

		var result map[string]interface{}
		if err := json.Unmarshal([]byte(actionsResponse), &result); err != nil {
			t.Errorf("Failed to parse response from %s: %v", bot.Name, err)
			continue
		}

		actionsRaw, ok := result["actions"].([]interface{})
		if !ok {
			t.Errorf("Invalid response from %s", bot.Name)
			continue
		}

		t.Logf("%s provides %d actions", bot.Name, len(actionsRaw))

		// Verify all nodes provide the same set of default actions
		defaultActions := []string{"ping", "get_capabilities", "get_info", "list_actions"}
		for _, expectedAction := range defaultActions {
			found := false
			for _, actionRaw := range actionsRaw {
				action, ok := actionRaw.(map[string]interface{})
				if !ok {
					continue
				}
				name, ok := action["name"].(string)
				if ok && name == expectedAction {
					found = true
					break
				}
			}
			if !found {
				t.Errorf("%s missing expected action: %s", bot.Name, expectedAction)
			}
		}
	}

	t.Log("\n=== Test PASSED ===")
}

// TestListActionsSequentialRequests tests multiple sequential list_actions requests
func TestListActionsSequentialRequests(t *testing.T) {
	t.Log("=== List Actions Sequential Requests Test ===")

	serverBot := createTestBot("Seq-Server", 21969)
	clientBot := createTestBot("Seq-Client", 21970)

	if err := serverBot.Start(); err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer serverBot.Stop()

	t.Logf("Server started on port %d", serverBot.RPCPort)
	time.Sleep(500 * time.Millisecond)

	// Send multiple sequential requests
	numRequests := 5
	for i := 0; i < numRequests; i++ {
		t.Logf("\n[Test] Sending request %d/%d", i+1, numRequests)

		response, err := clientBot.SendRPCRequest("127.0.0.1:21969", "list_actions", nil)
		if err != nil {
			t.Errorf("Request %d failed: %v", i+1, err)
			continue
		}

		var result map[string]interface{}
		if err := json.Unmarshal([]byte(response), &result); err != nil {
			t.Errorf("Request %d: failed to parse: %v", i+1, err)
			continue
		}

		actionsRaw, ok := result["actions"].([]interface{})
		if !ok {
			t.Errorf("Request %d: invalid response type", i+1)
			continue
		}

		t.Logf("Request %d: received %d actions", i+1, len(actionsRaw))

		// Small delay between requests
		time.Sleep(100 * time.Millisecond)
	}

	t.Log("\n=== Test PASSED ===")
}

// TestListActionsErrorHandling tests error handling for list_actions
func TestListActionsErrorHandling(t *testing.T) {
	t.Log("=== List Actions Error Handling Test ===")

	// Create client but no server (expect connection error)
	clientBot := createTestBot("Error-Client", 21971)

	t.Log("\n[Test] Sending request to non-existent server")

	_, err := clientBot.SendRPCRequest("127.0.0.1:29999", "list_actions", nil)
	if err == nil {
		t.Error("Expected error when connecting to non-existent server, got nil")
	} else {
		t.Logf("Correctly received error: %v", err)
	}

	t.Log("\n=== Test PASSED ===")
}

// TestListActionsServiceDiscovery demonstrates service discovery workflow
func TestListActionsServiceDiscovery(t *testing.T) {
	t.Log("=== Service Discovery Workflow Test ===")

	// Create a server with specific capabilities
	serverBot := createTestBot("Discovery-Server", 21972)
	clientBot := createTestBot("Discovery-Client", 21973)

	if err := serverBot.Start(); err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer serverBot.Stop()

	t.Logf("Discovery server started on port %d", serverBot.RPCPort)
	time.Sleep(500 * time.Millisecond)

	// Step 1: Ping to check if server is online
	t.Log("\n[Step 1] Ping server to check availability")

	pingResponse, err := clientBot.SendRPCRequest("127.0.0.1:21972", "ping", nil)
	if err != nil {
		t.Fatalf("Ping failed: %v", err)
	}

	t.Logf("Ping response: %s", pingResponse)

	var pingResult map[string]interface{}
	if err := json.Unmarshal([]byte(pingResponse), &pingResult); err != nil {
		t.Fatalf("Failed to parse ping: %v", err)
	}

	if pingResult["status"] != "ok" {
		t.Fatal("Server not available")
	}

	t.Log("Server is online ✓")

	// Step 2: Get capabilities
	t.Log("\n[Step 2] Query server capabilities")

	capsResponse, err := clientBot.SendRPCRequest("127.0.0.1:21972", "get_capabilities", nil)
	if err != nil {
		t.Fatalf("get_capabilities failed: %v", err)
	}

	t.Logf("Capabilities: %s", capsResponse)

	t.Log("Retrieved capabilities ✓")

	// Step 3: List all actions with schema
	t.Log("\n[Step 3] List all available actions")

	actionsResponse, err := clientBot.SendRPCRequest("127.0.0.1:21972", "list_actions", nil)
	if err != nil {
		t.Fatalf("list_actions failed: %v", err)
	}

	var actionsResult map[string]interface{}
	if err := json.Unmarshal([]byte(actionsResponse), &actionsResult); err != nil {
		t.Fatalf("Failed to parse actions: %v", err)
	}

	actionsRaw, ok := actionsResult["actions"].([]interface{})
	if !ok {
		t.Fatal("Invalid actions response")
	}

	t.Logf("Found %d actions", len(actionsRaw))

	// Step 4: Display action details
	t.Log("\n[Step 4] Action details:")
	for _, actionRaw := range actionsRaw {
		action, ok := actionRaw.(map[string]interface{})
		if !ok {
			continue
		}

		name, _ := action["name"].(string)
		desc, _ := action["description"].(string)

		t.Logf("  • %s: %s", name, desc)

		if params, ok := action["parameters"]; ok && params != nil {
			t.Logf("    Parameters: %+v", params)
		}
		if returns, ok := action["returns"]; ok && returns != nil {
			t.Logf("    Returns: %+v", returns)
		}
	}

	t.Log("\nService discovery workflow completed ✓")
	t.Log("\n=== Test PASSED ===")
}
