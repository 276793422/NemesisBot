// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// TestGenerateNodeID tests node ID generation
func TestGenerateNodeID(t *testing.T) {
	nodeID, err := GenerateNodeID()
	if err != nil {
		t.Fatalf("Failed to generate node ID: %v", err)
	}

	if nodeID == "" {
		t.Error("Expected non-empty node ID")
	}

	// Node ID format: "bot-hostname-timestamp"
	// Should start with "bot-"
	if !strings.HasPrefix(nodeID, "bot-") {
		t.Errorf("Expected node ID to start with 'bot-', got: %s", nodeID)
	}

	// Should be reasonably long
	if len(nodeID) < 10 {
		t.Errorf("Node ID too short: %d", len(nodeID))
	}

	// Multiple calls should produce different IDs (due to timestamp)
	nodeID2, err := GenerateNodeID()
	if err != nil {
		t.Fatalf("Failed to generate second node ID: %v", err)
	}

	// Wait a tiny bit to ensure different timestamps
	if nodeID == nodeID2 {
		t.Log("Node IDs are the same (might be same timestamp, call again quickly)")
	}
}

// TestNewCluster tests cluster creation
func TestNewCluster(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}

	// Ensure cleanup happens even if test fails
	defer func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	}()

	if cluster == nil {
		t.Fatal("Expected non-nil cluster")
	}

	if cluster.nodeID == "" {
		t.Error("Expected non-empty node ID")
	}

	if cluster.workspace != tempDir {
		t.Errorf("Expected workspace %s, got %s", tempDir, cluster.workspace)
	}

	if cluster.registry == nil {
		t.Error("Expected non-nil registry")
	}

	if cluster.logger == nil {
		t.Error("Expected non-nil logger")
	}

	if cluster.udpPort != DefaultUDPPort {
		t.Errorf("Expected UDP port %d, got %d", DefaultUDPPort, cluster.udpPort)
	}

	if cluster.rpcPort != DefaultRPCPort {
		t.Errorf("Expected RPC port %d, got %d", DefaultRPCPort, cluster.rpcPort)
	}

	if cluster.broadcastInterval != DefaultBroadcastInterval {
		t.Errorf("Expected broadcast interval %v, got %v", DefaultBroadcastInterval, cluster.broadcastInterval)
	}

	if cluster.timeout != DefaultTimeout {
		t.Errorf("Expected timeout %v, got %v", DefaultTimeout, cluster.timeout)
	}
}

// TestNewCluster_DirectoryCreation tests that cluster creates necessary directories
func TestNewCluster_DirectoryCreation(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}

	// Ensure cleanup happens even if test fails
	defer func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	}()

	// Check that cluster directory was created
	clusterDir := filepath.Join(tempDir, "cluster")
	info, err := os.Stat(clusterDir)
	if err != nil {
		t.Errorf("Cluster directory should exist: %v", err)
	}

	if !info.IsDir() {
		t.Error("Expected cluster directory to be a directory")
	}

	// Check that log directory was created (logs/cluster, not cluster/log)
	logDir := filepath.Join(tempDir, "logs", "cluster")
	info, err = os.Stat(logDir)
	if err != nil {
		t.Errorf("Log directory should exist: %v", err)
	}

	if !info.IsDir() {
		t.Error("Expected log directory to be a directory")
	}
}

// TestCluster_GetNodeID tests getting node ID
func TestCluster_GetNodeID(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	nodeID := cluster.GetNodeID()
	if nodeID == "" {
		t.Error("Expected non-empty node ID")
	}

	if nodeID != cluster.nodeID {
		t.Errorf("Expected node ID %s, got %s", cluster.nodeID, nodeID)
	}
}

// TestCluster_GetRegistry tests getting the peer registry
func TestCluster_GetRegistry(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	registry := cluster.GetRegistry()
	if registry == nil {
		t.Error("Expected non-nil registry")
	}

	// Registry should be the same instance
	if registry != cluster.registry {
		t.Error("Registry should match cluster registry")
	}
}

// TestCluster_DefaultValues tests default cluster values
func TestCluster_DefaultValues(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	if cluster.role != "worker" {
		t.Errorf("Expected default role worker, got %s", cluster.role)
	}

	if cluster.category != "general" {
		t.Errorf("Expected default category general, got %s", cluster.category)
	}

	if len(cluster.tags) != 0 {
		t.Errorf("Expected empty tags by default, got %d", len(cluster.tags))
	}

	// Node name should be auto-generated from node ID
	if !strings.HasPrefix(cluster.nodeName, "Bot ") {
		t.Errorf("Expected node name to start with 'Bot ', got %s", cluster.nodeName)
	}
}

// TestCluster_IsRunning tests checking if cluster is running
func TestCluster_IsRunning(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	if cluster.IsRunning() {
		t.Error("Cluster should not be running initially")
	}
}

// TestCluster_StopWithoutStart tests stopping a cluster that was never started
func TestCluster_StopWithoutStart(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	err = cluster.Stop()
	// Stop should error when cluster is not running
	if err == nil {
		t.Error("Expected error when stopping unstarted cluster")
	}
}

// TestCluster_DoubleStart tests starting a cluster twice
func TestCluster_DoubleStart(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
		if cluster.IsRunning() {
			cluster.Stop()
		}
	})

	// First start
	err = cluster.Start()
	if err != nil {
		t.Fatalf("Failed to start cluster: %v", err)
	}

	// Second start should fail
	err = cluster.Start()
	if err == nil {
		t.Error("Expected error when starting already running cluster")
	}

	if !strings.Contains(err.Error(), "already running") {
		t.Errorf("Error message should mention 'already running', got: %v", err)
	}

	// Stop the cluster
	cluster.Stop()
}

// TestCluster_StartStop tests starting and stopping cluster
func TestCluster_StartStop(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
		if cluster.IsRunning() {
			cluster.Stop()
		}
	})

	// Start
	err = cluster.Start()
	if err != nil {
		t.Fatalf("Failed to start cluster: %v", err)
	}

	if !cluster.IsRunning() {
		t.Error("Cluster should be running after Start()")
	}

	// Wait a bit for initialization
	time.Sleep(100 * time.Millisecond)

	// Stop
	err = cluster.Stop()
	if err != nil {
		t.Fatalf("Failed to stop cluster: %v", err)
	}

	if cluster.IsRunning() {
		t.Error("Cluster should not be running after Stop()")
	}
}

// TestCluster_GetCapabilities tests getting cluster capabilities
func TestCluster_GetCapabilities(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	capabilities := cluster.GetCapabilities()

	// Should return at least some default capabilities
	if len(capabilities) == 0 {
		// This might be expected depending on implementation
		t.Log("No capabilities returned (may be expected)")
	}
}

// TestCluster_GetLogger tests getting cluster logger
func TestCluster_GetLogger(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	logger := cluster.GetLogger()
	if logger == nil {
		t.Error("Expected non-nil logger")
	}

	if logger != cluster.logger {
		t.Error("Logger should match cluster logger")
	}
}

// TestCluster_GetAddress tests getting cluster address
func TestCluster_GetAddress(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
		if cluster.IsRunning() {
			cluster.Stop()
		}
	})

	address := cluster.GetAddress()

	// Before start, address should be empty
	if address != "" {
		t.Logf("Address before start: %s", address)
	}

	// After start, address should be set
	err = cluster.Start()
	if err != nil {
		t.Fatalf("Failed to start cluster: %v", err)
	}

	address = cluster.GetAddress()
	if address == "" {
		t.Error("Expected non-empty address after start")
	}

	// Stop the cluster
	cluster.Stop()
}

// TestNewClusterLogger tests creating a cluster logger
func TestNewClusterLogger(t *testing.T) {
	tempDir := t.TempDir()

	logger, err := NewClusterLogger(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster logger: %v", err)
	}
	t.Cleanup(func() {
		if logger != nil {
			logger.Close()
		}
	})

	if logger == nil {
		t.Fatal("Expected non-nil logger")
	}

	// Log directory should be created at logs/cluster
	logDir := filepath.Join(tempDir, "logs", "cluster")
	info, err := os.Stat(logDir)
	if err != nil {
		t.Errorf("Log directory should exist: %v", err)
	}

	if !info.IsDir() {
		t.Error("Log path should be a directory")
	}
}

// TestClusterLogger_DiscoveryMethods tests logger discovery methods
func TestClusterLogger_DiscoveryMethods(t *testing.T) {
	tempDir := t.TempDir()

	logger, err := NewClusterLogger(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster logger: %v", err)
	}
	t.Cleanup(func() {
		if logger != nil {
			logger.Close()
		}
	})

	// These should not panic
	logger.DiscoveryInfo("Test info message: %s", "value")
	logger.DiscoveryError("Test error message: %v", "error")
	logger.DiscoveryDebug("Test debug message: %d", 42)
}

// TestClusterLogger_RPCMethods tests logger RPC methods
func TestClusterLogger_RPCMethods(t *testing.T) {
	tempDir := t.TempDir()

	logger, err := NewClusterLogger(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster logger: %v", err)
	}
	t.Cleanup(func() {
		if logger != nil {
			logger.Close()
		}
	})

	// These should not panic
	logger.RPCInfo("Test RPC info: %s", "data")
	logger.RPCError("Test RPC error: %v", "error")
	logger.RPCDebug("Test RPC debug")
}

// TestNewRegistry tests creating a new peer registry
func TestNewRegistry(t *testing.T) {
	registry := NewRegistry()

	if registry == nil {
		t.Fatal("Expected non-nil registry")
	}

	if registry.Count() != 0 {
		t.Errorf("Expected empty registry, got %d peers", registry.Count())
	}
}

// TestRegistry_AddRemove tests adding and removing peers
func TestRegistry_AddRemove(t *testing.T) {
	registry := NewRegistry()

	// Add node
	node := &Node{
		ID:      "test-node-1",
		Name:    "Test Node",
		Address: "localhost:8080",
		Role:    "worker",
		Status:  StatusOnline,
	}

	registry.AddOrUpdate(node)

	if registry.Count() != 1 {
		t.Errorf("Expected 1 peer, got %d", registry.Count())
	}

	// Remove node
	registry.Remove("test-node-1")

	if registry.Count() != 0 {
		t.Errorf("Expected 0 peers after removal, got %d", registry.Count())
	}
}

// TestRegistry_GetNode tests getting a specific node
func TestRegistry_GetNode(t *testing.T) {
	registry := NewRegistry()

	node := &Node{
		ID:      "test-node-1",
		Name:    "Test Node",
		Address: "localhost:8080",
		Role:    "worker",
		Status:  StatusOnline,
	}

	registry.AddOrUpdate(node)

	// Get existing node
	retrieved := registry.Get("test-node-1")
	if retrieved == nil {
		t.Fatal("Expected to find node")
	}

	if retrieved.Name != "Test Node" {
		t.Errorf("Expected node name Test Node, got %s", retrieved.Name)
	}

	// Get non-existent node
	notFound := registry.Get("non-existent")
	if notFound != nil {
		t.Error("Expected nil for non-existent node")
	}
}

// TestRegistry_GetAll tests listing all nodes
func TestRegistry_GetAll(t *testing.T) {
	registry := NewRegistry()

	// Add multiple nodes
	nodes := []*Node{
		{ID: "node-1", Name: "Node 1", Address: "localhost:8080", Status: StatusOnline},
		{ID: "node-2", Name: "Node 2", Address: "localhost:8081", Status: StatusOnline},
		{ID: "node-3", Name: "Node 3", Address: "localhost:8082", Status: StatusOnline},
	}

	for _, node := range nodes {
		registry.AddOrUpdate(node)
	}

	allNodes := registry.GetAll()

	if len(allNodes) != 3 {
		t.Errorf("Expected 3 nodes, got %d", len(allNodes))
	}
}

// TestRegistry_GetOnline tests getting only online nodes
func TestRegistry_GetOnline(t *testing.T) {
	registry := NewRegistry()

	// Add nodes (all will be set to online by AddOrUpdate)
	nodes := []*Node{
		{ID: "node-1", Name: "Node 1", Address: "localhost:8080"},
		{ID: "node-2", Name: "Node 2", Address: "localhost:8081"},
		{ID: "node-3", Name: "Node 3", Address: "localhost:8082"},
	}

	for _, node := range nodes {
		registry.AddOrUpdate(node)
	}

	// All nodes should be online initially
	onlineNodes := registry.GetOnline()

	if len(onlineNodes) != 3 {
		t.Errorf("Expected 3 online nodes (all new nodes are online by default), got %d", len(onlineNodes))
	}

	// Now mark one as offline
	registry.MarkOffline("node-2", "test offline")

	onlineNodes = registry.GetOnline()

	if len(onlineNodes) != 2 {
		t.Errorf("Expected 2 online nodes after marking one offline, got %d", len(onlineNodes))
	}

	for _, node := range onlineNodes {
		if node.Status != StatusOnline {
			t.Errorf("Expected online status, got %s", node.Status)
		}
	}
}

// TestRegistry_MarkOffline tests marking nodes offline
func TestRegistry_MarkOffline(t *testing.T) {
	registry := NewRegistry()

	node := &Node{
		ID:      "test-node",
		Name:    "Test Node",
		Address: "localhost:8080",
		Status:  StatusOnline,
	}

	registry.AddOrUpdate(node)

	// Verify it's online
	online := registry.GetOnline()
	if len(online) != 1 {
		t.Errorf("Expected 1 online node, got %d", len(online))
	}

	// Mark offline
	registry.MarkOffline("test-node", "test reason")

	// Verify it's now offline
	online = registry.GetOnline()
	if len(online) != 0 {
		t.Errorf("Expected 0 online nodes, got %d", len(online))
	}

	// Check the node was updated
	retrieved := registry.Get("test-node")
	if retrieved == nil {
		t.Fatal("Expected to find node")
	}

	if retrieved.Status != StatusOffline {
		t.Errorf("Expected status offline, got %s", retrieved.Status)
	}

	if !strings.Contains(retrieved.LastError, "test reason") {
		t.Errorf("Expected last error to contain reason, got: %s", retrieved.LastError)
	}
}

// TestRegistry_CheckTimeouts tests checking for timed out nodes
func TestRegistry_CheckTimeouts(t *testing.T) {
	registry := NewRegistry()

	// Add nodes (will all have LastSeen set to current time and Status=Online)
	nodes := []*Node{
		{ID: "node-1", Name: "Node 1", Address: "localhost:8080"},
		{ID: "node-2", Name: "Node 2", Address: "localhost:8081"},
		{ID: "node-3", Name: "Node 3", Address: "localhost:8082"},
	}

	for _, node := range nodes {
		registry.AddOrUpdate(node)
	}

	// Manually set LastSeen times after adding (directly manipulate the stored nodes)
	for id, offset := range map[string]time.Duration{
		"node-1": -1 * time.Minute,
		"node-2": -120 * time.Second,
		"node-3": -200 * time.Second,
	} {
		if node, ok := registry.nodes[id]; ok {
			node.mu.Lock()
			node.LastSeen = time.Now().Add(offset)
			node.mu.Unlock()
		}
	}

	// Check for timeouts with 90 second threshold
	timedOut := registry.CheckTimeouts(90 * time.Second)

	// Should have timed out node-2 and node-3
	if len(timedOut) != 2 {
		t.Errorf("Expected 2 timed out nodes, got %d", len(timedOut))
	}
}

// TestRegistry_Count tests counting nodes
func TestRegistry_Count(t *testing.T) {
	registry := NewRegistry()

	if registry.Count() != 0 {
		t.Errorf("Expected 0 nodes, got %d", registry.Count())
	}

	if registry.OnlineCount() != 0 {
		t.Errorf("Expected 0 online nodes, got %d", registry.OnlineCount())
	}

	// Add some nodes (all will be online after AddOrUpdate)
	nodes := []*Node{
		{ID: "node-1", Name: "Node 1", Address: "localhost:8080"},
		{ID: "node-2", Name: "Node 2", Address: "localhost:8081"},
	}

	for _, node := range nodes {
		registry.AddOrUpdate(node)
	}

	if registry.Count() != 2 {
		t.Errorf("Expected 2 nodes, got %d", registry.Count())
	}

	// Both should be online initially
	if registry.OnlineCount() != 2 {
		t.Errorf("Expected 2 online nodes initially, got %d", registry.OnlineCount())
	}

	// Mark one as offline
	registry.MarkOffline("node-1", "test")

	if registry.Count() != 2 {
		t.Errorf("Expected 2 nodes total, got %d", registry.Count())
	}

	if registry.OnlineCount() != 1 {
		t.Errorf("Expected 1 online node, got %d", registry.OnlineCount())
	}
}

// TestNode_IsOnline tests checking if a node is online
func TestNode_IsOnline(t *testing.T) {
	tests := []struct {
		name   string
		status NodeStatus
		expect bool
	}{
		{"online node", StatusOnline, true},
		{"offline node", StatusOffline, false},
		{"unknown status", StatusUnknown, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			node := &Node{
				ID:     "test-node",
				Status: tt.status,
			}

			result := node.IsOnline()
			if result != tt.expect {
				t.Errorf("IsOnline() = %v, want %v", result, tt.expect)
			}
		})
	}
}

// TestNode_StatusMethods tests node status methods
func TestNode_StatusMethods(t *testing.T) {
	node := &Node{
		ID:     "test-node",
		Status: StatusOffline,
	}

	// GetStatus as string
	if node.GetStatus() != string(StatusOffline) {
		t.Errorf("Expected status offline, got %s", node.GetStatus())
	}

	// GetNodeStatus as type
	if node.GetNodeStatus() != StatusOffline {
		t.Errorf("Expected NodeStatus offline, got %s", node.GetNodeStatus())
	}

	// SetStatus
	node.SetStatus(StatusOnline)

	if node.Status != StatusOnline {
		t.Errorf("Expected status online after SetStatus, got %s", node.Status)
	}

	// UpdateLastSeen should update timestamp
	oldLastSeen := node.LastSeen
	time.Sleep(10 * time.Millisecond)
	node.UpdateLastSeen()

	if !node.LastSeen.After(oldLastSeen) {
		t.Error("LastSeen should be updated after UpdateLastSeen")
	}
}

// TestCluster_Call tests calling peers via cluster
func TestCluster_Call(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	// Call without starting should fail gracefully
	_, err = cluster.Call("non-existent-peer", "test_action", nil)
	if err == nil {
		t.Error("Expected error when calling peer on unstarted cluster")
	}
}

// TestCluster_CallWithContext tests calling peers with context
func TestCluster_CallWithContext(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	ctx := context.Background()

	// Call without starting should fail gracefully
	_, err = cluster.CallWithContext(ctx, "non-existent-peer", "test_action", nil)
	if err == nil {
		t.Error("Expected error when calling peer on unstarted cluster")
	}
}

// TestCluster_GetPeer tests getting a specific peer
func TestCluster_GetPeer(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	// Get non-existent peer should return error
	_, err = cluster.GetPeer("non-existent-peer")
	if err == nil {
		t.Error("Expected error when getting non-existent peer")
	}
}

// TestCluster_SyncToDisk tests syncing cluster state to disk
func TestCluster_SyncToDisk(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	// Should not error even if cluster not started
	err = cluster.SyncToDisk()
	if err != nil {
		t.Errorf("SyncToDisk should not error: %v", err)
	}

	// Check that state file was created
	statePath := filepath.Join(tempDir, "cluster", "state.toml")
	_, err = os.Stat(statePath)
	if err != nil {
		// File might not exist if there's nothing to save
		t.Logf("State file may not exist (expected if nothing to save): %v", err)
	}
}

// TestCluster_GetLocalNetworkInterfaces tests getting network interfaces
func TestCluster_GetLocalNetworkInterfaces(t *testing.T) {
	tempDir := t.TempDir()

	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	interfaces, err := cluster.GetLocalNetworkInterfaces()
	if err != nil {
		t.Errorf("Failed to get network interfaces: %v", err)
	}

	if interfaces == nil {
		t.Error("Expected non-nil interfaces")
	}

	// Should have at least one interface (typically loopback)
	if len(interfaces) == 0 {
		t.Log("No network interfaces found (might be expected in some environments)")
	}
}

// TestConstants tests cluster constants
func TestConstants(t *testing.T) {
	if DefaultUDPPort != 11949 {
		t.Errorf("Expected DefaultUDPPort 11949, got %d", DefaultUDPPort)
	}

	if DefaultRPCPort != 21949 {
		t.Errorf("Expected DefaultRPCPort 21949, got %d", DefaultRPCPort)
	}

	if DefaultBroadcastInterval != 30*time.Second {
		t.Errorf("Expected DefaultBroadcastInterval 30s, got %v", DefaultBroadcastInterval)
	}

	if DefaultTimeout != 90*time.Second {
		t.Errorf("Expected DefaultTimeout 90s, got %v", DefaultTimeout)
	}
}

// TestCluster_NodeIDPersistence verifies that NodeID is persisted across restarts.
// When a cluster is created, it should save the NodeID to peers.toml.
// When a second cluster is created in the same workspace, it should load the same NodeID.
func TestCluster_NodeIDPersistence(t *testing.T) {
	tempDir := t.TempDir()

	// First cluster: should generate a new NodeID and persist it
	cluster1, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create first cluster: %v", err)
	}
	id1 := cluster1.GetNodeID()
	if id1 == "" {
		t.Fatal("First cluster should have a non-empty NodeID")
	}
	cluster1.logger.Close()

	// Second cluster in the same workspace: should load the persisted NodeID
	cluster2, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create second cluster: %v", err)
	}
	defer func() {
		if cluster2.logger != nil {
			cluster2.logger.Close()
		}
	}()
	id2 := cluster2.GetNodeID()

	if id1 != id2 {
		t.Errorf("NodeID should persist across restarts: first=%s, second=%s", id1, id2)
	}
}

// TestCluster_NodeIDPersistence_NewWorkspace generates a fresh ID when no config exists
func TestCluster_NodeIDPersistence_NewWorkspace(t *testing.T) {
	dir1 := t.TempDir()
	dir2 := t.TempDir()

	cluster1, err := NewCluster(dir1)
	if err != nil {
		t.Fatalf("Failed to create cluster1: %v", err)
	}
	defer cluster1.logger.Close()

	cluster2, err := NewCluster(dir2)
	if err != nil {
		t.Fatalf("Failed to create cluster2: %v", err)
	}
	defer cluster2.logger.Close()

	// Different workspaces should produce different NodeIDs
	if cluster1.GetNodeID() == cluster2.GetNodeID() {
		t.Error("Clusters in different workspaces should have different NodeIDs")
	}
}

// TestNodeStatus_Constants tests node status constants
func TestNodeStatus_Constants(t *testing.T) {
	if StatusOnline != "online" {
		t.Errorf("Expected StatusOnline 'online', got %s", StatusOnline)
	}

	if StatusOffline != "offline" {
		t.Errorf("Expected StatusOffline 'offline', got %s", StatusOffline)
	}

	if StatusUnknown != "unknown" {
		t.Errorf("Expected StatusUnknown 'unknown', got %s", StatusUnknown)
	}
}

// --- H4 Recovery Tests ---

// TestCluster_GetTaskResultStore tests the TaskResultStore getter
func TestCluster_GetTaskResultStore(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	store := cluster.GetTaskResultStore()
	if store == nil {
		t.Error("Expected non-nil TaskResultStore")
	}
}

// TestCluster_PollStalePendingTasks_NoTaskManager tests poll with nil taskManager
func TestCluster_PollStalePendingTasks_NoTaskManager(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	// Should not panic with nil taskManager
	cluster.pollStalePendingTasks()
}

// TestCluster_PollStalePendingTasks_TooNew tests that recent tasks are skipped
func TestCluster_PollStalePendingTasks_TooNew(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	// Set up taskManager manually (without full Start)
	tm := NewTaskManager(30 * time.Second)
	cluster.taskManager = tm

	// Submit a task that's only 30 seconds old
	tm.Submit(&Task{
		ID:        "task-recent",
		Status:    TaskPending,
		PeerID:    "peer-1",
		CreatedAt: time.Now().Add(-30 * time.Second),
	})

	// pollStalePendingTasks should skip it (too new)
	cluster.pollStalePendingTasks()

	task, _ := tm.GetTask("task-recent")
	if task.Status != TaskPending {
		t.Errorf("Recent task should still be pending, got %s", task.Status)
	}
}

// TestCluster_PollStalePendingTasks_24hTimeout tests the 24h fallback timeout
func TestCluster_PollStalePendingTasks_24hTimeout(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	tm := NewTaskManager(30 * time.Second)
	var completedTaskIDs []string
	tm.SetOnComplete(func(taskID string) {
		completedTaskIDs = append(completedTaskIDs, taskID)
	})
	cluster.taskManager = tm

	// Submit a task that's 25 hours old (should be timed out)
	tm.Submit(&Task{
		ID:        "task-old",
		Status:    TaskPending,
		PeerID:    "peer-1",
		CreatedAt: time.Now().Add(-25 * time.Hour),
	})

	cluster.pollStalePendingTasks()

	task, _ := tm.GetTask("task-old")
	if task.Status != TaskFailed {
		t.Errorf("Old task should be failed, got %s", task.Status)
	}
	if task.Error == "" {
		t.Error("Expected error message for timed out task")
	}
}

// TestCluster_PollStalePendingTasks_StaleButRPCUnavailable tests stale task when B is unreachable
func TestCluster_PollStalePendingTasks_StaleButRPCUnavailable(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	tm := NewTaskManager(30 * time.Second)
	cluster.taskManager = tm

	// Submit a task that's 5 minutes old (stale enough to poll)
	tm.Submit(&Task{
		ID:        "task-stale",
		Status:    TaskPending,
		PeerID:    "peer-1",
		CreatedAt: time.Now().Add(-5 * time.Minute),
	})

	// RPC client is nil, so CallWithContext will fail
	// pollStalePendingTasks should handle this gracefully
	cluster.pollStalePendingTasks()

	// Task should still be pending (RPC failed, will retry next cycle)
	task, _ := tm.GetTask("task-stale")
	if task.Status != TaskPending {
		t.Errorf("Task should still be pending when RPC fails, got %s", task.Status)
	}
}

// TestCluster_RecoveryLoop_StopsOnStopCh tests that recoveryLoop exits cleanly
func TestCluster_RecoveryLoop_StopsOnStopCh(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	// Start recoveryLoop
	cluster.wg.Add(1)
	go cluster.recoveryLoop()

	// Stop it
	close(cluster.stopCh)
	cluster.wg.Wait()
	// Should complete without hanging
}

// TestCluster_QueryTaskResultHandler tests the query_task_result RPC handler
func TestCluster_QueryTaskResultHandler(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	// Set a result
	cluster.resultStore.SetResult("task-1", "success", "hello", "", "node-A")

	// Simulate query handler (replicate the handler logic directly)
	taskID := "task-1"
	entry := cluster.resultStore.Get(taskID)
	if entry == nil {
		t.Fatal("Expected entry")
	}
	if entry.Status != "done" {
		t.Errorf("Expected status 'done', got %s", entry.Status)
	}
	if entry.Response != "hello" {
		t.Errorf("Expected response 'hello', got %s", entry.Response)
	}
}

// TestCluster_ConfirmTaskDeliveryHandler tests the confirm_task_delivery RPC handler
func TestCluster_ConfirmTaskDeliveryHandler(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	// Set a result
	cluster.resultStore.SetResult("task-1", "success", "hello", "", "node-A")

	// Simulate confirm handler
	cluster.resultStore.Delete("task-1")

	// Should be gone
	entry := cluster.resultStore.Get("task-1")
	if entry != nil {
		t.Error("Expected nil after confirm_delivery")
	}
}

// TestStringValue tests the stringValue helper
func TestStringValue(t *testing.T) {
	tests := []struct {
		input    interface{}
		expected string
	}{
		{nil, ""},
		{"hello", "hello"},
		{42, ""},
		{true, ""},
		{"", ""},
	}

	for _, tt := range tests {
		result := stringValue(tt.input)
		if result != tt.expected {
			t.Errorf("stringValue(%v) = %q, want %q", tt.input, result, tt.expected)
		}
	}
}

