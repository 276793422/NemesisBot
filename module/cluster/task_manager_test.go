// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"testing"
	"time"
)

func TestTaskManager_ListPendingTasks(t *testing.T) {
	tm := NewTaskManager(30 * time.Second)
	tm.Start()
	defer tm.Stop()

	// Submit some tasks
	tm.Submit(&Task{ID: "task-1", Status: TaskPending, CreatedAt: time.Now()})
	tm.Submit(&Task{ID: "task-2", Status: TaskPending, CreatedAt: time.Now()})
	tm.Submit(&Task{ID: "task-3", Status: TaskPending, CreatedAt: time.Now()})

	// Complete one
	tm.CompleteTask("task-2", &TaskResult{TaskID: "task-2", Status: "success", Response: "ok"})

	// List pending
	pending, err := tm.ListPendingTasks()
	if err != nil {
		t.Fatalf("ListPendingTasks failed: %v", err)
	}

	if len(pending) != 2 {
		t.Errorf("Expected 2 pending tasks, got %d", len(pending))
	}

	// Verify the pending tasks are task-1 and task-3
	pendingIDs := make(map[string]bool)
	for _, task := range pending {
		pendingIDs[task.ID] = true
	}
	if !pendingIDs["task-1"] || !pendingIDs["task-3"] {
		t.Errorf("Expected task-1 and task-3 to be pending, got %v", pendingIDs)
	}
}

func TestTaskManager_ListPendingTasks_Empty(t *testing.T) {
	tm := NewTaskManager(30 * time.Second)

	pending, err := tm.ListPendingTasks()
	if err != nil {
		t.Fatalf("ListPendingTasks failed: %v", err)
	}
	if len(pending) != 0 {
		t.Errorf("Expected 0 pending tasks, got %d", len(pending))
	}
}

func TestTaskManager_CleanupCompleted_PendingTimeout(t *testing.T) {
	// Use a very short cleanup interval so the test doesn't wait long
	tm := NewTaskManager(10 * time.Millisecond)
	tm.Start()
	defer tm.Stop()

	// Create a task with CreatedAt 25 hours ago
	oldTask := &Task{
		ID:        "old-task",
		Status:    TaskPending,
		CreatedAt: time.Now().Add(-25 * time.Hour),
	}
	tm.Submit(oldTask)

	// Create a recent task that should NOT be timed out
	recentTask := &Task{
		ID:        "recent-task",
		Status:    TaskPending,
		CreatedAt: time.Now(),
	}
	tm.Submit(recentTask)

	// Wait for cleanup to run
	time.Sleep(100 * time.Millisecond)

	// Old task should be marked as failed
	oldTaskResult, err := tm.GetTask("old-task")
	if err != nil {
		t.Fatalf("GetTask failed: %v", err)
	}
	if oldTaskResult.Status != TaskFailed {
		t.Errorf("Expected old task to be failed, got %s", oldTaskResult.Status)
	}
	if oldTaskResult.Error == "" {
		t.Error("Expected error message for timed out task")
	}

	// Recent task should still be pending
	recentTaskResult, err := tm.GetTask("recent-task")
	if err != nil {
		t.Fatalf("GetTask failed: %v", err)
	}
	if recentTaskResult.Status != TaskPending {
		t.Errorf("Expected recent task to still be pending, got %s", recentTaskResult.Status)
	}
}

func TestTaskManager_CleanupCompleted_PendingNotYetTimedOut(t *testing.T) {
	tm := NewTaskManager(10 * time.Millisecond)
	tm.Start()
	defer tm.Stop()

	// Create a task that's only 1 hour old (well under 24h timeout)
	task := &Task{
		ID:        "task-1",
		Status:    TaskPending,
		CreatedAt: time.Now().Add(-1 * time.Hour),
	}
	tm.Submit(task)

	time.Sleep(100 * time.Millisecond)

	result, err := tm.GetTask("task-1")
	if err != nil {
		t.Fatalf("GetTask failed: %v", err)
	}
	if result.Status != TaskPending {
		t.Errorf("Expected task to still be pending, got %s", result.Status)
	}
}
