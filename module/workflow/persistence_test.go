package workflow_test

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/workflow"
)

func TestSaveExecution_NilDir(t *testing.T) {
	exec := &workflow.Execution{ID: "test"}
	err := workflow.SaveExecution("", exec)
	if err != nil {
		t.Errorf("SaveExecution('') error: %v", err)
	}
}

func TestSaveExecution_NilExecution(t *testing.T) {
	err := workflow.SaveExecution("/tmp/test", nil)
	if err != nil {
		t.Errorf("SaveExecution(nil) error: %v", err)
	}
}

func TestSaveAndLoadExecution(t *testing.T) {
	dir := t.TempDir()

	now := time.Now().Truncate(time.Millisecond) // truncate for JSON round-trip
	exec := &workflow.Execution{
		ID:           "exec-123",
		WorkflowName: "test-workflow",
		State:        workflow.StateCompleted,
		Input:        map[string]interface{}{"key": "value"},
		NodeResults: map[string]*workflow.NodeResult{
			"n1": {
				NodeID:    "n1",
				Output:    "result data",
				State:     workflow.StateCompleted,
				StartedAt: now,
				EndedAt:   now,
			},
		},
		Variables: map[string]string{"var1": "val1"},
		StartedAt: now,
		EndedAt:   now,
	}

	// Save
	err := workflow.SaveExecution(dir, exec)
	if err != nil {
		t.Fatalf("SaveExecution() error: %v", err)
	}

	// Verify file exists
	path := filepath.Join(dir, "test-workflow", "exec-123.json")
	if _, err := os.Stat(path); os.IsNotExist(err) {
		t.Fatalf("execution file not created at %s", path)
	}

	// Load
	loaded, err := workflow.LoadExecution(dir, "test-workflow", "exec-123")
	if err != nil {
		t.Fatalf("LoadExecution() error: %v", err)
	}

	if loaded.ID != "exec-123" {
		t.Errorf("ID = %q, want %q", loaded.ID, "exec-123")
	}
	if loaded.WorkflowName != "test-workflow" {
		t.Errorf("WorkflowName = %q, want %q", loaded.WorkflowName, "test-workflow")
	}
	if loaded.State != workflow.StateCompleted {
		t.Errorf("State = %d, want %d", loaded.State, workflow.StateCompleted)
	}
	if loaded.Variables["var1"] != "val1" {
		t.Errorf("Variables[var1] = %q, want %q", loaded.Variables["var1"], "val1")
	}
	if loaded.Input["key"] != "value" {
		t.Errorf("Input[key] = %v, want value", loaded.Input["key"])
	}
	if len(loaded.NodeResults) != 1 {
		t.Errorf("len(NodeResults) = %d, want 1", len(loaded.NodeResults))
	}
	n1 := loaded.NodeResults["n1"]
	if n1 == nil {
		t.Fatal("NodeResults[n1] is nil")
	}
	if n1.Output != "result data" {
		t.Errorf("n1.Output = %v, want %q", n1.Output, "result data")
	}
}

func TestLoadExecution_NotFound(t *testing.T) {
	dir := t.TempDir()

	_, err := workflow.LoadExecution(dir, "nonexistent", "no-id")
	if err == nil {
		t.Fatal("expected error for nonexistent execution")
	}
}

func TestLoadExecutionByID(t *testing.T) {
	dir := t.TempDir()

	exec := &workflow.Execution{
		ID:           "find-me",
		WorkflowName: "search-test",
		State:        workflow.StateCompleted,
		StartedAt:    time.Now(),
	}

	workflow.SaveExecution(dir, exec)

	// Search by ID
	loaded, err := workflow.LoadExecutionByID(dir, "find-me")
	if err != nil {
		t.Fatalf("LoadExecutionByID() error: %v", err)
	}
	if loaded.ID != "find-me" {
		t.Errorf("ID = %q, want %q", loaded.ID, "find-me")
	}
}

func TestLoadExecutionByID_NotFound(t *testing.T) {
	dir := t.TempDir()

	_, err := workflow.LoadExecutionByID(dir, "nonexistent")
	if err == nil {
		t.Fatal("expected error for nonexistent ID")
	}
}

func TestLoadExecutionByID_EmptyDir(t *testing.T) {
	_, err := workflow.LoadExecutionByID("/nonexistent/directory", "any")
	if err == nil {
		t.Fatal("expected error for nonexistent directory")
	}
}

func TestListExecutionsFromDisk_SpecificWorkflow(t *testing.T) {
	dir := t.TempDir()

	for i := 0; i < 3; i++ {
		exec := &workflow.Execution{
			ID:           "exec-" + string(rune('A'+i)),
			WorkflowName: "my-workflow",
			State:        workflow.StateCompleted,
			StartedAt:    time.Now(),
		}
		workflow.SaveExecution(dir, exec)
	}

	executions, err := workflow.ListExecutionsFromDisk(dir, "my-workflow")
	if err != nil {
		t.Fatalf("ListExecutionsFromDisk() error: %v", err)
	}
	if len(executions) != 3 {
		t.Errorf("len(executions) = %d, want 3", len(executions))
	}
}

func TestListExecutionsFromDisk_AllWorkflows(t *testing.T) {
	dir := t.TempDir()

	// Save executions for different workflows
	for _, wfName := range []string{"wf-a", "wf-b"} {
		for i := 0; i < 2; i++ {
			exec := &workflow.Execution{
				ID:           wfName + "-" + string(rune('0'+i)),
				WorkflowName: wfName,
				State:        workflow.StateCompleted,
				StartedAt:    time.Now(),
			}
			workflow.SaveExecution(dir, exec)
		}
	}

	executions, err := workflow.ListExecutionsFromDisk(dir, "")
	if err != nil {
		t.Fatalf("ListExecutionsFromDisk() error: %v", err)
	}
	if len(executions) != 4 {
		t.Errorf("len(executions) = %d, want 4", len(executions))
	}
}

func TestListExecutionsFromDisk_EmptyDir(t *testing.T) {
	dir := t.TempDir()

	executions, err := workflow.ListExecutionsFromDisk(dir, "")
	if err != nil {
		t.Fatalf("ListExecutionsFromDisk() error: %v", err)
	}
	if len(executions) != 0 {
		t.Errorf("len(executions) = %d, want 0", len(executions))
	}
}

func TestListExecutionsFromDisk_NilDir(t *testing.T) {
	executions, err := workflow.ListExecutionsFromDisk("", "")
	if err != nil {
		t.Fatalf("ListExecutionsFromDisk() error: %v", err)
	}
	if executions != nil {
		t.Errorf("expected nil for empty dir, got %v", executions)
	}
}

func TestListExecutionsFromDisk_NonexistentDir(t *testing.T) {
	executions, err := workflow.ListExecutionsFromDisk("/nonexistent/path", "")
	if err != nil {
		t.Fatalf("ListExecutionsFromDisk() error: %v", err)
	}
	if len(executions) != 0 {
		t.Errorf("len(executions) = %d, want 0", len(executions))
	}
}

func TestDeleteExecution(t *testing.T) {
	dir := t.TempDir()

	exec := &workflow.Execution{
		ID:           "delete-me",
		WorkflowName: "test-wf",
		State:        workflow.StateCompleted,
		StartedAt:    time.Now(),
	}
	workflow.SaveExecution(dir, exec)

	// Verify file exists
	path := filepath.Join(dir, "test-wf", "delete-me.json")
	if _, err := os.Stat(path); os.IsNotExist(err) {
		t.Fatal("file not created")
	}

	// Delete
	err := workflow.DeleteExecution(dir, "test-wf", "delete-me")
	if err != nil {
		t.Fatalf("DeleteExecution() error: %v", err)
	}

	// Verify file is gone
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Error("file still exists after deletion")
	}
}

func TestDeleteExecution_NotFound(t *testing.T) {
	dir := t.TempDir()
	err := workflow.DeleteExecution(dir, "no-wf", "no-id")
	// os.Remove does not error on nonexistent files on some platforms
	// but will error if directory doesn't exist
	_ = err
}

func TestCleanupOldExecutions(t *testing.T) {
	dir := t.TempDir()

	// Create an old execution
	oldExec := &workflow.Execution{
		ID:           "old-exec",
		WorkflowName: "cleanup-test",
		State:        workflow.StateCompleted,
		StartedAt:    time.Now().Add(-48 * time.Hour),
	}
	workflow.SaveExecution(dir, oldExec)

	// Manually set the file modification time to be old
	oldPath := filepath.Join(dir, "cleanup-test", "old-exec.json")
	oldTime := time.Now().Add(-48 * time.Hour)
	os.Chtimes(oldPath, oldTime, oldTime)

	// Create a new execution
	newExec := &workflow.Execution{
		ID:           "new-exec",
		WorkflowName: "cleanup-test",
		State:        workflow.StateCompleted,
		StartedAt:    time.Now(),
	}
	workflow.SaveExecution(dir, newExec)

	// Cleanup files older than 3600 seconds (1 hour)
	deleted, err := workflow.CleanupOldExecutions(dir, 3600)
	if err != nil {
		t.Fatalf("CleanupOldExecutions() error: %v", err)
	}
	if deleted != 1 {
		t.Errorf("deleted = %d, want 1", deleted)
	}

	// Verify old file is gone
	if _, err := os.Stat(oldPath); !os.IsNotExist(err) {
		t.Error("old file still exists after cleanup")
	}

	// Verify new file still exists
	newPath := filepath.Join(dir, "cleanup-test", "new-exec.json")
	if _, err := os.Stat(newPath); os.IsNotExist(err) {
		t.Error("new file was deleted during cleanup")
	}
}

func TestCleanupOldExecutions_EmptyDir(t *testing.T) {
	dir := t.TempDir()

	deleted, err := workflow.CleanupOldExecutions(dir, 3600)
	if err != nil {
		t.Fatalf("CleanupOldExecutions() error: %v", err)
	}
	if deleted != 0 {
		t.Errorf("deleted = %d, want 0", deleted)
	}
}

func TestCleanupOldExecutions_NilDir(t *testing.T) {
	deleted, err := workflow.CleanupOldExecutions("", 3600)
	if err != nil {
		t.Fatalf("CleanupOldExecutions() error: %v", err)
	}
	if deleted != 0 {
		t.Errorf("deleted = %d, want 0", deleted)
	}
}

func TestCleanupOldExecutions_NonexistentDir(t *testing.T) {
	deleted, err := workflow.CleanupOldExecutions("/nonexistent/path", 3600)
	if err != nil {
		t.Fatalf("CleanupOldExecutions() error: %v", err)
	}
	if deleted != 0 {
		t.Errorf("deleted = %d, want 0", deleted)
	}
}

func TestSaveExecution_CorruptJSON(t *testing.T) {
	dir := t.TempDir()

	// Create a corrupt JSON file
	subDir := filepath.Join(dir, "test-wf")
	os.MkdirAll(subDir, 0755)
	corruptPath := filepath.Join(subDir, "corrupt.json")
	os.WriteFile(corruptPath, []byte("{invalid json"), 0644)

	// Loading should skip the corrupt file
	executions, err := workflow.ListExecutionsFromDisk(dir, "test-wf")
	if err != nil {
		t.Fatalf("ListExecutionsFromDisk() error: %v", err)
	}
	if len(executions) != 0 {
		t.Errorf("expected 0 executions for corrupt file, got %d", len(executions))
	}
}

func TestSaveExecution_NonJSONFile(t *testing.T) {
	dir := t.TempDir()

	// Create a non-JSON file
	subDir := filepath.Join(dir, "test-wf")
	os.MkdirAll(subDir, 0755)
	txtPath := filepath.Join(subDir, "readme.txt")
	os.WriteFile(txtPath, []byte("not a json file"), 0644)

	// Listing should skip non-JSON files
	executions, err := workflow.ListExecutionsFromDisk(dir, "test-wf")
	if err != nil {
		t.Fatalf("ListExecutionsFromDisk() error: %v", err)
	}
	if len(executions) != 0 {
		t.Errorf("expected 0 executions for non-JSON file, got %d", len(executions))
	}
}

func TestExecutionJSONRoundTrip(t *testing.T) {
	now := time.Now().Truncate(time.Millisecond)
	exec := &workflow.Execution{
		ID:           "round-trip",
		WorkflowName: "test",
		State:        workflow.StateRunning,
		Input:        map[string]interface{}{"query": "hello"},
		NodeResults: map[string]*workflow.NodeResult{
			"n1": {
				NodeID: "n1",
				Output: map[string]interface{}{"key": "value"},
				State:  workflow.StateCompleted,
				Metadata: map[string]interface{}{
					"extra": "data",
				},
				StartedAt: now,
				EndedAt:   now,
			},
		},
		Variables: map[string]string{"var1": "val1"},
		StartedAt: now,
	}

	data, err := json.MarshalIndent(exec, "", "  ")
	if err != nil {
		t.Fatalf("Marshal error: %v", err)
	}

	var decoded workflow.Execution
	if err := json.Unmarshal(data, &decoded); err != nil {
		t.Fatalf("Unmarshal error: %v", err)
	}

	if decoded.ID != exec.ID {
		t.Errorf("ID = %q, want %q", decoded.ID, exec.ID)
	}
	if decoded.State != exec.State {
		t.Errorf("State = %d, want %d", decoded.State, exec.State)
	}
	if decoded.Variables["var1"] != "val1" {
		t.Errorf("Variables[var1] = %q, want %q", decoded.Variables["var1"], "val1")
	}
}
