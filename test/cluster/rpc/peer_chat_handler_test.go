// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc_test

import (
	"context"
	"fmt"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	clusterrpc "github.com/276793422/NemesisBot/module/cluster/rpc"
)

// mockTaskResultStore implements TaskResultStorer for testing
type mockTaskResultStore struct {
	mu        sync.Mutex
	running   map[string]bool
	results   map[string]*mockResult
	deleted   map[string]bool
}

type mockResult struct {
	resultStatus string
	response     string
	errMsg       string
	sourceNode   string
}

func newMockTaskResultStore() *mockTaskResultStore {
	return &mockTaskResultStore{
		running: make(map[string]bool),
		results: make(map[string]*mockResult),
		deleted: make(map[string]bool),
	}
}

func (m *mockTaskResultStore) SetRunning(taskID, sourceNode string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.running[taskID] = true
	delete(m.deleted, taskID)
}

func (m *mockTaskResultStore) SetResult(taskID, resultStatus, response, errMsg, sourceNode string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.results[taskID] = &mockResult{resultStatus, response, errMsg, sourceNode}
	delete(m.running, taskID)
	delete(m.deleted, taskID)
	return nil
}

func (m *mockTaskResultStore) Delete(taskID string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	delete(m.running, taskID)
	delete(m.results, taskID)
	m.deleted[taskID] = true
	return nil
}

func (m *mockTaskResultStore) isRunning(taskID string) bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.running[taskID]
}

func (m *mockTaskResultStore) hasResult(taskID string) bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.results[taskID] != nil
}

func (m *mockTaskResultStore) isDeleted(taskID string) bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.deleted[taskID]
}

// mockClusterWithResultStore extends mockCluster with injectable TaskResultStorer
type mockClusterWithResultStore struct {
	mockCluster
	resultStore  clusterrpc.TaskResultStorer
	callFail     bool // if true, CallWithContext always fails
	callCount    int
	callCountMu  sync.Mutex
}

func (m *mockClusterWithResultStore) GetTaskResultStorer() clusterrpc.TaskResultStorer {
	return m.resultStore
}

func (m *mockClusterWithResultStore) CallWithContext(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	m.callCountMu.Lock()
	m.callCount++
	m.callCountMu.Unlock()
	if m.callFail {
		return nil, fmt.Errorf("mock RPC failure")
	}
	return []byte(`{"status":"received"}`), nil
}

// TestPeerChatHandler_CallbackSuccess_DeletesResult tests that successful callback cleans up
func TestPeerChatHandler_CallbackSuccess_DeletesResult(t *testing.T) {
	mockStore := newMockTaskResultStore()
	cluster := &mockClusterWithResultStore{
		resultStore: mockStore,
	}
	handler := clusterrpc.NewPeerChatHandler(cluster, nil)

	payload := map[string]interface{}{
		"type":    "request",
		"content": "hello",
		"_source": map[string]interface{}{"node_id": "node-A"},
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Fatalf("Handle failed: %v", err)
	}

	status, _ := result["status"].(string)
	if status != "accepted" {
		t.Errorf("Expected status 'accepted', got %s", status)
	}

	// Wait for async processing to complete (RPCChannel is nil, so callback path triggers)
	time.Sleep(200 * time.Millisecond)

	// Since RPCChannel is nil, callback will try to call node-A
	// CallWithContext succeeds (mock), so result should be deleted
	taskID, _ := result["task_id"].(string)
	if !mockStore.isDeleted(taskID) {
		t.Error("Expected task result to be deleted after successful callback")
	}
}

// TestPeerChatHandler_CallbackFail_PersistsResult tests that failed callback persists result
// Note: sendCallback retries 3 times with backoff (5s, 10s), so this test takes ~15s
func TestPeerChatHandler_CallbackFail_PersistsResult(t *testing.T) {
	mockStore := newMockTaskResultStore()
	cluster := &mockClusterWithResultStore{
		resultStore: mockStore,
		callFail:    true, // All RPC calls fail
	}
	handler := clusterrpc.NewPeerChatHandler(cluster, nil)

	payload := map[string]interface{}{
		"type":    "request",
		"content": "hello",
		"_source": map[string]interface{}{"node_id": "node-A"},
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Fatalf("Handle failed: %v", err)
	}

	taskID, _ := result["task_id"].(string)

	// Wait for async processing with retries (3 retries with 5s+10s backoff = ~15s max)
	deadline := time.After(20 * time.Second)
	for {
		select {
		case <-deadline:
			t.Fatal("Timed out waiting for result persistence")
		default:
		}
		if mockStore.hasResult(taskID) {
			break
		}
		time.Sleep(100 * time.Millisecond)
	}

	// Callback failed, so result should be persisted
	if !mockStore.hasResult(taskID) {
		t.Error("Expected task result to be persisted after callback failure")
	}
}

// TestPeerChatHandler_SetRunningOnAccept tests that running state is set when task is accepted
func TestPeerChatHandler_SetRunningOnAccept(t *testing.T) {
	mockStore := newMockTaskResultStore()
	cluster := &mockClusterWithResultStore{
		resultStore: mockStore,
	}
	handler := clusterrpc.NewPeerChatHandler(cluster, nil)

	payload := map[string]interface{}{
		"type":    "request",
		"content": "hello",
		"_source": map[string]interface{}{"node_id": "node-A"},
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Fatalf("Handle failed: %v", err)
	}

	taskID, _ := result["task_id"].(string)

	// Running should be set
	if !mockStore.isRunning(taskID) {
		t.Error("Expected running state to be set")
	}
}

// TestPeerChatHandler_NoSourceNode_NoPersistence tests that no persistence happens without source
func TestPeerChatHandler_NoSourceNode_NoPersistence(t *testing.T) {
	mockStore := newMockTaskResultStore()
	cluster := &mockClusterWithResultStore{
		resultStore: mockStore,
		callFail:    true,
	}
	handler := clusterrpc.NewPeerChatHandler(cluster, nil)

	payload := map[string]interface{}{
		"type":    "request",
		"content": "hello",
		// No _source → no source node → callback fails silently
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Fatalf("Handle failed: %v", err)
	}

	// Wait for async processing
	time.Sleep(300 * time.Millisecond)

	taskID, _ := result["task_id"].(string)

	// Without source node, running shouldn't be set and no persistence
	if mockStore.isRunning(taskID) {
		t.Error("Expected no running state without source node")
	}
}

// TestPeerChatHandler_NilResultStore_NoPanic tests that nil resultStore doesn't cause panics
func TestPeerChatHandler_NilResultStore_NoPanic(t *testing.T) {
	cluster := &mockClusterWithResultStore{
		resultStore: nil,
		callFail:    true,
	}
	handler := clusterrpc.NewPeerChatHandler(cluster, nil)

	payload := map[string]interface{}{
		"type":    "request",
		"content": "hello",
		"_source": map[string]interface{}{"node_id": "node-A"},
	}

	result, err := handler.Handle(payload)
	if err != nil {
		t.Fatalf("Handle failed: %v", err)
	}
	if result == nil {
		t.Fatal("Expected non-nil result")
	}

	// Should not panic during async processing
	time.Sleep(300 * time.Millisecond)
}

// Ensure bus import is used
var _ = bus.InboundMessage{}
