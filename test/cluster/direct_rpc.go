// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package main

import (
	"encoding/json"
	"fmt"
	"net"
	"os"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/rpc"
	"github.com/276793422/NemesisBot/module/cluster/transport"
)

func main() {
	fmt.Println("===========================================")
	fmt.Println("Direct TCP RPC Test")
	fmt.Println("===========================================")
	fmt.Println()

	// Start a simple RPC server
	fmt.Println("[1] Starting RPC server on port 21951...")

	// Create a mock cluster for the server
	mockCluster := &rpcServerCluster{}
	server := rpc.NewServer(mockCluster)

	// Register a test handler
	server.RegisterHandler("test", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{
			"status":  "success",
			"message": "Test handler executed",
			"payload": payload,
		}, nil
	})

	// Start server
	go func() {
		if err := server.Start(21951); err != nil {
			fmt.Printf("Server error: %v\n", err)
		}
	}()

	time.Sleep(1 * time.Second)
	fmt.Println("✓ Server started")
	fmt.Println()

	// Create a direct TCP connection to the server
	fmt.Println("[2] Testing direct TCP connection...")

	conn, err := net.Dial("tcp", "127.0.0.1:21951")
	if err != nil {
		fmt.Printf("✗ Failed to connect: %v\n", err)
		os.Exit(1)
	}
	defer conn.Close()
	fmt.Println("✓ Connected to 127.0.0.1:21951")
	fmt.Println()

	// Create an RPC request
	fmt.Println("[3] Sending RPC request...")

	req := transport.NewRequest("test-client", "test-server", "test", map[string]interface{}{
		"message": "Hello from client!",
		"time":    time.Now().Unix(),
	})

	reqData, _ := req.Bytes()
	frameData, _ := transport.EncodeFrame(reqData)

	_, err = conn.Write(frameData)
	if err != nil {
		fmt.Printf("✗ Failed to send: %v\n", err)
		os.Exit(1)
	}
	fmt.Println("✓ Request sent")
	fmt.Println()

	// Read response
	fmt.Println("[4] Reading response...")

	conn.SetReadDeadline(time.Now().Add(5 * time.Second))
	respData, err := transport.DecodeFrame(conn)
	if err != nil {
		fmt.Printf("✗ Failed to read response: %v\n", err)
		os.Exit(1)
	}

	var resp transport.RPCMessage
	if err := json.Unmarshal(respData, &resp); err != nil {
		fmt.Printf("✗ Failed to unmarshal response: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("✓ Response received\n")
	fmt.Printf("  Type: %s\n", resp.Type)
	fmt.Printf("  Action: %s\n", resp.Action)
	fmt.Printf("  Payload: %v\n", resp.Payload)
	fmt.Println()

	// Stop server
	fmt.Println("[5] Stopping server...")
	server.Stop()
	fmt.Println("✓ Server stopped")
	fmt.Println()

	fmt.Println("===========================================")
	fmt.Println("Direct TCP RPC Test PASSED")
	fmt.Println("===========================================")
}

// rpcServerCluster implements the Cluster interface for the server
type rpcServerCluster struct{}

func (m *rpcServerCluster) GetRegistry() interface{}                      { return nil }
func (m *rpcServerCluster) GetNodeID() string                            { return "test-server" }
func (m *rpcServerCluster) GetAddress() string                           { return "" }
func (m *rpcServerCluster) GetCapabilities() []string                    { return []string{"test"} }
func (m *rpcServerCluster) GetOnlinePeers() []interface{}                { return nil }
func (m *rpcServerCluster) LogRPCInfo(msg string, args ...interface{})   { fmt.Printf("[INFO] %s\n", fmt.Sprintf(msg, args...)) }
func (m *rpcServerCluster) LogRPCError(msg string, args ...interface{})  { fmt.Printf("[ERROR] %s\n", fmt.Sprintf(msg, args...)) }
func (m *rpcServerCluster) LogRPCDebug(msg string, args ...interface{})  { fmt.Printf("[DEBUG] %s\n", fmt.Sprintf(msg, args...)) }
func (m *rpcServerCluster) GetPeer(peerID string) (interface{}, error)   { return nil, nil }
func (m *rpcServerCluster) GetLocalNetworkInterfaces() ([]rpc.LocalNetworkInterface, error) {
	return []rpc.LocalNetworkInterface{
		{IP: "127.0.0.1", Mask: "255.255.255.0"},
	}, nil
}
