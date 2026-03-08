// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster_test

import (
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/cluster"
)

// TestRegistryAdd tests adding nodes to registry
func TestRegistryAdd(t *testing.T) {
	registry := cluster.NewRegistry()

	node := &cluster.Node{
		ID:           "bot-1",
		Name:         "Test Bot 1",
		Address:      "192.168.1.1:49200",
		Role:         "worker",
		Capabilities: []string{"test", "demo"},
		Priority:     1,
	}

	registry.AddOrUpdate(node)

	// Verify node was added
	retrieved := registry.Get("bot-1")
	if retrieved == nil {
		t.Fatal("Node not found in registry")
	}

	if retrieved.ID != node.ID {
		t.Errorf("Expected ID %s, got %s", node.ID, retrieved.ID)
	}
}

// TestRegistryGetOnline tests getting online nodes
func TestRegistryGetOnline(t *testing.T) {
	registry := cluster.NewRegistry()

	// Add online node
	onlineNode := &cluster.Node{
		ID:      "bot-online",
		Name:    "Online Bot",
		Address: "192.168.1.1:49200",
	}
	registry.AddOrUpdate(onlineNode)

	// Add offline node (manually set after adding)
	offlineNode := &cluster.Node{
		ID:      "bot-offline",
		Name:    "Offline Bot",
		Address: "192.168.1.2:49200",
	}
	registry.AddOrUpdate(offlineNode)
	// Manually mark as offline after adding
	offlineNode.SetStatus(cluster.StatusOffline)

	// Get online nodes
	online := registry.GetOnline()
	if len(online) != 1 {
		t.Errorf("Expected 1 online node, got %d", len(online))
	}

	if online[0].ID != "bot-online" {
		t.Errorf("Expected bot-online, got %s", online[0].ID)
	}
}

// TestRegistryGetCapabilities tests getting all capabilities
func TestRegistryGetCapabilities(t *testing.T) {
	registry := cluster.NewRegistry()

	// Add nodes with different capabilities
	node1 := &cluster.Node{
		ID:           "bot-1",
		Name:         "Bot 1",
		Address:      "192.168.1.1:49200",
		Capabilities: []string{"code", "test"},
	}
	node2 := &cluster.Node{
		ID:           "bot-2",
		Name:         "Bot 2",
		Address:      "192.168.1.2:49200",
		Capabilities: []string{"translate", "test"},
	}

	registry.AddOrUpdate(node1)
	registry.AddOrUpdate(node2)

	// Get capabilities
	caps := registry.GetCapabilities()

	// Should have 3 unique capabilities (code, test, translate)
	// Note: "test" appears in both, should be deduplicated
	if len(caps) != 3 {
		t.Errorf("Expected 3 unique capabilities, got %d: %v", len(caps), caps)
	}
}

// TestRegistryFindByCapability tests finding nodes by capability
func TestRegistryFindByCapability(t *testing.T) {
	registry := cluster.NewRegistry()

	// Add nodes with different capabilities
	node1 := &cluster.Node{
		ID:           "bot-code",
		Name:         "Code Bot",
		Address:      "192.168.1.1:49200",
		Capabilities: []string{"code", "analysis"},
	}
	node2 := &cluster.Node{
		ID:           "bot-both",
		Name:         "Multi Bot",
		Address:      "192.168.1.2:49200",
		Capabilities: []string{"code", "translate"},
	}

	registry.AddOrUpdate(node1)
	registry.AddOrUpdate(node2)

	// Find by capability
	results := registry.FindByCapability("code")

	if len(results) != 2 {
		t.Errorf("Expected 2 nodes with 'code' capability, got %d", len(results))
	}

	// Check that we got the right nodes
	foundIDs := make(map[string]bool)
	for _, node := range results {
		foundIDs[node.ID] = true
	}

	if !foundIDs["bot-code"] || !foundIDs["bot-both"] {
		t.Error("Did not find expected nodes")
	}
}

// TestRegistryCheckTimeouts tests timeout checking
func TestRegistryCheckTimeouts(t *testing.T) {
	registry := cluster.NewRegistry()

	// Add a node and manually set old last seen time
	oldNode := &cluster.Node{
		ID:      "bot-old",
		Name:    "Old Bot",
		Address: "192.168.1.1:49200",
	}
	registry.AddOrUpdate(oldNode)
	// Manually set old last seen time
	oldNode.LastSeen = time.Now().Add(-2 * time.Minute)

	// Check for timeouts with 90 second timeout
	expired := registry.CheckTimeouts(90 * time.Second)

	if len(expired) != 1 {
		t.Errorf("Expected 1 expired node, got %d", len(expired))
		return // avoid panic on empty slice
	}

	if expired[0] != "bot-old" {
		t.Errorf("Expected bot-old to expire, got %s", expired[0])
	}

	// Verify node is marked offline
	node := registry.Get("bot-old")
	if node.Status != cluster.StatusOffline {
		t.Errorf("Expected status offline, got %s", node.Status)
	}
}

// TestNodeIsOnline tests node online status check
func TestNodeIsOnline(t *testing.T) {
	node := &cluster.Node{
		ID:       "bot-test",
		Name:     "Test Bot",
		Address:  "192.168.1.1:49200",
		Status:   cluster.StatusOnline,
		LastSeen: time.Now(),
	}

	if !node.IsOnline() {
		t.Error("Expected node to be online")
	}

	// Mark as offline
	node.MarkOffline("test")

	if node.IsOnline() {
		t.Error("Expected node to be offline after marking")
	}
}

// TestNodeHasCapability tests capability checking
func TestNodeHasCapability(t *testing.T) {
	node := &cluster.Node{
		ID:           "bot-test",
		Name:         "Test Bot",
		Address:      "192.168.1.1:49200",
		Capabilities: []string{"code", "test", "debug"},
	}

	if !node.HasCapability("code") {
		t.Error("Expected node to have 'code' capability")
	}

	if node.HasCapability("nonexistent") {
		t.Error("Node should not have 'nonexistent' capability")
	}
}

// TestNodeToConfig tests converting node to config
func TestNodeToConfig(t *testing.T) {
	node := &cluster.Node{
		ID:             "bot-test",
		Name:           "Test Bot",
		Address:        "192.168.1.1:49200",
		Role:           "worker",
		Capabilities:   []string{"code"},
		Priority:       1,
		Status:         cluster.StatusOnline,
		LastSeen:       time.Now(),
		TasksCompleted: 10,
		SuccessRate:    0.95,
	}

	config := node.ToConfig()

	if config.ID != node.ID {
		t.Errorf("Expected config ID %s, got %s", node.ID, config.ID)
	}

	if config.Name != node.Name {
		t.Errorf("Expected config Name %s, got %s", node.Name, config.Name)
	}

	if config.Status.State != string(cluster.StatusOnline) {
		t.Errorf("Expected config state %s, got %s", cluster.StatusOnline, config.Status.State)
	}
}
