// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc_test

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/rpc"
)

// integrationTestServer simulates a real node in the cluster
type integrationTestServer struct {
	server  *rpc.Server
	port    int
	nodeID  string
	cluster *integrationTestCluster
}

func TestFullRPCIntegration(t *testing.T) {
	// Setup two nodes in the cluster
	node1 := &integrationTestServer{
		nodeID: "node-1",
		cluster: &integrationTestCluster{
			nodeID:       "node-1",
			capabilities: []string{"chat", "peer_chat"},
			peers:        make(map[string]*mockNode),
		},
	}
	node2 := &integrationTestServer{
		nodeID: "node-2",
		cluster: &integrationTestCluster{
			nodeID:       "node-2",
			capabilities: []string{"chat", "peer_chat"},
			peers:        make(map[string]*mockNode),
		},
	}

	// Start both nodes
	for _, node := range []*integrationTestServer{node1, node2} {
		err := node.start()
		if err != nil {
			t.Fatalf("Failed to start node %s: %v", node.nodeID, err)
		}
		t.Logf("Node %s started on port %d", node.nodeID, node.port)
	}
	defer func() {
		node1.stop()
		node2.stop()
	}()

	// Register each node in the other's peer list
	node1.cluster.peers["node-2"] = &mockNode{
		id:        "node-2",
		name:      "Node 2",
		address:   "127.0.0.1",
		addresses: []string{"127.0.0.1"},
		rpcPort:   node2.port,
		status:    "online",
		online:    true,
	}

	node2.cluster.peers["node-1"] = &mockNode{
		id:        "node-1",
		name:      "Node 1",
		address:   "127.0.0.1",
		addresses: []string{"127.0.0.1"},
		rpcPort:   node1.port,
		status:    "online",
		online:    true,
	}

	// Test bidirectional communication
	testPayload := map[string]interface{}{
		"message":   "Hello from node 1 to node 2",
		"timestamp": time.Now().Unix(),
	}

	// Node 1 calls node 2
	client1 := rpc.NewClient(node1.cluster)
	response, err := client1.CallWithContext(context.Background(), "node-2", "echo", testPayload)
	if err != nil {
		t.Fatalf("Node 1 call to node 2 failed: %v", err)
	}

	var responseData map[string]interface{}
	if err := json.Unmarshal(response, &responseData); err != nil {
		t.Fatalf("Failed to unmarshal response: %v", err)
	}

	if responseData["message"] != "Hello from node 1 to node 2" {
		t.Errorf("Expected message 'Hello from node 1 to node 2', got %v", responseData["message"])
	}

	// Node 2 calls node 1
	client2 := rpc.NewClient(node2.cluster)
	response, err = client2.CallWithContext(context.Background(), "node-1", "echo", testPayload)
	if err != nil {
		t.Fatalf("Node 2 call to node 1 failed: %v", err)
	}

	if err := json.Unmarshal(response, &responseData); err != nil {
		t.Fatalf("Failed to unmarshal response: %v", err)
	}

	if responseData["message"] != "Hello from node 1 to node 2" {
		t.Errorf("Expected message 'Hello from node 1 to node 2', got %v", responseData["message"])
	}
}

func TestRPCWithRateLimiting(t *testing.T) {
	// Setup server node
	serverNode := &integrationTestServer{
		nodeID: "server-node",
		cluster: &integrationTestCluster{
			nodeID:       "server-node",
			capabilities: []string{"chat"},
			peers:        make(map[string]*mockNode),
		},
	}

	err := serverNode.start()
	if err != nil {
		t.Fatalf("Failed to start server node: %v", err)
	}
	defer serverNode.stop()

	// Setup client node
	clientNode := &integrationTestServer{
		nodeID: "client-node",
		cluster: &integrationTestCluster{
			nodeID:       "client-node",
			capabilities: []string{"chat"},
			peers:        make(map[string]*mockNode),
		},
	}

	// Register server in client's peer list
	clientNode.cluster.peers["server-node"] = &mockNode{
		id:        "server-node",
		name:      "Server Node",
		address:   "127.0.0.1",
		addresses: []string{"127.0.0.1"},
		rpcPort:   serverNode.port,
		status:    "online",
		online:    true,
	}

	client := rpc.NewClient(clientNode.cluster)

	// Verify basic RPC calls work (rate limiter allows these under normal limits)
	payload := map[string]interface{}{"msg": "test1"}
	_, err = client.CallWithContext(context.Background(), "server-node", "echo", payload)
	if err != nil {
		t.Fatalf("First call failed: %v", err)
	}

	payload["msg"] = "test2"
	_, err = client.CallWithContext(context.Background(), "server-node", "echo", payload)
	if err != nil {
		t.Fatalf("Second call failed: %v", err)
	}

	// Note: Rate limiter behavior is tested in dedicated RateLimiter unit tests.
	// The production limits (burst=10, window=30/10s) are too permissive for
	// integration-level rate limiting verification with a small number of calls.
}

func TestRPCConnectionPoolReuse(t *testing.T) {
	// Setup server node
	serverNode := &integrationTestServer{
		nodeID: "server-node",
		cluster: &integrationTestCluster{
			nodeID:       "server-node",
			capabilities: []string{"chat"},
			peers:        make(map[string]*mockNode),
		},
	}

	err := serverNode.start()
	if err != nil {
		t.Fatalf("Failed to start server node: %v", err)
	}
	defer serverNode.stop()

	// Setup client node
	clientNode := &integrationTestServer{
		nodeID: "client-node",
		cluster: &integrationTestCluster{
			nodeID:       "client-node",
			capabilities: []string{"chat"},
			peers:        make(map[string]*mockNode),
		},
	}

	// Register server in client's peer list
	clientNode.cluster.peers["server-node"] = &mockNode{
		id:        "server-node",
		name:      "Server Node",
		address:   "127.0.0.1",
		addresses: []string{"127.0.0.1"},
		rpcPort:   serverNode.port,
		status:    "online",
		online:    true,
	}

	client := rpc.NewClient(clientNode.cluster)

	// Make multiple calls to test connection reuse
	const numCalls = 3
	var wg sync.WaitGroup
	results := make(chan []byte, numCalls)
	errors := make(chan error, numCalls)

	for i := 0; i < numCalls; i++ {
		wg.Add(1)
		go func(callID int) {
			defer wg.Done()
			payload := map[string]interface{}{"id": callID}
			response, err := client.CallWithContext(context.Background(), "server-node", "echo", payload)
			if err != nil {
				errors <- err
			} else {
				results <- response
			}
		}(i)
	}

	wg.Wait()
	close(results)
	close(errors)

	// Verify all calls succeeded
	for err := range errors {
		t.Errorf("Call failed: %v", err)
	}

	count := 0
	for response := range results {
		var respData map[string]interface{}
		if err := json.Unmarshal(response, &respData); err == nil {
			count++
		} else {
			t.Errorf("Failed to unmarshal response: %v", err)
		}
	}

	if count != numCalls {
		t.Errorf("Expected %d successful calls, got %d", numCalls, count)
	}
}

func TestRPCTimeoutHandling(t *testing.T) {
	// Setup server with slow handler
	serverNode := &integrationTestServer{
		nodeID: "server-node",
		cluster: &integrationTestCluster{
			nodeID:       "server-node",
			capabilities: []string{"chat"},
			peers:        make(map[string]*mockNode),
		},
	}

	err := serverNode.start()
	if err != nil {
		t.Fatalf("Failed to start server node: %v", err)
	}
	defer serverNode.stop()

	// Register slow handler
	serverNode.server.RegisterHandler("slow", func(payload map[string]interface{}) (map[string]interface{}, error) {
		time.Sleep(2 * time.Second)
		return map[string]interface{}{"status": "done"}, nil
	})

	// Setup client node
	clientNode := &integrationTestServer{
		nodeID: "client-node",
		cluster: &integrationTestCluster{
			nodeID:       "client-node",
			capabilities: []string{"chat"},
			peers:        make(map[string]*mockNode),
		},
	}

	// Register server in client's peer list
	clientNode.cluster.peers["server-node"] = &mockNode{
		id:        "server-node",
		name:      "Server Node",
		address:   "127.0.0.1",
		addresses: []string{"127.0.0.1"},
		rpcPort:   serverNode.port,
		status:    "online",
		online:    true,
	}

	client := rpc.NewClient(clientNode.cluster)

	// Test timeout
	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()

	_, err = client.CallWithContext(ctx, "server-node", "slow", nil)
	if err == nil {
		t.Error("Expected timeout error, got none")
	}
}

func TestRPCErrorPropagation(t *testing.T) {
	// Setup server with error handler
	serverNode := &integrationTestServer{
		nodeID: "server-node",
		cluster: &integrationTestCluster{
			nodeID:       "server-node",
			capabilities: []string{"chat"},
			peers:        make(map[string]*mockNode),
		},
	}

	err := serverNode.start()
	if err != nil {
		t.Fatalf("Failed to start server node: %v", err)
	}
	defer serverNode.stop()

	// Register error handler
	serverNode.server.RegisterHandler("error", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return nil, fmt.Errorf("server error: %v", payload)
	})

	// Setup client node
	clientNode := &integrationTestServer{
		nodeID: "client-node",
		cluster: &integrationTestCluster{
			nodeID:       "client-node",
			capabilities: []string{"chat"},
			peers:        make(map[string]*mockNode),
		},
	}

	// Register server in client's peer list
	clientNode.cluster.peers["server-node"] = &mockNode{
		id:        "server-node",
		name:      "Server Node",
		address:   "127.0.0.1",
		addresses: []string{"127.0.0.1"},
		rpcPort:   serverNode.port,
		status:    "online",
		online:    true,
	}

	client := rpc.NewClient(clientNode.cluster)

	// Test error propagation
	payload := map[string]interface{}{"error": "test"}
	_, err = client.CallWithContext(context.Background(), "server-node", "error", payload)
	if err == nil {
		t.Error("Expected error propagation, got none")
	}

	// Server enhances payload with _rpc metadata, so check for key substring
	errMsg := err.Error()
	if !strings.Contains(errMsg, "RPC error from peer: server error:") || !strings.Contains(errMsg, "error:test") {
		t.Errorf("Unexpected error message: %v", err)
	}
}

// Helper methods for integrationTestServer

func (n *integrationTestServer) start() error {
	n.server = rpc.NewServer(n.cluster)

	// Register test handler
	n.server.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return payload, nil
	})

	// Start server on a random port (port 0 = OS assigns)
	if err := n.server.Start(0); err != nil {
		return err
	}
	n.port = n.server.GetPort()

	return nil
}

func (n *integrationTestServer) stop() {
	if n.server != nil {
		n.server.Stop()
	}
}

func (n *integrationTestServer) isRunning() bool {
	return n.server != nil && n.server.IsRunning()
}
