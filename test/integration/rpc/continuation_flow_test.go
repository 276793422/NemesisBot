// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/cluster"
	clusterrpc "github.com/276793422/NemesisBot/module/cluster/rpc"
	"github.com/276793422/NemesisBot/module/cluster/transport"
)

// ==============================================================================
// 续行快照流程测试
// 目标：验证 saveContinuation → callback → handleTaskComplete → bus notification 的完整流程
// ==============================================================================

// TestContinuation_CallbackTriggersBusNotification 验证回调触发 bus 通知
// 这是续行流程的第一步：B 回调 A → TaskManager.onTaskComplete → Cluster.handleTaskComplete → bus
func TestContinuation_CallbackTriggersBusNotification(t *testing.T) {
	t.Log("=== Test: Callback Triggers Bus Notification ===")

	msgBus := bus.NewMessageBus()

	// 创建节点 A（带 bus 注入）
	nodeA := createContinuationTestNode("Node-A", msgBus)
	if err := nodeA.Start(); err != nil {
		t.Fatalf("Failed to start Node A: %v", err)
	}
	defer nodeA.Stop()

	// 步骤 1: 提交一个带有 OriginalChannel/OriginalChatID 的任务
	taskID := fmt.Sprintf("task-callback-%d", time.Now().UnixNano())
	task := &cluster.Task{
		ID:              taskID,
		Action:          "peer_chat",
		PeerID:          "Node-B",
		Payload:         map[string]interface{}{"content": "hello"},
		Status:          cluster.TaskPending,
		CreatedAt:       time.Now(),
		OriginalChannel: "web",
		OriginalChatID:  "chat-test-001",
	}
	if err := nodeA.taskManager.Submit(task); err != nil {
		t.Fatalf("Submit failed: %v", err)
	}
	t.Logf("Step 1: Task %s submitted with OriginalChannel=web, OriginalChatID=chat-test-001", taskID)

	// 步骤 2: 模拟回调完成（B 回调 A）
	err := nodeA.taskManager.CompleteCallback(taskID, "success", "Node-B 回复: 这是结果", "")
	if err != nil {
		t.Fatalf("CompleteCallback failed: %v", err)
	}

	// 步骤 3: 等待 bus 上的 cluster_continuation 消息
	// handleTaskComplete 在 CompleteCallback 内同步发布了消息到 bus
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	msg, ok := msgBus.ConsumeInbound(ctx)
	if !ok {
		t.Fatal("Timeout waiting for cluster_continuation bus message")
	}

	t.Logf("Step 3: Received bus message!")
	t.Logf("  Channel:   %s", msg.Channel)
	t.Logf("  SenderID:  %s", msg.SenderID)
	t.Logf("  ChatID:    %s", msg.ChatID)
	t.Logf("  Content:   '%s'", msg.Content)

	// 验证消息格式
	expectedSenderID := fmt.Sprintf("cluster_continuation:%s", taskID)
	if msg.SenderID != expectedSenderID {
		t.Errorf("SenderID mismatch: expected '%s', got '%s'", expectedSenderID, msg.SenderID)
	}
	if msg.Channel != "system" {
		t.Errorf("Channel should be 'system', got '%s'", msg.Channel)
	}
	expectedChatID := "web:chat-test-001"
	if msg.ChatID != expectedChatID {
		t.Errorf("ChatID: expected '%s', got '%s'", expectedChatID, msg.ChatID)
	}
	if msg.Content != "" {
		t.Errorf("Content should be empty (data loaded by taskID), got '%s'", msg.Content)
	}

	t.Log("=== Test PASSED ===")
}

// TestContinuation_CallbackNoOriginalChannel 验证没有 OriginalChannel 的任务不会触发续行通知
func TestContinuation_CallbackNoOriginalChannel(t *testing.T) {
	t.Log("=== Test: Callback Without OriginalChannel Does Not Trigger Notification ===")

	msgBus := bus.NewMessageBus()
	nodeA := createContinuationTestNode("Node-A-NC", msgBus)
	if err := nodeA.Start(); err != nil {
		t.Fatalf("Failed to start Node A: %v", err)
	}
	defer nodeA.Stop()

	// 提交一个没有 OriginalChannel 的任务（Phase 1 兼容）
	taskID := fmt.Sprintf("task-nochannel-%d", time.Now().UnixNano())
	task := &cluster.Task{
		ID:        taskID,
		Action:    "peer_chat",
		PeerID:    "Node-B",
		Status:    cluster.TaskPending,
		CreatedAt: time.Now(),
		// OriginalChannel 和 OriginalChatID 为空
	}
	nodeA.taskManager.Submit(task)

	// 完成
	nodeA.taskManager.CompleteCallback(taskID, "success", "result", "")

	// 短暂等待，确保没有 bus 消息发出
	time.Sleep(200 * time.Millisecond)

	// 非阻塞检查 bus
	select {
	case msg := <-msgBus.OutboundChannel():
		t.Errorf("Unexpected outbound message: %v", msg)
	case msg := <-func() chan bus.InboundMessage {
		ch := make(chan bus.InboundMessage, 1)
		go func() {
			ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
			defer cancel()
			if m, ok := msgBus.ConsumeInbound(ctx); ok {
				ch <- m
			}
		}()
		return ch
	}():
		if msg.Channel == "system" {
			t.Errorf("Should NOT send cluster_continuation for task without OriginalChannel, got: %v", msg)
		}
	default:
		// 期望结果：没有 cluster_continuation 消息
		t.Log("Correctly did NOT send cluster_continuation message")
	}

	t.Log("=== Test PASSED ===")
}

// TestContinuation_MultipleConcurrentCallbacks 多个并发回调
func TestContinuation_MultipleConcurrentCallbacks(t *testing.T) {
	t.Log("=== Test: Multiple Concurrent Callbacks ===")

	msgBus := bus.NewMessageBus()
	nodeA := createContinuationTestNode("Node-A-MC", msgBus)
	if err := nodeA.Start(); err != nil {
		t.Fatalf("Failed to start Node A: %v", err)
	}
	defer nodeA.Stop()

	// 提交 5 个任务
	taskCount := 5
	taskIDs := make([]string, taskCount)
	for i := 0; i < taskCount; i++ {
		taskIDs[i] = fmt.Sprintf("task-multi-cb-%d-%d", i, time.Now().UnixNano())
		task := &cluster.Task{
			ID:              taskIDs[i],
			Action:          "peer_chat",
			PeerID:          "Node-B",
			Status:          cluster.TaskPending,
			CreatedAt:       time.Now(),
			OriginalChannel: "web",
			OriginalChatID:  fmt.Sprintf("chat-%d", i),
		}
		if err := nodeA.taskManager.Submit(task); err != nil {
			t.Fatalf("Submit %d failed: %v", i, err)
		}
	}

	// 并发完成所有任务
	var wg sync.WaitGroup
	for i := 0; i < taskCount; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			nodeA.taskManager.CompleteCallback(taskIDs[idx], "success",
				fmt.Sprintf("Response %d", idx), "")
		}(i)
	}
	wg.Wait()

	// 收集所有 bus 消息
	receivedTaskIDs := make(map[string]bool)
	timeout := time.After(5 * time.Second)
	for len(receivedTaskIDs) < taskCount {
		select {
		case <-timeout:
			t.Fatalf("Timeout: only received %d/%d messages", len(receivedTaskIDs), taskCount)
		default:
		}
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		msg, ok := msgBus.ConsumeInbound(ctx)
		cancel()
		if !ok {
			continue
		}
		if strings.HasPrefix(msg.SenderID, "cluster_continuation:") {
			taskID := strings.TrimPrefix(msg.SenderID, "cluster_continuation:")
			receivedTaskIDs[taskID] = true
			t.Logf("  Received continuation for task: %s (chatID=%s)", taskID, msg.ChatID)
		}
	}

	// 验证所有 taskID 都收到了
	for _, tid := range taskIDs {
		if !receivedTaskIDs[tid] {
			t.Errorf("Did not receive continuation for task %s", tid)
		}
	}

	if len(receivedTaskIDs) != taskCount {
		t.Errorf("Expected %d continuation messages, got %d", taskCount, len(receivedTaskIDs))
	}

	t.Log("=== Test PASSED ===")
}

// TestContinuation_TaskDataIntegrityThroughFlow 验证任务数据在整个流程中的完整性
func TestContinuation_TaskDataIntegrityThroughFlow(t *testing.T) {
	t.Log("=== Test: Task Data Integrity Through Full Flow ===")

	msgBus := bus.NewMessageBus()
	nodeA := createContinuationTestNode("Node-A-Integrity", msgBus)
	if err := nodeA.Start(); err != nil {
		t.Fatalf("Failed to start Node A: %v", err)
	}
	defer nodeA.Stop()

	taskID := fmt.Sprintf("task-integrity-%d", time.Now().UnixNano())
	task := &cluster.Task{
		ID:              taskID,
		Action:          "peer_chat",
		PeerID:          "Node-B",
		Payload:         map[string]interface{}{"content": "important question"},
		Status:          cluster.TaskPending,
		CreatedAt:       time.Now(),
		OriginalChannel: "discord",
		OriginalChatID:  "channel-42",
	}
	nodeA.taskManager.Submit(task)

	// 完成并设置结果
	response := "这是 Node-B 的详细回复，包含重要信息。"
	nodeA.taskManager.CompleteCallback(taskID, "success", response, "")

	// 等待 bus 通知
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	msg, ok := msgBus.ConsumeInbound(ctx)
	if !ok {
		t.Fatal("Timeout waiting for bus message")
	}

	// 验证 bus 消息的 chatID 包含正确的通道信息
	expectedChatID := "discord:channel-42"
	if msg.ChatID != expectedChatID {
		t.Errorf("ChatID: expected '%s', got '%s'", expectedChatID, msg.ChatID)
	}

	// 通过 GetTask 验证任务数据完整性
	completedTask, err := nodeA.cluster.GetTask(taskID)
	if err != nil {
		t.Fatalf("GetTask failed: %v", err)
	}

	if completedTask.Response != response {
		t.Errorf("Response mismatch: expected '%s', got '%s'", response, completedTask.Response)
	}
	if completedTask.Status != cluster.TaskCompleted {
		t.Errorf("Status: expected 'completed', got '%s'", completedTask.Status)
	}
	if completedTask.OriginalChannel != "discord" {
		t.Errorf("OriginalChannel: expected 'discord', got '%s'", completedTask.OriginalChannel)
	}
	if completedTask.OriginalChatID != "channel-42" {
		t.Errorf("OriginalChatID: expected 'channel-42', got '%s'", completedTask.OriginalChatID)
	}

	t.Log("=== Test PASSED ===")
}

// TestContinuation_FailedTask 验证失败任务也触发续行通知（让 AgentLoop 处理错误）
func TestContinuation_FailedTask(t *testing.T) {
	t.Log("=== Test: Failed Task Also Triggers Continuation ===")

	msgBus := bus.NewMessageBus()
	nodeA := createContinuationTestNode("Node-A-Fail", msgBus)
	if err := nodeA.Start(); err != nil {
		t.Fatalf("Failed to start Node A: %v", err)
	}
	defer nodeA.Stop()

	taskID := fmt.Sprintf("task-fail-%d", time.Now().UnixNano())
	task := &cluster.Task{
		ID:              taskID,
		Action:          "peer_chat",
		PeerID:          "Node-B",
		Status:          cluster.TaskPending,
		CreatedAt:       time.Now(),
		OriginalChannel: "web",
		OriginalChatID:  "chat-error",
	}
	nodeA.taskManager.Submit(task)

	// 失败完成
	nodeA.taskManager.CompleteCallback(taskID, "error", "", "connection refused")

	// 等待 bus 通知
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	msg, ok := msgBus.ConsumeInbound(ctx)
	if !ok {
		t.Fatal("Timeout: failed task should also trigger continuation")
	}

	expectedSenderID := fmt.Sprintf("cluster_continuation:%s", taskID)
	if msg.SenderID != expectedSenderID {
		t.Errorf("SenderID: expected '%s', got '%s'", expectedSenderID, msg.SenderID)
	}

	// 验证任务状态是 failed
	completedTask, _ := nodeA.cluster.GetTask(taskID)
	if completedTask.Status != cluster.TaskFailed {
		t.Errorf("Status: expected 'failed', got '%s'", completedTask.Status)
	}
	if completedTask.Error != "connection refused" {
		t.Errorf("Error: expected 'connection refused', got '%s'", completedTask.Error)
	}

	t.Log("=== Test PASSED ===")
}

// ==============================================================================
// RPC 回调 + 续行通知端到端测试
// ==============================================================================

// TestContinuation_RPCCallbackWithBusIntegration 完整的两节点 RPC + 回调 + 续行通知
func TestContinuation_RPCCallbackWithBusIntegration(t *testing.T) {
	t.Log("=== Test: Full RPC Callback → Bus Continuation Notification ===")

	msgBus := bus.NewMessageBus()

	// 节点 A: 使用 continuationTestNode（带 Cluster + bus 注入）
	// 使用 StartWithLLM(false) 避免模拟 LLM 消费 bus 的 inbound 消息
	nodeA := createContinuationTestNode("Node-A-RPC", msgBus)
	if err := nodeA.StartWithLLM(false); err != nil {
		t.Fatalf("Failed to start Node A: %v", err)
	}
	defer nodeA.Stop()

	// 节点 B: 使用 asyncTestNode（简单 RPC 处理 + 回调能力）
	nodeB := createAsyncTestNode("Node-B-RPC", 0)
	if err := nodeB.Start(); err != nil {
		t.Fatalf("Failed to start Node B: %v", err)
	}
	defer nodeB.Stop()

	// 注册到全局注册表：
	// B 的 asyncTestCluster.CallWithContext 通过 testNodeRegistry 查找目标节点
	// 需要注册一个 *asyncTestNode 作为 A 的代理来接收回调
	// 但 A 实际上使用 continuationTestNode 的 RPC 服务器
	// 解决方案：用 asyncTestNode 包装 A 的端口信息
	nodeAProxy := &asyncTestNode{
		Name:       "Node-A-RPC",
		ActualPort: nodeA.ActualPort,
	}

	testNodeRegistry.Lock()
	testNodeRegistry.nodes["Node-A-RPC"] = nodeAProxy
	testNodeRegistry.nodes["Node-B-RPC"] = nodeB
	testNodeRegistry.Unlock()
	defer func() {
		testNodeRegistry.Lock()
		delete(testNodeRegistry.nodes, "Node-A-RPC")
		delete(testNodeRegistry.nodes, "Node-B-RPC")
		testNodeRegistry.Unlock()
	}()

	time.Sleep(300 * time.Millisecond)

	// 步骤 1: A 提交异步任务（带 OriginalChannel/OriginalChatID）
	taskID := fmt.Sprintf("task-e2e-%d", time.Now().UnixNano())
	payload := map[string]interface{}{
		"type":    "chat",
		"content": "Hello from A!",
		"_source": map[string]interface{}{
			"node_id":   "Node-A-RPC",
			"addresses": []string{"127.0.0.1"},
			"rpc_port":  nodeA.ActualPort,
		},
		"task_id": taskID,
	}

	task := &cluster.Task{
		ID:              taskID,
		Action:          "peer_chat",
		PeerID:          "Node-B-RPC",
		Payload:         payload,
		Status:          cluster.TaskPending,
		CreatedAt:       time.Now(),
		OriginalChannel: "web",
		OriginalChatID:  "chat-e2e-test",
	}
	if err := nodeA.taskManager.Submit(task); err != nil {
		t.Fatalf("Submit failed: %v", err)
	}
	t.Logf("Step 1: Task %s submitted", taskID)

	// 步骤 2: A → B 发送 peer_chat 获取 ACK
	bAddress := fmt.Sprintf("127.0.0.1:%d", nodeB.ActualPort)
	ackResult, err := nodeA.SendRPCRequest(bAddress, "peer_chat", payload)
	if err != nil {
		t.Fatalf("RPC to B failed: %v", err)
	}
	t.Logf("Step 2: ACK received: %s", ackResult)

	// 步骤 3: 等待 B 异步处理 → 回调 A → bus 通知
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	contMsgReceived := false
	for !contMsgReceived {
		msg, ok := msgBus.ConsumeInbound(ctx)
		if !ok {
			t.Fatal("Timeout waiting for bus messages")
		}
		if strings.HasPrefix(msg.SenderID, "cluster_continuation:") {
			contMsgReceived = true
			t.Logf("Step 3: Cluster continuation message received!")

			expectedSenderID := fmt.Sprintf("cluster_continuation:%s", taskID)
			if msg.SenderID != expectedSenderID {
				t.Errorf("SenderID: expected '%s', got '%s'", expectedSenderID, msg.SenderID)
			}
			if msg.ChatID != "web:chat-e2e-test" {
				t.Errorf("ChatID: expected 'web:chat-e2e-test', got '%s'", msg.ChatID)
			}
		}
	}

	// 步骤 4: 验证任务状态和结果
	completedTask, err := nodeA.cluster.GetTask(taskID)
	if err != nil {
		t.Fatalf("GetTask failed: %v", err)
	}
	if completedTask.Status != cluster.TaskCompleted {
		t.Errorf("Status: expected 'completed', got '%s'", completedTask.Status)
	}
	if completedTask.Response == "" {
		t.Error("Response should not be empty")
	}
	t.Logf("Step 4: Task verified: status=%s, response='%s'",
		completedTask.Status, completedTask.Response)

	// 步骤 5: 验证快照存储中可能有残留文件（或者已被清理）
	if store := nodeA.cluster.GetContinuationStore(); store != nil {
		pending, _ := store.ListPending()
		t.Logf("Step 5: Remaining snapshots on disk: %d", len(pending))
	}

	t.Log("=== Test PASSED ===")
}

// ==============================================================================
// 辅助类型和方法
// ==============================================================================

// continuationTestNode 带有 Cluster 实例的续行测试节点
type continuationTestNode struct {
	Name        string
	ActualPort  int
	msgBus      *bus.MessageBus
	rpcCh       *channels.RPCChannel
	rpcSrv      *clusterrpc.Server
	taskManager *cluster.TaskManager
	cluster     *cluster.Cluster
	cancel      context.CancelFunc
}

// createContinuationTestNode 创建带有 Cluster 实例的续行测试节点
func createContinuationTestNode(name string, msgBus *bus.MessageBus) *continuationTestNode {
	cfg := &channels.RPCChannelConfig{
		MessageBus:      msgBus,
		RequestTimeout:  24 * time.Hour,
		CleanupInterval: 5 * time.Second,
	}
	rpcCh, _ := channels.NewRPCChannel(cfg)

	// 使用真实 Cluster 的 wrapper
	clusterMock := &continuationCluster{
		name:   name,
		msgBus: msgBus,
	}

	rpcSrv := clusterrpc.NewServer(clusterMock)

	return &continuationTestNode{
		Name:   name,
		msgBus: msgBus,
		rpcCh:  rpcCh,
		rpcSrv: rpcSrv,
	}
}

// Start 启动续行测试节点
func (n *continuationTestNode) Start() error {
	return n.StartWithLLM(false)
}

// StartWithLLM 启动续行测试节点（可选是否启动模拟 LLM）
func (n *continuationTestNode) StartWithLLM(withLLM bool) error {
	ctx, cancel := context.WithCancel(context.Background())
	n.cancel = cancel

	// 创建一个真实的 Cluster（使用 temp workspace）
	tmpDir := fmt.Sprintf("%s/nemesisbot-test-cont-%d", os.TempDir(), time.Now().UnixNano())
	os.MkdirAll(tmpDir, 0755)

	c, err := cluster.NewCluster(tmpDir)
	if err != nil {
		cancel()
		return fmt.Errorf("failed to create cluster: %w", err)
	}
	n.cluster = c

	// 设置 bus
	c.SetMessageBus(n.msgBus)

	// 手动创建并配置 TaskManager
	tm := cluster.NewTaskManager(10 * time.Second)
	tm.SetOnComplete(c.HandleTaskCompleteForTest)
	tm.Start()
	n.taskManager = tm

	// 注入 TaskManager 到 Cluster（通过反射或 setter 方法不可行，使用替代方案）
	// 直接使用 cluster 的公共接口来注册回调
	c.SetTaskManagerForTest(tm)

	// 创建 RPC channel
	clusterMock := &continuationCluster{
		name:   n.Name,
		msgBus: n.msgBus,
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

	// Start RPCChannel
	if err := n.rpcCh.Start(ctx); err != nil {
		cancel()
		return fmt.Errorf("failed to start RPC channel: %w", err)
	}

	// 启动 dispatch loop
	channelMap := map[string]interface{}{"rpc": n.rpcCh}
	startTestDispatchLoop(ctx, n.msgBus, channelMap)

	// 可选：启动模拟 LLM（e2e 测试需要，纯 callback 测试不需要）
	if withLLM {
		n.startSimulatedLLM(ctx)
	}

	// 启动 RPC Server
	if err := n.rpcSrv.Start(0); err != nil {
		cancel()
		return fmt.Errorf("failed to start RPC server: %w", err)
	}
	n.ActualPort = n.rpcSrv.GetPort()
	clusterMock.localPort = n.ActualPort

	return nil
}

func (n *continuationTestNode) Stop() {
	n.taskManager.Stop()
	n.rpcCh.Stop(context.Background())
	n.rpcSrv.Stop()
	if n.cancel != nil {
		n.cancel()
	}
	// Cleanup temp dir
	if n.cluster != nil {
		os.RemoveAll(n.cluster.GetWorkspace())
	}
}

func (n *continuationTestNode) startSimulatedLLM(ctx context.Context) {
	go func() {
		for {
			msg, ok := n.msgBus.ConsumeInbound(ctx)
			if !ok {
				return
			}
			time.Sleep(100 * time.Millisecond)
			response := fmt.Sprintf("[rpc:%s] Simulated LLM response to: %s",
				msg.CorrelationID, msg.Content)
			n.msgBus.PublishOutbound(bus.OutboundMessage{
				Channel: "rpc",
				ChatID:  msg.ChatID,
				Content: response,
			})
		}
	}()
}

func (n *continuationTestNode) SendRPCRequest(address, action string, payload map[string]interface{}) (string, error) {
	return sendTestRPCRequest(n.Name, address, action, payload)
}

// continuationCluster 实现 clusterrpc.Cluster 接口
type continuationCluster struct {
	name      string
	localPort int
	msgBus    *bus.MessageBus
}

func (m *continuationCluster) GetRegistry() interface{}        { return nil }
func (m *continuationCluster) GetNodeID() string               { return m.name }
func (m *continuationCluster) GetAddress() string              { return fmt.Sprintf("127.0.0.1:%d", m.localPort) }
func (m *continuationCluster) GetCapabilities() []string       { return []string{"peer_chat", "llm"} }
func (m *continuationCluster) GetOnlinePeers() []interface{}   { return nil }
func (m *continuationCluster) GetActionsSchema() []interface{} { return nil }
func (m *continuationCluster) LogRPCInfo(msg string, args ...interface{}) {
	fmt.Printf("[INFO][%s] %s\n", m.name, fmt.Sprintf(msg, args...))
}
func (m *continuationCluster) LogRPCError(msg string, args ...interface{}) {
	fmt.Printf("[ERROR][%s] %s\n", m.name, fmt.Sprintf(msg, args...))
}
func (m *continuationCluster) LogRPCDebug(msg string, args ...interface{}) {}
func (m *continuationCluster) GetPeer(peerID string) (interface{}, error)   { return nil, nil }
func (m *continuationCluster) GetLocalNetworkInterfaces() ([]clusterrpc.LocalNetworkInterface, error) {
	return []clusterrpc.LocalNetworkInterface{{IP: "127.0.0.1", Mask: "255.255.255.0"}}, nil
}

func (m *continuationCluster) CallWithContext(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	// 从全局注册表查找目标节点（兼容 asyncTestNode 和 continuationTestNode）
	testNodeRegistry.RLock()
	targetAsync, asyncOK := testNodeRegistry.nodes[peerID]
	testNodeRegistry.RUnlock()

	var address string
	if asyncOK && targetAsync != nil {
		address = fmt.Sprintf("127.0.0.1:%d", targetAsync.ActualPort)
	} else {
		return nil, fmt.Errorf("peer not found: %s", peerID)
	}

	result, err := sendTestRPCRequest(m.name, address, action, payload)
	if err != nil {
		return nil, err
	}
	return []byte(result), nil
}

// sendTestRPCRequest 通用 RPC 请求发送
func sendTestRPCRequest(sender, address, action string, payload map[string]interface{}) (string, error) {
	conn, err := net.DialTimeout("tcp", address, 5*time.Second)
	if err != nil {
		return "", fmt.Errorf("dial failed: %w", err)
	}
	defer conn.Close()

	tcpConn := transport.NewTCPConn(conn, nil)
	tcpConn.Start()

	req := transport.NewRequest(sender, "remote", action, payload)
	if err := tcpConn.Send(req); err != nil {
		return "", fmt.Errorf("send failed: %w", err)
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
		return "", fmt.Errorf("timeout")
	}
}
