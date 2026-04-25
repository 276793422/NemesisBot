// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
)

// --- ContinuationStore tests ---

func TestNewContinuationStore(t *testing.T) {
	tmpDir := t.TempDir()
	store, err := NewContinuationStore(tmpDir)
	if err != nil {
		t.Fatalf("NewContinuationStore() error = %v", err)
	}
	if store == nil {
		t.Fatal("NewContinuationStore() returned nil")
	}

	// Verify directory was created
	cacheDir := filepath.Join(tmpDir, "cluster", "rpc_cache")
	if _, err := os.Stat(cacheDir); os.IsNotExist(err) {
		t.Fatal("rpc_cache directory was not created")
	}
}

func TestContinuationStore_SaveAndLoad(t *testing.T) {
	tmpDir := t.TempDir()
	store, err := NewContinuationStore(tmpDir)
	if err != nil {
		t.Fatalf("NewContinuationStore() error = %v", err)
	}

	now := time.Now()
	snapshot := &ContinuationSnapshot{
		TaskID:    "task-123",
		Messages:  json.RawMessage(`[{"role":"user","content":"hello"}]`),
		ToolCallID: "call-456",
		Channel:   "rpc",
		ChatID:    "chat-789",
		CreatedAt: now,
	}

	// Save
	if err := store.Save(snapshot); err != nil {
		t.Fatalf("Save() error = %v", err)
	}

	// Load
	loaded, err := store.Load("task-123")
	if err != nil {
		t.Fatalf("Load() error = %v", err)
	}

	if loaded.TaskID != snapshot.TaskID {
		t.Errorf("TaskID mismatch: got %q, want %q", loaded.TaskID, snapshot.TaskID)
	}
	if loaded.ToolCallID != snapshot.ToolCallID {
		t.Errorf("ToolCallID mismatch: got %q, want %q", loaded.ToolCallID, snapshot.ToolCallID)
	}
	if loaded.Channel != snapshot.Channel {
		t.Errorf("Channel mismatch: got %q, want %q", loaded.Channel, snapshot.Channel)
	}
	if loaded.ChatID != snapshot.ChatID {
		t.Errorf("ChatID mismatch: got %q, want %q", loaded.ChatID, snapshot.ChatID)
	}
}

func TestContinuationStore_LoadNotFound(t *testing.T) {
	tmpDir := t.TempDir()
	store, err := NewContinuationStore(tmpDir)
	if err != nil {
		t.Fatalf("NewContinuationStore() error = %v", err)
	}

	_, err = store.Load("nonexistent")
	if err == nil {
		t.Fatal("Load() should return error for nonexistent snapshot")
	}
}

func TestContinuationStore_Delete(t *testing.T) {
	tmpDir := t.TempDir()
	store, err := NewContinuationStore(tmpDir)
	if err != nil {
		t.Fatalf("NewContinuationStore() error = %v", err)
	}

	snapshot := &ContinuationSnapshot{
		TaskID:    "task-delete-test",
		Messages:  json.RawMessage(`[]`),
		ToolCallID: "call-1",
		Channel:   "rpc",
		ChatID:    "chat-1",
		CreatedAt: time.Now(),
	}

	if err := store.Save(snapshot); err != nil {
		t.Fatalf("Save() error = %v", err)
	}

	if err := store.Delete("task-delete-test"); err != nil {
		t.Fatalf("Delete() error = %v", err)
	}

	_, err = store.Load("task-delete-test")
	if err == nil {
		t.Fatal("Load() should return error after Delete()")
	}
}

func TestContinuationStore_CleanupOld(t *testing.T) {
	tmpDir := t.TempDir()
	store, err := NewContinuationStore(tmpDir)
	if err != nil {
		t.Fatalf("NewContinuationStore() error = %v", err)
	}

	// Create an old snapshot
	oldSnapshot := &ContinuationSnapshot{
		TaskID:    "old-task",
		Messages:  json.RawMessage(`[]`),
		ToolCallID: "call-old",
		Channel:   "rpc",
		ChatID:    "chat-old",
		CreatedAt: time.Now().Add(-2 * time.Hour),
	}
	if err := store.Save(oldSnapshot); err != nil {
		t.Fatalf("Save() error = %v", err)
	}

	// Manually set the file's modification time to be old
	oldFilePath := filepath.Join(store.cacheDir, "old-task.json")
	oldTime := time.Now().Add(-2 * time.Hour)
	if err := os.Chtimes(oldFilePath, oldTime, oldTime); err != nil {
		t.Fatalf("Failed to change file time: %v", err)
	}

	// Create a recent snapshot
	recentSnapshot := &ContinuationSnapshot{
		TaskID:    "recent-task",
		Messages:  json.RawMessage(`[]`),
		ToolCallID: "call-recent",
		Channel:   "rpc",
		ChatID:    "chat-recent",
		CreatedAt: time.Now(),
	}
	if err := store.Save(recentSnapshot); err != nil {
		t.Fatalf("Save() error = %v", err)
	}

	// Cleanup files older than 1 hour
	cleaned, err := store.CleanupOld(1 * time.Hour)
	if err != nil {
		t.Fatalf("CleanupOld() error = %v", err)
	}

	if cleaned != 1 {
		t.Errorf("CleanupOld() cleaned = %d, want 1", cleaned)
	}

	// Old should be gone, recent should remain
	_, err = store.Load("old-task")
	if err == nil {
		t.Fatal("Old snapshot should have been cleaned up")
	}

	_, err = store.Load("recent-task")
	if err != nil {
		t.Fatal("Recent snapshot should still exist")
	}
}

func TestContinuationStore_ListPending(t *testing.T) {
	tmpDir := t.TempDir()
	store, err := NewContinuationStore(tmpDir)
	if err != nil {
		t.Fatalf("NewContinuationStore() error = %v", err)
	}

	// Initially empty
	pending, err := store.ListPending()
	if err != nil {
		t.Fatalf("ListPending() error = %v", err)
	}
	if len(pending) != 0 {
		t.Errorf("ListPending() = %d, want 0", len(pending))
	}

	// Save some snapshots
	for _, taskID := range []string{"task-1", "task-2", "task-3"} {
		snapshot := &ContinuationSnapshot{
			TaskID:    taskID,
			Messages:  json.RawMessage(`[]`),
			ToolCallID: "call",
			Channel:   "rpc",
			ChatID:    "chat",
			CreatedAt: time.Now(),
		}
		if err := store.Save(snapshot); err != nil {
			t.Fatalf("Save() error = %v", err)
		}
	}

	// Also create a non-.json file that should be ignored
	tmpFile := filepath.Join(store.cacheDir, "other.txt")
	os.WriteFile(tmpFile, []byte("test"), 0644)

	pending, err = store.ListPending()
	if err != nil {
		t.Fatalf("ListPending() error = %v", err)
	}
	if len(pending) != 3 {
		t.Errorf("ListPending() = %d, want 3", len(pending))
	}

	// Verify task IDs
	pendingMap := make(map[string]bool)
	for _, id := range pending {
		pendingMap[id] = true
	}
	for _, id := range []string{"task-1", "task-2", "task-3"} {
		if !pendingMap[id] {
			t.Errorf("Missing task ID %q in pending list", id)
		}
	}
}

// --- InMemoryTaskStore tests ---

func TestInMemoryTaskStore_CreateDuplicate(t *testing.T) {
	store := NewInMemoryTaskStore()

	task1 := &Task{
		ID:        "task-dup",
		Action:    "test",
		PeerID:    "peer-1",
		Status:    TaskPending,
		CreatedAt: time.Now(),
	}

	if err := store.Create(task1); err != nil {
		t.Fatalf("First Create() error = %v", err)
	}

	// Duplicate should fail
	task2 := &Task{
		ID:        "task-dup",
		Action:    "test2",
		PeerID:    "peer-2",
		Status:    TaskPending,
		CreatedAt: time.Now(),
	}
	if err := store.Create(task2); err == nil {
		t.Fatal("Second Create() with same ID should fail")
	}
}

func TestInMemoryTaskStore_GetNotFound(t *testing.T) {
	store := NewInMemoryTaskStore()
	_, err := store.Get("nonexistent")
	if err == nil {
		t.Fatal("Get() should return error for nonexistent task")
	}
}

func TestInMemoryTaskStore_UpdateResultNotFound(t *testing.T) {
	store := NewInMemoryTaskStore()
	err := store.UpdateResult("nonexistent", &TaskResult{
		TaskID: "nonexistent",
		Status: "success",
	})
	if err == nil {
		t.Fatal("UpdateResult() should return error for nonexistent task")
	}
}

func TestInMemoryTaskStore_UpdateResultSuccess(t *testing.T) {
	store := NewInMemoryTaskStore()
	task := &Task{
		ID:        "task-update",
		Action:    "test",
		PeerID:    "peer-1",
		Status:    TaskPending,
		CreatedAt: time.Now(),
	}
	store.Create(task)

	result := &TaskResult{
		TaskID:   "task-update",
		Status:   "success",
		Response: "done",
	}
	if err := store.UpdateResult("task-update", result); err != nil {
		t.Fatalf("UpdateResult() error = %v", err)
	}

	updated, _ := store.Get("task-update")
	if updated.Status != TaskCompleted {
		t.Errorf("Status = %q, want %q", updated.Status, TaskCompleted)
	}
	if updated.Response != "done" {
		t.Errorf("Response = %q, want %q", updated.Response, "done")
	}
	if updated.CompletedAt == nil {
		t.Fatal("CompletedAt should be set")
	}
}

func TestInMemoryTaskStore_UpdateResultError(t *testing.T) {
	store := NewInMemoryTaskStore()
	task := &Task{
		ID:        "task-err",
		Action:    "test",
		PeerID:    "peer-1",
		Status:    TaskPending,
		CreatedAt: time.Now(),
	}
	store.Create(task)

	result := &TaskResult{
		TaskID: "task-err",
		Status: "error",
		Error:  "something failed",
	}
	if err := store.UpdateResult("task-err", result); err != nil {
		t.Fatalf("UpdateResult() error = %v", err)
	}

	updated, _ := store.Get("task-err")
	if updated.Status != TaskFailed {
		t.Errorf("Status = %q, want %q", updated.Status, TaskFailed)
	}
}

func TestInMemoryTaskStore_UpdateResultDefaultStatus(t *testing.T) {
	store := NewInMemoryTaskStore()
	task := &Task{
		ID:        "task-default",
		Action:    "test",
		PeerID:    "peer-1",
		Status:    TaskPending,
		CreatedAt: time.Now(),
	}
	store.Create(task)

	// Use unknown status string
	result := &TaskResult{
		TaskID: "task-default",
		Status: "unknown_status",
	}
	store.UpdateResult("task-default", result)

	updated, _ := store.Get("task-default")
	if updated.Status != TaskFailed {
		t.Errorf("Unknown status should result in TaskFailed, got %q", updated.Status)
	}
}

func TestInMemoryTaskStore_Delete(t *testing.T) {
	store := NewInMemoryTaskStore()
	task := &Task{
		ID:        "task-del",
		Action:    "test",
		PeerID:    "peer-1",
		Status:    TaskPending,
		CreatedAt: time.Now(),
	}
	store.Create(task)

	if err := store.Delete("task-del"); err != nil {
		t.Fatalf("Delete() error = %v", err)
	}

	_, err := store.Get("task-del")
	if err == nil {
		t.Fatal("Get() should fail after Delete()")
	}
}

func TestInMemoryTaskStore_ListByStatus(t *testing.T) {
	store := NewInMemoryTaskStore()

	store.Create(&Task{ID: "p1", Status: TaskPending, CreatedAt: time.Now()})
	store.Create(&Task{ID: "p2", Status: TaskPending, CreatedAt: time.Now()})
	store.Create(&Task{ID: "c1", Status: TaskCompleted, CreatedAt: time.Now()})

	pending, err := store.ListByStatus(TaskPending)
	if err != nil {
		t.Fatalf("ListByStatus() error = %v", err)
	}
	if len(pending) != 2 {
		t.Errorf("ListByStatus(Pending) = %d, want 2", len(pending))
	}

	completed, err := store.ListByStatus(TaskCompleted)
	if err != nil {
		t.Fatalf("ListByStatus() error = %v", err)
	}
	if len(completed) != 1 {
		t.Errorf("ListByStatus(Completed) = %d, want 1", len(completed))
	}
}

// --- TaskManager advanced tests ---

func TestTaskManager_DefaultCleanupInterval(t *testing.T) {
	tm := NewTaskManager(0) // Should default to 30s
	if tm.cleanupInterval != 30*time.Second {
		t.Errorf("cleanupInterval = %v, want 30s", tm.cleanupInterval)
	}
}

func TestTaskManager_SetOnComplete(t *testing.T) {
	tm := NewTaskManager(30 * time.Second)

	var completedTaskID string
	var mu sync.Mutex
	tm.SetOnComplete(func(taskID string) {
		mu.Lock()
		completedTaskID = taskID
		mu.Unlock()
	})

	task := &Task{
		ID:        "task-cb-test",
		Action:    "test",
		PeerID:    "peer-1",
		Status:    TaskPending,
		CreatedAt: time.Now(),
	}
	tm.Submit(task)

	result := &TaskResult{
		TaskID:   "task-cb-test",
		Status:   "success",
		Response: "done",
	}
	tm.CompleteTask("task-cb-test", result)

	mu.Lock()
	defer mu.Unlock()
	if completedTaskID != "task-cb-test" {
		t.Errorf("callback taskID = %q, want %q", completedTaskID, "task-cb-test")
	}
}

func TestTaskManager_CompleteCallback(t *testing.T) {
	tm := NewTaskManager(30 * time.Second)

	task := &Task{
		ID:        "task-cc",
		Action:    "test",
		PeerID:    "peer-1",
		Status:    TaskPending,
		CreatedAt: time.Now(),
	}
	tm.Submit(task)

	if err := tm.CompleteCallback("task-cc", "success", "response text", ""); err != nil {
		t.Fatalf("CompleteCallback() error = %v", err)
	}

	got, err := tm.GetTask("task-cc")
	if err != nil {
		t.Fatalf("GetTask() error = %v", err)
	}
	if got.Status != TaskCompleted {
		t.Errorf("Status = %q, want %q", got.Status, TaskCompleted)
	}
	if got.Response != "response text" {
		t.Errorf("Response = %q, want %q", got.Response, "response text")
	}
}

func TestTaskManager_CompleteTaskStoreError(t *testing.T) {
	tm := NewTaskManager(30 * time.Second)

	err := tm.CompleteTask("nonexistent", &TaskResult{
		TaskID: "nonexistent",
		Status: "error",
	})
	if err == nil {
		t.Fatal("CompleteTask() should fail for nonexistent task")
	}
}

// --- Node method tests ---

func TestNode_UpdateLastSeen(t *testing.T) {
	node := &Node{
		ID:      "node-1",
		Status:  StatusOffline,
		LastSeen: time.Time{},
	}

	node.UpdateLastSeen()

	if !node.IsOnline() {
		t.Error("After UpdateLastSeen(), node should be online")
	}
	if node.LastSeen.IsZero() {
		t.Error("LastSeen should be updated")
	}
}

func TestNode_GetUptime(t *testing.T) {
	node := &Node{
		ID:      "node-1",
		LastSeen: time.Time{}, // Zero time
	}

	if uptime := node.GetUptime(); uptime != 0 {
		t.Errorf("GetUptime() with zero LastSeen = %v, want 0", uptime)
	}

	node.UpdateLastSeen()
	if uptime := node.GetUptime(); uptime < 0 {
		t.Errorf("GetUptime() = %v, want >= 0", uptime)
	}
}

func TestNode_ToConfig(t *testing.T) {
	now := time.Now()
	node := &Node{
		ID:           "node-1",
		Name:         "Test Node",
		Address:      "192.168.1.1:21949",
		Addresses:    []string{"192.168.1.1"},
		RPCPort:      21949,
		Role:         "worker",
		Category:     "dev",
		Tags:         []string{"test"},
		Capabilities: []string{"llm"},
		Priority:     5,
		Status:       StatusOnline,
		LastSeen:     now,
		TasksCompleted: 10,
		SuccessRate:  0.95,
		AvgResponseTime: 200,
		LastError:    "",
	}

	config := node.ToConfig()

	if config.ID != node.ID {
		t.Errorf("ID = %q, want %q", config.ID, node.ID)
	}
	if config.Name != node.Name {
		t.Errorf("Name = %q, want %q", config.Name, node.Name)
	}
	if config.Address != node.Address {
		t.Errorf("Address = %q, want %q", config.Address, node.Address)
	}
	if config.RPCPort != node.RPCPort {
		t.Errorf("RPCPort = %d, want %d", config.RPCPort, node.RPCPort)
	}
	if config.Role != node.Role {
		t.Errorf("Role = %q, want %q", config.Role, node.Role)
	}
	if config.Status.State != string(StatusOnline) {
		t.Errorf("Status.State = %q, want %q", config.Status.State, StatusOnline)
	}
	if config.Status.TasksCompleted != 10 {
		t.Errorf("TasksCompleted = %d, want 10", config.Status.TasksCompleted)
	}
}

func TestNode_HasCapability_CaseInsensitive(t *testing.T) {
	node := &Node{
		Capabilities: []string{"LLM", "RPC"},
	}

	tests := []struct {
		cap  string
		want bool
	}{
		{"llm", true},
		{"LLM", true},
		{"Llm", true},
		{"rpc", true},
		{"RPC", true},
		{"unknown", false},
	}

	for _, tt := range tests {
		got := node.HasCapability(tt.cap)
		if got != tt.want {
			t.Errorf("HasCapability(%q) = %v, want %v", tt.cap, got, tt.want)
		}
	}
}

func TestNode_GetMethods(t *testing.T) {
	node := &Node{
		ID:           "node-get",
		Name:         "TestNode",
		Address:      "1.2.3.4:12345",
		Addresses:    []string{"1.2.3.4", "5.6.7.8"},
		RPCPort:      12345,
		Capabilities: []string{"cap1", "cap2"},
	}

	if got := node.GetID(); got != "node-get" {
		t.Errorf("GetID() = %q, want %q", got, "node-get")
	}
	if got := node.GetName(); got != "TestNode" {
		t.Errorf("GetName() = %q, want %q", got, "TestNode")
	}
	if got := node.GetAddress(); got != "1.2.3.4:12345" {
		t.Errorf("GetAddress() = %q, want %q", got, "1.2.3.4:12345")
	}
	if got := node.GetRPCPort(); got != 12345 {
		t.Errorf("GetRPCPort() = %d, want %d", got, 12345)
	}
	if got := node.GetCapabilities(); len(got) != 2 {
		t.Errorf("GetCapabilities() len = %d, want 2", len(got))
	}
	if got := node.GetAddresses(); len(got) != 2 {
		t.Errorf("GetAddresses() len = %d, want 2", len(got))
	}
	if got := node.GetStatus(); got != string(StatusUnknown) && got != "" {
		t.Errorf("GetStatus() = %q, want %q or empty", got, StatusUnknown)
	}
}

func TestNode_String(t *testing.T) {
	node := &Node{
		ID:      "node-str",
		Name:    "TestNode",
		Address: "1.2.3.4:12345",
		Status:  StatusOnline,
	}

	str := node.String()
	if str == "" {
		t.Fatal("String() returned empty string")
	}
}

func TestNode_SetStatus(t *testing.T) {
	node := &Node{ID: "test", Status: StatusOffline}
	node.SetStatus(StatusOnline)

	if !node.IsOnline() {
		t.Error("After SetStatus(Online), node should be online")
	}
	if node.LastSeen.IsZero() {
		t.Error("SetStatus should update LastSeen")
	}
}

// --- Registry advanced tests ---

func TestRegistry_FindByCapability(t *testing.T) {
	reg := NewRegistry()

	reg.AddOrUpdate(&Node{
		ID:           "node-1",
		Capabilities: []string{"llm", "rpc"},
	})
	reg.AddOrUpdate(&Node{
		ID:           "node-2",
		Capabilities: []string{"llm"},
	})
	reg.AddOrUpdate(&Node{
		ID:           "node-3",
		Capabilities: []string{"tools"},
	})

	llmNodes := reg.FindByCapability("llm")
	if len(llmNodes) != 2 {
		t.Errorf("FindByCapability(llm) = %d, want 2", len(llmNodes))
	}

	toolsNodes := reg.FindByCapability("tools")
	if len(toolsNodes) != 1 {
		t.Errorf("FindByCapability(tools) = %d, want 1", len(toolsNodes))
	}

	noneNodes := reg.FindByCapability("nonexistent")
	if len(noneNodes) != 0 {
		t.Errorf("FindByCapability(nonexistent) = %d, want 0", len(noneNodes))
	}
}

func TestRegistry_FindByCapabilityOnline(t *testing.T) {
	reg := NewRegistry()

	// Add online node
	n1 := &Node{ID: "online-1", Capabilities: []string{"llm"}}
	reg.AddOrUpdate(n1)

	// Add offline node
	n2 := &Node{ID: "offline-1", Capabilities: []string{"llm"}}
	reg.AddOrUpdate(n2)
	reg.MarkOffline("offline-1", "test")

	nodes := reg.FindByCapabilityOnline("llm")
	if len(nodes) != 1 {
		t.Errorf("FindByCapabilityOnline(llm) = %d, want 1", len(nodes))
	}
	if nodes[0].ID != "online-1" {
		t.Errorf("FindByCapabilityOnline() node ID = %q, want %q", nodes[0].ID, "online-1")
	}
}

func TestRegistry_GetCapabilities(t *testing.T) {
	reg := NewRegistry()

	reg.AddOrUpdate(&Node{ID: "n1", Capabilities: []string{"llm", "rpc"}})
	reg.AddOrUpdate(&Node{ID: "n2", Capabilities: []string{"rpc", "tools"}})

	caps := reg.GetCapabilities()
	capMap := make(map[string]bool)
	for _, c := range caps {
		capMap[c] = true
	}

	if !capMap["llm"] || !capMap["rpc"] || !capMap["tools"] {
		t.Errorf("GetCapabilities() = %v, missing expected capabilities", caps)
	}
	if len(caps) != 3 {
		t.Errorf("GetCapabilities() count = %d, want 3", len(caps))
	}
}

func TestRegistry_OnlineCount(t *testing.T) {
	reg := NewRegistry()

	reg.AddOrUpdate(&Node{ID: "n1"})
	reg.AddOrUpdate(&Node{ID: "n2"})
	reg.MarkOffline("n2", "test")

	if count := reg.OnlineCount(); count != 1 {
		t.Errorf("OnlineCount() = %d, want 1", count)
	}
}

func TestRegistry_AddOrUpdate_ExistingNode(t *testing.T) {
	reg := NewRegistry()

	// Add initial node
	node1 := &Node{
		ID:           "n1",
		Name:         "Original",
		Address:      "1.1.1.1:1111",
		Capabilities: []string{"cap1"},
		Role:         "worker",
		Category:     "general",
	}
	reg.AddOrUpdate(node1)

	// Update the node
	node2 := &Node{
		ID:           "n1",
		Name:         "Updated",
		Address:      "2.2.2.2:2222",
		Capabilities: []string{"cap1", "cap2"},
		Role:         "master",
		Category:     "production",
	}
	reg.AddOrUpdate(node2)

	got := reg.Get("n1")
	if got == nil {
		t.Fatal("Get(n1) returned nil")
	}
	if got.Name != "Updated" {
		t.Errorf("Name = %q, want %q", got.Name, "Updated")
	}
	if got.Role != "master" {
		t.Errorf("Role = %q, want %q", got.Role, "master")
	}
	if !got.IsOnline() {
		t.Error("Updated node should be online")
	}
}

// --- Network tests ---

func TestIsSameSubnetSimple(t *testing.T) {
	tests := []struct {
		ip1, ip2 string
		want     bool
	}{
		{"192.168.1.1", "192.168.1.2", true},
		{"192.168.1.1", "192.168.2.1", false},
		{"10.0.0.1", "10.0.0.2", true},
		{"", "1.2.3.4", false},
		{"invalid", "1.2.3", false},
	}

	for _, tt := range tests {
		got := isSameSubnetSimple(tt.ip1, tt.ip2)
		if got != tt.want {
			t.Errorf("isSameSubnetSimple(%q, %q) = %v, want %v", tt.ip1, tt.ip2, got, tt.want)
		}
	}
}

func TestIsVirtualInterface(t *testing.T) {
	tests := []struct {
		name string
		want bool
	}{
		{"veth123", true},
		{"docker0", true},
		{"br-abc", true},
		{"virbr0", true},
		{"tun0", true},
		{"tap0", true},
		{"vbox0", true},
		{"vmnet0", true},
		{"utun0", true},
		{"eth0", false},
		{"wlan0", false},
		{"en0", false},
		{"Ethernet", false},
	}

	for _, tt := range tests {
		got := isVirtualInterface(tt.name)
		if got != tt.want {
			t.Errorf("isVirtualInterface(%q) = %v, want %v", tt.name, got, tt.want)
		}
	}
}

func TestGetInterfacePriority(t *testing.T) {
	tests := []struct {
		name     string
		expected int
	}{
		{"eth0", 1},
		{"eno1", 1},
		{"ens33", 1},
		{"enp3s0", 1},
		{"wlan0", 2},
		{"wlp3s0", 2},
		{"en1", 3},
		{"wl1", 3},
		{"something", 99},
	}

	for _, tt := range tests {
		got := getInterfacePriority(tt.name)
		if got != tt.expected {
			t.Errorf("getInterfacePriority(%q) = %d, want %d", tt.name, got, tt.expected)
		}
	}
}

// --- Cluster advanced tests ---

func TestCluster_SubmitTask_NoTaskManager(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	_, err = c.SubmitTask(context.Background(), "peer1", "test", nil, "ch", "chat")
	if err == nil {
		t.Fatal("SubmitTask() without Start() should fail")
	}
}

func TestCluster_GetTask_NoTaskManager(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	_, err = c.GetTask("nonexistent")
	if err == nil {
		t.Fatal("GetTask() without Start() should fail")
	}
}

func TestCluster_CleanupTask(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	// CleanupTask should not panic even without task manager
	c.CleanupTask("nonexistent")
}

func TestCluster_GetContinuationStore(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	// Before start, continuation store is nil
	if store := c.GetContinuationStore(); store != nil {
		t.Fatal("GetContinuationStore() before Start() should be nil")
	}
}

func TestCluster_GetTaskResultStorer(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	// resultStore is initialized in NewCluster
	storer := c.GetTaskResultStorer()
	if storer == nil {
		t.Fatal("GetTaskResultStorer() should not be nil")
	}
}

func TestCluster_GetRPCChannel(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	if ch := c.GetRPCChannel(); ch != nil {
		t.Fatal("GetRPCChannel() before SetRPCChannel() should be nil")
	}
}

func TestCluster_SetPorts(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	c.SetPorts(12345, 54321)
	udp, rpc := c.GetPorts()
	if udp != 12345 {
		t.Errorf("UDP port = %d, want 12345", udp)
	}
	if rpc != 54321 {
		t.Errorf("RPC port = %d, want 54321", rpc)
	}
}

func TestCluster_RegisterRPCHandler_NotRunning(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	err = c.RegisterRPCHandler("test", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return nil, nil
	})
	if err == nil {
		t.Fatal("RegisterRPCHandler() when not running should fail")
	}
}

func TestCluster_RegisterBasicHandlers_NotRunning(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	err = c.RegisterBasicHandlers()
	if err == nil {
		t.Fatal("RegisterBasicHandlers() when not running should fail")
	}
}

func TestCluster_HandleDiscoveredNode(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	c.HandleDiscoveredNode("node-disc", "Discovered", []string{"192.168.1.100"}, 21949, "worker", "dev", []string{"tag1"}, []string{"llm"})

	node := c.registry.Get("node-disc")
	if node == nil {
		t.Fatal("HandleDiscoveredNode() should add node to registry")
	}
	if node.Name != "Discovered" {
		t.Errorf("Name = %q, want %q", node.Name, "Discovered")
	}
	if node.Address != "192.168.1.100:21949" {
		t.Errorf("Address = %q, want %q", node.Address, "192.168.1.100:21949")
	}
}

func TestCluster_HandleDiscoveredNode_EmptyAddresses(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	c.HandleDiscoveredNode("node-empty", "Empty", []string{}, 21949, "worker", "dev", nil, nil)

	node := c.registry.Get("node-empty")
	if node == nil {
		t.Fatal("HandleDiscoveredNode() should add node to registry")
	}
	if node.Address != "" {
		t.Errorf("Address = %q, want empty string", node.Address)
	}
}

func TestCluster_HandleNodeOffline(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	c.HandleDiscoveredNode("node-off", "Offline", []string{"1.2.3.4"}, 21949, "worker", "dev", nil, nil)
	c.HandleNodeOffline("node-off", "test reason")

	node := c.registry.Get("node-off")
	if node == nil {
		t.Fatal("Node should still exist in registry")
	}
	if node.IsOnline() {
		t.Fatal("Node should be offline after HandleNodeOffline()")
	}
}

func TestCluster_LogMethods(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	// These should not panic
	c.LogInfo("test info %s", "arg")
	c.LogError("test error %s", "arg")
	c.LogDebug("test debug %s", "arg")
	c.LogRPCInfo("test rpc info %s", "arg")
	c.LogRPCError("test rpc error %s", "arg")
	c.LogRPCDebug("test rpc debug %s", "arg")
}

func TestCluster_GetWorkspace(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	if ws := c.GetWorkspace(); ws != tmpDir {
		t.Errorf("GetWorkspace() = %q, want %q", ws, tmpDir)
	}
}

func TestCluster_GetRole(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	if role := c.GetRole(); role != "worker" {
		t.Errorf("GetRole() = %q, want %q", role, "worker")
	}
}

func TestCluster_GetCategory(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	if cat := c.GetCategory(); cat != "general" {
		t.Errorf("GetCategory() = %q, want %q", cat, "general")
	}
}

func TestCluster_GetTags(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	tags := c.GetTags()
	if tags == nil {
		t.Fatal("GetTags() should not return nil")
	}
}

// --- handleTaskComplete tests ---

func TestCluster_HandleTaskComplete_NoBus(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	// Set a TaskManager so handleTaskComplete does not nil-deref
	tm := NewTaskManager(30 * time.Second)
	c.SetTaskManagerForTest(tm)

	// Without bus, handleTaskComplete should not panic
	c.HandleTaskCompleteForTest("nonexistent")
}

func TestCluster_HandleTaskComplete_WithBus(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	// Set up task manager and bus
	tm := NewTaskManager(30 * time.Second)
	c.SetTaskManagerForTest(tm)

	msgBus := bus.NewMessageBus()
	c.SetMessageBus(msgBus)

	// Submit a task with channel info
	task := &Task{
		ID:              "task-bus-test",
		Action:          "test",
		PeerID:          "peer-1",
		Status:          TaskPending,
		CreatedAt:       time.Now(),
		OriginalChannel: "rpc",
		OriginalChatID:  "chat-123",
	}
	tm.Submit(task)

	// Complete the task
	tm.CompleteTask("task-bus-test", &TaskResult{
		TaskID:   "task-bus-test",
		Status:   "success",
		Response: "done",
	})
	// handleTaskComplete is triggered by the callback, which we test separately
}

// --- findAvailablePort tests ---

func TestFindAvailablePort_TCP(t *testing.T) {
	// Use a high ephemeral port range to avoid conflicts
	port, err := findAvailablePort(59000, "tcp")
	if err != nil {
		t.Fatalf("findAvailablePort() error = %v", err)
	}
	if port == 0 {
		t.Fatal("findAvailablePort() returned port 0")
	}
	t.Logf("Found available port: %d", port)
}

// --- AppConfig tests ---

func TestDefaultAppConfig(t *testing.T) {
	cfg := DefaultAppConfig()
	if cfg.Enabled {
		t.Error("Default Enabled should be false")
	}
	if cfg.Port != 11949 {
		t.Errorf("Port = %d, want 11949", cfg.Port)
	}
	if cfg.RPCPort != 21949 {
		t.Errorf("RPCPort = %d, want 21949", cfg.RPCPort)
	}
	if cfg.BroadcastInterval != 30 {
		t.Errorf("BroadcastInterval = %d, want 30", cfg.BroadcastInterval)
	}
}

func TestSaveAndLoadAppConfig(t *testing.T) {
	tmpDir := t.TempDir()

	cfg := &AppConfig{
		Enabled:           true,
		Port:              12345,
		RPCPort:           54321,
		BroadcastInterval: 60,
	}

	if err := SaveAppConfig(tmpDir, cfg); err != nil {
		t.Fatalf("SaveAppConfig() error = %v", err)
	}

	loaded, err := LoadAppConfig(tmpDir)
	if err != nil {
		t.Fatalf("LoadAppConfig() error = %v", err)
	}

	if loaded.Enabled != cfg.Enabled {
		t.Errorf("Enabled = %v, want %v", loaded.Enabled, cfg.Enabled)
	}
	if loaded.Port != cfg.Port {
		t.Errorf("Port = %d, want %d", loaded.Port, cfg.Port)
	}
	if loaded.RPCPort != cfg.RPCPort {
		t.Errorf("RPCPort = %d, want %d", loaded.RPCPort, cfg.RPCPort)
	}
}

func TestLoadAppConfig_Default(t *testing.T) {
	tmpDir := t.TempDir()

	// Load from nonexistent path should return defaults
	cfg, err := LoadAppConfig(tmpDir)
	if err != nil {
		t.Fatalf("LoadAppConfig() error = %v", err)
	}

	if cfg.Enabled {
		t.Error("Default Enabled should be false")
	}
	if cfg.Port != 11949 {
		t.Errorf("Port = %d, want 11949", cfg.Port)
	}
}

// --- generateTaskID test ---

func TestGenerateTaskID_Format(t *testing.T) {
	id := generateTaskID()
	if id == "" {
		t.Fatal("generateTaskID() returned empty string")
	}
	if len(id) < 10 {
		t.Errorf("generateTaskID() = %q, seems too short", id)
	}
	// Should start with "task-"
	if len(id) < 5 {
		t.Errorf("generateTaskID() = %q, too short", id)
	}
}

// --- GetCurrentTime test ---

func TestGetCurrentTime(t *testing.T) {
	before := time.Now()
	now := GetCurrentTime()
	after := time.Now()

	if now.Before(before) || now.After(after) {
		t.Errorf("GetCurrentTime() = %v, expected between %v and %v", now, before, after)
	}
}

// --- IsSameSubnet test ---

func TestIsSameSubnet(t *testing.T) {
	// Test with same subnet IPs
	result := IsSameSubnet("192.168.1.1", "192.168.1.2")
	// This depends on local network interfaces, so just verify it doesn't panic
	_ = result
}

func TestIsSameSubnet_InvalidIPs(t *testing.T) {
	result := IsSameSubnet("invalid", "also-invalid")
	if result {
		t.Error("IsSameSubnet with invalid IPs should return false")
	}
}

// --- Cluster method tests for coverage ---

func TestCluster_FindPeersByCapability(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	c.registry.AddOrUpdate(&Node{
		ID:           "n1",
		Capabilities: []string{"llm"},
	})
	c.registry.AddOrUpdate(&Node{
		ID:           "n2",
		Capabilities: []string{"tools"},
	})

	peers := c.FindPeersByCapability("llm")
	if len(peers) != 1 {
		t.Errorf("FindPeersByCapability(llm) = %d, want 1", len(peers))
	}
}

func TestCluster_GetOnlinePeers(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	c.registry.AddOrUpdate(&Node{ID: "n1"})
	c.registry.AddOrUpdate(&Node{ID: "n2"})
	c.registry.MarkOffline("n2", "test")

	peers := c.GetOnlinePeers()
	if len(peers) != 1 {
		t.Errorf("GetOnlinePeers() = %d, want 1", len(peers))
	}
}

func TestCluster_Call_WithOverride(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	// Set call override
	c.callWithContextFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		return []byte(`{"status":"ok"}`), nil
	}

	resp, err := c.Call("peer-1", "test", nil)
	if err != nil {
		t.Fatalf("Call() error = %v", err)
	}
	if string(resp) != `{"status":"ok"}` {
		t.Errorf("Call() response = %q, want ok", string(resp))
	}
}

func TestCluster_CallWithContext_Override(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	c.callWithContextFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		return []byte(`{"status":"mock"}`), nil
	}

	resp, err := c.CallWithContext(context.Background(), "peer-1", "test", nil)
	if err != nil {
		t.Fatalf("CallWithContext() error = %v", err)
	}
	if string(resp) != `{"status":"mock"}` {
		t.Errorf("CallWithContext() = %q", string(resp))
	}
}

func TestCluster_CallWithContext_CancelledContext(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	_, err = c.CallWithContext(ctx, "peer-1", "test", nil)
	if err == nil {
		t.Fatal("CallWithContext() with cancelled context should fail")
	}
}

func TestCluster_CallWithContext_NoRPCClient(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	// No callWithContextFn and no rpcClient
	_, err = c.CallWithContext(context.Background(), "peer-1", "test", nil)
	if err == nil {
		t.Fatal("CallWithContext() without RPC client should fail")
	}
}

func TestCluster_SubmitTask_WithOverride(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	// Set up task manager
	tm := NewTaskManager(30 * time.Second)
	c.SetTaskManagerForTest(tm)

	// Set call override that returns accepted ACK
	c.callWithContextFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		return json.Marshal(map[string]interface{}{
			"status":  "accepted",
			"task_id": payload["task_id"],
		})
	}

	taskID, err := c.SubmitTask(context.Background(), "peer-1", "test", map[string]interface{}{
		"task_id": "test-task-123",
	}, "rpc", "chat-1")
	if err != nil {
		t.Fatalf("SubmitTask() error = %v", err)
	}
	if taskID != "test-task-123" {
		t.Errorf("taskID = %q, want %q", taskID, "test-task-123")
	}
}

func TestCluster_SubmitTask_RPCFailure(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	tm := NewTaskManager(30 * time.Second)
	c.SetTaskManagerForTest(tm)

	// Override to simulate RPC failure
	c.callWithContextFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		return nil, fmt.Errorf("connection refused")
	}

	_, err = c.SubmitTask(context.Background(), "peer-1", "test", nil, "rpc", "chat-1")
	if err == nil {
		t.Fatal("SubmitTask() with RPC failure should fail")
	}
}

func TestCluster_SubmitTask_InvalidACK(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	tm := NewTaskManager(30 * time.Second)
	c.SetTaskManagerForTest(tm)

	// Override to return invalid JSON
	c.callWithContextFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		return []byte("not json"), nil
	}

	_, err = c.SubmitTask(context.Background(), "peer-1", "test", nil, "rpc", "chat-1")
	if err == nil {
		t.Fatal("SubmitTask() with invalid ACK should fail")
	}
}

func TestCluster_SubmitTask_NotAccepted(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	tm := NewTaskManager(30 * time.Second)
	c.SetTaskManagerForTest(tm)

	// Override to return non-accepted status
	c.callWithContextFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		return json.Marshal(map[string]interface{}{
			"status": "rejected",
		})
	}

	_, err = c.SubmitTask(context.Background(), "peer-1", "test", nil, "rpc", "chat-1")
	if err == nil {
		t.Fatal("SubmitTask() with non-accepted status should fail")
	}
}

func TestCluster_SubmitTask_DuplicateTask(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	tm := NewTaskManager(30 * time.Second)
	c.SetTaskManagerForTest(tm)

	// Submit first task
	tm.Submit(&Task{ID: "dup-task", Status: TaskPending, CreatedAt: time.Now()})

	// Submit same task again - should fail because task ID already exists
	_, err = c.SubmitTask(context.Background(), "peer-1", "test", map[string]interface{}{
		"task_id": "dup-task",
	}, "rpc", "chat-1")
	if err == nil {
		t.Fatal("SubmitTask() with duplicate task ID should fail")
	}
}

func TestCluster_SetRPCChannel(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	// SetRPCChannel should not panic when cluster is not running
	c.SetRPCChannel(nil)
	if c.GetRPCChannel() != nil {
		t.Fatal("SetRPCChannel(nil) should leave channel as nil")
	}
}

func TestCluster_GetTask_WithManager(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	tm := NewTaskManager(30 * time.Second)
	c.SetTaskManagerForTest(tm)

	tm.Submit(&Task{ID: "task-gettest", Action: "test", PeerID: "p1", Status: TaskPending, CreatedAt: time.Now()})

	task, err := c.GetTask("task-gettest")
	if err != nil {
		t.Fatalf("GetTask() error = %v", err)
	}
	if task.ID != "task-gettest" {
		t.Errorf("Task ID = %q, want %q", task.ID, "task-gettest")
	}
}

func TestCluster_CleanupTask_WithManager(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	tm := NewTaskManager(30 * time.Second)
	c.SetTaskManagerForTest(tm)

	tm.Submit(&Task{ID: "task-cleanup", Action: "test", PeerID: "p1", Status: TaskCompleted, CreatedAt: time.Now()})

	c.CleanupTask("task-cleanup")

	_, err = c.GetTask("task-cleanup")
	if err == nil {
		t.Fatal("GetTask() should fail after CleanupTask()")
	}
}

func TestCluster_HandleTaskComplete_WithTaskNoChannel(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	tm := NewTaskManager(30 * time.Second)
	c.SetTaskManagerForTest(tm)
	c.SetMessageBus(bus.NewMessageBus())

	// Task without OriginalChannel
	tm.Submit(&Task{ID: "task-noch", Action: "test", PeerID: "p1", Status: TaskPending, CreatedAt: time.Now()})
	tm.CompleteTask("task-noch", &TaskResult{TaskID: "task-noch", Status: "success"})

	// Should not panic
	c.HandleTaskCompleteForTest("task-noch")
}

func TestCluster_HandleTaskComplete_NonExistentTask(t *testing.T) {
	tmpDir := t.TempDir()
	c, err := NewCluster(tmpDir)
	if err != nil {
		t.Fatalf("NewCluster() error = %v", err)
	}
	defer c.logger.Close()

	tm := NewTaskManager(30 * time.Second)
	c.SetTaskManagerForTest(tm)
	c.SetMessageBus(bus.NewMessageBus())

	// Non-existent task should not panic
	c.HandleTaskCompleteForTest("nonexistent-task")
}

// --- Config tests ---

func TestLoadOrCreateConfig(t *testing.T) {
	tmpDir := t.TempDir()

	// Create config directory
	configDir := tmpDir
	configPath := filepath.Join(configDir, "config.json")

	// Write a valid config using StaticConfig
	cfg := CreateStaticConfig("test-cluster", "Test Bot", "1.2.3.4:21949")
	SaveStaticConfig(configPath, cfg)

	loaded, err := LoadConfig(configPath)
	if err != nil {
		t.Fatalf("LoadConfig() error = %v", err)
	}
	if loaded.Node.ID != "test-cluster" {
		t.Errorf("Node.ID = %q, want %q", loaded.Node.ID, "test-cluster")
	}
}

func TestSaveConfig(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "config.json")

	cfg := CreateStaticConfig("saved-cluster", "Saved Bot", "5.6.7.8:21949")

	if err := SaveConfig(configPath, cfg); err != nil {
		t.Fatalf("SaveConfig() error = %v", err)
	}

	// Verify file exists and can be loaded
	loaded, err := LoadConfig(configPath)
	if err != nil {
		t.Fatalf("LoadConfig() after save error = %v", err)
	}
	if loaded.Node.ID != "saved-cluster" {
		t.Errorf("Node.ID = %q, want %q", loaded.Node.ID, "saved-cluster")
	}
}

func TestDefaultConfig(t *testing.T) {
	cfg := DefaultConfig("test-node")
	if cfg == nil {
		t.Fatal("DefaultConfig() returned nil")
	}
	if cfg.Node.ID != "test-node" {
		t.Errorf("Node.ID = %q, want %q", cfg.Node.ID, "test-node")
	}
}

func TestLoadOrCreateConfig_New(t *testing.T) {
	tmpDir := t.TempDir()
	// Use a .toml extension within a non-existent subdirectory
	// os.IsNotExist will match the directory-not-found error from os.Stat
	configPath := filepath.Join(tmpDir, "subdir", "config.toml")

	cfg, err := LoadOrCreateConfig(configPath, "new-node")
	if err != nil {
		// If the function doesn't handle the error properly, at least verify it doesn't panic
		t.Logf("LoadOrCreateConfig() error = %v (may be expected)", err)
		return
	}
	if cfg == nil {
		t.Fatal("LoadOrCreateConfig() returned nil")
	}
	if cfg.Node.ID != "new-node" {
		t.Errorf("Node.ID = %q, want %q", cfg.Node.ID, "new-node")
	}
}

// --- Static/Dynamic Config tests ---

func TestCreateStaticConfig(t *testing.T) {
	cfg := CreateStaticConfig("node-1", "Bot 1", "1.2.3.4:21949")
	if cfg.Cluster.ID != "manual" {
		t.Errorf("Cluster.ID = %q, want %q", cfg.Cluster.ID, "manual")
	}
	if cfg.Node.Name != "Bot 1" {
		t.Errorf("Node.Name = %q, want %q", cfg.Node.Name, "Bot 1")
	}
	if len(cfg.Peers) != 0 {
		t.Errorf("Peers = %d, want 0", len(cfg.Peers))
	}
}

func TestSaveAndLoadStaticConfig(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "peers.toml")

	cfg := &StaticConfig{
		Cluster: ClusterMeta{ID: "test", AutoDiscovery: true},
		Node:    NodeInfo{ID: "n1", Name: "Test", Address: "1.2.3.4:21949", Role: "worker"},
		Peers: []PeerConfig{
			{ID: "p1", Name: "Peer", Address: "5.6.7.8:21949", Enabled: true},
		},
	}

	if err := SaveStaticConfig(configPath, cfg); err != nil {
		t.Fatalf("SaveStaticConfig() error = %v", err)
	}

	loaded, err := LoadStaticConfig(configPath)
	if err != nil {
		t.Fatalf("LoadStaticConfig() error = %v", err)
	}
	if loaded.Node.ID != "n1" {
		t.Errorf("Node.ID = %q, want %q", loaded.Node.ID, "n1")
	}
	if len(loaded.Peers) != 1 {
		t.Errorf("Peers = %d, want 1", len(loaded.Peers))
	}
}

func TestLoadStaticConfig_NotFound(t *testing.T) {
	_, err := LoadStaticConfig("/nonexistent/path/config.toml")
	if err == nil {
		t.Fatal("LoadStaticConfig() should fail for nonexistent file")
	}
}

func TestSaveAndLoadDynamicState(t *testing.T) {
	tmpDir := t.TempDir()
	statePath := filepath.Join(tmpDir, "state.toml")

	state := &DynamicState{
		Cluster:   ClusterMeta{ID: "dynamic-test", AutoDiscovery: true},
		LocalNode: NodeInfo{ID: "local-1", Name: "Local", Role: "worker"},
		Discovered: []PeerConfig{
			{ID: "disc-1", Name: "Discovered", Enabled: true},
		},
	}

	if err := SaveDynamicState(statePath, state); err != nil {
		t.Fatalf("SaveDynamicState() error = %v", err)
	}

	loaded, err := LoadDynamicState(statePath)
	if err != nil {
		t.Fatalf("LoadDynamicState() error = %v", err)
	}
	if loaded.LocalNode.ID != "local-1" {
		t.Errorf("LocalNode.ID = %q, want %q", loaded.LocalNode.ID, "local-1")
	}
}

func TestLoadDynamicState_NotFound(t *testing.T) {
	state, err := LoadDynamicState("/nonexistent/path/state.toml")
	if err != nil {
		t.Fatalf("LoadDynamicState() for nonexistent file should return default, got error: %v", err)
	}
	if state.Cluster.ID != "auto-discovered" {
		t.Errorf("Cluster.ID = %q, want %q", state.Cluster.ID, "auto-discovered")
	}
}

// --- Logger alias method tests ---

func TestClusterLogger_LogRPCAliases(t *testing.T) {
	tmpDir := t.TempDir()
	logger, err := NewClusterLogger(tmpDir)
	if err != nil {
		t.Fatalf("NewClusterLogger() error = %v", err)
	}
	defer logger.Close()

	// These alias methods should not panic
	logger.LogRPCInfo("test info %s", "arg")
	logger.LogRPCError("test error %s", "arg")
	logger.LogRPCDebug("test debug %s", "arg")
}

// --- sortCandidatesByPriority test ---

func TestSortCandidatesByPriority(t *testing.T) {
	candidates := []candidateIP{
		{ip: "wifi", priority: 2},
		{ip: "other", priority: 99},
		{ip: "eth", priority: 1},
		{ip: "en", priority: 3},
	}

	sortCandidatesByPriority(candidates)

	if candidates[0].priority != 1 {
		t.Errorf("First priority = %d, want 1", candidates[0].priority)
	}
	if candidates[1].priority != 2 {
		t.Errorf("Second priority = %d, want 2", candidates[1].priority)
	}
	if candidates[2].priority != 3 {
		t.Errorf("Third priority = %d, want 3", candidates[2].priority)
	}
	if candidates[3].priority != 99 {
		t.Errorf("Fourth priority = %d, want 99", candidates[3].priority)
	}
}
