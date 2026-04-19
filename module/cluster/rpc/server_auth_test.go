// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc

import (
	"context"
	"fmt"
	"net"
	"strings"
	"testing"
	"time"
)

// mockCluster implements Cluster interface for testing
type mockCluster struct {
	nodeID string
	logs   []string
}

func (m *mockCluster) GetNodeID() string {
	return m.nodeID
}

func (m *mockCluster) GetRegistry() interface{} {
	return nil
}

func (m *mockCluster) GetAddress() string {
	return "127.0.0.1"
}

func (m *mockCluster) GetCapabilities() []string {
	return []string{"test"}
}

func (m *mockCluster) GetOnlinePeers() []interface{} {
	return []interface{}{}
}

func (m *mockCluster) GetActionsSchema() []interface{} {
	return []interface{}{}
}

func (m *mockCluster) GetPeer(peerID string) (interface{}, error) {
	return nil, fmt.Errorf("not found")
}

func (m *mockCluster) GetLocalNetworkInterfaces() ([]LocalNetworkInterface, error) {
	return []LocalNetworkInterface{}, nil
}

func (m *mockCluster) LogRPCInfo(format string, args ...interface{}) {
	m.logs = append(m.logs, fmt.Sprintf("[INFO] "+format, args...))
}

func (m *mockCluster) LogRPCError(format string, args ...interface{}) {
	m.logs = append(m.logs, fmt.Sprintf("[ERROR] "+format, args...))
}

func (m *mockCluster) LogRPCDebug(format string, args ...interface{}) {
	m.logs = append(m.logs, fmt.Sprintf("[DEBUG] "+format, args...))
}

func (m *mockCluster) CallWithContext(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	return []byte(`{"status":"ok"}`), nil
}

func (m *mockCluster) GetTaskResultStorer() TaskResultStorer { return nil }

// TestRPCServer_Authentication tests RPC server authentication
func TestRPCServer_Authentication(t *testing.T) {
	tests := []struct {
		name          string
		serverToken   string
		clientToken   string
		expectSuccess bool
		description   string
	}{
		{
			name:          "valid_token",
			serverToken:   "secret-token-123",
			clientToken:   "secret-token-123",
			expectSuccess: true,
			description:   "Valid token should authenticate successfully",
		},
		{
			name:          "invalid_token",
			serverToken:   "secret-token-123",
			clientToken:   "wrong-token",
			expectSuccess: false,
			description:   "Invalid token should be rejected",
		},
		{
			name:          "empty_token",
			serverToken:   "secret-token-123",
			clientToken:   "",
			expectSuccess: false,
			description:   "Empty token should be rejected when auth is enabled",
		},
		{
			name:          "no_auth_configured",
			serverToken:   "",
			clientToken:   "anything",
			expectSuccess: true,
			description:   "No auth should allow any connection",
		},
		{
			name:          "token_with_whitespace",
			serverToken:   "secret-token-123",
			clientToken:   "  secret-token-123  ",
			expectSuccess: true,
			description:   "Token with whitespace should be trimmed and match",
		},
		{
			name:          "token_with_newline",
			serverToken:   "secret-token-123",
			clientToken:   "secret-token-123\n",
			expectSuccess: true,
			description:   "Token with newline should be trimmed and match",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Create mock cluster
			mockCluster := &mockCluster{nodeID: "test-node"}

			// Create RPC server
			server := NewServer(mockCluster)
			if tt.serverToken != "" {
				server.SetAuthToken(tt.serverToken)
			}

			// Start server on random port
			if err := server.Start(0); err != nil {
				t.Fatalf("Failed to start server: %v", err)
			}
			defer server.Stop()

			port := server.GetPort()

			// Connect client
			conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", port), 5*time.Second)
			if err != nil {
				t.Fatalf("Failed to connect to server: %v", err)
			}
			defer conn.Close()

			// Send authentication token (if auth is configured)
			if tt.serverToken != "" {
				// Set write deadline
				conn.SetWriteDeadline(time.Now().Add(5 * time.Second))

				// Send token
				_, err = conn.Write([]byte(tt.clientToken + "\n"))
				if err != nil {
					t.Fatalf("Failed to send token: %v", err)
				}

				// Reset deadline
				conn.SetWriteDeadline(time.Time{})
			}

			// Wait a bit for server to process
			time.Sleep(100 * time.Millisecond)

			// Try to send a test message
			// If authentication failed, connection should be closed
			conn.SetWriteDeadline(time.Now().Add(2 * time.Second))
			testMessage := "test\r\n"
			_, err = conn.Write([]byte(testMessage))

			if tt.expectSuccess {
				// Should succeed
				if err != nil {
					t.Errorf("Expected connection to remain open, but got error: %v", err)
				}
			} else {
				// Should fail (connection closed by server)
				if err == nil {
					// Wait a bit more to see if connection gets closed
					time.Sleep(200 * time.Millisecond)
					conn.SetWriteDeadline(time.Now().Add(1 * time.Second))
					_, err = conn.Write([]byte("another test\r\n"))
					if err == nil {
						t.Errorf("Expected connection to be closed by server, but it remains open")
					}
				}
			}
		})
	}
}

// TestRPCServer_AuthTimeout tests authentication timeout
func TestRPCServer_AuthTimeout(t *testing.T) {
	mockCluster := &mockCluster{nodeID: "test-node"}
	server := NewServer(mockCluster)
	server.SetAuthToken("secret-token")

	// Start server
	if err := server.Start(0); err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer server.Stop()

	port := server.GetPort()

	// Connect client but don't send token
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", port), 5*time.Second)
	if err != nil {
		t.Fatalf("Failed to connect to server: %v", err)
	}
	defer conn.Close()

	// Wait for auth timeout (10 seconds + buffer)
	// The server should close the connection after 10 seconds
	time.Sleep(11 * time.Second)

	// Try to read from connection - should get EOF (connection closed by server)
	conn.SetReadDeadline(time.Now().Add(1 * time.Second))
	buf := make([]byte, 1024)
	_, err = conn.Read(buf)
	if err == nil {
		// If read succeeds, connection is still open
		t.Errorf("Expected connection to be closed after auth timeout, but it remains open")
	} else {
		// Check if it's an EOF or connection reset error
		if !strings.Contains(err.Error(), "EOF") &&
		   !strings.Contains(err.Error(), "closed") &&
		   !strings.Contains(err.Error(), "reset") &&
		   !strings.Contains(err.Error(), "deadline exceeded") {
			t.Logf("Connection error: %v (this is expected)", err)
		}
	}
}

// TestRPCServer_AuthenticationConcurrent tests concurrent connections with auth
// Note: This test is simplified to avoid timing issues
func TestRPCServer_AuthenticationConcurrent(t *testing.T) {
	mockCluster := &mockCluster{nodeID: "test-node"}
	server := NewServer(mockCluster)
	server.SetAuthToken("secret-token")

	// Start server
	if err := server.Start(0); err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer server.Stop()

	port := server.GetPort()

	// Test 5 concurrent valid connections
	numClients := 5
	results := make(chan error, numClients)

	for i := 0; i < numClients; i++ {
		go func() {
			conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", port), 5*time.Second)
			if err != nil {
				results <- err
				return
			}
			defer conn.Close()

			// Send valid token
			conn.SetWriteDeadline(time.Now().Add(5 * time.Second))
			_, err = conn.Write([]byte("secret-token\n"))
			if err != nil {
				results <- err
				return
			}

			// Wait a bit for server to process
			time.Sleep(100 * time.Millisecond)

			// Try to send data
			conn.SetWriteDeadline(time.Now().Add(2 * time.Second))
			_, err = conn.Write([]byte("test\r\n"))
			results <- err
		}()
	}

	// Collect results
	successCount := 0
	for i := 0; i < numClients; i++ {
		if <-results == nil {
			successCount++
		}
	}

	// All valid connections should succeed
	if successCount != numClients {
		t.Errorf("Expected all %d valid connections to succeed, but only %d did", numClients, successCount)
	}
}

// TestRPCServer_SetAuthToken tests SetAuthToken method
func TestRPCServer_SetAuthToken(t *testing.T) {
	mockCluster := &mockCluster{nodeID: "test-node"}
	server := NewServer(mockCluster)

	// Initial token should be empty
	server.mu.RLock()
	initialToken := server.authToken
	server.mu.RUnlock()

	if initialToken != "" {
		t.Errorf("Expected initial token to be empty, got: %s", initialToken)
	}

	// Set token
	testToken := "test-token-456"
	server.SetAuthToken(testToken)

	// Verify token is set
	server.mu.RLock()
	setToken := server.authToken
	server.mu.RUnlock()

	if setToken != testToken {
		t.Errorf("Expected token to be %s, got: %s", testToken, setToken)
	}

	// Update token
	newToken := "new-token-789"
	server.SetAuthToken(newToken)

	// Verify token is updated
	server.mu.RLock()
	updatedToken := server.authToken
	server.mu.RUnlock()

	if updatedToken != newToken {
		t.Errorf("Expected token to be %s, got: %s", newToken, updatedToken)
	}
}

// TestRPCServer_AuthenticationLogging tests authentication logging
func TestRPCServer_AuthenticationLogging(t *testing.T) {
	tests := []struct {
		name           string
		serverToken    string
		clientToken    string
		expectLog      string
		expectLogLevel string
	}{
		{
			name:           "successful_auth",
			serverToken:    "secret",
			clientToken:    "secret",
			expectLog:      "Authenticated",
			expectLogLevel: "INFO",
		},
		{
			name:           "failed_auth",
			serverToken:    "secret",
			clientToken:    "wrong",
			expectLog:      "Unauthorized",
			expectLogLevel: "ERROR",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			mockCluster := &mockCluster{nodeID: "test-node"}
			server := NewServer(mockCluster)
			server.SetAuthToken(tt.serverToken)

			// Start server
			if err := server.Start(0); err != nil {
				t.Fatalf("Failed to start server: %v", err)
			}
			defer server.Stop()

			port := server.GetPort()

			// Connect and send token
			conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", port), 5*time.Second)
			if err != nil {
				t.Fatalf("Failed to connect: %v", err)
			}
			defer conn.Close()

			// Clear previous logs
			mockCluster.logs = []string{}

			// Send token
			conn.SetWriteDeadline(time.Now().Add(5 * time.Second))
			conn.Write([]byte(tt.clientToken + "\n"))

			// Wait for server to process
			time.Sleep(200 * time.Millisecond)

			// Check logs
			foundLog := false
			for _, log := range mockCluster.logs {
				if strings.Contains(log, tt.expectLog) && strings.Contains(log, tt.expectLogLevel) {
					foundLog = true
					break
				}
			}

			if !foundLog {
				t.Errorf("Expected to find log containing '%s' with level '%s', got logs: %v",
					tt.expectLog, tt.expectLogLevel, mockCluster.logs)
			}
		})
	}
}

// =====================================================================
// P2P RPC Auth Tests — client.SetAuthToken + server.SetAuthToken
// =====================================================================

// p2pCluster implements Cluster with a peer registry for P2P RPC tests.
type p2pCluster struct {
	nodeID       string
	capabilities []string
	peers        map[string]*p2pNode
	logs         []string
}

type p2pNode struct {
	id        string
	name      string
	address   string
	addresses []string
	rpcPort   int
	online    bool
}

func (n *p2pNode) GetID() string             { return n.id }
func (n *p2pNode) GetName() string           { return n.name }
func (n *p2pNode) GetAddress() string        { return n.address }
func (n *p2pNode) GetAddresses() []string    { return n.addresses }
func (n *p2pNode) GetRPCPort() int           { return n.rpcPort }
func (n *p2pNode) GetCapabilities() []string { return nil }
func (n *p2pNode) GetStatus() string         { return "online" }
func (n *p2pNode) IsOnline() bool            { return n.online }

func newP2PCluster(nodeID string) *p2pCluster {
	return &p2pCluster{
		nodeID:       nodeID,
		capabilities: []string{"test"},
		peers:        make(map[string]*p2pNode),
	}
}

func (c *p2pCluster) GetRegistry() interface{}        { return nil }
func (c *p2pCluster) GetNodeID() string               { return c.nodeID }
func (c *p2pCluster) GetAddress() string              { return "127.0.0.1" }
func (c *p2pCluster) GetCapabilities() []string       { return c.capabilities }
func (c *p2pCluster) GetOnlinePeers() []interface{}   { return nil }
func (c *p2pCluster) GetActionsSchema() []interface{} { return nil }
func (c *p2pCluster) LogRPCInfo(msg string, args ...interface{}) {
	c.logs = append(c.logs, fmt.Sprintf("[INFO] "+msg, args...))
}
func (c *p2pCluster) LogRPCError(msg string, args ...interface{}) {
	c.logs = append(c.logs, fmt.Sprintf("[ERROR] "+msg, args...))
}
func (c *p2pCluster) LogRPCDebug(msg string, args ...interface{}) {
	c.logs = append(c.logs, fmt.Sprintf("[DEBUG] "+msg, args...))
}
func (c *p2pCluster) GetPeer(peerID string) (interface{}, error) {
	if p, ok := c.peers[peerID]; ok {
		return p, nil
	}
	return nil, fmt.Errorf("peer not found: %s", peerID)
}
func (c *p2pCluster) GetLocalNetworkInterfaces() ([]LocalNetworkInterface, error) {
	return []LocalNetworkInterface{{IP: "127.0.0.1", Mask: "255.0.0.0"}}, nil
}
func (c *p2pCluster) CallWithContext(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	return nil, fmt.Errorf("not implemented")
}
func (c *p2pCluster) GetTaskResultStorer() TaskResultStorer { return nil }

// TestP2P_RPC_ClientServerAuth_SameToken verifies that when both client and server
// have the same auth token, RPC communication succeeds end-to-end.
// This is the bug fix verification: Client.SetAuthToken was missing before.
func TestP2P_RPC_ClientServerAuth_SameToken(t *testing.T) {
	token := "p2p-shared-secret"

	// Create server (Node B)
	clusterB := newP2PCluster("node-B")
	serverB := NewServer(clusterB)
	serverB.SetAuthToken(token)

	// Register a handler
	serverB.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{
			"status":  "ok",
			"echo_id": payload["id"],
		}, nil
	})

	if err := serverB.Start(0); err != nil {
		t.Fatalf("Failed to start serverB: %v", err)
	}
	defer serverB.Stop()

	portB := serverB.GetPort()

	// Create client (Node A) and set the SAME token
	clusterA := newP2PCluster("node-A")
	clusterA.peers["node-B"] = &p2pNode{
		id:        "node-B",
		name:      "Node B",
		address:   fmt.Sprintf("127.0.0.1:%d", portB),
		addresses: []string{fmt.Sprintf("127.0.0.1:%d", portB)},
		rpcPort:   portB,
		online:    true,
	}

	clientA := NewClient(clusterA)
	clientA.SetAuthToken(token) // THIS is the line that was missing before the fix

	// Make RPC call from A to B
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	resp, err := clientA.CallWithContext(ctx, "node-B", "echo", map[string]interface{}{
		"id":      "test-123",
		"message": "hello from A",
	})
	if err != nil {
		t.Fatalf("RPC call failed: %v", err)
	}

	// Verify response
	if string(resp) == "" {
		t.Error("Expected non-empty response")
	}

	// Verify server logged successful auth
	found := false
	for _, log := range clusterB.logs {
		if strings.Contains(log, "Authenticated") {
			found = true
			break
		}
	}
	if !found {
		t.Error("Server should have logged 'Authenticated' for successful auth")
	}
}

// TestP2P_RPC_ClientServerAuth_WrongToken verifies that when client has a
// different token than server, the RPC connection is rejected.
func TestP2P_RPC_ClientServerAuth_WrongToken(t *testing.T) {
	// Create server with token "correct-token"
	clusterB := newP2PCluster("node-B")
	serverB := NewServer(clusterB)
	serverB.SetAuthToken("correct-token")

	serverB.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{"status": "ok"}, nil
	})

	if err := serverB.Start(0); err != nil {
		t.Fatalf("Failed to start serverB: %v", err)
	}
	defer serverB.Stop()

	portB := serverB.GetPort()

	// Create client with WRONG token
	clusterA := newP2PCluster("node-A")
	clusterA.peers["node-B"] = &p2pNode{
		id:        "node-B",
		name:      "Node B",
		address:   fmt.Sprintf("127.0.0.1:%d", portB),
		addresses: []string{fmt.Sprintf("127.0.0.1:%d", portB)},
		rpcPort:   portB,
		online:    true,
	}

	clientA := NewClient(clusterA)
	clientA.SetAuthToken("wrong-token") // Different from server's token

	// Make RPC call — should fail
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	_, err := clientA.CallWithContext(ctx, "node-B", "echo", map[string]interface{}{
		"message": "should fail",
	})
	if err == nil {
		t.Error("Expected RPC call to fail with wrong token, but it succeeded")
	}

	// Verify server logged unauthorized
	found := false
	for _, log := range clusterB.logs {
		if strings.Contains(log, "Unauthorized") || strings.Contains(log, "Failed to read auth") {
			found = true
			break
		}
	}
	if !found {
		t.Error("Server should have logged auth rejection")
	}
}

// TestP2P_RPC_ClientNoToken_ServerHasToken verifies that when server requires
// auth but client has no token, the connection is rejected.
// This simulates the original BUG: cluster.go only called rpcServer.SetAuthToken
// but not rpcClient.SetAuthToken.
func TestP2P_RPC_ClientNoToken_ServerHasToken(t *testing.T) {
	clusterB := newP2PCluster("node-B")
	serverB := NewServer(clusterB)
	serverB.SetAuthToken("server-token")

	serverB.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{"status": "ok"}, nil
	})

	if err := serverB.Start(0); err != nil {
		t.Fatalf("Failed to start serverB: %v", err)
	}
	defer serverB.Stop()

	portB := serverB.GetPort()

	// Client without token (simulates the BUG: Client.SetAuthToken was never called)
	clusterA := newP2PCluster("node-A")
	clusterA.peers["node-B"] = &p2pNode{
		id:        "node-B",
		name:      "Node B",
		address:   fmt.Sprintf("127.0.0.1:%d", portB),
		addresses: []string{fmt.Sprintf("127.0.0.1:%d", portB)},
		rpcPort:   portB,
		online:    true,
	}

	clientA := NewClient(clusterA)
	// Deliberately NOT calling clientA.SetAuthToken() — this was the original bug

	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	_, err := clientA.CallWithContext(ctx, "node-B", "echo", map[string]interface{}{
		"message": "should fail — no auth",
	})
	if err == nil {
		t.Error("Expected RPC call to fail when client has no token but server requires one")
	}
}

// TestP2P_RPC_NoAuth_BackwardCompat verifies that when neither server nor client
// has a token, RPC communication works (backward compatibility).
func TestP2P_RPC_NoAuth_BackwardCompat(t *testing.T) {
	// Server with no auth
	clusterB := newP2PCluster("node-B")
	serverB := NewServer(clusterB)
	// No SetAuthToken call

	serverB.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{"status": "ok", "received": payload["msg"]}, nil
	})

	if err := serverB.Start(0); err != nil {
		t.Fatalf("Failed to start serverB: %v", err)
	}
	defer serverB.Stop()

	portB := serverB.GetPort()

	// Client with no auth
	clusterA := newP2PCluster("node-A")
	clusterA.peers["node-B"] = &p2pNode{
		id:        "node-B",
		name:      "Node B",
		address:   fmt.Sprintf("127.0.0.1:%d", portB),
		addresses: []string{fmt.Sprintf("127.0.0.1:%d", portB)},
		rpcPort:   portB,
		online:    true,
	}

	clientA := NewClient(clusterA)
	// No SetAuthToken call

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	resp, err := clientA.CallWithContext(ctx, "node-B", "echo", map[string]interface{}{
		"msg": "hello no-auth",
	})
	if err != nil {
		t.Fatalf("RPC call should succeed without auth, got error: %v", err)
	}
	if string(resp) == "" {
		t.Error("Expected non-empty response")
	}
}
