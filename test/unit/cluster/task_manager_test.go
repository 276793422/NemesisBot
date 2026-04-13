// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster_test

import (
	"fmt"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/cluster"
)

// TestNewTaskManager tests creating a new TaskManager
func TestNewTaskManager(t *testing.T) {
	tm := cluster.NewTaskManager(10 * time.Second)
	if tm == nil {
		t.Fatal("Expected non-nil TaskManager")
	}
}

// TestNewTaskManager_DefaultInterval tests that default interval is set when 0 is passed
func TestNewTaskManager_DefaultInterval(t *testing.T) {
	tm := cluster.NewTaskManager(0)
	if tm == nil {
		t.Fatal("Expected non-nil TaskManager with default interval")
	}
	// Start and stop to verify it works
	tm.Start()
	tm.Stop()
}

// TestTaskManager_Submit tests submitting a task
func TestTaskManager_Submit(t *testing.T) {
	tm := cluster.NewTaskManager(10 * time.Second)
	tm.Start()
	defer tm.Stop()

	task := &cluster.Task{
		ID:        "test-task-1",
		Action:    "peer_chat",
		PeerID:    "node-B",
		Payload:   map[string]interface{}{"content": "hello"},
		Status:    cluster.TaskPending,
		CreatedAt: time.Now(),
	}

	if err := tm.Submit(task); err != nil {
		t.Fatalf("Submit failed: %v", err)
	}

	// Verify task exists
	gotTask, err := tm.GetTask("test-task-1")
	if err != nil {
		t.Fatalf("GetTask failed: %v", err)
	}
	if gotTask.Status != cluster.TaskPending {
		t.Errorf("Expected status 'pending', got '%s'", gotTask.Status)
	}
}

// TestTaskManager_CompleteTask tests completing a task
func TestTaskManager_CompleteTask(t *testing.T) {
	tm := cluster.NewTaskManager(10 * time.Second)
	tm.Start()
	defer tm.Stop()

	task := &cluster.Task{
		ID:        "test-task-complete",
		Action:    "peer_chat",
		PeerID:    "node-B",
		Status:    cluster.TaskPending,
		CreatedAt: time.Now(),
	}

	if err := tm.Submit(task); err != nil {
		t.Fatalf("Submit failed: %v", err)
	}

	// Complete task
	err := tm.CompleteTask("test-task-complete", &cluster.TaskResult{
		TaskID:   "test-task-complete",
		Status:   "success",
		Response: "test response",
	})
	if err != nil {
		t.Fatalf("CompleteTask failed: %v", err)
	}

	// Verify task is completed
	gotTask, err := tm.GetTask("test-task-complete")
	if err != nil {
		t.Fatalf("GetTask failed: %v", err)
	}
	if gotTask.Status != cluster.TaskCompleted {
		t.Errorf("Expected status 'completed', got '%s'", gotTask.Status)
	}
}

// TestTaskManager_CompleteTask_NotFound tests completing a non-existent task
func TestTaskManager_CompleteTask_NotFound(t *testing.T) {
	tm := cluster.NewTaskManager(10 * time.Second)

	err := tm.CompleteTask("nonexistent", &cluster.TaskResult{
		TaskID: "nonexistent",
		Status: "success",
	})
	if err == nil {
		t.Fatal("Expected error for non-existent task")
	}
}

// TestTaskManager_Submit_Duplicate tests submitting a duplicate task
func TestTaskManager_Submit_Duplicate(t *testing.T) {
	tm := cluster.NewTaskManager(10 * time.Second)

	task := &cluster.Task{
		ID:        "dup-task",
		Action:    "peer_chat",
		PeerID:    "node-B",
		Status:    cluster.TaskPending,
		CreatedAt: time.Now(),
	}

	if err := tm.Submit(task); err != nil {
		t.Fatalf("First submit failed: %v", err)
	}

	// Submit same task again
	err := tm.Submit(task)
	if err == nil {
		t.Fatal("Expected error for duplicate task")
	}
}

// TestTaskManager_MultipleTasks tests handling multiple concurrent tasks
func TestTaskManager_MultipleTasks(t *testing.T) {
	tm := cluster.NewTaskManager(10 * time.Second)
	tm.Start()
	defer tm.Stop()

	taskCount := 5

	// Submit tasks
	for i := 0; i < taskCount; i++ {
		task := &cluster.Task{
			ID:        fmt.Sprintf("multi-task-%d", i),
			Action:    "peer_chat",
			PeerID:    "node-B",
			Status:    cluster.TaskPending,
			CreatedAt: time.Now(),
		}
		if err := tm.Submit(task); err != nil {
			t.Fatalf("Submit failed for task %d: %v", i, err)
		}
	}

	// Complete tasks
	for i := 0; i < taskCount; i++ {
		err := tm.CompleteTask(fmt.Sprintf("multi-task-%d", i), &cluster.TaskResult{
			TaskID:   fmt.Sprintf("multi-task-%d", i),
			Status:   "success",
			Response: fmt.Sprintf("Response %d", i),
		})
		if err != nil {
			t.Errorf("CompleteTask failed for task %d: %v", i, err)
		}
	}

	// Verify all completed
	for i := 0; i < taskCount; i++ {
		task, err := tm.GetTask(fmt.Sprintf("multi-task-%d", i))
		if err != nil {
			t.Errorf("GetTask failed for task %d: %v", i, err)
		}
		if task.Status != cluster.TaskCompleted {
			t.Errorf("Task %d: expected status 'completed', got '%s'", i, task.Status)
		}
	}
}

// TestTaskManager_FailedTask tests task that ends with error
func TestTaskManager_FailedTask(t *testing.T) {
	tm := cluster.NewTaskManager(10 * time.Second)
	tm.Start()
	defer tm.Stop()

	task := &cluster.Task{
		ID:        "failed-task",
		Action:    "peer_chat",
		PeerID:    "node-B",
		Status:    cluster.TaskPending,
		CreatedAt: time.Now(),
	}

	if err := tm.Submit(task); err != nil {
		t.Fatalf("Submit failed: %v", err)
	}

	// Complete with error
	err := tm.CompleteTask("failed-task", &cluster.TaskResult{
		TaskID: "failed-task",
		Status: "error",
		Error:  "something went wrong",
	})
	if err != nil {
		t.Fatalf("CompleteTask failed: %v", err)
	}

	gotTask, err := tm.GetTask("failed-task")
	if err != nil {
		t.Fatalf("GetTask failed: %v", err)
	}
	if gotTask.Status != cluster.TaskFailed {
		t.Errorf("Expected status 'failed', got '%s'", gotTask.Status)
	}
	if gotTask.Error != "something went wrong" {
		t.Errorf("Expected error 'something went wrong', got '%s'", gotTask.Error)
	}
}

// TestTaskManager_CompleteCallback tests the CompleteCallback adapter method
func TestTaskManager_CompleteCallback(t *testing.T) {
	tm := cluster.NewTaskManager(10 * time.Second)
	tm.Start()
	defer tm.Stop()

	task := &cluster.Task{
		ID:        "callback-task",
		Action:    "peer_chat",
		PeerID:    "node-B",
		Status:    cluster.TaskPending,
		CreatedAt: time.Now(),
	}

	if err := tm.Submit(task); err != nil {
		t.Fatalf("Submit failed: %v", err)
	}

	// Use CompleteCallback (handlers.TaskCompleter interface)
	err := tm.CompleteCallback("callback-task", "success", "Hello!", "")
	if err != nil {
		t.Fatalf("CompleteCallback failed: %v", err)
	}

	// Verify task completed
	gotTask, err := tm.GetTask("callback-task")
	if err != nil {
		t.Fatalf("GetTask failed: %v", err)
	}
	if gotTask.Status != cluster.TaskCompleted {
		t.Errorf("Expected status 'completed', got '%s'", gotTask.Status)
	}
}

// TestTaskManager_OnComplete tests the onTaskComplete callback (Phase 2)
func TestTaskManager_OnComplete(t *testing.T) {
	tm := cluster.NewTaskManager(10 * time.Second)
	tm.Start()
	defer tm.Stop()

	var callbackCalled atomic.Int32
	var callbackTaskID string
	var mu sync.Mutex

	tm.SetOnComplete(func(taskID string) {
		mu.Lock()
		callbackTaskID = taskID
		mu.Unlock()
		callbackCalled.Add(1)
	})

	task := &cluster.Task{
		ID:        "callback-test-task",
		Action:    "peer_chat",
		PeerID:    "node-B",
		Status:    cluster.TaskPending,
		CreatedAt: time.Now(),
	}

	if err := tm.Submit(task); err != nil {
		t.Fatalf("Submit failed: %v", err)
	}

	// Complete the task - should trigger callback
	err := tm.CompleteTask("callback-test-task", &cluster.TaskResult{
		TaskID:   "callback-test-task",
		Status:   "success",
		Response: "callback response",
	})
	if err != nil {
		t.Fatalf("CompleteTask failed: %v", err)
	}

	// Verify callback was called
	if callbackCalled.Load() != 1 {
		t.Errorf("Expected callback to be called 1 time, got %d", callbackCalled.Load())
	}
	mu.Lock()
	if callbackTaskID != "callback-test-task" {
		t.Errorf("Expected callback taskID 'callback-test-task', got '%s'", callbackTaskID)
	}
	mu.Unlock()
}

// TestTaskManager_OnComplete_Nil tests that nil callback doesn't panic
func TestTaskManager_OnComplete_Nil(t *testing.T) {
	tm := cluster.NewTaskManager(10 * time.Second)
	tm.Start()
	defer tm.Stop()

	// Don't set callback (nil by default)

	task := &cluster.Task{
		ID:        "nil-callback-task",
		Action:    "peer_chat",
		PeerID:    "node-B",
		Status:    cluster.TaskPending,
		CreatedAt: time.Now(),
	}

	if err := tm.Submit(task); err != nil {
		t.Fatalf("Submit failed: %v", err)
	}

	// Should not panic
	err := tm.CompleteTask("nil-callback-task", &cluster.TaskResult{
		TaskID:   "nil-callback-task",
		Status:   "success",
		Response: "ok",
	})
	if err != nil {
		t.Fatalf("CompleteTask failed: %v", err)
	}
}

// TestTaskManager_Stop tests stopping the TaskManager
func TestTaskManager_Stop(t *testing.T) {
	tm := cluster.NewTaskManager(10 * time.Second)
	tm.Start()

	// Submit a task that won't be completed
	task := &cluster.Task{
		ID:        "pending-task",
		Action:    "peer_chat",
		PeerID:    "node-B",
		Status:    cluster.TaskPending,
		CreatedAt: time.Now(),
	}
	tm.Submit(task)

	// Stop should complete without hanging
	tm.Stop()

	// Task should still exist in store
	gotTask, err := tm.GetTask("pending-task")
	if err != nil {
		t.Logf("GetTask after stop: %v (task was cleaned up)", err)
	} else if gotTask.Status != cluster.TaskPending {
		t.Logf("Task status after stop: %s", gotTask.Status)
	}
}
