// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

//go:build ignore
// +build ignore

package main

import (
	"encoding/json"
	"fmt"
	"net"
	"os"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/rpc"
	"github.com/276793422/NemesisBot/module/cluster/transport"
)

func main() {
	fmt.Println("===========================================")
	fmt.Println("TCP RPC Integration & Stress Test")
	fmt.Println("===========================================")
	fmt.Println()

	testCount := 0
	passCount := 0
	failCount := 0

	// Test 1: Basic RPC communication
	fmt.Println("[Test 1] Basic RPC Communication")
	fmt.Println("-------------------------------------------")
	if testBasicRPC() {
		passCount++
		fmt.Println("✓ PASSED")
	} else {
		failCount++
		fmt.Println("✗ FAILED")
	}
	testCount++
	fmt.Println()

	// Test 2: Concurrent RPC calls
	fmt.Println("[Test 2] Concurrent RPC Calls (10 simultaneous)")
	fmt.Println("-------------------------------------------")
	if testConcurrentRPC() {
		passCount++
		fmt.Println("✓ PASSED")
	} else {
		failCount++
		fmt.Println("✗ FAILED")
	}
	testCount++
	fmt.Println()

	// Test 3: Sequential RPC calls
	fmt.Println("[Test 3] Sequential RPC Calls (50 calls)")
	fmt.Println("-------------------------------------------")
	if testSequentialRPC() {
		passCount++
		fmt.Println("✓ PASSED")
	} else {
		failCount++
		fmt.Println("✗ FAILED")
	}
	testCount++
	fmt.Println()

	// Test 4: Large payload
	fmt.Println("[Test 4] Large Payload RPC (1MB)")
	fmt.Println("-------------------------------------------")
	if testLargePayload() {
		passCount++
		fmt.Println("✓ PASSED")
	} else {
		failCount++
		fmt.Println("✗ FAILED")
	}
	testCount++
	fmt.Println()

	// Test 5: Timeout handling
	fmt.Println("[Test 5] RPC Timeout Handling")
	fmt.Println("-------------------------------------------")
	if testTimeout() {
		passCount++
		fmt.Println("✓ PASSED")
	} else {
		failCount++
		fmt.Println("✗ FAILED")
	}
	testCount++
	fmt.Println()

	// Test 6: Connection pool
	fmt.Println("[Test 6] Connection Pool Multiple Connections")
	fmt.Println("-------------------------------------------")
	if testConnectionPool() {
		passCount++
		fmt.Println("✓ PASSED")
	} else {
		failCount++
		fmt.Println("✗ FAILED")
	}
	testCount++
	fmt.Println()

	// Summary
	fmt.Println("===========================================")
	fmt.Println("Test Summary")
	fmt.Println("===========================================")
	fmt.Printf("Total:  %d\n", testCount)
	fmt.Printf("Passed: %d\n", passCount)
	fmt.Printf("Failed: %d\n", failCount)
	fmt.Printf("Rate:   %.1f%%\n", float64(passCount)*100/float64(testCount))
	fmt.Println()

	if failCount == 0 {
		fmt.Println("✓ ALL TESTS PASSED")
		os.Exit(0)
	} else {
		fmt.Println("✗ SOME TESTS FAILED")
		os.Exit(1)
	}
}

// testBasicRPC tests basic RPC communication
func testBasicRPC() bool {
	// Start server
	server := rpc.NewServer(&mockCluster{})
	server.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return payload, nil
	})

	go func() {
		server.Start(21960)
	}()
	defer server.Stop()
	time.Sleep(500 * time.Millisecond)

	// Create client connection
	conn, err := net.Dial("tcp", "127.0.0.1:21960")
	if err != nil {
		fmt.Printf("Failed to connect: %v\n", err)
		return false
	}
	defer conn.Close()

	// Send request
	req := transport.NewRequest("client", "server", "echo", map[string]interface{}{
		"message": "hello",
	})
	reqData, _ := req.Bytes()
	frameData, _ := transport.EncodeFrame(reqData)

	if _, err := conn.Write(frameData); err != nil {
		fmt.Printf("Failed to send: %v\n", err)
		return false
	}

	// Read response
	conn.SetReadDeadline(time.Now().Add(5 * time.Second))
	respData, err := transport.DecodeFrame(conn)
	if err != nil {
		fmt.Printf("Failed to read response: %v\n", err)
		return false
	}

	var resp transport.RPCMessage
	if err := json.Unmarshal(respData, &resp); err != nil {
		fmt.Printf("Failed to unmarshal: %v\n", err)
		return false
	}

	if resp.Type != transport.RPCTypeResponse {
		fmt.Printf("Wrong type: %s\n", resp.Type)
		return false
	}

	return true
}

// testConcurrentRPC tests concurrent RPC calls
func testConcurrentRPC() bool {
	server := rpc.NewServer(&mockCluster{})
	server.RegisterHandler("ping", func(payload map[string]interface{}) (map[string]interface{}, error) {
		time.Sleep(100 * time.Millisecond) // Simulate work
		return map[string]interface{}{"status": "ok"}, nil
	})

	go func() {
		server.Start(21961)
	}()
	defer server.Stop()
	time.Sleep(500 * time.Millisecond)

	// Launch concurrent calls
	var wg sync.WaitGroup
	errors := make(chan error, 10)
	successCount := 0
	var mu sync.Mutex

	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()

			conn, err := net.Dial("tcp", "127.0.0.1:21961")
			if err != nil {
				errors <- err
				return
			}
			defer conn.Close()

			req := transport.NewRequest(fmt.Sprintf("client-%d", id), "server", "ping", nil)
			reqData, _ := req.Bytes()
			frameData, _ := transport.EncodeFrame(reqData)

			if _, err := conn.Write(frameData); err != nil {
				errors <- err
				return
			}

			conn.SetReadDeadline(time.Now().Add(10 * time.Second))
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

			if resp.Type == transport.RPCTypeResponse {
				mu.Lock()
				successCount++
				mu.Unlock()
			}
		}(i)
	}

	wg.Wait()
	close(errors)

	for err := range errors {
		fmt.Printf("Error: %v\n", err)
	}

	return successCount == 10
}

// testSequentialRPC tests many sequential RPC calls
func testSequentialRPC() bool {
	server := rpc.NewServer(&mockCluster{})
	server.RegisterHandler("counter", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{"count": payload["n"]}, nil
	})

	go func() {
		server.Start(21962)
	}()
	defer server.Stop()
	time.Sleep(500 * time.Millisecond)

	successCount := 0
	for i := 0; i < 50; i++ {
		conn, err := net.Dial("tcp", "127.0.0.1:21962")
		if err != nil {
			fmt.Printf("Failed to connect (iteration %d): %v\n", i, err)
			continue
		}

		req := transport.NewRequest("client", "server", "counter", map[string]interface{}{
			"n": i,
		})
		reqData, _ := req.Bytes()
		frameData, _ := transport.EncodeFrame(reqData)

		if _, err := conn.Write(frameData); err != nil {
			conn.Close()
			fmt.Printf("Failed to send (iteration %d): %v\n", i, err)
			continue
		}

		conn.SetReadDeadline(time.Now().Add(5 * time.Second))
		respData, err := transport.DecodeFrame(conn)
		conn.Close()

		if err != nil {
			fmt.Printf("Failed to read (iteration %d): %v\n", i, err)
			continue
		}

		var resp transport.RPCMessage
		if err := json.Unmarshal(respData, &resp); err != nil {
			fmt.Printf("Failed to unmarshal (iteration %d): %v\n", i, err)
			continue
		}

		if resp.Type == transport.RPCTypeResponse {
			successCount++
		}

		// Small delay to prevent overwhelming
		if i%10 == 9 {
			time.Sleep(10 * time.Millisecond)
		}
	}

	return successCount >= 45 // Allow some failures
}

// testLargePayload tests RPC with large payload
func testLargePayload() bool {
	server := rpc.NewServer(&mockCluster{})
	server.RegisterHandler("large", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{
			"status":        "ok",
			"received_size": len(payload),
		}, nil
	})

	go func() {
		server.Start(21963)
	}()
	defer server.Stop()
	time.Sleep(500 * time.Millisecond)

	// Create 1MB payload
	largeData := make([]byte, 1024*1024)
	for i := range largeData {
		largeData[i] = byte(i % 256)
	}

	conn, err := net.Dial("tcp", "127.0.0.1:21963")
	if err != nil {
		fmt.Printf("Failed to connect: %v\n", err)
		return false
	}
	defer conn.Close()

	req := transport.NewRequest("client", "server", "large", map[string]interface{}{
		"data": largeData,
	})
	reqData, _ := req.Bytes()
	frameData, _ := transport.EncodeFrame(reqData)

	start := time.Now()
	if _, err := conn.Write(frameData); err != nil {
		fmt.Printf("Failed to send: %v\n", err)
		return false
	}

	conn.SetReadDeadline(time.Now().Add(30 * time.Second))
	respData, err := transport.DecodeFrame(conn)
	if err != nil {
		fmt.Printf("Failed to read: %v\n", err)
		return false
	}
	elapsed := time.Since(start)

	var resp transport.RPCMessage
	if err := json.Unmarshal(respData, &resp); err != nil {
		fmt.Printf("Failed to unmarshal: %v\n", err)
		return false
	}

	fmt.Printf("  Transfer time: %v (1MB)\n", elapsed)
	return resp.Type == transport.RPCTypeResponse
}

// testTimeout tests RPC timeout handling
func testTimeout() bool {
	server := rpc.NewServer(&mockCluster{})
	server.RegisterHandler("slow", func(payload map[string]interface{}) (map[string]interface{}, error) {
		time.Sleep(5 * time.Second) // Longer than client timeout
		return map[string]interface{}{"status": "ok"}, nil
	})

	go func() {
		server.Start(21964)
	}()
	defer server.Stop()
	time.Sleep(500 * time.Millisecond)

	conn, err := net.Dial("tcp", "127.0.0.1:21964")
	if err != nil {
		fmt.Printf("Failed to connect: %v\n", err)
		return false
	}
	defer conn.Close()

	req := transport.NewRequest("client", "server", "slow", nil)
	reqData, _ := req.Bytes()
	frameData, _ := transport.EncodeFrame(reqData)

	if _, err := conn.Write(frameData); err != nil {
		fmt.Printf("Failed to send: %v\n", err)
		return false
	}

	// Set short deadline
	conn.SetReadDeadline(time.Now().Add(1 * time.Second))
	_, err = transport.DecodeFrame(conn)

	// Should timeout
	if err != nil {
		// Check if error is a timeout (either net.Error or contains "timeout")
		if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
			return true // Timeout is expected
		}
		// Also check error message for timeout
		if containsTimeout(err.Error()) {
			return true // Timeout detected in error message
		}
		fmt.Printf("Unexpected error: %v\n", err)
		return false
	}

	// If we got here, no timeout - test failed
	fmt.Println("Expected timeout but got response")
	return false
}

// containsTimeout checks if error message indicates a timeout
func containsTimeout(s string) bool {
	return len(s) > 0 && (contains(s, "timeout") || contains(s, "i/o timeout"))
}

// contains is a simple string contains helper
func contains(s, substr string) bool {
	return len(s) >= len(substr) && findSubstring(s, substr)
}

func findSubstring(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}

// testConnectionPool tests connection pool with multiple connections
func testConnectionPool() bool {
	server := rpc.NewServer(&mockCluster{})
	server.RegisterHandler("pool_test", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{"status": "ok", "conn_id": payload["id"]}, nil
	})

	go func() {
		server.Start(21965)
	}()
	defer server.Stop()
	time.Sleep(500 * time.Millisecond)

	// Create connection pool
	pool := transport.NewPool()
	defer pool.Close()

	successCount := 0
	for i := 0; i < 5; i++ {
		addr := fmt.Sprintf("127.0.0.1:21965")
		conn, err := pool.Get("test-node", addr)
		if err != nil {
			fmt.Printf("Failed to get connection %d: %v\n", i, err)
			continue
		}

		// Send request using the pooled connection
		req := transport.NewRequest("client", "server", "pool_test", map[string]interface{}{
			"id": i,
		})

		if err := conn.Send(req); err != nil {
			fmt.Printf("Failed to send %d: %v\n", i, err)
			continue
		}

		// Wait for response
		select {
		case msg := <-conn.Receive():
			if msg != nil && msg.Type == transport.RPCTypeResponse {
				successCount++
			}
		case <-time.After(5 * time.Second):
			fmt.Printf("Timeout waiting for response %d\n", i)
		}
	}

	return successCount >= 4 // Allow some failures
}

// mockCluster implements Cluster interface for testing
type mockCluster struct{}

func (m *mockCluster) GetRegistry() interface{}      { return nil }
func (m *mockCluster) GetNodeID() string             { return "test-server" }
func (m *mockCluster) GetAddress() string            { return "" }
func (m *mockCluster) GetCapabilities() []string     { return []string{"test"} }
func (m *mockCluster) GetOnlinePeers() []interface{} { return nil }
func (m *mockCluster) LogRPCInfo(msg string, args ...interface{}) {
	fmt.Printf("[INFO] %s\n", fmt.Sprintf(msg, args...))
}
func (m *mockCluster) LogRPCError(msg string, args ...interface{}) {
	fmt.Printf("[ERROR] %s\n", fmt.Sprintf(msg, args...))
}
func (m *mockCluster) LogRPCDebug(msg string, args ...interface{}) {}
func (m *mockCluster) GetPeer(peerID string) (interface{}, error)  { return nil, nil }
func (m *mockCluster) GetLocalNetworkInterfaces() ([]rpc.LocalNetworkInterface, error) {
	return nil, nil
}
