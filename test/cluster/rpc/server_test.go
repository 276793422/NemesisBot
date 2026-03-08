// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc_test

import (
	"encoding/json"
	"fmt"
	"net"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/rpc"
	"github.com/276793422/NemesisBot/module/cluster/transport"
)

// mockCluster implements Cluster interface for testing
type mockCluster struct {
	nodeID       string
	capabilities []string
}

func (m *mockCluster) GetRegistry() interface{}                 { return nil }
func (m *mockCluster) GetNodeID() string                       { return m.nodeID }
func (m *mockCluster) GetAddress() string                      { return "" }
func (m *mockCluster) GetCapabilities() []string               { return m.capabilities }
func (m *mockCluster) GetOnlinePeers() []interface{}           { return nil }
func (m *mockCluster) GetActionsSchema() []interface{}         { return []interface{}{} }
func (m *mockCluster) LogRPCInfo(msg string, args ...interface{}) {}
func (m *mockCluster) LogRPCError(msg string, args ...interface{}) {}
func (m *mockCluster) LogRPCDebug(msg string, args ...interface{}) {}
func (m *mockCluster) GetPeer(peerID string) (interface{}, error) { return nil, nil }
func (m *mockCluster) GetLocalNetworkInterfaces() ([]rpc.LocalNetworkInterface, error) {
	return nil, nil
}

func TestNewServer(t *testing.T) {
	mCluster := &mockCluster{nodeID: "test-node-1"}
	server := rpc.NewServer(mCluster)

	if server == nil {
		t.Fatal("NewServer() returned nil")
	}
}

func TestServerRegisterHandler(t *testing.T) {
	mCluster := &mockCluster{nodeID: "test-node-1"}
	server := rpc.NewServer(mCluster)

	handler := func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{"ok": true}, nil
	}

	server.RegisterHandler("test_action", handler)
}

func TestServerStartStop(t *testing.T) {
	mCluster := &mockCluster{nodeID: "test-node-1"}
	server := rpc.NewServer(mCluster)

	// Find available port
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("Failed to find port: %v", err)
	}
	port := listener.Addr().(*net.TCPAddr).Port
	listener.Close()

	// Start server
	err = server.Start(port)
	if err != nil {
		t.Fatalf("Start() failed: %v", err)
	}

	if !server.IsRunning() {
		t.Error("Start() failed to set running flag")
	}

	// Stop server
	err = server.Stop()
	if err != nil {
		t.Fatalf("Stop() failed: %v", err)
	}
}

func TestServerRPCCommunication(t *testing.T) {
	mCluster := &mockCluster{nodeID: "test-server"}
	server := rpc.NewServer(mCluster)

	// Register test handler
	server.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return payload, nil
	})

	// Start server
	port := 21951 // Use fixed port for testing
	err := server.Start(port)
	if err != nil {
		// Try alternative port
		listener, _ := net.Listen("tcp", "127.0.0.1:0")
		port = listener.Addr().(*net.TCPAddr).Port
		listener.Close()

		err = server.Start(port)
		if err != nil {
			t.Skipf("Failed to start server: %v", err)
			return
		}
	}
	defer server.Stop()

	// Connect as client
	addr := fmt.Sprintf("127.0.0.1:%d", port)
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		t.Fatalf("Failed to dial: %v", err)
	}
	defer conn.Close()

	// Create and send request
	req := transport.NewRequest("test-client", "test-server", "echo", map[string]interface{}{
		"message": "hello",
	})

	reqData, _ := req.Bytes()
	frameData, _ := transport.EncodeFrame(reqData)

	_, err = conn.Write(frameData)
	if err != nil {
		t.Fatalf("Failed to send: %v", err)
	}

	// Set read deadline
	conn.SetReadDeadline(time.Now().Add(5 * time.Second))

	// Read response
	respData, err := transport.DecodeFrame(conn)
	if err != nil {
		t.Fatalf("Failed to read response: %v", err)
	}

	var resp transport.RPCMessage
	if err := json.Unmarshal(respData, &resp); err != nil {
		t.Fatalf("Failed to unmarshal response: %v", err)
	}

	// Verify response
	if resp.Type != transport.RPCTypeResponse {
		t.Errorf("Response type = %s, want response", resp.Type)
	}

	if resp.Action != "echo" {
		t.Errorf("Response action = %s, want echo", resp.Action)
	}
}

func TestServerConcurrentConnections(t *testing.T) {
	mCluster := &mockCluster{nodeID: "test-server"}
	server := rpc.NewServer(mCluster)

	// Register ping handler
	server.RegisterHandler("ping", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{
			"status":  "ok",
			"node_id": mCluster.GetNodeID(),
		}, nil
	})

	// Start server
	listener, _ := net.Listen("tcp", "127.0.0.1:0")
	port := listener.Addr().(*net.TCPAddr).Port
	listener.Close()

	err := server.Start(port)
	if err != nil {
		t.Fatalf("Start() failed: %v", err)
	}
	defer server.Stop()

	// Multiple concurrent connections
	const numClients = 3
	var wg sync.WaitGroup
	errors := make(chan error, numClients)

	for i := 0; i < numClients; i++ {
		wg.Add(1)
		go func(clientID int) {
			defer wg.Done()

			addr := fmt.Sprintf("127.0.0.1:%d", port)
			conn, err := net.Dial("tcp", addr)
			if err != nil {
				errors <- err
				return
			}
			defer conn.Close()

			// Send ping request
			req := transport.NewRequest(fmt.Sprintf("client-%d", clientID), "test-server", "ping", nil)
			reqData, _ := req.Bytes()
			frameData, _ := transport.EncodeFrame(reqData)

			if _, err := conn.Write(frameData); err != nil {
				errors <- err
				return
			}

			// Read response
			conn.SetReadDeadline(time.Now().Add(5 * time.Second))
			respData, err := transport.DecodeFrame(conn)
			if err != nil {
				errors <- err
				return
			}

			var resp transport.RPCMessage
			if err := json.Unmarshal(respData, &resp); err != nil {
				errors <- err
				return
			}

			// Verify response
			if resp.Type != transport.RPCTypeResponse {
				errors <- fmt.Errorf("client %d: wrong response type", clientID)
				return
			}
		}(i)
	}

	wg.Wait()
	close(errors)

	for err := range errors {
		t.Errorf("Concurrent connection error: %v", err)
	}
}

func TestServerGetConnectionCount(t *testing.T) {
	mCluster := &mockCluster{nodeID: "test-server"}
	server := rpc.NewServer(mCluster)

	// Before start
	if server.GetConnectionCount() != 0 {
		t.Error("New server should have 0 connections")
	}

	// Start server
	listener, _ := net.Listen("tcp", "127.0.0.1:0")
	port := listener.Addr().(*net.TCPAddr).Port
	listener.Close()

	err := server.Start(port)
	if err != nil {
		t.Fatalf("Start() failed: %v", err)
	}

	// Connect
	addr := fmt.Sprintf("127.0.0.1:%d", port)
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		t.Fatalf("Failed to dial: %v", err)
	}

	// Wait a bit for connection to be registered
	time.Sleep(50 * time.Millisecond)

	count := server.GetConnectionCount()
	t.Logf("Connection count: %d", count)

	conn.Close()
	server.Stop()
}
