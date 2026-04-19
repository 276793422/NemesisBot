// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"testing"
)

func TestNewTaskResultStore(t *testing.T) {
	tempDir := t.TempDir()

	store, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create TaskResultStore: %v", err)
	}
	if store == nil {
		t.Fatal("Expected non-nil store")
	}

	// Check directory was created
	dataDir := filepath.Join(tempDir, "cluster", "task_results")
	info, err := os.Stat(dataDir)
	if err != nil {
		t.Fatalf("task_results directory should exist: %v", err)
	}
	if !info.IsDir() {
		t.Error("Expected task_results to be a directory")
	}
}

func TestTaskResultStore_SetRunningAndGet(t *testing.T) {
	tempDir := t.TempDir()
	store, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	// Get non-existent task should return nil
	entry := store.Get("non-existent")
	if entry != nil {
		t.Error("Expected nil for non-existent task")
	}

	// Set running
	store.SetRunning("task-1", "node-A")

	// Get should return running entry
	entry = store.Get("task-1")
	if entry == nil {
		t.Fatal("Expected entry for running task")
	}
	if entry.Status != "running" {
		t.Errorf("Expected status 'running', got %s", entry.Status)
	}
	if entry.TaskID != "task-1" {
		t.Errorf("Expected task_id 'task-1', got %s", entry.TaskID)
	}
}

func TestTaskResultStore_SetResultAndGet(t *testing.T) {
	tempDir := t.TempDir()
	store, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	// Set running first
	store.SetRunning("task-1", "node-A")

	// Set result
	err = store.SetResult("task-1", "success", "hello world", "", "node-A")
	if err != nil {
		t.Fatalf("SetResult failed: %v", err)
	}

	// Get should return done entry
	entry := store.Get("task-1")
	if entry == nil {
		t.Fatal("Expected entry for done task")
	}
	if entry.Status != "done" {
		t.Errorf("Expected status 'done', got %s", entry.Status)
	}
	if entry.ResultStatus != "success" {
		t.Errorf("Expected result_status 'success', got %s", entry.ResultStatus)
	}
	if entry.Response != "hello world" {
		t.Errorf("Expected response 'hello world', got %s", entry.Response)
	}
	if entry.SourceNode != "node-A" {
		t.Errorf("Expected source_node 'node-A', got %s", entry.SourceNode)
	}
}

func TestTaskResultStore_SetResultWithError(t *testing.T) {
	tempDir := t.TempDir()
	store, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	err = store.SetResult("task-2", "error", "", "something failed", "node-B")
	if err != nil {
		t.Fatalf("SetResult failed: %v", err)
	}

	entry := store.Get("task-2")
	if entry == nil {
		t.Fatal("Expected entry")
	}
	if entry.ResultStatus != "error" {
		t.Errorf("Expected result_status 'error', got %s", entry.ResultStatus)
	}
	if entry.Error != "something failed" {
		t.Errorf("Expected error 'something failed', got %s", entry.Error)
	}
}

func TestTaskResultStore_Delete(t *testing.T) {
	tempDir := t.TempDir()
	store, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	store.SetRunning("task-1", "node-A")
	store.SetResult("task-1", "success", "response", "", "node-A")

	// Delete
	err = store.Delete("task-1")
	if err != nil {
		t.Fatalf("Delete failed: %v", err)
	}

	// Get should return nil after delete
	entry := store.Get("task-1")
	if entry != nil {
		t.Error("Expected nil after delete")
	}
}

func TestTaskResultStore_DeleteNonExistent(t *testing.T) {
	tempDir := t.TempDir()
	store, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	// Delete non-existent should not error
	err = store.Delete("non-existent")
	if err != nil {
		t.Errorf("Delete non-existent should not error: %v", err)
	}
}

func TestTaskResultStore_DeleteRunning(t *testing.T) {
	tempDir := t.TempDir()
	store, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	store.SetRunning("task-1", "node-A")

	// Delete should clear running status
	err = store.Delete("task-1")
	if err != nil {
		t.Fatalf("Delete failed: %v", err)
	}

	entry := store.Get("task-1")
	if entry != nil {
		t.Error("Expected nil after deleting running task")
	}
}

func TestTaskResultStore_DiskPersistence(t *testing.T) {
	tempDir := t.TempDir()

	// Create store and set result
	store1, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store1: %v", err)
	}

	err = store1.SetResult("task-1", "success", "response data", "", "node-A")
	if err != nil {
		t.Fatalf("SetResult failed: %v", err)
	}

	// Verify data file exists
	dataFile := filepath.Join(tempDir, "cluster", "task_results", "task-1.json")
	if _, err := os.Stat(dataFile); os.IsNotExist(err) {
		t.Error("Data file should exist on disk")
	}

	// Verify index file exists
	indexFile := filepath.Join(tempDir, "cluster", "task_results", "index.json")
	if _, err := os.Stat(indexFile); os.IsNotExist(err) {
		t.Error("Index file should exist on disk")
	}
}

func TestTaskResultStore_RestoreFromDisk(t *testing.T) {
	tempDir := t.TempDir()

	// Create store, set result, then close
	store1, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store1: %v", err)
	}
	store1.SetResult("task-1", "success", "hello", "", "node-A")
	store1.SetResult("task-2", "error", "", "timeout", "node-B")

	// Create new store from same directory (simulates restart)
	store2, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store2: %v", err)
	}

	// Should be able to get task-1 from disk
	entry := store2.Get("task-1")
	if entry == nil {
		t.Fatal("Expected entry after restart")
	}
	if entry.Status != "done" {
		t.Errorf("Expected status 'done', got %s", entry.Status)
	}
	if entry.Response != "hello" {
		t.Errorf("Expected response 'hello', got %s", entry.Response)
	}

	// Should be able to get task-2 from disk
	entry2 := store2.Get("task-2")
	if entry2 == nil {
		t.Fatal("Expected entry2 after restart")
	}
	if entry2.ResultStatus != "error" {
		t.Errorf("Expected result_status 'error', got %s", entry2.ResultStatus)
	}

	// Running state is not persisted — new store should not have running tasks
	store2.SetRunning("task-3", "node-C")
	// Simulate restart by creating another store
	store3, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store3: %v", err)
	}
	entry3 := store3.Get("task-3")
	if entry3 != nil {
		t.Error("Running state should not persist across restarts")
	}
}

func TestTaskResultStore_SetResultClearsRunning(t *testing.T) {
	tempDir := t.TempDir()
	store, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	store.SetRunning("task-1", "node-A")

	// SetResult should clear running and move to done
	err = store.SetResult("task-1", "success", "ok", "", "node-A")
	if err != nil {
		t.Fatalf("SetResult failed: %v", err)
	}

	entry := store.Get("task-1")
	if entry == nil {
		t.Fatal("Expected entry")
	}
	if entry.Status != "done" {
		t.Errorf("Expected status 'done', got %s", entry.Status)
	}
}

func TestTaskResultStore_AtomicWrite(t *testing.T) {
	tempDir := t.TempDir()
	store, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	err = store.SetResult("task-1", "success", "response", "", "node-A")
	if err != nil {
		t.Fatalf("SetResult failed: %v", err)
	}

	// Verify data file content is valid JSON
	dataFile := filepath.Join(tempDir, "cluster", "task_results", "task-1.json")
	data, err := os.ReadFile(dataFile)
	if err != nil {
		t.Fatalf("Failed to read data file: %v", err)
	}

	var entry TaskResultEntry
	if err := json.Unmarshal(data, &entry); err != nil {
		t.Fatalf("Data file should contain valid JSON: %v", err)
	}
	if entry.TaskID != "task-1" {
		t.Errorf("Expected task_id 'task-1', got %s", entry.TaskID)
	}

	// No tmp files should remain
	tmpFile := dataFile + ".tmp"
	if _, err := os.Stat(tmpFile); !os.IsNotExist(err) {
		t.Error("Temp file should not remain after atomic write")
	}
}

func TestTaskResultStore_MultipleTasksIndex(t *testing.T) {
	tempDir := t.TempDir()
	store, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	// Set multiple results
	for i := 0; i < 5; i++ {
		taskID := fmt.Sprintf("task-%d", i)
		store.SetResult(taskID, "success", fmt.Sprintf("response-%d", i), "", "node-A")
	}

	// All should be retrievable
	for i := 0; i < 5; i++ {
		taskID := fmt.Sprintf("task-%d", i)
		entry := store.Get(taskID)
		if entry == nil {
			t.Errorf("Expected entry for %s", taskID)
		}
		if entry.Response != fmt.Sprintf("response-%d", i) {
			t.Errorf("Expected response-%d, got %s", i, entry.Response)
		}
	}

	// Delete one and verify index is updated
	store.Delete("task-2")
	if store.Get("task-2") != nil {
		t.Error("task-2 should be deleted")
	}
	// Others should still be there
	if store.Get("task-1") == nil {
		t.Error("task-1 should still exist")
	}
	if store.Get("task-3") == nil {
		t.Error("task-3 should still exist")
	}
}

func TestTaskResultStore_IndexRestoredOnNewStore(t *testing.T) {
	tempDir := t.TempDir()

	// Write some results
	store1, _ := NewTaskResultStore(tempDir)
	store1.SetResult("task-1", "success", "resp1", "", "node-A")
	store1.SetResult("task-2", "error", "", "err2", "node-B")

	// Delete one via store1
	store1.Delete("task-1")

	// New store should see only task-2
	store2, _ := NewTaskResultStore(tempDir)
	if store2.Get("task-1") != nil {
		t.Error("task-1 should not exist after delete and restart")
	}
	entry := store2.Get("task-2")
	if entry == nil {
		t.Fatal("task-2 should exist after restart")
	}
	if entry.ResultStatus != "error" {
		t.Errorf("Expected result_status 'error', got %s", entry.ResultStatus)
	}
}

func TestTaskResultStore_EmptyIndexLoad(t *testing.T) {
	tempDir := t.TempDir()

	// Create store with no data
	store, err := NewTaskResultStore(tempDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	// Get on empty store
	if store.Get("anything") != nil {
		t.Error("Expected nil on empty store")
	}
}
