//go:build !cross_compile

package process

import (
	"testing"
)

// --- WindowsExecutor tests ---

func TestNewWindowsExecutor_NilConfig(t *testing.T) {
	executor := NewWindowsExecutor(nil)
	if executor == nil {
		t.Fatal("NewWindowsExecutor returned nil")
	}
	if !executor.config.HideWindow {
		t.Error("Default config should have HideWindow=true")
	}
}

func TestNewWindowsExecutor_CustomConfig(t *testing.T) {
	cfg := &ExecutorConfig{HideWindow: false}
	executor := NewWindowsExecutor(cfg)
	if executor.config.HideWindow != false {
		t.Error("HideWindow should be false")
	}
}

func TestNewWindowsExecutor_DefaultConfig(t *testing.T) {
	cfg := &ExecutorConfig{HideWindow: true}
	executor := NewWindowsExecutor(cfg)
	if !executor.config.HideWindow {
		t.Error("HideWindow should be true")
	}
}

// --- PlatformExecutor interface conformance test ---

func TestWindowsExecutor_ImplementsPlatformExecutor(t *testing.T) {
	var _ PlatformExecutor = NewWindowsExecutor(nil)
}

// --- ExecutorConfig tests ---

func TestExecutorConfig_Default(t *testing.T) {
	cfg := &ExecutorConfig{HideWindow: true}
	if !cfg.HideWindow {
		t.Error("HideWindow should be true")
	}
}

// --- ProcessManager additional tests ---

func TestProcessManager_GetChildByType_Empty(t *testing.T) {
	pm := NewProcessManager()
	defer pm.cancel()

	child, ok := pm.GetChildByType("approval")
	if ok {
		t.Error("Should not find child when none exist")
	}
	if child != nil {
		t.Error("Child should be nil")
	}
}

func TestProcessManager_MultipleChildrenSameType(t *testing.T) {
	pm := NewProcessManager()
	// Don't call Stop() since it would try to terminate children with nil Platform

	pm.mu.Lock()
	pm.children["child-1"] = &ChildProcess{
		ID:         "child-1",
		WindowType: "approval",
	}
	pm.children["child-2"] = &ChildProcess{
		ID:         "child-2",
		WindowType: "dashboard",
	}
	pm.children["child-3"] = &ChildProcess{
		ID:         "child-3",
		WindowType: "approval",
	}
	pm.mu.Unlock()

	// GetChildByType should return the first matching child
	child, ok := pm.GetChildByType("approval")
	if !ok {
		t.Error("Should find an approval child")
	}
	if child == nil {
		t.Fatal("Child should not be nil")
	}
	if child.WindowType != "approval" {
		t.Errorf("WindowType = %q, want 'approval'", child.WindowType)
	}

	// Dashboard should also be found
	dashboard, ok := pm.GetChildByType("dashboard")
	if !ok {
		t.Error("Should find a dashboard child")
	}
	if dashboard.ID != "child-2" {
		t.Errorf("ID = %q, want 'child-2'", dashboard.ID)
	}

	// Unknown type should not be found
	_, ok = pm.GetChildByType("nonexistent")
	if ok {
		t.Error("Should not find nonexistent type")
	}

	// Clean up manually to avoid nil pointer dereference in Stop()
	pm.mu.Lock()
	pm.children = make(map[string]*ChildProcess)
	pm.mu.Unlock()
	pm.cancel()
}

// --- ProcessStatus tests (additional coverage) ---

func TestProcessStatus_Values(t *testing.T) {
	statuses := []ProcessStatus{
		ProcessStatusStarting,
		ProcessStatusRunning,
		ProcessStatusHandshaking,
		ProcessStatusConnected,
		ProcessStatusTerminated,
		ProcessStatusFailed,
	}

	// Verify expected ordering
	if ProcessStatusStarting != 0 {
		t.Errorf("ProcessStatusStarting = %d, want 0", ProcessStatusStarting)
	}
	if ProcessStatusFailed != 5 {
		t.Errorf("ProcessStatusFailed = %d, want 5", ProcessStatusFailed)
	}

	// Verify all distinct
	for i, s1 := range statuses {
		for j, s2 := range statuses {
			if i != j && s1 == s2 {
				t.Errorf("Duplicate status at indices %d and %d: %d", i, j, s1)
			}
		}
	}
}

// --- ChildProcess field tests ---

func TestChildProcess_Fields(t *testing.T) {
	child := &ChildProcess{
		ID:         "test-child",
		PID:        12345,
		WindowType: "approval",
	}
	if child.ID != "test-child" {
		t.Error("ID mismatch")
	}
	if child.PID != 12345 {
		t.Error("PID mismatch")
	}
	if child.WindowType != "approval" {
		t.Error("WindowType mismatch")
	}
}

// --- WindowsSpecific tests ---

func TestWindowsSpecific_DoneChannel(t *testing.T) {
	done := make(chan struct{})
	spec := &WindowsSpecific{
		JobObject: 0,
		Done:      done,
	}
	if spec.Done == nil {
		t.Error("Done channel should not be nil")
	}
	// Close it to simulate process exit
	close(done)
}

// --- Windows constants test ---

func TestWindowsConstants(t *testing.T) {
	if CREATE_NO_WINDOW != 0x08000000 {
		t.Errorf("CREATE_NO_WINDOW = 0x%X, want 0x08000000", CREATE_NO_WINDOW)
	}
}

// --- ErrPopupNotSupported test ---

func TestErrPopupNotSupported(t *testing.T) {
	if ErrPopupNotSupported == nil {
		t.Fatal("ErrPopupNotSupported should not be nil")
	}
	if ErrPopupNotSupported.Error() == "" {
		t.Error("Error message should not be empty")
	}
}
