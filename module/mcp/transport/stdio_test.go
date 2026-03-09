// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package transport

import (
	"context"
	"encoding/json"
	"testing"
	"time"
)

func TestNewStdioTransport(t *testing.T) {
	tests := []struct {
		name    string
		command string
		args    []string
		env     []string
		wantErr bool
	}{
		{
			name:    "valid transport",
			command: "echo",
			args:    []string{"hello"},
			env:     []string{"TEST=value"},
			wantErr: false,
		},
		{
			name:    "empty command",
			command: "",
			args:    []string{},
			env:     []string{},
			wantErr: true,
		},
		{
			name:    "command with no args",
			command: "echo",
			args:    nil,
			env:     nil,
			wantErr: false,
		},
		{
			name:    "command with empty env",
			command: "echo",
			args:    []string{},
			env:     []string{},
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			trans, err := NewStdioTransport(tt.command, tt.args, tt.env)
			if (err != nil) != tt.wantErr {
				t.Errorf("NewStdioTransport() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr {
				if trans == nil {
					t.Error("NewStdioTransport() returned nil transport")
				}
				if trans.command != tt.command {
					t.Errorf("transport.command = %v, want %v", trans.command, tt.command)
				}
				if trans.Name() != "stdio" {
					t.Errorf("transport.Name() = %v, want stdio", trans.Name())
				}
				if trans.IsConnected() {
					t.Error("NewStdioTransport() should not be connected initially")
				}
			}
		})
	}
}

func TestStdioTransport_Connect(t *testing.T) {
	tests := []struct {
		name    string
		command string
		args    []string
		wantErr bool
	}{
		{
			name:    "valid command - echo",
			command: "echo",
			args:    []string{"test"},
			wantErr: false,
		},
		{
			name:    "valid command - cat",
			command: "cat",
			args:    []string{},
			wantErr: false,
		},
		{
			name:    "invalid command",
			command: "nonexistentcommand12345",
			args:    []string{},
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			trans, err := NewStdioTransport(tt.command, tt.args, nil)
			if err != nil {
				t.Fatalf("NewStdioTransport() failed: %v", err)
			}

			ctx := context.Background()
			err = trans.Connect(ctx)
			if (err != nil) != tt.wantErr {
				t.Errorf("Connect() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			if !tt.wantErr {
				if !trans.IsConnected() {
					t.Error("Connect() did not set connected state")
				}

				// Test double connect - should be idempotent
				err = trans.Connect(ctx)
				if err != nil {
					t.Errorf("Second Connect() should be no-op, got error: %v", err)
				}

				// Clean up
				if err := trans.Close(); err != nil {
					t.Errorf("Close() failed: %v", err)
				}
			}
		})
	}
}

func TestStdioTransport_Connect_WithEnv(t *testing.T) {
	trans, err := NewStdioTransport("printenv", []string{"TEST_VAR"}, []string{"TEST_VAR=test_value"})
	if err != nil {
		t.Fatalf("NewStdioTransport() failed: %v", err)
	}

	ctx := context.Background()
	err = trans.Connect(ctx)
	if err != nil {
		// printenv might not be available on all systems
		t.Skipf("printenv not available: %v", err)
	}
	defer trans.Close()

	if !trans.IsConnected() {
		t.Error("Connect() did not set connected state")
	}
}

func TestStdioTransport_Close(t *testing.T) {
	tests := []struct {
		name    string
		command string
	}{
		{
			name:    "close echo process",
			command: "echo",
		},
		{
			name:    "close cat process",
			command: "cat",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			trans, err := NewStdioTransport(tt.command, []string{}, nil)
			if err != nil {
				t.Fatalf("NewStdioTransport() failed: %v", err)
			}

			ctx := context.Background()
			err = trans.Connect(ctx)
			if err != nil {
				t.Fatalf("Connect() failed: %v", err)
			}

			if !trans.IsConnected() {
				t.Fatal("transport should be connected before close")
			}

			err = trans.Close()
			if err != nil {
				t.Errorf("Close() error = %v", err)
			}

			if trans.IsConnected() {
				t.Error("transport should not be connected after close")
			}

			// Test double close - should be idempotent
			err = trans.Close()
			if err != nil {
				t.Errorf("Second Close() should be no-op, got error: %v", err)
			}
		})
	}
}

func TestStdioTransport_Close_NotConnected(t *testing.T) {
	trans, err := NewStdioTransport("echo", []string{}, nil)
	if err != nil {
		t.Fatalf("NewStdioTransport() failed: %v", err)
	}

	// Close without connecting - should be no-op
	err = trans.Close()
	if err != nil {
		t.Errorf("Close() without Connect() should be no-op, got error: %v", err)
	}
}

func TestStdioTransport_Send_NotConnected(t *testing.T) {
	trans, err := NewStdioTransport("echo", []string{}, nil)
	if err != nil {
		t.Fatalf("NewStdioTransport() failed: %v", err)
	}

	ctx := context.Background()
	req := &JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      1,
		Method:  "test",
	}

	_, err = trans.Send(ctx, req)
	if err == nil {
		t.Error("Send() should return error when not connected")
	}
}

func TestStdioTransport_ContextTimeout(t *testing.T) {
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

	// Create a context with very short timeout
	shortCtx, cancel := context.WithTimeout(context.Background(), 1*time.Millisecond)
	defer cancel()

	req := &JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      1,
		Method:  "test",
	}

	// Send should timeout because cat won't respond
	start := time.Now()
	_, err = trans.Send(shortCtx, req)
	elapsed := time.Since(start)

	if err == nil {
		t.Error("Send() should timeout with short context")
	}

	// Should timeout quickly (allow some margin for execution)
	if elapsed > 200*time.Millisecond {
		t.Errorf("Send() took too long to timeout: %v", elapsed)
	}
}

func TestStdioTransport_Send_InvalidRequest(t *testing.T) {
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

	// Create a request with invalid data that can't be marshaled
	req := &JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      make(chan int), // channels can't be marshaled
		Method:  "test",
	}

	_, err = trans.Send(ctx, req)
	if err == nil {
		t.Error("Send() should return error for unmarshalable request")
	}
}

func TestJSONRPCRequest_Marshal(t *testing.T) {
	tests := []struct {
		name    string
		req     JSONRPCRequest
		wantErr bool
	}{
		{
			name: "valid request with params",
			req: JSONRPCRequest{
				JSONRPC: "2.0",
				ID:      1,
				Method:  "test_method",
				Params:  json.RawMessage(`{"param1":"value1"}`),
			},
			wantErr: false,
		},
		{
			name: "valid request without params",
			req: JSONRPCRequest{
				JSONRPC: "2.0",
				ID:      "test-id",
				Method:  "test_method",
			},
			wantErr: false,
		},
		{
			name: "valid notification (no ID)",
			req: JSONRPCRequest{
				JSONRPC: "2.0",
				Method:  "notify",
				Params:  json.RawMessage(`{"data":"test"}`),
			},
			wantErr: false,
		},
		{
			name: "request with nil params",
			req: JSONRPCRequest{
				JSONRPC: "2.0",
				ID:      1,
				Method:  "test",
				Params:  nil,
			},
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			data, err := json.Marshal(tt.req)
			if (err != nil) != tt.wantErr {
				t.Errorf("Marshal() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr {
				if len(data) == 0 {
					t.Error("Marshal() returned empty data")
				}

				// Verify it can be unmarshaled back
				var unmarshaled JSONRPCRequest
				if err := json.Unmarshal(data, &unmarshaled); err != nil {
					t.Errorf("Unmarshal failed: %v", err)
				}
			}
		})
	}
}

func TestJSONRPCResponse_Unmarshal(t *testing.T) {
	tests := []struct {
		name    string
		data    string
		wantErr bool
		check   func(*JSONRPCResponse) bool
	}{
		{
			name: "valid response with result",
			data: `{"jsonrpc":"2.0","id":1,"result":{"status":"ok"}}`,
			wantErr: false,
			check: func(resp *JSONRPCResponse) bool {
				return resp.Result != nil && resp.Error == nil
			},
		},
		{
			name:    "valid response with error",
			data:    `{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid Request"}}`,
			wantErr: false,
			check: func(resp *JSONRPCResponse) bool {
				return resp.Result == nil && resp.Error != nil && resp.Error.Code == -32600
			},
		},
		{
			name:    "response with string ID",
			data:    `{"jsonrpc":"2.0","id":"test-id","result":{}}`,
			wantErr: false,
			check: func(resp *JSONRPCResponse) bool {
				return resp.ID == "test-id"
			},
		},
		{
			name:    "response with null ID (notification)",
			data:    `{"jsonrpc":"2.0","id":null,"result":{}}`,
			wantErr: false,
			check: func(resp *JSONRPCResponse) bool {
				return resp.ID == nil
			},
		},
		{
			name:    "invalid JSON",
			data:    `{invalid json}`,
			wantErr: true,
			check:   nil,
		},
		{
			name:    "missing jsonrpc field",
			data:    `{"id":1,"result":{}}`,
			wantErr: false, // JSON unmarshal won't fail, but validation might
			check:   nil,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var resp JSONRPCResponse
			err := json.Unmarshal([]byte(tt.data), &resp)
			if (err != nil) != tt.wantErr {
				t.Errorf("Unmarshal() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr && tt.check != nil {
				if !tt.check(&resp) {
					t.Error("Unmarshal() produced invalid response")
				}
			}
		})
	}
}

func TestRPCError(t *testing.T) {
	tests := []struct {
		name    string
		error   RPCError
		data    string
		wantErr bool
	}{
		{
			name: "error with data",
			error: RPCError{
				Code:    -32601,
				Message: "Method not found",
				Data:    "test_data",
			},
			data:    `{"code":-32601,"message":"Method not found","data":"test_data"}`,
			wantErr: false,
		},
		{
			name: "error without data",
			error: RPCError{
				Code:    -32600,
				Message: "Invalid Request",
			},
			data:    `{"code":-32600,"message":"Invalid Request"}`,
			wantErr: false,
		},
		{
			name: "standard JSON-RPC errors",
			error: RPCError{
				Code:    -32700,
				Message: "Parse error",
			},
			data:    `{"code":-32700,"message":"Parse error"}`,
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Test marshal
			data, err := json.Marshal(tt.error)
			if (err != nil) != tt.wantErr {
				t.Errorf("Marshal() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			// Test unmarshal
			var unmarshaled RPCError
			err = json.Unmarshal(data, &unmarshaled)
			if (err != nil) != tt.wantErr {
				t.Errorf("Unmarshal() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			if !tt.wantErr {
				if unmarshaled.Code != tt.error.Code {
					t.Errorf("Code = %v, want %v", unmarshaled.Code, tt.error.Code)
				}
				if unmarshaled.Message != tt.error.Message {
					t.Errorf("Message = %v, want %v", unmarshaled.Message, tt.error.Message)
				}
			}
		})
	}
}

func TestStdioTransport_ConcurrentAccess(t *testing.T) {
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

	// Test concurrent reads of IsConnected
	done := make(chan bool)
	for i := 0; i < 10; i++ {
		go func() {
			trans.IsConnected()
			done <- true
		}()
	}

	// Wait for all goroutines
	for i := 0; i < 10; i++ {
		<-done
	}

	// Should still be connected
	if !trans.IsConnected() {
		t.Error("transport should still be connected after concurrent reads")
	}
}

func TestStdioTransport_Name(t *testing.T) {
	trans, err := NewStdioTransport("echo", []string{}, nil)
	if err != nil {
		t.Fatalf("NewStdioTransport() failed: %v", err)
	}

	name := trans.Name()
	if name != "stdio" {
		t.Errorf("Name() = %v, want stdio", name)
	}
}

// Benchmark for marshaling/unmarshaling JSON-RPC messages
func BenchmarkJSONRPCRequest_Marshal(b *testing.B) {
	req := JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      1,
		Method:  "test_method",
		Params:  json.RawMessage(`{"param1":"value1","param2":"value2"}`),
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_, err := json.Marshal(req)
		if err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkJSONRPCResponse_Unmarshal(b *testing.B) {
	data := []byte(`{"jsonrpc":"2.0","id":1,"result":{"status":"ok","data":"test"}}`)

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		var resp JSONRPCResponse
		err := json.Unmarshal(data, &resp)
		if err != nil {
			b.Fatal(err)
		}
	}
}
