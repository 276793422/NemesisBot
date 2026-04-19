// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package p2p

import (
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/cluster"
	"github.com/276793422/NemesisBot/module/cluster/discovery"
	"github.com/276793422/NemesisBot/module/cluster/rpc"
)

// ============================================================
// Mock Types
// ============================================================

// testNode implements rpc.Node interface
type testNode struct {
	id           string
	name         string
	address      string
	addresses    []string
	rpcPort      int
	capabilities []string
	online       bool
}

func (n *testNode) GetID() string             { return n.id }
func (n *testNode) GetName() string           { return n.name }
func (n *testNode) GetAddress() string        { return n.address }
func (n *testNode) GetAddresses() []string    { return n.addresses }
func (n *testNode) GetRPCPort() int           { return n.rpcPort }
func (n *testNode) GetCapabilities() []string { return n.capabilities }
func (n *testNode) GetStatus() string         { return "online" }
func (n *testNode) IsOnline() bool            { return n.online }

// testCluster implements rpc.Cluster interface.
// CallWithContext delegates to the real rpcClient for TCP callback tests.
type testCluster struct {
	nodeID       string
	capabilities []string
	peers        map[string]*testNode
	logs         []string
	logMu        sync.Mutex
	rpcClient    *rpc.Client // injected after creation for CallWithContext delegation
}

func newTestCluster(nodeID string, caps []string) *testCluster {
	return &testCluster{
		nodeID:       nodeID,
		capabilities: caps,
		peers:        make(map[string]*testNode),
	}
}

func (tc *testCluster) GetRegistry() interface{}        { return nil }
func (tc *testCluster) GetNodeID() string               { return tc.nodeID }
func (tc *testCluster) GetAddress() string              { return "127.0.0.1" }
func (tc *testCluster) GetCapabilities() []string       { return tc.capabilities }
func (tc *testCluster) GetOnlinePeers() []interface{}   { return nil }
func (tc *testCluster) GetActionsSchema() []interface{} { return nil }

func (tc *testCluster) LogRPCInfo(msg string, args ...interface{}) {
	tc.logMu.Lock()
	defer tc.logMu.Unlock()
	tc.logs = append(tc.logs, fmt.Sprintf("INFO: "+msg, args...))
}

func (tc *testCluster) LogRPCError(msg string, args ...interface{}) {
	tc.logMu.Lock()
	defer tc.logMu.Unlock()
	tc.logs = append(tc.logs, fmt.Sprintf("ERROR: "+msg, args...))
}

func (tc *testCluster) LogRPCDebug(msg string, args ...interface{}) {
	tc.logMu.Lock()
	defer tc.logMu.Unlock()
	tc.logs = append(tc.logs, fmt.Sprintf("DEBUG: "+msg, args...))
}

func (tc *testCluster) GetPeer(peerID string) (interface{}, error) {
	if peer, ok := tc.peers[peerID]; ok {
		return peer, nil
	}
	return nil, fmt.Errorf("peer not found: %s", peerID)
}

func (tc *testCluster) GetLocalNetworkInterfaces() ([]rpc.LocalNetworkInterface, error) {
	return []rpc.LocalNetworkInterface{{IP: "127.0.0.1", Mask: "255.0.0.0"}}, nil
}

// CallWithContext delegates to the real rpcClient so callbacks traverse real TCP.
func (tc *testCluster) CallWithContext(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	if tc.rpcClient != nil {
		return tc.rpcClient.CallWithContext(ctx, peerID, action, payload)
	}
	return nil, fmt.Errorf("rpc client not initialized")
}

func (tc *testCluster) GetTaskResultStorer() rpc.TaskResultStorer { return nil }

// testDiscoveryCallbacks implements discovery.ClusterCallbacks
type testDiscoveryCallbacks struct {
	nodeID   string
	address  string
	rpcPort  int
	localIPs []string
	role     string
	category string
	tags     []string

	mu              sync.Mutex
	discoveredNodes []discoveredNodeInfo
	offlineEvents   []offlineEvent
}

type discoveredNodeInfo struct {
	NodeID    string
	Name      string
	Addresses []string
	RPCPort   int
	Role      string
	Category  string
}

type offlineEvent struct {
	NodeID string
	Reason string
}

func newTestDiscoveryCallbacks(nodeID string) *testDiscoveryCallbacks {
	return &testDiscoveryCallbacks{
		nodeID:   nodeID,
		address:  "127.0.0.1",
		rpcPort:  0,
		localIPs: []string{"127.0.0.1"},
		role:     "worker",
		category: "development",
		tags:     []string{"test"},
	}
}

func (cb *testDiscoveryCallbacks) GetNodeID() string      { return cb.nodeID }
func (cb *testDiscoveryCallbacks) GetAddress() string      { return cb.address }
func (cb *testDiscoveryCallbacks) GetRPCPort() int         { return cb.rpcPort }
func (cb *testDiscoveryCallbacks) GetAllLocalIPs() []string { return cb.localIPs }
func (cb *testDiscoveryCallbacks) GetRole() string         { return cb.role }
func (cb *testDiscoveryCallbacks) GetCategory() string     { return cb.category }
func (cb *testDiscoveryCallbacks) GetTags() []string       { return cb.tags }

func (cb *testDiscoveryCallbacks) LogInfo(msg string, args ...interface{})    {}
func (cb *testDiscoveryCallbacks) LogError(msg string, args ...interface{})   {}
func (cb *testDiscoveryCallbacks) LogDebug(msg string, args ...interface{})   {}

func (cb *testDiscoveryCallbacks) HandleDiscoveredNode(nodeID, name string, addresses []string, rpcPort int, role, category string, tags []string, capabilities []string) {
	cb.mu.Lock()
	defer cb.mu.Unlock()
	cb.discoveredNodes = append(cb.discoveredNodes, discoveredNodeInfo{
		NodeID:    nodeID,
		Name:      name,
		Addresses: addresses,
		RPCPort:   rpcPort,
		Role:      role,
		Category:  category,
	})
}

func (cb *testDiscoveryCallbacks) HandleNodeOffline(nodeID, reason string) {
	cb.mu.Lock()
	defer cb.mu.Unlock()
	cb.offlineEvents = append(cb.offlineEvents, offlineEvent{
		NodeID: nodeID,
		Reason: reason,
	})
}

func (cb *testDiscoveryCallbacks) SyncToDisk() error { return nil }

func (cb *testDiscoveryCallbacks) getDiscoveredNodes() []discoveredNodeInfo {
	cb.mu.Lock()
	defer cb.mu.Unlock()
	result := make([]discoveredNodeInfo, len(cb.discoveredNodes))
	copy(result, cb.discoveredNodes)
	return result
}

func (cb *testDiscoveryCallbacks) getOfflineEvents() []offlineEvent {
	cb.mu.Lock()
	defer cb.mu.Unlock()
	result := make([]offlineEvent, len(cb.offlineEvents))
	copy(result, cb.offlineEvents)
	return result
}

// ============================================================
// Helper Functions
// ============================================================

// startServer creates and starts an RPC server on a dynamic port.
func startServer(tc *testCluster, token string) (*rpc.Server, error) {
	server := rpc.NewServer(tc)
	if token != "" {
		server.SetAuthToken(token)
	}
	if err := server.Start(0); err != nil {
		return nil, err
	}
	return server, nil
}

// newClient creates an RPC client with optional auth token.
func newClient(tc *testCluster, token string) *rpc.Client {
	client := rpc.NewClient(tc)
	if token != "" {
		client.SetAuthToken(token)
	}
	return client
}

// setupP2PPair creates two fully-connected P2P nodes with servers, clients,
// and mutual peer registration. Returns all components plus a cleanup function.
func setupP2PPair(t *testing.T, tokenA, tokenB string) (
	clusterA *testCluster, serverA *rpc.Server, clientA *rpc.Client,
	clusterB *testCluster, serverB *rpc.Server, clientB *rpc.Client,
	cleanup func(),
) {
	t.Helper()

	caps := []string{"test"}

	// Node A
	clusterA = newTestCluster("node-A", caps)
	var err error
	serverA, err = startServer(clusterA, tokenA)
	if err != nil {
		t.Fatalf("Failed to start serverA: %v", err)
	}
	clientA = newClient(clusterA, tokenA)
	clusterA.rpcClient = clientA

	// Node B
	clusterB = newTestCluster("node-B", caps)
	serverB, err = startServer(clusterB, tokenB)
	if err != nil {
		serverA.Stop()
		clientA.Close()
		t.Fatalf("Failed to start serverB: %v", err)
	}
	clientB = newClient(clusterB, tokenB)
	clusterB.rpcClient = clientB

	portA := serverA.GetPort()
	portB := serverB.GetPort()

	// Register each other as peers
	clusterA.peers["node-B"] = &testNode{
		id:        "node-B",
		name:      "Node B",
		address:   fmt.Sprintf("127.0.0.1:%d", portB),
		addresses: []string{fmt.Sprintf("127.0.0.1:%d", portB)},
		rpcPort:   portB,
		online:    true,
	}
	clusterB.peers["node-A"] = &testNode{
		id:        "node-A",
		name:      "Node A",
		address:   fmt.Sprintf("127.0.0.1:%d", portA),
		addresses: []string{fmt.Sprintf("127.0.0.1:%d", portA)},
		rpcPort:   portA,
		online:    true,
	}

	cleanup = func() {
		clientA.Close()
		clientB.Close()
		serverA.Stop()
		serverB.Stop()
	}

	return
}

// allocateUDPPort finds an available UDP port for testing.
func allocateUDPPort(t *testing.T) int {
	t.Helper()
	conn, err := net.ListenPacket("udp", ":0")
	if err != nil {
		t.Fatalf("Failed to allocate UDP port: %v", err)
	}
	port := conn.LocalAddr().(*net.UDPAddr).Port
	conn.Close()
	return port
}

// encryptForTest encrypts data using AES-256-GCM (mirrors discovery.encryptData).
func encryptForTest(key []byte, plaintext []byte) ([]byte, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return nil, err
	}
	nonce := make([]byte, gcm.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		return nil, err
	}
	return append(nonce, gcm.Seal(nil, nonce, plaintext, nil)...), nil
}

// sendRawFrame sends a length-prefixed frame over a raw TCP connection.
func sendRawFrame(conn net.Conn, data []byte) error {
	frame := make([]byte, 4+len(data))
	binary.BigEndian.PutUint32(frame[:4], uint32(len(data)))
	copy(frame[4:], data)
	conn.SetWriteDeadline(time.Now().Add(5 * time.Second))
	_, err := conn.Write(frame)
	return err
}

// readRawFrame reads a length-prefixed frame from a raw TCP connection.
func readRawFrame(conn net.Conn) ([]byte, error) {
	conn.SetReadDeadline(time.Now().Add(5 * time.Second))
	header := make([]byte, 4)
	if _, err := io.ReadFull(conn, header); err != nil {
		return nil, err
	}
	dataLen := binary.BigEndian.Uint32(header)
	if dataLen > 16*1024*1024 {
		return nil, fmt.Errorf("frame too large: %d", dataLen)
	}
	data := make([]byte, dataLen)
	if _, err := io.ReadFull(conn, data); err != nil {
		return nil, err
	}
	return data, nil
}

// ============================================================
// Test Functions
// ============================================================

// TestP2P_TwoNodePing tests basic two-node RPC communication.
// B's client calls A's default "ping" handler and verifies the response.
func TestP2P_TwoNodePing(t *testing.T) {
	_, _, _, _, _, clientB, cleanup := setupP2PPair(t, "", "")
	defer cleanup()

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	resp, err := clientB.CallWithContext(ctx, "node-A", "ping", nil)
	if err != nil {
		t.Fatalf("Ping call failed: %v", err)
	}

	var result map[string]interface{}
	if err := json.Unmarshal(resp, &result); err != nil {
		t.Fatalf("Failed to unmarshal response: %v", err)
	}

	if result["status"] != "ok" {
		t.Errorf("Expected status 'ok', got '%v'", result["status"])
	}
	if result["node_id"] != "node-A" {
		t.Errorf("Expected node_id 'node-A', got '%v'", result["node_id"])
	}
}

// TestP2P_BidirectionalCommunication tests simultaneous A↔B RPC calls.
// Both sides register unique handlers and call each other concurrently.
func TestP2P_BidirectionalCommunication(t *testing.T) {
	_, serverA, clientA, _, serverB, clientB, cleanup := setupP2PPair(t, "", "")
	defer cleanup()

	serverA.RegisterHandler("echo_A", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{
			"source":  "A",
			"echo_id": payload["id"],
		}, nil
	})
	serverB.RegisterHandler("echo_B", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{
			"source":  "B",
			"echo_id": payload["id"],
		}, nil
	})

	type callResult struct {
		resp map[string]interface{}
		err  error
		from string
	}
	results := make(chan callResult, 2)

	// A → B
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		resp, err := clientA.CallWithContext(ctx, "node-B", "echo_B", map[string]interface{}{
			"id": "call-A-to-B",
		})
		var m map[string]interface{}
		if err == nil {
			json.Unmarshal(resp, &m)
		}
		results <- callResult{m, err, "A→B"}
	}()

	// B → A
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		resp, err := clientB.CallWithContext(ctx, "node-A", "echo_A", map[string]interface{}{
			"id": "call-B-to-A",
		})
		var m map[string]interface{}
		if err == nil {
			json.Unmarshal(resp, &m)
		}
		results <- callResult{m, err, "B→A"}
	}()

	for i := 0; i < 2; i++ {
		r := <-results
		if r.err != nil {
			t.Errorf("%s call failed: %v", r.from, r.err)
			continue
		}
		if r.resp["source"] == nil {
			t.Errorf("%s: missing 'source' in response", r.from)
		}
		if r.resp["echo_id"] == nil {
			t.Errorf("%s: missing 'echo_id' in response", r.from)
		}
	}
}

// TestP2P_TaskDispatchAndCallback tests the async peer_chat flow:
// A → B (peer_chat) → B returns ACK → B async callback → A (peer_chat_callback).
// The callback goes through real TCP via CallWithContext delegation.
func TestP2P_TaskDispatchAndCallback(t *testing.T) {
	_, serverA, clientA, clusterB, serverB, _, cleanup := setupP2PPair(t, "", "")
	defer cleanup()

	// Track callbacks received by A
	var callbackMu sync.Mutex
	var callbacks []map[string]interface{}

	// A: peer_chat_callback handler (records incoming callbacks)
	serverA.RegisterHandler("peer_chat_callback", func(payload map[string]interface{}) (map[string]interface{}, error) {
		callbackMu.Lock()
		defer callbackMu.Unlock()
		callbacks = append(callbacks, payload)
		return map[string]interface{}{
			"status":  "received",
			"task_id": payload["task_id"],
		}, nil
	})

	// B: peer_chat handler (ACK + async callback via real TCP)
	serverB.RegisterHandler("peer_chat", func(payload map[string]interface{}) (map[string]interface{}, error) {
		taskID, _ := payload["task_id"].(string)
		if taskID == "" {
			taskID = fmt.Sprintf("task-%d", time.Now().UnixNano())
		}

		sourceInfo, _ := payload["_source"].(map[string]interface{})

		// Async callback — delegates to real RPC client (TCP)
		go func() {
			time.Sleep(100 * time.Millisecond) // Simulate LLM processing

			callbackPayload := map[string]interface{}{
				"task_id":  taskID,
				"status":   "success",
				"response": "Processed by node-B",
			}

			if sourceNodeID, ok := sourceInfo["node_id"].(string); ok && sourceNodeID != "" {
				ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
				defer cancel()
				clusterB.CallWithContext(ctx, sourceNodeID, "peer_chat_callback", callbackPayload)
			}
		}()

		// Immediate ACK
		return map[string]interface{}{
			"status":  "accepted",
			"task_id": taskID,
		}, nil
	})

	// A calls B's peer_chat
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	taskID := fmt.Sprintf("task-test-%d", time.Now().UnixNano())
	resp, err := clientA.CallWithContext(ctx, "node-B", "peer_chat", map[string]interface{}{
		"content": "Hello from A",
		"type":    "chat",
		"task_id": taskID,
		"_source": map[string]interface{}{
			"node_id": "node-A",
		},
	})
	if err != nil {
		t.Fatalf("peer_chat call failed: %v", err)
	}

	// Verify ACK
	var ack map[string]interface{}
	if err := json.Unmarshal(resp, &ack); err != nil {
		t.Fatalf("Failed to unmarshal ACK: %v", err)
	}
	if ack["status"] != "accepted" {
		t.Errorf("Expected ACK status 'accepted', got '%v'", ack["status"])
	}

	// Wait for async callback
	deadline := time.After(5 * time.Second)
	for {
		callbackMu.Lock()
		count := len(callbacks)
		callbackMu.Unlock()
		if count > 0 {
			break
		}
		select {
		case <-deadline:
			t.Fatal("Timeout waiting for callback from B")
		default:
			time.Sleep(50 * time.Millisecond)
		}
	}

	// Verify callback content
	callbackMu.Lock()
	defer callbackMu.Unlock()
	if callbacks[0]["task_id"] != taskID {
		t.Errorf("Expected task_id '%s', got '%v'", taskID, callbacks[0]["task_id"])
	}
	if callbacks[0]["status"] != "success" {
		t.Errorf("Expected callback status 'success', got '%v'", callbacks[0]["status"])
	}
	if callbacks[0]["response"] != "Processed by node-B" {
		t.Errorf("Unexpected callback response: %v", callbacks[0]["response"])
	}
}

// TestP2P_TaskStatusLifecycle tests TaskManager state transitions
// without any network (pure unit test).
func TestP2P_TaskStatusLifecycle(t *testing.T) {
	tm := cluster.NewTaskManager(1 * time.Hour) // Long cleanup to avoid interference
	tm.Start()
	defer tm.Stop()

	// Track completion callback
	var completedTasks []string
	var cbMu sync.Mutex
	tm.SetOnComplete(func(taskID string) {
		cbMu.Lock()
		defer cbMu.Unlock()
		completedTasks = append(completedTasks, taskID)
	})

	// --- Success path ---
	task1 := &cluster.Task{
		ID:        "task-lifecycle-1",
		Action:    "peer_chat",
		PeerID:    "node-B",
		Payload:   map[string]interface{}{"content": "test"},
		Status:    cluster.TaskPending,
		CreatedAt: time.Now(),
	}
	if err := tm.Submit(task1); err != nil {
		t.Fatalf("Submit failed: %v", err)
	}

	got, err := tm.GetTask("task-lifecycle-1")
	if err != nil {
		t.Fatalf("GetTask failed: %v", err)
	}
	if got.Status != cluster.TaskPending {
		t.Errorf("Expected status pending, got %s", got.Status)
	}

	if err := tm.CompleteTask("task-lifecycle-1", &cluster.TaskResult{
		TaskID:   "task-lifecycle-1",
		Status:   "success",
		Response: "Done!",
	}); err != nil {
		t.Fatalf("CompleteTask failed: %v", err)
	}

	got, _ = tm.GetTask("task-lifecycle-1")
	if got.Status != cluster.TaskCompleted {
		t.Errorf("Expected status completed, got %s", got.Status)
	}
	if got.Response != "Done!" {
		t.Errorf("Expected response 'Done!', got '%s'", got.Response)
	}

	// Verify onTaskComplete callback fired
	cbMu.Lock()
	if len(completedTasks) != 1 || completedTasks[0] != "task-lifecycle-1" {
		t.Errorf("Expected callback for task-lifecycle-1, got %v", completedTasks)
	}
	cbMu.Unlock()

	// --- Error path ---
	task2 := &cluster.Task{
		ID:        "task-lifecycle-2",
		Action:    "peer_chat",
		PeerID:    "node-C",
		Status:    cluster.TaskPending,
		CreatedAt: time.Now(),
	}
	tm.Submit(task2)

	tm.CompleteTask("task-lifecycle-2", &cluster.TaskResult{
		TaskID: "task-lifecycle-2",
		Status: "error",
		Error:  "connection refused",
	})

	got2, _ := tm.GetTask("task-lifecycle-2")
	if got2.Status != cluster.TaskFailed {
		t.Errorf("Expected status failed, got %s", got2.Status)
	}
	if got2.Error != "connection refused" {
		t.Errorf("Expected error 'connection refused', got '%s'", got2.Error)
	}

	// --- CompleteCallback interface conversion ---
	task3 := &cluster.Task{
		ID:        "task-cb-test",
		Action:    "peer_chat",
		PeerID:    "node-D",
		Status:    cluster.TaskPending,
		CreatedAt: time.Now(),
	}
	tm.Submit(task3)

	if err := tm.CompleteCallback("task-cb-test", "success", "callback response", ""); err != nil {
		t.Fatalf("CompleteCallback failed: %v", err)
	}

	got3, _ := tm.GetTask("task-cb-test")
	if got3.Status != cluster.TaskCompleted {
		t.Errorf("Expected completed from CompleteCallback, got %s", got3.Status)
	}
	if got3.Response != "callback response" {
		t.Errorf("Expected response 'callback response', got '%s'", got3.Response)
	}
}

// TestP2P_ConcurrentMultiTask tests that multiple concurrent RPC calls
// don't suffer from cross-contamination.
func TestP2P_ConcurrentMultiTask(t *testing.T) {
	_, serverA, _, _, _, clientB, cleanup := setupP2PPair(t, "", "")
	defer cleanup()

	// "work" handler with simulated processing delay
	serverA.RegisterHandler("work", func(payload map[string]interface{}) (map[string]interface{}, error) {
		time.Sleep(100 * time.Millisecond)
		return map[string]interface{}{
			"status":  "done",
			"work_id": payload["id"],
		}, nil
	})

	numTasks := 5
	type taskResult struct {
		workID string
		resp   map[string]interface{}
		err    error
	}
	results := make(chan taskResult, numTasks)

	for i := 0; i < numTasks; i++ {
		go func(idx int) {
			ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
			defer cancel()

			id := fmt.Sprintf("work-%d", idx)
			resp, err := clientB.CallWithContext(ctx, "node-A", "work", map[string]interface{}{
				"id": id,
			})
			var m map[string]interface{}
			if err == nil {
				json.Unmarshal(resp, &m)
			}
			results <- taskResult{id, m, err}
		}(i)
	}

	for i := 0; i < numTasks; i++ {
		r := <-results
		if r.err != nil {
			t.Errorf("Task %s failed: %v", r.workID, r.err)
			continue
		}
		if r.resp["status"] != "done" {
			t.Errorf("Task %s: expected status 'done', got '%v'", r.workID, r.resp["status"])
		}
		if r.resp["work_id"] != r.workID {
			t.Errorf("Cross-contamination: expected work_id '%s', got '%v'", r.workID, r.resp["work_id"])
		}
	}
}

// TestP2P_AuthTokenEnforcement validates authentication in four scenarios.
func TestP2P_AuthTokenEnforcement(t *testing.T) {
	// Sub-test 1: Same token → success
	t.Run("same_token_success", func(t *testing.T) {
		_, _, clientA, _, serverB, _, cleanup := setupP2PPair(t, "shared-secret", "shared-secret")
		defer cleanup()

		serverB.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
			return map[string]interface{}{"status": "ok"}, nil
		})

		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()

		resp, err := clientA.CallWithContext(ctx, "node-B", "echo", nil)
		if err != nil {
			t.Fatalf("Expected success with same token, got: %v", err)
		}
		var m map[string]interface{}
		json.Unmarshal(resp, &m)
		if m["status"] != "ok" {
			t.Errorf("Unexpected response: %v", m)
		}
	})

	// Sub-test 2: Different token → failure
	t.Run("different_token_failure", func(t *testing.T) {
		_, _, clientA, _, serverB, _, cleanup := setupP2PPair(t, "token-A", "token-B")
		defer cleanup()

		serverB.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
			return map[string]interface{}{"status": "ok"}, nil
		})

		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()

		_, err := clientA.CallWithContext(ctx, "node-B", "echo", nil)
		if err == nil {
			t.Error("Expected failure with different tokens, but succeeded")
		}
	})

	// Sub-test 3: Client no token + server has token → failure (original BUG reproduction)
	t.Run("client_no_token_server_has_token", func(t *testing.T) {
		clusterB := newTestCluster("node-B", []string{"test"})
		serverB, err := startServer(clusterB, "server-token")
		if err != nil {
			t.Fatalf("Failed to start server: %v", err)
		}
		defer serverB.Stop()

		serverB.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
			return map[string]interface{}{"status": "ok"}, nil
		})

		clusterA := newTestCluster("node-A", []string{"test"})
		clusterA.peers["node-B"] = &testNode{
			id:        "node-B",
			name:      "Node B",
			addresses: []string{fmt.Sprintf("127.0.0.1:%d", serverB.GetPort())},
			rpcPort:   serverB.GetPort(),
			online:    true,
		}

		clientA := rpc.NewClient(clusterA)
		defer clientA.Close()
		// Deliberately NOT calling clientA.SetAuthToken() — reproduces the original bug

		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()

		_, err = clientA.CallWithContext(ctx, "node-B", "echo", nil)
		if err == nil {
			t.Error("Expected failure when client has no token but server requires one")
		}
	})

	// Sub-test 4: Both no token → success (backward compatible)
	t.Run("both_no_token_success", func(t *testing.T) {
		_, _, clientA, _, serverB, _, cleanup := setupP2PPair(t, "", "")
		defer cleanup()

		serverB.RegisterHandler("echo", func(payload map[string]interface{}) (map[string]interface{}, error) {
			return map[string]interface{}{"status": "ok"}, nil
		})

		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()

		_, err := clientA.CallWithContext(ctx, "node-B", "echo", nil)
		if err != nil {
			t.Fatalf("Expected success without tokens, got: %v", err)
		}
	})
}

// TestP2P_RoleCapabilities tests that node capabilities are correctly
// returned through the default get_capabilities handler.
func TestP2P_RoleCapabilities(t *testing.T) {
	capsA := []string{"code_analysis", "code_generation"}
	capsB := []string{"testing", "deployment"}

	clusterA := newTestCluster("node-A", capsA)
	serverA, err := startServer(clusterA, "")
	if err != nil {
		t.Fatalf("Failed to start serverA: %v", err)
	}
	defer serverA.Stop()

	clusterB := newTestCluster("node-B", capsB)
	clientB := newClient(clusterB, "")
	defer clientB.Close()

	clusterB.peers["node-A"] = &testNode{
		id:        "node-A",
		name:      "Node A",
		addresses: []string{fmt.Sprintf("127.0.0.1:%d", serverA.GetPort())},
		rpcPort:   serverA.GetPort(),
		online:    true,
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// B queries A's capabilities
	resp, err := clientB.CallWithContext(ctx, "node-A", "get_capabilities", nil)
	if err != nil {
		t.Fatalf("get_capabilities failed: %v", err)
	}

	var capsResp map[string]interface{}
	if err := json.Unmarshal(resp, &capsResp); err != nil {
		t.Fatalf("Failed to unmarshal: %v", err)
	}

	caps, ok := capsResp["capabilities"].([]interface{})
	if !ok {
		t.Fatalf("Expected capabilities array, got %T", capsResp["capabilities"])
	}

	found := map[string]bool{}
	for _, c := range caps {
		if s, ok := c.(string); ok {
			found[s] = true
		}
	}
	if !found["code_analysis"] || !found["code_generation"] {
		t.Errorf("Expected code_analysis and code_generation, got %v", caps)
	}

	// B queries A's info (get_info)
	resp2, err := clientB.CallWithContext(ctx, "node-A", "get_info", nil)
	if err != nil {
		t.Fatalf("get_info failed: %v", err)
	}

	var infoResp map[string]interface{}
	json.Unmarshal(resp2, &infoResp)

	if infoResp["node_id"] != "node-A" {
		t.Errorf("Expected node_id 'node-A', got '%v'", infoResp["node_id"])
	}
}

// TestP2P_EncryptedDiscovery tests encrypted UDP discovery:
// same key → discovery works; different key → message silently dropped.
func TestP2P_EncryptedDiscovery(t *testing.T) {
	encKey := discovery.DeriveKey("test-cluster-secret")

	// --- Same key: successful discovery ---
	port := allocateUDPPort(t)
	cb := newTestDiscoveryCallbacks("node-A")
	cb.rpcPort = 9999

	disc, err := discovery.NewDiscovery(port, cb, encKey)
	if err != nil {
		t.Fatalf("Failed to create discovery: %v", err)
	}
	disc.SetBroadcastInterval(200 * time.Millisecond)

	if err := disc.Start(); err != nil {
		t.Fatalf("Failed to start discovery: %v", err)
	}
	defer disc.Stop()

	time.Sleep(100 * time.Millisecond)

	// Send encrypted announce via unicast
	announceMsg := discovery.NewAnnounceMessage(
		"node-B", "Node B",
		[]string{"192.168.1.2"}, 9900,
		"worker", "development", nil, nil,
	)
	msgData, _ := announceMsg.Bytes()

	encryptedData, err := encryptForTest(encKey, msgData)
	if err != nil {
		t.Fatalf("Failed to encrypt: %v", err)
	}

	targetAddr, _ := net.ResolveUDPAddr("udp", fmt.Sprintf("127.0.0.1:%d", port))
	conn, err := net.DialUDP("udp", nil, targetAddr)
	if err != nil {
		t.Fatalf("Failed to dial UDP: %v", err)
	}
	conn.Write(encryptedData)
	conn.Close()

	// Wait for discovery
	deadline := time.After(3 * time.Second)
	for {
		nodes := cb.getDiscoveredNodes()
		if len(nodes) > 0 {
			if nodes[0].NodeID != "node-B" {
				t.Errorf("Expected nodeID 'node-B', got '%s'", nodes[0].NodeID)
			}
			if nodes[0].RPCPort != 9900 {
				t.Errorf("Expected RPCPort 9900, got %d", nodes[0].RPCPort)
			}
			if nodes[0].Role != "worker" {
				t.Errorf("Expected Role 'worker', got '%s'", nodes[0].Role)
			}
			break
		}
		select {
		case <-deadline:
			t.Fatal("Timeout waiting for encrypted discovery")
		default:
			time.Sleep(100 * time.Millisecond)
		}
	}

	// --- Different key: message silently dropped ---
	port2 := allocateUDPPort(t)
	cb2 := newTestDiscoveryCallbacks("node-C")
	disc2, err := discovery.NewDiscovery(port2, cb2, discovery.DeriveKey("wrong-key"))
	if err != nil {
		t.Fatalf("Failed to create discovery2: %v", err)
	}
	disc2.Start()
	defer disc2.Stop()

	time.Sleep(100 * time.Millisecond)

	// Encrypt with the original key — listener2 has a different key
	encryptedData2, _ := encryptForTest(encKey, msgData)
	targetAddr2, _ := net.ResolveUDPAddr("udp", fmt.Sprintf("127.0.0.1:%d", port2))
	conn2, _ := net.DialUDP("udp", nil, targetAddr2)
	conn2.Write(encryptedData2)
	conn2.Close()

	time.Sleep(500 * time.Millisecond)

	nodes2 := cb2.getDiscoveredNodes()
	if len(nodes2) > 0 {
		t.Error("Should NOT decrypt with wrong key, but discovered a node")
	}
}

// TestP2P_NodeOfflineByeMessage tests that a bye message triggers
// HandleNodeOffline on the receiving discovery instance.
func TestP2P_NodeOfflineByeMessage(t *testing.T) {
	encKey := discovery.DeriveKey("test-cluster-secret")

	port := allocateUDPPort(t)
	cb := newTestDiscoveryCallbacks("node-A")

	disc, err := discovery.NewDiscovery(port, cb, encKey)
	if err != nil {
		t.Fatalf("Failed to create discovery: %v", err)
	}

	if err := disc.Start(); err != nil {
		t.Fatalf("Failed to start discovery: %v", err)
	}
	defer disc.Stop()

	time.Sleep(100 * time.Millisecond)

	// Send encrypted bye message via unicast
	byeMsg := discovery.NewByeMessage("node-B")
	msgData, _ := byeMsg.Bytes()

	encryptedData, err := encryptForTest(encKey, msgData)
	if err != nil {
		t.Fatalf("Failed to encrypt: %v", err)
	}

	targetAddr, _ := net.ResolveUDPAddr("udp", fmt.Sprintf("127.0.0.1:%d", port))
	conn, _ := net.DialUDP("udp", nil, targetAddr)
	conn.Write(encryptedData)
	conn.Close()

	// Wait for offline event
	deadline := time.After(3 * time.Second)
	for {
		events := cb.getOfflineEvents()
		if len(events) > 0 {
			if events[0].NodeID != "node-B" {
				t.Errorf("Expected offline node 'node-B', got '%s'", events[0].NodeID)
			}
			if events[0].Reason != "node shutdown" {
				t.Errorf("Expected reason 'node shutdown', got '%s'", events[0].Reason)
			}
			return
		}
		select {
		case <-deadline:
			t.Fatal("Timeout waiting for HandleNodeOffline")
		default:
			time.Sleep(100 * time.Millisecond)
		}
	}
}

// TestP2P_ErrorHandling_InvalidAction verifies that calling an unregistered
// action returns a structured no_handler response instead of panicking.
func TestP2P_ErrorHandling_InvalidAction(t *testing.T) {
	clusterA := newTestCluster("node-A", []string{"test"})
	serverA, err := startServer(clusterA, "")
	if err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer serverA.Stop()

	clusterB := newTestCluster("node-B", []string{"test"})
	clusterB.peers["node-A"] = &testNode{
		id:        "node-A",
		name:      "Node A",
		addresses: []string{fmt.Sprintf("127.0.0.1:%d", serverA.GetPort())},
		rpcPort:   serverA.GetPort(),
		online:    true,
	}
	clientB := newClient(clusterB, "")
	defer clientB.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	resp, err := clientB.CallWithContext(ctx, "node-A", "nonexistent_action", map[string]interface{}{
		"data": "test",
	})
	if err != nil {
		t.Fatalf("Expected response (not connection error), got: %v", err)
	}

	var result map[string]interface{}
	json.Unmarshal(resp, &result)

	if result["status"] != "no_handler" {
		t.Errorf("Expected status 'no_handler', got '%v'", result["status"])
	}
	errMsg, _ := result["error"].(string)
	if errMsg == "" {
		t.Error("Expected error message in no_handler response")
	}
}

// TestP2P_MessageValidation tests server-side message validation
// using raw TCP frames (bypassing the RPC client).
func TestP2P_MessageValidation(t *testing.T) {
	clusterA := newTestCluster("node-A", []string{"test"})
	serverA, err := startServer(clusterA, "")
	if err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer serverA.Stop()

	// Connect directly via raw TCP
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", serverA.GetPort()), 5*time.Second)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	time.Sleep(100 * time.Millisecond) // Let server accept and set up TCPConn

	// --- Sub-test 1: Message without "action" field ---
	invalidMsg := map[string]interface{}{
		"version": "1.0",
		"id":      "test-msg-1",
		"type":    "request",
		"from":    "test-node",
		"to":      "node-A",
		// Missing: "action"
	}
	msgData, _ := json.Marshal(invalidMsg)

	if err := sendRawFrame(conn, msgData); err != nil {
		t.Fatalf("Failed to send invalid frame: %v", err)
	}

	respData, err := readRawFrame(conn)
	if err != nil {
		t.Fatalf("Failed to read validation response: %v", err)
	}

	var resp map[string]interface{}
	json.Unmarshal(respData, &resp)

	if resp["type"] != "error" {
		t.Errorf("Expected error response type, got '%v'", resp["type"])
	}
	errMsg, _ := resp["error"].(string)
	if errMsg == "" {
		t.Error("Expected error message in validation response")
	}

	// --- Sub-test 2: Empty payload (valid message) ---
	emptyMsg := map[string]interface{}{
		"version": "1.0",
		"id":      "test-msg-2",
		"type":    "request",
		"from":    "test-node",
		"to":      "node-A",
		"action":  "ping",
		"payload": nil,
	}
	emptyData, _ := json.Marshal(emptyMsg)

	if err := sendRawFrame(conn, emptyData); err != nil {
		t.Fatalf("Failed to send empty payload frame: %v", err)
	}

	respData2, err := readRawFrame(conn)
	if err != nil {
		t.Fatalf("Failed to read ping response: %v", err)
	}

	var resp2 map[string]interface{}
	json.Unmarshal(respData2, &resp2)

	if resp2["type"] == "error" {
		t.Errorf("Ping should accept empty payload, got error: %v", resp2["error"])
	}
}

// TestP2P_FullEndToEnd_EncryptedAuthFlow exercises the complete chain:
// encrypted discovery → token authentication → peer_chat → callback.
func TestP2P_FullEndToEnd_EncryptedAuthFlow(t *testing.T) {
	sharedToken := "cluster-shared-secret-token"
	encKey := discovery.DeriveKey(sharedToken)

	// --- Phase 1: Encrypted discovery ---
	discPortA := allocateUDPPort(t)
	cbA := newTestDiscoveryCallbacks("node-A")

	discA, err := discovery.NewDiscovery(discPortA, cbA, encKey)
	if err != nil {
		t.Fatalf("Failed to create discovery A: %v", err)
	}

	// --- Phase 2: Setup authenticated RPC for both nodes ---
	capsA := []string{"code_analysis", "orchestration"}
	capsB := []string{"code_generation", "testing"}

	clusterA := newTestCluster("node-A", capsA)
	serverA, err := startServer(clusterA, sharedToken)
	if err != nil {
		t.Fatalf("Failed to start serverA: %v", err)
	}
	defer serverA.Stop()
	clientA := newClient(clusterA, sharedToken)
	defer clientA.Close()
	clusterA.rpcClient = clientA
	cbA.rpcPort = serverA.GetPort()

	// Track callbacks on A
	var callbackMu sync.Mutex
	var callbacks []map[string]interface{}

	serverA.RegisterHandler("peer_chat_callback", func(payload map[string]interface{}) (map[string]interface{}, error) {
		callbackMu.Lock()
		defer callbackMu.Unlock()
		callbacks = append(callbacks, payload)
		return map[string]interface{}{"status": "received", "task_id": payload["task_id"]}, nil
	})

	clusterB := newTestCluster("node-B", capsB)
	serverB, err := startServer(clusterB, sharedToken)
	if err != nil {
		t.Fatalf("Failed to start serverB: %v", err)
	}
	defer serverB.Stop()
	clientB := newClient(clusterB, sharedToken)
	defer clientB.Close()
	clusterB.rpcClient = clientB

	// Start discovery
	if err := discA.Start(); err != nil {
		t.Fatalf("Failed to start discovery A: %v", err)
	}
	defer discA.Stop()

	time.Sleep(100 * time.Millisecond)

	// Send encrypted announce from "node-B" to discovery A
	announceMsg := discovery.NewAnnounceMessage(
		"node-B", "Node B",
		[]string{"127.0.0.1"}, serverB.GetPort(),
		"worker", "development", nil, capsB,
	)
	msgData, _ := announceMsg.Bytes()
	encryptedData, err := encryptForTest(encKey, msgData)
	if err != nil {
		t.Fatalf("Failed to encrypt: %v", err)
	}

	targetAddr, _ := net.ResolveUDPAddr("udp", fmt.Sprintf("127.0.0.1:%d", discPortA))
	udpConn, _ := net.DialUDP("udp", nil, targetAddr)
	udpConn.Write(encryptedData)
	udpConn.Close()

	// Wait for discovery
	deadline := time.After(3 * time.Second)
	for {
		nodes := cbA.getDiscoveredNodes()
		if len(nodes) > 0 {
			if nodes[0].NodeID != "node-B" {
				t.Errorf("Expected to discover 'node-B', got '%s'", nodes[0].NodeID)
			}
			break
		}
		select {
		case <-deadline:
			t.Fatal("Timeout: discovery did not find node-B")
		default:
			time.Sleep(100 * time.Millisecond)
		}
	}

	// --- Phase 3: Authenticated RPC communication ---
	// Register peer (simulating post-discovery peer registration)
	portA := serverA.GetPort()
	portB := serverB.GetPort()
	clusterA.peers["node-B"] = &testNode{
		id:           "node-B",
		name:         "Node B",
		addresses:    []string{fmt.Sprintf("127.0.0.1:%d", portB)},
		rpcPort:      portB,
		capabilities: capsB,
		online:       true,
	}
	clusterB.peers["node-A"] = &testNode{
		id:           "node-A",
		name:         "Node A",
		addresses:    []string{fmt.Sprintf("127.0.0.1:%d", portA)},
		rpcPort:      portA,
		capabilities: capsA,
		online:       true,
	}

	// B: peer_chat handler with async callback
	serverB.RegisterHandler("peer_chat", func(payload map[string]interface{}) (map[string]interface{}, error) {
		taskID, _ := payload["task_id"].(string)
		if taskID == "" {
			taskID = fmt.Sprintf("e2e-task-%d", time.Now().UnixNano())
		}
		sourceInfo, _ := payload["_source"].(map[string]interface{})

		go func() {
			time.Sleep(100 * time.Millisecond)

			callbackPayload := map[string]interface{}{
				"task_id":  taskID,
				"status":   "success",
				"response": "E2E: Processed by node-B",
			}

			if sourceNodeID, ok := sourceInfo["node_id"].(string); ok && sourceNodeID != "" {
				ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
				defer cancel()
				clusterB.CallWithContext(ctx, sourceNodeID, "peer_chat_callback", callbackPayload)
			}
		}()

		return map[string]interface{}{"status": "accepted", "task_id": taskID}, nil
	})

	// A calls B via authenticated RPC
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	taskID := fmt.Sprintf("e2e-task-%d", time.Now().UnixNano())
	resp, err := clientA.CallWithContext(ctx, "node-B", "peer_chat", map[string]interface{}{
		"content": "E2E test message",
		"type":    "chat",
		"task_id": taskID,
		"_source": map[string]interface{}{
			"node_id": "node-A",
		},
	})
	if err != nil {
		t.Fatalf("Authenticated RPC call failed: %v", err)
	}

	var ack map[string]interface{}
	json.Unmarshal(resp, &ack)
	if ack["status"] != "accepted" {
		t.Errorf("Expected 'accepted', got '%v'", ack["status"])
	}

	// --- Phase 4: Verify callback ---
	deadline2 := time.After(5 * time.Second)
	for {
		callbackMu.Lock()
		count := len(callbacks)
		callbackMu.Unlock()
		if count > 0 {
			break
		}
		select {
		case <-deadline2:
			t.Fatal("Timeout waiting for callback")
		default:
			time.Sleep(50 * time.Millisecond)
		}
	}

	callbackMu.Lock()
	defer callbackMu.Unlock()
	if callbacks[0]["task_id"] != taskID {
		t.Errorf("Expected task_id '%s', got '%v'", taskID, callbacks[0]["task_id"])
	}
	if callbacks[0]["status"] != "success" {
		t.Errorf("Expected callback status 'success', got '%v'", callbacks[0]["status"])
	}
	if callbacks[0]["response"] != "E2E: Processed by node-B" {
		t.Errorf("Unexpected callback response: %v", callbacks[0]["response"])
	}
}
