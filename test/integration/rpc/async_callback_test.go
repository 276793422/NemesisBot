// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/cluster"
	clusterrpc "github.com/276793422/NemesisBot/module/cluster/rpc"
	"github.com/276793422/NemesisBot/module/cluster/transport"
)

// testNodeRegistry 全局测试节点注册表（用于节点间路由）
var testNodeRegistry = &struct {
	sync.RWMutex
	nodes map[string]*asyncTestNode // nodeID → node
}{
	nodes: make(map[string]*asyncTestNode),
}

// TestTwoNodeAsyncRPCCallback 测试完整的异步回调流程：
// 1. A 提交 peer_chat → B 立即返回 ACK
// 2. B 异步处理 LLM → 回调 A 的 peer_chat_callback
// 3. A 匹配 taskID → onTaskComplete 回调触发
func TestTwoNodeAsyncRPCCallback(t *testing.T) {
	t.Log("=== Two-Node Async RPC Callback Integration Test (Phase 2) ===")

	// ---- 创建节点 A（请求方） ----
	nodeA := createAsyncTestNode("Node-A", 0)
	// ---- 创建节点 B（处理方） ----
	nodeB := createAsyncTestNode("Node-B", 0)

	// 启动 B
	if err := nodeB.Start(); err != nil {
		t.Fatalf("Failed to start Node B: %v", err)
	}
	defer nodeB.Stop()
	t.Logf("Node B started on port %d", nodeB.ActualPort)

	// 启动 A
	if err := nodeA.Start(); err != nil {
		t.Fatalf("Failed to start Node A: %v", err)
	}
	defer nodeA.Stop()
	t.Logf("Node A started on port %d", nodeA.ActualPort)

	// 注册节点到全局注册表（用于回调路由）
	testNodeRegistry.Lock()
	testNodeRegistry.nodes["Node-A"] = nodeA
	testNodeRegistry.nodes["Node-B"] = nodeB
	testNodeRegistry.Unlock()
	defer func() {
		testNodeRegistry.Lock()
		delete(testNodeRegistry.nodes, "Node-A")
		delete(testNodeRegistry.nodes, "Node-B")
		testNodeRegistry.Unlock()
	}()

	// 等待服务器就绪
	time.Sleep(300 * time.Millisecond)

	// ---- 步骤 1: A 提交异步任务 ----
	taskID := fmt.Sprintf("test-task-%d", time.Now().UnixNano())
	payload := map[string]interface{}{
		"type":    "chat",
		"content": "Hello from Node A!",
		"_source": map[string]interface{}{
			"node_id":   "Node-A",
			"addresses": []string{"127.0.0.1"},
			"rpc_port":  nodeA.ActualPort,
		},
		"task_id": taskID,
	}

	// A 提交任务到 TaskManager
	task := &cluster.Task{
		ID:              taskID,
		Action:          "peer_chat",
		PeerID:          "Node-B",
		Payload:         payload,
		Status:          cluster.TaskPending,
		CreatedAt:       time.Now(),
		OriginalChannel: "web",
		OriginalChatID:  "test-chat",
	}
	if err := nodeA.taskManager.Submit(task); err != nil {
		t.Fatalf("Failed to submit task: %v", err)
	}
	t.Logf("Step 1: Task %s submitted on Node A", taskID)

	// ---- 步骤 2: A 发送 peer_chat 到 B（短同步获取 ACK） ----
	bAddress := fmt.Sprintf("127.0.0.1:%d", nodeB.ActualPort)
	ackResult, err := nodeA.SendRPCRequest(bAddress, "peer_chat", payload)
	if err != nil {
		t.Fatalf("Failed to send RPC request to B: %v", err)
	}
	t.Logf("Step 2: Node A received ACK from B: %s", ackResult)

	// 验证 ACK
	var ack map[string]interface{}
	if err := json.Unmarshal([]byte(ackResult), &ack); err != nil {
		t.Fatalf("Failed to parse ACK: %v", err)
	}
	if ack["status"] != "accepted" {
		t.Fatalf("Expected ACK status 'accepted', got '%v'", ack["status"])
	}
	t.Logf("  ACK verified: status=accepted, task_id=%v", ack["task_id"])

	// ---- 步骤 3: 等待 B 异步处理 + 回调 A ----
	// Phase 2: 轮询等待任务完成（因为不再有阻塞的 WaitForTask）
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	var result *cluster.Task
	for {
		select {
		case <-ctx.Done():
			t.Fatalf("Timeout waiting for task completion")
		default:
		}
		gotTask, err := nodeA.taskManager.GetTask(taskID)
		if err != nil {
			t.Fatalf("GetTask failed: %v", err)
		}
		if gotTask.Status == cluster.TaskCompleted || gotTask.Status == cluster.TaskFailed {
			result = gotTask
			break
		}
		time.Sleep(100 * time.Millisecond)
	}

	t.Logf("Step 3: Callback received!")
	t.Logf("  status=%s", result.Status)
	t.Logf("  response=%s", result.Response)

	if result.Status != cluster.TaskCompleted {
		t.Errorf("Expected task status 'completed', got '%s'", result.Status)
	}
	if result.Response == "" {
		t.Error("Expected non-empty response in callback result")
	}

	t.Log("\n=== Two-Node Async RPC Callback Test PASSED ===")
}

// TestTwoNodeAsyncRPC_MissingContent 测试空 content 时直接返回错误
func TestTwoNodeAsyncRPC_MissingContent(t *testing.T) {
	t.Log("=== Two-Node Async RPC Missing Content Test ===")

	nodeB := createAsyncTestNode("Node-B-Content", 0)
	if err := nodeB.Start(); err != nil {
		t.Fatalf("Failed to start Node B: %v", err)
	}
	defer nodeB.Stop()

	time.Sleep(300 * time.Millisecond)

	payload := map[string]interface{}{
		"type":    "chat",
		"content": "", // 空 content
	}

	bAddress := fmt.Sprintf("127.0.0.1:%d", nodeB.ActualPort)
	result, err := nodeB.SendRPCRequest(bAddress, "peer_chat", payload)
	if err != nil {
		t.Fatalf("RPC request failed: %v", err)
	}

	var response map[string]interface{}
	json.Unmarshal([]byte(result), &response)

	if response["status"] != "error" {
		t.Errorf("Expected error for empty content, got: %v", response)
	} else {
		t.Logf("Correctly rejected empty content: %v", response["response"])
	}

	t.Log("=== Missing Content Test PASSED ===")
}

// TestTaskManagerOnComplete 测试 onTaskComplete 回调（Phase 2 核心功能）
func TestTaskManagerOnComplete(t *testing.T) {
	t.Log("=== TaskManager onTaskComplete Callback Test ===")

	nodeA := createAsyncTestNode("Node-A-CB", 0)
	if err := nodeA.Start(); err != nil {
		t.Fatalf("Failed to start Node A: %v", err)
	}
	defer nodeA.Stop()

	var callbackTaskID string
	var cbMu sync.Mutex
	nodeA.taskManager.SetOnComplete(func(tid string) {
		cbMu.Lock()
		callbackTaskID = tid
		cbMu.Unlock()
	})

	taskID := fmt.Sprintf("cb-task-%d", time.Now().UnixNano())
	task := &cluster.Task{
		ID:              taskID,
		Action:          "peer_chat",
		PeerID:          "Node-B",
		Status:          cluster.TaskPending,
		CreatedAt:       time.Now(),
		OriginalChannel: "web",
		OriginalChatID:  "test-chat",
	}

	if err := nodeA.taskManager.Submit(task); err != nil {
		t.Fatalf("Submit failed: %v", err)
	}

	// 通过 CompleteCallback 模拟回调完成
	if err := nodeA.taskManager.CompleteCallback(taskID, "success", "Hello response", ""); err != nil {
		t.Fatalf("CompleteCallback failed: %v", err)
	}

	// 等待回调触发
	time.Sleep(100 * time.Millisecond)

	cbMu.Lock()
	if callbackTaskID != taskID {
		t.Errorf("Expected callback taskID '%s', got '%s'", taskID, callbackTaskID)
	}
	cbMu.Unlock()

	t.Logf("Callback correctly triggered for task: %s", taskID)
	t.Log("=== onTaskComplete Callback Test PASSED ===")
}

// asyncTestNode 异步测试节点
type asyncTestNode struct {
	Name        string
	ActualPort  int
	msgBus      *bus.MessageBus
	rpcCh       *channels.RPCChannel
	rpcSrv      *clusterrpc.Server
	taskManager *cluster.TaskManager
	cancel      context.CancelFunc
}

// createAsyncTestNode 创建异步测试节点
func createAsyncTestNode(name string, port int) *asyncTestNode {
	msgBus := bus.NewMessageBus()

	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  24 * time.Hour,
		CleanupInterval: 5 * time.Second,
	}
	rpcCh, _ := channels.NewRPCChannel(cfg)

	clusterMock := &asyncTestCluster{
		name: name,
	}
	rpcSrv := clusterrpc.NewServer(clusterMock)

	return &asyncTestNode{
		Name:        name,
		msgBus:      msgBus,
		rpcCh:       rpcCh,
		rpcSrv:      rpcSrv,
		taskManager: cluster.NewTaskManager(10 * time.Second),
	}
}

// Start 启动测试节点
func (n *asyncTestNode) Start() error {
	ctx, cancel := context.WithCancel(context.Background())
	n.cancel = cancel

	// 启动 TaskManager
	n.taskManager.Start()

	// 启动 RPCChannel
	if err := n.rpcCh.Start(ctx); err != nil {
		cancel()
		return fmt.Errorf("failed to start RPC channel: %w", err)
	}

	// 启动 dispatch loop（路由 outbound 消息到 RPCChannel）
	channelMap := map[string]interface{}{"rpc": n.rpcCh}
	startTestDispatchLoop(ctx, n.msgBus, channelMap)

	// 启动模拟 LLM goroutine：
	// 监听 InboundMessage，模拟 LLM 响应（通过 OutboundMessage）
	n.startSimulatedLLM(ctx)

	// 注册 peer_chat handler（使用真实的异步 PeerChatHandler）
	clusterMock := &asyncTestCluster{
		name: n.Name,
	}
	peerChatHandler := clusterrpc.NewPeerChatHandler(clusterMock, n.rpcCh)
	n.rpcSrv.RegisterHandler("peer_chat", peerChatHandler.Handle)

	// 注册 callback handler
	n.rpcSrv.RegisterHandler("peer_chat_callback", func(payload map[string]interface{}) (map[string]interface{}, error) {
		taskID, _ := payload["task_id"].(string)
		status, _ := payload["status"].(string)
		response, _ := payload["response"].(string)
		errMsg, _ := payload["error"].(string)

		if taskID == "" {
			return map[string]interface{}{
				"status": "error",
				"error":  "task_id is required",
			}, nil
		}

		if err := n.taskManager.CompleteCallback(taskID, status, response, errMsg); err != nil {
			return map[string]interface{}{
				"status":  "error",
				"task_id": taskID,
				"error":   err.Error(),
			}, nil
		}

		return map[string]interface{}{
			"status":  "received",
			"task_id": taskID,
		}, nil
	})

	// 启动 RPC Server
	if err := n.rpcSrv.Start(0); err != nil {
		cancel()
		return fmt.Errorf("failed to start RPC server: %w", err)
	}
	n.ActualPort = n.rpcSrv.GetPort()

	// 更新 cluster mock 的端口
	clusterMock.localPort = n.ActualPort

	return nil
}

// Stop 停止测试节点
func (n *asyncTestNode) Stop() {
	n.taskManager.Stop()
	n.rpcCh.Stop(context.Background())
	n.rpcSrv.Stop()
	if n.cancel != nil {
		n.cancel()
	}
}

// startSimulatedLLM 模拟 LLM 处理：
// 通过 ConsumeInbound 监听入站消息，发送带 CorrelationID 前缀的 OutboundMessage
func (n *asyncTestNode) startSimulatedLLM(ctx context.Context) {
	go func() {
		for {
			msg, ok := n.msgBus.ConsumeInbound(ctx)
			if !ok {
				return
			}
			// 模拟 LLM 延迟
			time.Sleep(100 * time.Millisecond)

			// 构造带 CorrelationID 前缀的响应
			response := fmt.Sprintf("[rpc:%s] Simulated LLM response to: %s",
				msg.CorrelationID, msg.Content)

			// 通过 OutboundMessage 返回（dispatch loop 会路由到 RPCChannel）
			n.msgBus.PublishOutbound(bus.OutboundMessage{
				Channel: "rpc",
				ChatID:  msg.ChatID,
				Content: response,
			})
		}
	}()
}

// SendRPCRequest 发送 RPC 请求
func (n *asyncTestNode) SendRPCRequest(address, action string, payload map[string]interface{}) (string, error) {
	conn, err := net.DialTimeout("tcp", address, 5*time.Second)
	if err != nil {
		return "", fmt.Errorf("failed to dial %s: %w", address, err)
	}
	defer conn.Close()

	tcpConn := transport.NewTCPConn(conn, nil)
	tcpConn.Start()

	req := transport.NewRequest(n.Name, "remote", action, payload)
	if err := tcpConn.Send(req); err != nil {
		return "", fmt.Errorf("failed to send: %w", err)
	}

	respCh := tcpConn.Receive()
	select {
	case msg := <-respCh:
		if msg.Type == transport.RPCTypeError {
			return "", fmt.Errorf("RPC error: %s", msg.Error)
		}
		if msg.Type == transport.RPCTypeResponse {
			responseBytes, _ := json.Marshal(msg.Payload)
			return string(responseBytes), nil
		}
		return "", fmt.Errorf("unexpected message type: %s", msg.Type)
	case <-time.After(10 * time.Second):
		return "", fmt.Errorf("timeout waiting for response")
	}
}

// asyncTestCluster 实现 clusterrpc.Cluster 接口
type asyncTestCluster struct {
	name      string
	localPort int
}

func (m *asyncTestCluster) GetRegistry() interface{}        { return nil }
func (m *asyncTestCluster) GetNodeID() string               { return m.name }
func (m *asyncTestCluster) GetAddress() string              { return fmt.Sprintf("127.0.0.1:%d", m.localPort) }
func (m *asyncTestCluster) GetCapabilities() []string       { return []string{"peer_chat", "llm"} }
func (m *asyncTestCluster) GetOnlinePeers() []interface{}   { return nil }
func (m *asyncTestCluster) GetActionsSchema() []interface{} { return nil }
func (m *asyncTestCluster) LogRPCInfo(msg string, args ...interface{}) {
	fmt.Printf("[INFO][%s] %s\n", m.name, fmt.Sprintf(msg, args...))
}
func (m *asyncTestCluster) LogRPCError(msg string, args ...interface{}) {
	fmt.Printf("[ERROR][%s] %s\n", m.name, fmt.Sprintf(msg, args...))
}
func (m *asyncTestCluster) LogRPCDebug(msg string, args ...interface{}) {}
func (m *asyncTestCluster) GetPeer(peerID string) (interface{}, error)   { return nil, nil }
func (m *asyncTestCluster) GetLocalNetworkInterfaces() ([]clusterrpc.LocalNetworkInterface, error) {
	return []clusterrpc.LocalNetworkInterface{{IP: "127.0.0.1", Mask: "255.255.255.0"}}, nil
}
func (m *asyncTestCluster) GetTaskResultStorer() clusterrpc.TaskResultStorer { return nil }

// CallWithContext 实现 B 端回调 A 端
// PeerChatHandler.sendCallback 通过此方法发送回调
func (m *asyncTestCluster) CallWithContext(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	// 从全局注册表查找目标节点
	testNodeRegistry.RLock()
	targetNode, exists := testNodeRegistry.nodes[peerID]
	testNodeRegistry.RUnlock()

	if !exists {
		return nil, fmt.Errorf("peer not found: %s", peerID)
	}

	// 通过 TCP 连接发送回调请求到目标节点
	address := fmt.Sprintf("127.0.0.1:%d", targetNode.ActualPort)
	result, err := m.sendCallbackTCP(address, action, payload)
	if err != nil {
		return nil, err
	}

	return result, nil
}

// sendCallbackTCP 通过 TCP 发送回调请求
func (m *asyncTestCluster) sendCallbackTCP(address, action string, payload map[string]interface{}) ([]byte, error) {
	conn, err := net.DialTimeout("tcp", address, 5*time.Second)
	if err != nil {
		return nil, fmt.Errorf("callback dial failed: %w", err)
	}
	defer conn.Close()

	tcpConn := transport.NewTCPConn(conn, nil)
	tcpConn.Start()

	req := transport.NewRequest(m.name, "remote", action, payload)
	if err := tcpConn.Send(req); err != nil {
		return nil, fmt.Errorf("callback send failed: %w", err)
	}

	respCh := tcpConn.Receive()
	select {
	case msg := <-respCh:
		if msg.Type == transport.RPCTypeError {
			return nil, fmt.Errorf("callback RPC error: %s", msg.Error)
		}
		if msg.Type == transport.RPCTypeResponse {
			responseBytes, _ := json.Marshal(msg.Payload)
			return responseBytes, nil
		}
		return nil, fmt.Errorf("unexpected callback response type: %s", msg.Type)
	case <-time.After(10 * time.Second):
		return nil, fmt.Errorf("callback timeout")
	}
}
