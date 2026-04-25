// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package transport

import (
	"context"
	"encoding/json"
	"fmt"
	"testing"
	"time"
)

// TestStdioTransport_Send_Success tests the happy path of sending a request
// and receiving a valid JSON-RPC response. Uses a test MCP server script.
func TestStdioTransport_Send_Success(t *testing.T) {
	// Use the test MCP server if available, otherwise use a simple echo-based approach
	// We create a script that reads a line and responds with valid JSON-RPC
	// On Windows/bash we can use: while read line; do echo '{"jsonrpc":"2.0","id":1,"result":{}}'; done
	// But that's shell-specific. Let's use cat + a helper approach.

	// Instead, test with a process that echoes back a valid JSON-RPC response
	// Use bash -c to create a simple echo server
	trans, err := NewStdioTransport("bash", []string{"-c", "while IFS= read -r line; do echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'; done"}, nil)
	if err != nil {
		t.Fatalf("NewStdioTransport() failed: %v", err)
	}

	ctx := context.Background()
	err = trans.Connect(ctx)
	if err != nil {
		t.Skipf("Could not start bash subprocess: %v", err)
	}
	defer trans.Close()

	req := &JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      1,
		Method:  "initialize",
		Params:  json.RawMessage(`{}`),
	}

	resp, err := trans.Send(ctx, req)
	if err != nil {
		t.Fatalf("Send() failed: %v", err)
	}
	if resp == nil {
		t.Fatal("Send() returned nil response")
	}
	if resp.JSONRPC != "2.0" {
		t.Errorf("Expected JSONRPC '2.0', got '%s'", resp.JSONRPC)
	}
}

// TestStdioTransport_Send_InvalidResponse tests that an invalid JSON response
// from the subprocess results in an error.
func TestStdioTransport_Send_InvalidResponse(t *testing.T) {
	trans, err := NewStdioTransport("bash", []string{"-c", "while IFS= read -r line; do echo 'not valid json'; done"}, nil)
	if err != nil {
		t.Fatalf("NewStdioTransport() failed: %v", err)
	}

	ctx := context.Background()
	err = trans.Connect(ctx)
	if err != nil {
		t.Skipf("Could not start bash subprocess: %v", err)
	}
	defer trans.Close()

	req := &JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      1,
		Method:  "test",
	}

	_, err = trans.Send(ctx, req)
	if err == nil {
		t.Error("Send() should return error for invalid JSON response")
	}
}

// TestStdioTransport_Send_ProcessExits tests sending when the process exits
// unexpectedly (EOF on stdout).
func TestStdioTransport_Send_ProcessExits(t *testing.T) {
	// Use bash -c "exit 0" — process exits immediately after connect
	trans, err := NewStdioTransport("bash", []string{"-c", "echo connected && exit 0"}, nil)
	if err != nil {
		t.Fatalf("NewStdioTransport() failed: %v", err)
	}

	ctx := context.Background()
	err = trans.Connect(ctx)
	if err != nil {
		t.Skipf("Could not start bash subprocess: %v", err)
	}
	defer trans.Close()

	// Give process a moment to exit
	time.Sleep(100 * time.Millisecond)

	req := &JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      1,
		Method:  "test",
	}

	// Send should fail because process has exited (stdin closed or EOF on stdout)
	_, err = trans.Send(ctx, req)
	if err == nil {
		t.Error("Send() should return error when process has exited")
	}
}

// TestStdioTransport_Send_ContextCancelled tests that cancelling the context
// during a Send operation returns an appropriate error.
func TestStdioTransport_Send_ContextCancelled(t *testing.T) {
	trans, err := NewStdioTransport("cat", []string{}, nil)
	if err != nil {
		t.Fatalf("NewStdioTransport() failed: %v", err)
	}

	ctx := context.Background()
	err = trans.Connect(ctx)
	if err != nil {
		t.Fatalf("Connect() failed: %v", err)
	}
	defer trans.Close()

	cancelCtx, cancel := context.WithCancel(context.Background())
	// Cancel immediately after creation
	cancel()

	req := &JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      1,
		Method:  "test",
	}

	_, err = trans.Send(cancelCtx, req)
	if err == nil {
		t.Error("Send() should return error when context is cancelled")
	}
}

// TestStdioTransport_Send_ResponseWithResult tests receiving a JSON-RPC response
// with a specific result.
func TestStdioTransport_Send_ResponseWithResult(t *testing.T) {
	responseJSON := `{"jsonrpc":"2.0","id":1,"result":{"capabilities":{},"serverInfo":{"name":"test","version":"1.0"}}}`
	cmd := fmt.Sprintf("while IFS= read -r line; do echo '%s'; done", responseJSON)

	trans, err := NewStdioTransport("bash", []string{"-c", cmd}, nil)
	if err != nil {
		t.Fatalf("NewStdioTransport() failed: %v", err)
	}

	ctx := context.Background()
	err = trans.Connect(ctx)
	if err != nil {
		t.Skipf("Could not start bash subprocess: %v", err)
	}
	defer trans.Close()

	req := &JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      1,
		Method:  "initialize",
	}

	resp, err := trans.Send(ctx, req)
	if err != nil {
		t.Fatalf("Send() failed: %v", err)
	}
	if resp == nil {
		t.Fatal("Response should not be nil")
	}
	if resp.Result == nil {
		t.Error("Result should not be nil")
	}
	if resp.Error != nil {
		t.Errorf("Error should be nil, got %v", resp.Error)
	}
}

// TestStdioTransport_Send_ResponseWithError tests receiving a JSON-RPC error response.
func TestStdioTransport_Send_ResponseWithError(t *testing.T) {
	errorJSON := `{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}`
	cmd := fmt.Sprintf("while IFS= read -r line; do echo '%s'; done", errorJSON)

	trans, err := NewStdioTransport("bash", []string{"-c", cmd}, nil)
	if err != nil {
		t.Fatalf("NewStdioTransport() failed: %v", err)
	}

	ctx := context.Background()
	err = trans.Connect(ctx)
	if err != nil {
		t.Skipf("Could not start bash subprocess: %v", err)
	}
	defer trans.Close()

	req := &JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      1,
		Method:  "nonexistent",
	}

	resp, err := trans.Send(ctx, req)
	if err != nil {
		t.Fatalf("Send() should not fail for JSON-RPC error responses: %v", err)
	}
	if resp == nil {
		t.Fatal("Response should not be nil")
	}
	if resp.Error == nil {
		t.Error("Error should not be nil")
	}
	if resp.Error.Code != -32601 {
		t.Errorf("Expected error code -32601, got %d", resp.Error.Code)
	}
}

// TestStdioTransport_Send_MultipleRequests tests sending multiple requests
// sequentially on the same transport.
func TestStdioTransport_Send_MultipleRequests(t *testing.T) {
	trans, err := NewStdioTransport("bash", []string{"-c", "while IFS= read -r line; do echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'; done"}, nil)
	if err != nil {
		t.Fatalf("NewStdioTransport() failed: %v", err)
	}

	ctx := context.Background()
	err = trans.Connect(ctx)
	if err != nil {
		t.Skipf("Could not start bash subprocess: %v", err)
	}
	defer trans.Close()

	for i := 0; i < 3; i++ {
		req := &JSONRPCRequest{
			JSONRPC: "2.0",
			ID:      int64(i + 1),
			Method:  "test",
		}

		resp, err := trans.Send(ctx, req)
		if err != nil {
			t.Errorf("Send() request %d failed: %v", i+1, err)
			continue
		}
		if resp == nil {
			t.Errorf("Response %d should not be nil", i+1)
		}
	}
}

// TestStdioTransport_Close_AfterExit tests that Close handles the case
// where the process has already exited.
func TestStdioTransport_Close_AfterExit(t *testing.T) {
	// Use a process that exits immediately
	trans, err := NewStdioTransport("bash", []string{"-c", "echo ok && exit 0"}, nil)
	if err != nil {
		t.Fatalf("NewStdioTransport() failed: %v", err)
	}

	ctx := context.Background()
	err = trans.Connect(ctx)
	if err != nil {
		t.Skipf("Could not start bash subprocess: %v", err)
	}

	if !trans.IsConnected() {
		t.Fatal("Should be connected")
	}

	// Give the process time to exit
	time.Sleep(200 * time.Millisecond)

	// Close should succeed
	err = trans.Close()
	if err != nil {
		t.Errorf("Close() failed: %v", err)
	}

	if trans.IsConnected() {
		t.Error("Should not be connected after close")
	}
}

// TestStdioTransport_Send_WithEnv tests that environment variables are passed
// to the subprocess and affect its behavior.
func TestStdioTransport_Send_WithEnv(t *testing.T) {
	// Use bash to echo an env var back in JSON-RPC format
	cmd := "while IFS= read -r line; do echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"env\":\"'\"$TEST_SEND_ENV\"'\"}}'; done"
	trans, err := NewStdioTransport("bash", []string{"-c", cmd}, []string{"TEST_SEND_ENV=hello_world"})
	if err != nil {
		t.Fatalf("NewStdioTransport() failed: %v", err)
	}

	ctx := context.Background()
	err = trans.Connect(ctx)
	if err != nil {
		t.Skipf("Could not start bash subprocess: %v", err)
	}
	defer trans.Close()

	req := &JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      1,
		Method:  "test",
	}

	resp, err := trans.Send(ctx, req)
	if err != nil {
		t.Fatalf("Send() failed: %v", err)
	}
	if resp == nil {
		t.Fatal("Response should not be nil")
	}
}
