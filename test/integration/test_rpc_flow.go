// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package main

import (
	"fmt"
	"log"
	"net"
	"os"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/rpc"
	"github.com/276793422/NemesisBot/module/cluster/transport"
)

// 模拟的 Cluster 接口
type mockCluster struct {
	nodeID string
}

func (m *mockCluster) GetNodeID() string {
	return m.nodeID
}

func (m *mockCluster) LogRPCInfo(format string, args ...interface{}) {
	log.Printf("[RPC INFO] "+format+"\n", args...)
}

func (m *mockCluster) LogRPCError(format string, args ...interface{}) {
	log.Printf("[RPC ERROR] "+format+"\n", args...)
}

func (m *mockCluster) LogRPCDebug(format string, args ...interface{}) {
	log.Printf("[RPC DEBUG] "+format+"\n", args...)
}

func main() {
	if len(os.Args) < 2 {
		fmt.Println("Usage: go run rpc_test.go <server|client>")
		os.Exit(1)
	}

	mode := os.Args[1]

	if mode == "server" {
		runServer()
	} else if mode == "client" {
		runClient()
	} else {
		fmt.Println("Unknown mode:", mode)
		os.Exit(1)
	}
}

func runServer() {
	fmt.Println("=== RPC Server Test ===")
	log.Println("Starting server...")

	// 创建 mock cluster
	cluster := &mockCluster{nodeID: "server-node"}

	// 创建 RPC server
	server := rpc.NewServer(cluster)
	port := 21949

	// 注册 hello handler
	server.RegisterHandler("hello", func(payload map[string]interface{}) (map[string]interface{}, error) {
		log.Println("[HANDLER] Hello handler called!")
		log.Printf("[HANDLER] Received payload: %+v\n", payload)

		// 提取参数
		from := ""
		if f, ok := payload["from"].(string); ok {
			from = f
		}

		timestamp := ""
		if ts, ok := payload["timestamp"].(string); ok {
			timestamp = ts
		}

		log.Printf("[HANDLER] Extracted: from=%s, timestamp=%s\n", from, timestamp)

		// 构建响应
		response := map[string]interface{}{
			"greeting":  fmt.Sprintf("Hello! Received your greeting from %s", from),
			"timestamp": time.Now().Format(time.RFC3339),
			"node_id":   "server-node",
			"status":    "ok",
		}

		log.Printf("[HANDLER] Sending response: %+v\n", response)

		return response, nil
	})

	// 启动服务器
	if err := server.Start(port); err != nil {
		log.Fatalf("Failed to start server: %v", err)
	}

	log.Println("Server started on port", port)
	log.Println("Waiting for connections...")
	log.Println("")
	log.Println("Press Ctrl+C to stop")

	// 保持运行
	select {}
}

func runClient() {
	fmt.Println("=== RPC Client Test ===")
	log.Println("Starting client...")

	// 创建 mock cluster
	cluster := &mockCluster{nodeID: "client-node"}

	// 创建 RPC client
	client := rpc.NewClient(cluster)

	// 连接到服务器
	address := "127.0.0.1:21949"
	peerID := "server-node"

	// 等待服务器启动
	log.Println("Waiting 2 seconds for server to start...")
	time.Sleep(2 * time.Second)

	log.Println("Connecting to server at", address)

	// 创建连接
	conn, err := net.Dial("tcp", address)
	if err != nil {
		log.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	log.Println("Connected! Creating RPC message...")

	// 创建 RPC 请求消息
	req := transport.NewRequest("client-node", "server-node", "hello", map[string]interface{}{
		"from":      "client-node",
		"timestamp": time.Now().Format(time.RFC3339),
	})

	log.Printf("[CLIENT] Sending request: action=%s, from=%s, id=%s\n", req.Action, req.From, req.ID)
	log.Printf("[CLIENT] Payload: %+v\n", req.Payload)

	// 发送请求
	log.Println("[CLIENT] Sending request to server...")
	if err := conn.Write(transport.MessageToBytes(req)); err != nil {
		log.Fatalf("Failed to send: %v", err)
	}

	log.Println("[CLIENT] Request sent, waiting for response...")

	// 读取响应
	buf := make([]byte, 4096)
	n, err := conn.Read(buf)
	if err != nil {
		log.Fatalf("Failed to read response: %v", err)
	}

	log.Println("[CLIENT] Received response!")

	// 解析响应
	resp, err := transport.BytesToMessage(buf[:n])
	if err != nil {
		log.Fatalf("Failed to parse response: %v", err)
	}

	log.Printf("[CLIENT] Response type: %s\n", resp.Type)
	log.Printf("[CLIENT] Response action: %s\n", resp.Action)
	log.Printf("[CLIENT] Response from: %s\n", resp.From)
	log.Printf("[CLIENT] Response id: %s\n", resp.ID)
	log.Printf("[CLIENT] Response payload: %+v\n", resp.Payload)

	fmt.Println("")
	fmt.Println("=== Test Complete ===")
	fmt.Println("Check the logs above to see the complete flow")
}
