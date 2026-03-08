// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

//go:build ignore
// +build ignore

package main

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"time"
)

type JSONRPCRequest struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      interface{}     `json:"id,omitempty"`
	Method  string          `json:"method"`
	Params  json.RawMessage `json:"params,omitempty"`
}

type JSONRPCResponse struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      interface{}     `json:"id"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *RPCError       `json:"error,omitempty"`
}

type RPCError struct {
	Code    int         `json:"code"`
	Message string      `json:"message"`
	Data    interface{} `json:"data,omitempty"`
}

func main() {
	// Start test server
	cmd := exec.Command("C:\\AI\\NemesisBot\\NemesisBot_go\\test\\mcp\\server\\mcp-test-server.exe")
	stdin, err := cmd.StdinPipe()
	if err != nil {
		fmt.Printf("Failed to create stdin pipe: %v\n", err)
		os.Exit(1)
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		fmt.Printf("Failed to create stdout pipe: %v\n", err)
		os.Exit(1)
	}

	if err := cmd.Start(); err != nil {
		fmt.Printf("Failed to start server: %v\n", err)
		os.Exit(1)
	}

	defer cmd.Process.Kill()

	// Give server time to start
	time.Sleep(100 * time.Millisecond)

	// Test 1: Initialize
	initReq := JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      1,
		Method:  "initialize",
		Params:  json.RawMessage(`{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}`),
	}

	fmt.Println("Test 1: Sending initialize request")
	sendAndReceive(stdin, stdout, initReq)

	// Test 2: List tools
	listReq := JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      2,
		Method:  "tools/list",
	}

	fmt.Println("\nTest 2: Sending tools/list request")
	resp := sendAndReceive(stdin, stdout, listReq)

	// Parse response
	var result struct {
		Tools []struct {
			Name string `json:"name"`
		} `json:"tools"`
	}
	if err := json.Unmarshal(resp.Result, &result); err != nil {
		fmt.Printf("Failed to parse tools list: %v\n", err)
	} else {
		fmt.Printf("Found %d tools:\n", len(result.Tools))
		for _, tool := range result.Tools {
			fmt.Printf("  - %s\n", tool.Name)
		}
	}

	// Test 3: Call echo tool
	callReq := JSONRPCRequest{
		JSONRPC: "2.0",
		ID:      3,
		Method:  "tools/call",
		Params:  json.RawMessage(`{"name":"echo","arguments":{"text":"Hello from test!"}}`),
	}

	fmt.Println("\nTest 3: Calling echo tool")
	sendAndReceive(stdin, stdout, callReq)

	fmt.Println("\nAll tests completed successfully!")
	time.Sleep(100 * time.Millisecond)
}

func sendAndReceive(stdin io.WriteCloser, stdout io.Reader, req JSONRPCRequest) *JSONRPCResponse {
	data, err := json.Marshal(req)
	if err != nil {
		fmt.Printf("Failed to marshal request: %v\n", err)
		os.Exit(1)
	}

	// Send request
	fmt.Fprintf(stdin, "%s\n", string(data))

	// Read response
	buf := make([]byte, 4096)
	n, err := stdout.Read(buf)
	if err != nil {
		fmt.Printf("Failed to read response: %v\n", err)
		os.Exit(1)
	}

	var resp JSONRPCResponse
	if err := json.Unmarshal(buf[:n], &resp); err != nil {
		fmt.Printf("Failed to unmarshal response: %v (raw: %s)\n", err, string(buf[:n]))
		os.Exit(1)
	}

	fmt.Printf("Response: %s\n", string(buf[:n]))
	return &resp
}
