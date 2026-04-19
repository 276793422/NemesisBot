// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// --- Handler Closure Tests (query_task_result / confirm_task_delivery) ---

// TestQueryTaskResultHandler_MissingTaskID tests the query handler with empty task_id
func TestQueryTaskResultHandler_MissingTaskID(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	handler := cluster.buildQueryTaskResultHandler()

	resp, err := handler(map[string]interface{}{})
	if err != nil {
		t.Fatalf("Handler returned error: %v", err)
	}
	if resp["status"] != "error" {
		t.Errorf("Expected status 'error', got %v", resp["status"])
	}
	if resp["error"] != "task_id is required" {
		t.Errorf("Expected 'task_id is required', got %v", resp["error"])
	}
}

// TestQueryTaskResultHandler_NotFound tests querying a non-existent task
func TestQueryTaskResultHandler_NotFound(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	handler := cluster.buildQueryTaskResultHandler()

	resp, err := handler(map[string]interface{}{"task_id": "nonexistent"})
	if err != nil {
		t.Fatalf("Handler returned error: %v", err)
	}
	if resp["status"] != "not_found" {
		t.Errorf("Expected status 'not_found', got %v", resp["status"])
	}
	if resp["task_id"] != "nonexistent" {
		t.Errorf("Expected task_id echoed back, got %v", resp["task_id"])
	}
}

// TestQueryTaskResultHandler_RunningTask tests querying a running task
func TestQueryTaskResultHandler_RunningTask(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	cluster.resultStore.SetRunning("task-running", "node-A")

	handler := cluster.buildQueryTaskResultHandler()

	resp, err := handler(map[string]interface{}{"task_id": "task-running"})
	if err != nil {
		t.Fatalf("Handler returned error: %v", err)
	}
	if resp["status"] != "running" {
		t.Errorf("Expected status 'running', got %v", resp["status"])
	}
}

// TestQueryTaskResultHandler_DoneTask tests querying a done task with full fields
func TestQueryTaskResultHandler_DoneTask(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	cluster.resultStore.SetResult("task-done", "success", "hello world", "", "node-A")

	handler := cluster.buildQueryTaskResultHandler()

	resp, err := handler(map[string]interface{}{"task_id": "task-done"})
	if err != nil {
		t.Fatalf("Handler returned error: %v", err)
	}
	if resp["status"] != "done" {
		t.Errorf("Expected status 'done', got %v", resp["status"])
	}
	if resp["result_status"] != "success" {
		t.Errorf("Expected result_status 'success', got %v", resp["result_status"])
	}
	if resp["response"] != "hello world" {
		t.Errorf("Expected response 'hello world', got %v", resp["response"])
	}
}

// TestQueryTaskResultHandler_DoneWithError tests querying a done task with error result
func TestQueryTaskResultHandler_DoneWithError(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	cluster.resultStore.SetResult("task-err", "error", "", "LLM timeout", "node-B")

	handler := cluster.buildQueryTaskResultHandler()

	resp, err := handler(map[string]interface{}{"task_id": "task-err"})
	if err != nil {
		t.Fatalf("Handler returned error: %v", err)
	}
	if resp["status"] != "done" {
		t.Errorf("Expected status 'done', got %v", resp["status"])
	}
	if resp["result_status"] != "error" {
		t.Errorf("Expected result_status 'error', got %v", resp["result_status"])
	}
	if resp["error"] != "LLM timeout" {
		t.Errorf("Expected error 'LLM timeout', got %v", resp["error"])
	}
}

// TestConfirmTaskDeliveryHandler_MissingTaskID tests the confirm handler with empty task_id
func TestConfirmTaskDeliveryHandler_MissingTaskID(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	handler := cluster.buildConfirmTaskDeliveryHandler()

	resp, err := handler(map[string]interface{}{})
	if err != nil {
		t.Fatalf("Handler returned error: %v", err)
	}
	if resp["status"] != "error" {
		t.Errorf("Expected status 'error', got %v", resp["status"])
	}
}

// TestConfirmTaskDeliveryHandler_Confirms tests a successful confirm
func TestConfirmTaskDeliveryHandler_Confirms(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	cluster.resultStore.SetResult("task-1", "success", "ok", "", "node-A")

	handler := cluster.buildConfirmTaskDeliveryHandler()

	resp, err := handler(map[string]interface{}{"task_id": "task-1"})
	if err != nil {
		t.Fatalf("Handler returned error: %v", err)
	}
	if resp["status"] != "confirmed" {
		t.Errorf("Expected status 'confirmed', got %v", resp["status"])
	}
	if resp["task_id"] != "task-1" {
		t.Errorf("Expected task_id echoed back, got %v", resp["task_id"])
	}

	// Result should be deleted
	if cluster.resultStore.Get("task-1") != nil {
		t.Error("Expected task result to be deleted after confirm")
	}
}

// --- TaskResultStore Edge Cases ---

// TestTaskResultStore_CorruptedIndexRecovery tests loading a corrupted index.json
func TestTaskResultStore_CorruptedIndexRecovery(t *testing.T) {
	tempDir := t.TempDir()
	dataDir := filepath.Join(tempDir, "cluster", "task_results")
	os.MkdirAll(dataDir, 0755)

	// Write a corrupted index.json
	indexPath := filepath.Join(dataDir, "index.json")
	os.WriteFile(indexPath, []byte("this is not valid json{{{"), 0644)

	// Should not fail, should start with empty index
	store, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Should not fail with corrupted index: %v", err)
	}

	// Store should work normally (empty index)
	if store.Get("anything") != nil {
		t.Error("Expected nil on store with recovered empty index")
	}

	// Should be able to set and get new results after recovery
	store.SetResult("task-1", "success", "ok", "", "node-A")
	entry := store.Get("task-1")
	if entry == nil || entry.Response != "ok" {
		t.Error("Expected to work normally after corrupted index recovery")
	}
}

// TestTaskResultStore_OverwriteExistingResult tests SetResult on an already-done task
func TestTaskResultStore_OverwriteExistingResult(t *testing.T) {
	tempDir := t.TempDir()
	store, _ := NewTaskResultStore(tempDir)

	store.SetResult("task-1", "success", "first response", "", "node-A")
	store.SetResult("task-1", "error", "", "overwritten error", "node-B")

	entry := store.Get("task-1")
	if entry == nil {
		t.Fatal("Expected entry")
	}
	if entry.ResultStatus != "error" {
		t.Errorf("Expected result_status 'error' (overwritten), got %s", entry.ResultStatus)
	}
	if entry.Error != "overwritten error" {
		t.Errorf("Expected error 'overwritten error', got %s", entry.Error)
	}
	if entry.SourceNode != "node-B" {
		t.Errorf("Expected source_node 'node-B', got %s", entry.SourceNode)
	}
}

// TestTaskResultStore_DataFileContent verifies data file has all fields
func TestTaskResultStore_DataFileContent(t *testing.T) {
	tempDir := t.TempDir()
	store, _ := NewTaskResultStore(tempDir)

	store.SetResult("task-1", "success", "test response", "", "node-A")

	// Read data file directly
	dataFile := filepath.Join(tempDir, "cluster", "task_results", "task-1.json")
	data, err := os.ReadFile(dataFile)
	if err != nil {
		t.Fatalf("Failed to read data file: %v", err)
	}

	var entry TaskResultEntry
	json.Unmarshal(data, &entry)

	if entry.TaskID != "task-1" {
		t.Errorf("Data file task_id mismatch: %s", entry.TaskID)
	}
	if entry.Status != "done" {
		t.Errorf("Data file status mismatch: %s", entry.Status)
	}
	if entry.ResultStatus != "success" {
		t.Errorf("Data file result_status mismatch: %s", entry.ResultStatus)
	}
	if entry.Response != "test response" {
		t.Errorf("Data file response mismatch: %s", entry.Response)
	}
	if entry.SourceNode != "node-A" {
		t.Errorf("Data file source_node mismatch: %s", entry.SourceNode)
	}
	if entry.CreatedAt.IsZero() {
		t.Error("Data file created_at should not be zero")
	}
}

// --- Recovery Loop: Mock RPC via callWithContextFn ---

// TestPollStalePendingTasks_DoneResponse tests recovery when B returns "done"
func TestPollStalePendingTasks_DoneResponse(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	tm := NewTaskManager(30 * time.Second)
	var completedIDs []string
	tm.SetOnComplete(func(taskID string) {
		completedIDs = append(completedIDs, taskID)
	})
	cluster.taskManager = tm

	// Mock CallWithContext to simulate B responding with "done"
	cluster.callWithContextFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		if action == "query_task_result" {
			return json.Marshal(map[string]interface{}{
				"status":        "done",
				"task_id":       "task-1",
				"result_status": "success",
				"response":      "recovered response",
				"error":         "",
			})
		}
		if action == "confirm_task_delivery" {
			return json.Marshal(map[string]interface{}{"status": "confirmed"})
		}
		return nil, nil
	}

	tm.Submit(&Task{
		ID:        "task-1",
		Status:    TaskPending,
		PeerID:    "peer-B",
		CreatedAt: time.Now().Add(-5 * time.Minute),
	})

	cluster.pollStalePendingTasks()

	// Task should be completed
	task, _ := tm.GetTask("task-1")
	if task.Status != TaskCompleted {
		t.Errorf("Expected task completed, got %s", task.Status)
	}
	if task.Response != "recovered response" {
		t.Errorf("Expected response 'recovered response', got %s", task.Response)
	}
	if len(completedIDs) != 1 || completedIDs[0] != "task-1" {
		t.Errorf("Expected completion callback for task-1, got %v", completedIDs)
	}
}

// TestPollStalePendingTasks_RunningResponse tests recovery when B returns "running"
func TestPollStalePendingTasks_RunningResponse(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	tm := NewTaskManager(30 * time.Second)
	cluster.taskManager = tm

	cluster.callWithContextFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		return json.Marshal(map[string]interface{}{
			"status":  "running",
			"task_id": "task-1",
		})
	}

	tm.Submit(&Task{
		ID:        "task-1",
		Status:    TaskPending,
		PeerID:    "peer-B",
		CreatedAt: time.Now().Add(-5 * time.Minute),
	})

	cluster.pollStalePendingTasks()

	task, _ := tm.GetTask("task-1")
	if task.Status != TaskPending {
		t.Errorf("Expected task still pending when B says running, got %s", task.Status)
	}
}

// TestPollStalePendingTasks_NotFoundResponse tests recovery when B returns "not_found"
func TestPollStalePendingTasks_NotFoundResponse(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	tm := NewTaskManager(30 * time.Second)
	cluster.taskManager = tm

	cluster.callWithContextFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		return json.Marshal(map[string]interface{}{
			"status":  "not_found",
			"task_id": "task-1",
		})
	}

	tm.Submit(&Task{
		ID:        "task-1",
		Status:    TaskPending,
		PeerID:    "peer-B",
		CreatedAt: time.Now().Add(-5 * time.Minute),
	})

	cluster.pollStalePendingTasks()

	task, _ := tm.GetTask("task-1")
	if task.Status != TaskFailed {
		t.Errorf("Expected task failed when B says not_found, got %s", task.Status)
	}
	if task.Error != "remote task not found" {
		t.Errorf("Expected error 'remote task not found', got %s", task.Error)
	}
}

// TestPollStalePendingTasks_InvalidJSONResponse tests recovery when B returns invalid JSON
func TestPollStalePendingTasks_InvalidJSONResponse(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	tm := NewTaskManager(30 * time.Second)
	cluster.taskManager = tm

	cluster.callWithContextFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		return []byte("invalid json{{{"), nil
	}

	tm.Submit(&Task{
		ID:        "task-1",
		Status:    TaskPending,
		PeerID:    "peer-B",
		CreatedAt: time.Now().Add(-5 * time.Minute),
	})

	cluster.pollStalePendingTasks()

	// Should still be pending (JSON parse failed, skip)
	task, _ := tm.GetTask("task-1")
	if task.Status != TaskPending {
		t.Errorf("Expected task still pending with invalid JSON response, got %s", task.Status)
	}
}

// TestPollStalePendingTasks_DoneWithErrorResponse tests recovery of a failed task
func TestPollStalePendingTasks_DoneWithErrorResponse(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	tm := NewTaskManager(30 * time.Second)
	cluster.taskManager = tm

	cluster.callWithContextFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		if action == "query_task_result" {
			return json.Marshal(map[string]interface{}{
				"status":        "done",
				"task_id":       "task-1",
				"result_status": "error",
				"response":      "",
				"error":         "LLM processing timeout",
			})
		}
		return json.Marshal(map[string]interface{}{"status": "confirmed"})
	}

	tm.Submit(&Task{
		ID:        "task-1",
		Status:    TaskPending,
		PeerID:    "peer-B",
		CreatedAt: time.Now().Add(-5 * time.Minute),
	})

	cluster.pollStalePendingTasks()

	task, _ := tm.GetTask("task-1")
	if task.Status != TaskFailed {
		t.Errorf("Expected task failed, got %s", task.Status)
	}
	if task.Error != "LLM processing timeout" {
		t.Errorf("Expected error 'LLM processing timeout', got %s", task.Error)
	}
}

// TestConfirmDelivery tests the confirmDelivery method
func TestConfirmDelivery(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	var receivedAction string
	var receivedPayload map[string]interface{}
	cluster.callWithContextFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		receivedAction = action
		receivedPayload = payload
		return json.Marshal(map[string]interface{}{"status": "confirmed"})
	}

	cluster.confirmDelivery("peer-B", "task-1")

	if receivedAction != "confirm_task_delivery" {
		t.Errorf("Expected action 'confirm_task_delivery', got %s", receivedAction)
	}
	if receivedPayload["task_id"] != "task-1" {
		t.Errorf("Expected task_id 'task-1', got %v", receivedPayload["task_id"])
	}
}

// TestPollStalePendingTasks_MultipleTasks tests recovery with multiple pending tasks
func TestPollStalePendingTasks_MultipleTasks(t *testing.T) {
	tempDir := t.TempDir()
	cluster, err := NewCluster(tempDir)
	if err != nil {
		t.Fatalf("Failed to create cluster: %v", err)
	}
	t.Cleanup(func() {
		if cluster.logger != nil {
			cluster.logger.Close()
		}
	})

	tm := NewTaskManager(30 * time.Second)
	cluster.taskManager = tm

	// Mock: task-1 → done, task-2 → running, task-3 → not_found
	cluster.callWithContextFn = func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
		taskID := payload["task_id"].(string)
		switch taskID {
		case "task-1":
			return json.Marshal(map[string]interface{}{
				"status":        "done",
				"result_status": "success",
				"response":      "response-1",
			})
		case "task-2":
			return json.Marshal(map[string]interface{}{"status": "running"})
		case "task-3":
			return json.Marshal(map[string]interface{}{"status": "not_found"})
		}
		return nil, nil
	}

	tm.Submit(&Task{ID: "task-1", Status: TaskPending, PeerID: "peer-B", CreatedAt: time.Now().Add(-5 * time.Minute)})
	tm.Submit(&Task{ID: "task-2", Status: TaskPending, PeerID: "peer-B", CreatedAt: time.Now().Add(-5 * time.Minute)})
	tm.Submit(&Task{ID: "task-3", Status: TaskPending, PeerID: "peer-B", CreatedAt: time.Now().Add(-5 * time.Minute)})
	// Also add a too-new task that should be skipped
	tm.Submit(&Task{ID: "task-4", Status: TaskPending, PeerID: "peer-B", CreatedAt: time.Now()})

	cluster.pollStalePendingTasks()

	// task-1: completed
	t1, _ := tm.GetTask("task-1")
	if t1.Status != TaskCompleted {
		t.Errorf("task-1 should be completed, got %s", t1.Status)
	}

	// task-2: still pending (B says running)
	t2, _ := tm.GetTask("task-2")
	if t2.Status != TaskPending {
		t.Errorf("task-2 should still be pending, got %s", t2.Status)
	}

	// task-3: failed (not_found)
	t3, _ := tm.GetTask("task-3")
	if t3.Status != TaskFailed {
		t.Errorf("task-3 should be failed, got %s", t3.Status)
	}

	// task-4: still pending (too new, skipped)
	t4, _ := tm.GetTask("task-4")
	if t4.Status != TaskPending {
		t.Errorf("task-4 should still be pending (too new), got %s", t4.Status)
	}
}
