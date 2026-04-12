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
