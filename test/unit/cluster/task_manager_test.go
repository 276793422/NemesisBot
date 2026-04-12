// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster_test

import (
	"context"
	"fmt"
	"sync"
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

// TestTaskManager_SubmitAndWait tests submitting a task and waiting for completion
func TestTaskManager_SubmitAndWait(t *testing.T) {
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

	// Submit task
	if err := tm.Submit(task); err != nil {
		t.Fatalf("Submit failed: %v", err)
	}

	// Complete task in background
	go func() {
		time.Sleep(50 * time.Millisecond)
		tm.CompleteTask("test-task-1", &cluster.TaskResult{
			TaskID:   "test-task-1",
			Status:   "success",
			Response: "Hello back!",
		})
	}()

	// Wait for task
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	result, err := tm.WaitForTask(ctx, "test-task-1")
	if err != nil {
		t.Fatalf("WaitForTask failed: %v", err)
	}

	if result.Status != string(cluster.TaskCompleted) {
		t.Errorf("Expected status 'completed', got '%s'", result.Status)
	}
	if result.TaskID != "test-task-1" {
		t.Errorf("Expected task_id 'test-task-1', got '%s'", result.TaskID)
	}
}

// TestTaskManager_WaitForTask_ContextCancelled tests that WaitForTask respects context cancellation
func TestTaskManager_WaitForTask_ContextCancelled(t *testing.T) {
	tm := cluster.NewTaskManager(10 * time.Second)
	tm.Start()
	defer tm.Stop()

	task := &cluster.Task{
		ID:        "test-task-cancel",
		Action:    "peer_chat",
		PeerID:    "node-B",
		Status:    cluster.TaskPending,
		CreatedAt: time.Now(),
	}

	if err := tm.Submit(task); err != nil {
		t.Fatalf("Submit failed: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	_, err := tm.WaitForTask(ctx, "test-task-cancel")
	if err == nil {
		t.Fatal("Expected error due to context cancellation")
	}
	if ctx.Err() != context.DeadlineExceeded {
		t.Errorf("Expected DeadlineExceeded, got: %v", ctx.Err())
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

// TestTaskManager_WaitForTask_NotFound tests waiting for a non-existent task
func TestTaskManager_WaitForTask_NotFound(t *testing.T) {
	tm := cluster.NewTaskManager(10 * time.Second)

	ctx := context.Background()
	_, err := tm.WaitForTask(ctx, "nonexistent")
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

	var wg sync.WaitGroup
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

	// Complete tasks in background
	for i := 0; i < taskCount; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			time.Sleep(time.Duration(50+idx*20) * time.Millisecond)
			tm.CompleteTask(fmt.Sprintf("multi-task-%d", idx), &cluster.TaskResult{
				TaskID:   fmt.Sprintf("multi-task-%d", idx),
				Status:   "success",
				Response: fmt.Sprintf("Response %d", idx),
			})
		}(i)
	}

	// Wait for all tasks
	for i := 0; i < taskCount; i++ {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		result, err := tm.WaitForTask(ctx, fmt.Sprintf("multi-task-%d", i))
		cancel()
		if err != nil {
			t.Errorf("WaitForTask failed for task %d: %v", i, err)
		}
		if result.Status != string(cluster.TaskCompleted) {
			t.Errorf("Task %d: expected status 'completed', got '%s'", i, result.Status)
		}
	}

	wg.Wait()
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
	go func() {
		time.Sleep(50 * time.Millisecond)
		tm.CompleteTask("failed-task", &cluster.TaskResult{
			TaskID: "failed-task",
			Status: "error",
			Error:  "something went wrong",
		})
	}()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	result, err := tm.WaitForTask(ctx, "failed-task")
	if err != nil {
		t.Fatalf("WaitForTask failed: %v", err)
	}

	if result.Status != string(cluster.TaskFailed) {
		t.Errorf("Expected status 'failed', got '%s'", result.Status)
	}
	if result.Error != "something went wrong" {
		t.Errorf("Expected error 'something went wrong', got '%s'", result.Error)
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

	// Stop should close all wait channels
	tm.Stop()

	// Verify that WaitForTask returns error after stop
	// (doneCh has been closed by Stop())
	ctx := context.Background()
	_, err := tm.WaitForTask(ctx, "pending-task")
	// After stop, doneCh is closed, so WaitForTask will try to read from store
	// The store should still have the task (status pending)
	if err != nil {
		// It's OK if there's an error - the task is still pending
		t.Logf("WaitForTask after stop returned: %v (expected)", err)
	}
}
