// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package transport_test

import (
	"io"
	"net"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/transport"
)

// Test helper to create a pair of connected TCPConns
func createTestConnPair() (net.Listener, *transport.TCPConn, *transport.TCPConn, error) {
	// Create listener
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, nil, nil, err
	}

	// Accept connection in goroutine
	connChan := make(chan net.Conn, 1)
	errChan := make(chan error, 1)

	go func() {
		conn, err := listener.Accept()
		if err != nil {
			errChan <- err
			return
		}
		connChan <- conn
	}()

	// Dial connection
	clientConn, err := net.Dial("tcp", listener.Addr().String())
	if err != nil {
		listener.Close()
		return nil, nil, nil, err
	}

	// Get server connection
	select {
	case serverConn := <-connChan:
		config := transport.DefaultTCPConnConfig("test-node", "127.0.0.1:12345")

		serverTCP := transport.NewTCPConn(serverConn, config)
		clientTCP := transport.NewTCPConn(clientConn, config)

		return listener, serverTCP, clientTCP, nil
	case err := <-errChan:
		listener.Close()
		clientConn.Close()
		return nil, nil, nil, err

	case <-time.After(5 * time.Second):
		listener.Close()
		clientConn.Close()
		return nil, nil, nil, io.ErrNoProgress
	}
}

func TestNewTCPConn(t *testing.T) {
	conn1, conn2 := net.Pipe()
	defer conn1.Close()
	defer conn2.Close()

	config := transport.DefaultTCPConnConfig("node-1", "127.0.0.1:8080")
	tc := transport.NewTCPConn(conn1, config)

	if tc == nil {
		t.Fatal("NewTCPConn() returned nil")
	}

	if tc.GetNodeID() != "node-1" {
		t.Errorf("NewTCPConn() nodeID = %s, want node-1", tc.GetNodeID())
	}

	if tc.GetAddress() != "127.0.0.1:8080" {
		t.Errorf("NewTCPConn() address = %s, want 127.0.0.1:8080", tc.GetAddress())
	}
}

func TestTCPConnStart(t *testing.T) {
	conn1, conn2 := net.Pipe()
	defer conn1.Close()
	defer conn2.Close()

	config := transport.DefaultTCPConnConfig("node-1", "127.0.0.1:8080")
	config.ReadBufferSize = 10
	config.SendBufferSize = 10

	tc := transport.NewTCPConn(conn1, config)

	tc.Start()

	tc.Close()
}

func TestTCPConnSendReceive(t *testing.T) {
	listener, server, client, err := createTestConnPair()
	if err != nil {
		t.Fatalf("createTestConnPair() failed: %v", err)
	}
	defer listener.Close()

	// Start both connections
	server.Start()
	defer server.Close()

	client.Start()
	defer client.Close()

	// Send message from client to server
	msg := transport.NewRequest("client-1", "server-1", "ping", map[string]interface{}{
		"data": "test",
	})

	err = client.Send(msg)
	if err != nil {
		t.Fatalf("Send() failed: %v", err)
	}

	// Receive on server
	select {
	case receivedMsg := <-server.Receive():
		if receivedMsg.ID != msg.ID {
			t.Errorf("Receive() ID = %s, want %s", receivedMsg.ID, msg.ID)
		}
		if receivedMsg.Action != msg.Action {
			t.Errorf("Receive() Action = %s, want %s", receivedMsg.Action, msg.Action)
		}

	case <-time.After(5 * time.Second):
		t.Fatal("Receive() timeout")
	}
}

func TestTCPConnBidirectional(t *testing.T) {
	listener, server, client, err := createTestConnPair()
	if err != nil {
		t.Fatalf("createTestConnPair() failed: %v", err)
	}
	defer listener.Close()

	// Start both connections
	server.Start()
	defer server.Close()

	client.Start()
	defer client.Close()

	// Send both directions
	var wg sync.WaitGroup
	wg.Add(2)

	// Client to server
	go func() {
		defer wg.Done()
		msg := transport.NewRequest("client-1", "server-1", "ping", nil)
		if err := client.Send(msg); err != nil {
			t.Errorf("Client send failed: %v", err)
			return
		}

		select {
		case <-server.Receive():
			// OK
		case <-time.After(5 * time.Second):
			t.Error("Server receive timeout")
		}
	}()

	// Server to client
	go func() {
		defer wg.Done()
		msg := transport.NewResponse(&transport.RPCMessage{
			ID:     "test-1",
			From:   "client-1",
			To:     "server-1",
			Action: "ping",
		}, map[string]interface{}{
			"status": "ok",
		})

		if err := server.Send(msg); err != nil {
			t.Errorf("Server send failed: %v", err)
			return
		}

		select {
		case <-client.Receive():
			// OK
		case <-time.After(5 * time.Second):
			t.Error("Client receive timeout")
		}
	}()

	wg.Wait()
}

func TestTCPConnClose(t *testing.T) {
	conn1, conn2 := net.Pipe()

	config := transport.DefaultTCPConnConfig("node-1", "127.0.0.1:8080")
	tc := transport.NewTCPConn(conn1, config)
	tc.Start()

	// Close the connection
	err := tc.Close()
	if err != nil {
		t.Fatalf("Close() failed: %v", err)
	}

	// Send should fail
	msg := transport.NewRequest("a", "b", "test", nil)
	err = tc.Send(msg)
	if err != transport.ErrConnClosed {
		t.Errorf("Send() after close = %v, want ErrConnClosed", err)
	}

	// Close should be idempotent
	err = tc.Close()
	if err != nil {
		t.Errorf("Close() again failed: %v", err)
	}

	conn2.Close()
}

func TestTCPConnIsActive(t *testing.T) {
	conn1, _ := net.Pipe()

	config := transport.DefaultTCPConnConfig("node-1", "127.0.0.1:8080")
	config.IdleTimeout = 100 * time.Millisecond

	tc := transport.NewTCPConn(conn1, config)

	if !tc.IsActive() {
		t.Error("New TCPConn should be active")
	}

	tc.Close()

	if tc.IsActive() {
		t.Error("Closed TCPConn should not be active")
	}
}

func TestTCPConnGetters(t *testing.T) {
	conn1, _ := net.Pipe()

	config := transport.DefaultTCPConnConfig("test-node", "192.168.1.1:9999")
	tc := transport.NewTCPConn(conn1, config)

	// Test GetNodeID
	if tc.GetNodeID() != "test-node" {
		t.Errorf("GetNodeID() = %s, want test-node", tc.GetNodeID())
	}

	// Test GetAddress
	if tc.GetAddress() != "192.168.1.1:9999" {
		t.Errorf("GetAddress() = %s, want 192.168.1.1:9999", tc.GetAddress())
	}

	// Test GetCreatedAt
	if tc.GetCreatedAt().IsZero() {
		t.Error("GetCreatedAt() returned zero time")
	}

	// Test GetLastUsed
	if tc.GetLastUsed().IsZero() {
		t.Error("GetLastUsed() returned zero time")
	}

	tc.Close()
}

func TestTCPConnUpdateLastUsed(t *testing.T) {
	conn1, _ := net.Pipe()

	config := transport.DefaultTCPConnConfig("node-1", "127.0.0.1:8080")
	tc := transport.NewTCPConn(conn1, config)

	oldTime := tc.GetLastUsed()
	time.Sleep(10 * time.Millisecond)

	tc.UpdateLastUsed()

	newTime := tc.GetLastUsed()
	if !newTime.After(oldTime) {
		t.Error("UpdateLastUsed() failed to update time")
	}

	tc.Close()
}

func TestTCPConnSetNodeID(t *testing.T) {
	conn1, _ := net.Pipe()

	config := transport.DefaultTCPConnConfig("old-node", "127.0.0.1:8080")
	tc := transport.NewTCPConn(conn1, config)

	tc.SetNodeID("new-node")

	if tc.GetNodeID() != "new-node" {
		t.Errorf("SetNodeID() failed: got %s, want new-node", tc.GetNodeID())
	}

	tc.Close()
}

func TestTCPConnMultipleMessages(t *testing.T) {
	listener, server, client, err := createTestConnPair()
	if err != nil {
		t.Fatalf("createTestConnPair() failed: %v", err)
	}
	defer listener.Close()

	// Start both connections
	server.Start()
	defer server.Close()

	client.Start()
	defer client.Close()

	// Send multiple messages
	count := 10
	for i := 0; i < count; i++ {
		msg := transport.NewRequest("client-1", "server-1", "ping", map[string]interface{}{
			"index": i,
		})

		if err := client.Send(msg); err != nil {
			t.Fatalf("Send(%d) failed: %v", i, err)
		}
	}

	// Receive all messages
	received := 0
	timeout := time.After(10 * time.Second)

	for {
		select {
		case <-server.Receive():
			received++
			if received == count {
				return // All received
			}
		case <-timeout:
			t.Fatalf("Receive() timeout: got %d/%d messages", received, count)
		}
	}
}
