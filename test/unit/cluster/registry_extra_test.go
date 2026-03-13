// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster_test

import (
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/cluster"
)

// TestRegistryRemove tests removing nodes from registry
func TestRegistryRemove(t *testing.T) {
	registry := cluster.NewRegistry()

	node := &cluster.Node{
		ID:      "bot-1",
		Name:    "Test Bot 1",
		Address: "192.168.1.1:49200",
	}

	registry.AddOrUpdate(node)

	// Remove node
	registry.Remove("bot-1")

	// Verify removal
	if registry.Get("bot-1") != nil {
		t.Error("Node should be removed")
	}
}

// TestRegistryCount tests counting nodes in registry
func TestRegistryCount(t *testing.T) {
	registry := cluster.NewRegistry()

	if registry.Count() != 0 {
		t.Errorf("Expected 0 nodes, got %d", registry.Count())
	}

	node := &cluster.Node{
		ID:      "bot-1",
		Name:    "Test Bot 1",
		Address: "192.168.1.1:49200",
	}

	registry.AddOrUpdate(node)

	if registry.Count() != 1 {
		t.Errorf("Expected 1 node, got %d", registry.Count())
	}
}

// TestRegistryGetAll tests getting all nodes from registry
func TestRegistryGetAll(t *testing.T) {
	registry := cluster.NewRegistry()

	nodes := []*cluster.Node{
		{ID: "bot-1", Name: "Bot 1", Address: "192.168.1.1:49200"},
		{ID: "bot-2", Name: "Bot 2", Address: "192.168.1.2:49200"},
	}

	for _, node := range nodes {
		registry.AddOrUpdate(node)
	}

	all := registry.GetAll()
	if len(all) != 2 {
		t.Errorf("Expected 2 nodes, got %d", len(all))
	}
}

// TestRegistryOnlineCount tests counting online nodes
func TestRegistryOnlineCount(t *testing.T) {
	registry := cluster.NewRegistry()

	onlineNode := &cluster.Node{
		ID:      "bot-online",
		Name:    "Online Bot",
		Address: "192.168.1.1:49200",
	}
	registry.AddOrUpdate(onlineNode)

	offlineNode := &cluster.Node{
		ID:      "bot-offline",
		Name:    "Offline Bot",
		Address: "192.168.1.2:49200",
	}
	registry.AddOrUpdate(offlineNode)
	offlineNode.SetStatus(cluster.StatusOffline)

	count := registry.OnlineCount()
	if count != 1 {
		t.Errorf("Expected 1 online node, got %d", count)
	}
}

// TestRegistryMarkOffline tests marking a node offline
func TestRegistryMarkOffline(t *testing.T) {
	registry := cluster.NewRegistry()

	node := &cluster.Node{
		ID:      "bot-1",
		Name:    "Test Bot",
		Address: "192.168.1.1:49200",
	}
	registry.AddOrUpdate(node)

	registry.MarkOffline("bot-1", "test reason")

	retrieved := registry.Get("bot-1")
	if retrieved.GetStatus() != string(cluster.StatusOffline) {
		t.Errorf("Expected status offline, got %s", retrieved.GetStatus())
	}
}

// TestNodeUpdateLastSeen tests updating node's last seen time
func TestNodeUpdateLastSeen(t *testing.T) {
	node := &cluster.Node{
		ID:       "bot-test",
		Name:     "Test Bot",
		Address:  "192.168.1.1:49200",
		LastSeen: time.Now().Add(-1 * time.Hour),
	}

	initialTime := node.LastSeen

	time.Sleep(10 * time.Millisecond)

	node.UpdateLastSeen()

	if !node.LastSeen.After(initialTime) {
		t.Error("LastSeen should be more recent after UpdateLastSeen")
	}
}

// TestNodeGetUptime tests getting node uptime
func TestNodeGetUptime(t *testing.T) {
	node := &cluster.Node{
		ID:       "bot-test",
		Name:     "Test Bot",
		Address:  "192.168.1.1:49200",
		LastSeen: time.Now(),
	}

	uptime := node.GetUptime()
	if uptime < 0 {
		t.Errorf("Uptime should be non-negative, got %v", uptime)
	}
}

// TestNodeStatusTransitions tests node status transitions
func TestNodeStatusTransitions(t *testing.T) {
	node := &cluster.Node{
		ID:      "bot-test",
		Name:    "Test Bot",
		Address: "192.168.1.1:49200",
	}

	node.SetStatus(cluster.StatusOnline)
	if node.GetStatus() != string(cluster.StatusOnline) {
		t.Errorf("Expected status online, got %s", node.GetStatus())
	}

	node.MarkOffline("test")
	if node.GetStatus() != string(cluster.StatusOffline) {
		t.Errorf("Expected status offline, got %s", node.GetStatus())
	}
}

// TestNodeString tests node string representation
func TestNodeString(t *testing.T) {
	node := &cluster.Node{
		ID:      "bot-1",
		Name:    "Test Bot",
		Address: "192.168.1.1:49200",
		Status:  cluster.StatusOnline,
	}

	str := node.String()
	if str == "" {
		t.Error("String() should not return empty string")
	}
}

// TestRegistryFindByCapabilityOnline tests finding online nodes by capability
func TestRegistryFindByCapabilityOnline(t *testing.T) {
	registry := cluster.NewRegistry()

	node1 := &cluster.Node{
		ID:           "bot-1",
		Name:         "Bot 1",
		Address:      "192.168.1.1:49200",
		Capabilities: []string{"code"},
	}
	node2 := &cluster.Node{
		ID:           "bot-2",
		Name:         "Bot 2",
		Address:      "192.168.1.2:49200",
		Capabilities: []string{"code"},
	}

	registry.AddOrUpdate(node1)
	registry.AddOrUpdate(node2)

	// Mark bot-2 as offline
	node2.SetStatus(cluster.StatusOffline)

	results := registry.FindByCapabilityOnline("code")
	if len(results) != 1 {
		t.Errorf("Expected 1 online node with 'code', got %d", len(results))
	}

	if len(results) > 0 && results[0].ID != "bot-1" {
		t.Errorf("Expected bot-1, got %s", results[0].ID)
	}
}
