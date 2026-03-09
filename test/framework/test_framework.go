// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

// Package framework provides comprehensive testing utilities for NemesisBot
package framework

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// TempWorkspace provides a temporary workspace directory for testing
type TempWorkspace struct {
	root    string
	workspace string
	t       *testing.T
}

// NewTempWorkspace creates a new temporary workspace for testing
func NewTempWorkspace(t *testing.T) *TempWorkspace {
	t.Helper()

	root := t.TempDir()
	workspace := filepath.Join(root, ".nemesisbot")

	err := os.MkdirAll(workspace, 0755)
	if err != nil {
		t.Fatalf("Failed to create workspace: %v", err)
	}

	// Create subdirectories
	subdirs := []string{
		filepath.Join(workspace, "memory"),
		filepath.Join(workspace, "sessions"),
		filepath.Join(workspace, "state"),
		filepath.Join(workspace, "logs"),
	}

	for _, dir := range subdirs {
		if err := os.MkdirAll(dir, 0755); err != nil {
			t.Fatalf("Failed to create directory %s: %v", dir, err)
		}
	}

	return &TempWorkspace{
		root:      root,
		workspace: workspace,
		t:         t,
	}
}

// Path returns the workspace path
func (w *TempWorkspace) Path() string {
	return w.workspace
}

// Root returns the root temp directory
func (w *TempWorkspace) Root() string {
	return w.root
}

// MemoryPath returns the memory directory path
func (w *TempWorkspace) MemoryPath() string {
	return filepath.Join(w.workspace, "memory")
}

// SessionsPath returns the sessions directory path
func (w *TempWorkspace) SessionsPath() string {
	return filepath.Join(w.workspace, "sessions")
}

// StatePath returns the state directory path
func (w *TempWorkspace) StatePath() string {
	return filepath.Join(w.workspace, "state")
}

// LogsPath returns the logs directory path
func (w *TempWorkspace) LogsPath() string {
	return filepath.Join(w.workspace, "logs")
}

// CreateFile creates a file in the workspace with the given content
func (w *TempWorkspace) CreateFile(name, content string) error {
	path := filepath.Join(w.workspace, name)
	dir := filepath.Dir(path)

	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	return os.WriteFile(path, []byte(content), 0644)
}

// Cleanup removes all temporary files
func (w *TempWorkspace) Cleanup() {
	// Normally handled by t.TempDir(), but explicit cleanup is available
}

// TestContext provides a test context with timeout
func TestContext(t *testing.T, timeout time.Duration) context.Context {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	t.Cleanup(cancel)
	return ctx
}

// AssertNoError fails the test if err is not nil
func AssertNoError(t *testing.T, err error, msg ...interface{}) {
	t.Helper()
	if err != nil {
		t.Fatalf("Unexpected error: %v - %v", err, msg)
	}
}

// AssertError fails the test if err is nil
func AssertError(t *testing.T, err error, msg ...interface{}) {
	t.Helper()
	if err == nil {
		t.Fatal("Expected error but got nil - ", msg)
	}
}

// AssertEqual fails the test if got != want
func AssertEqual(t *testing.T, got, want interface{}, msg ...interface{}) {
	t.Helper()
	if got != want {
		t.Fatalf("Expected %v, got %v - %v", want, got, msg)
	}
}

// AssertNotNil fails the test if value is nil
func AssertNotNil(t *testing.T, value interface{}, msg ...interface{}) {
	t.Helper()
	if value == nil {
		t.Fatal("Expected non-nil value - ", msg)
	}
}

// Eventually retries a condition until it succeeds or times out
func Eventually(t *testing.T, condition func() bool, timeout time.Duration, msg string) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	ticker := time.NewTicker(10 * time.Millisecond)
	defer ticker.Stop()

	for time.Now().Before(deadline) {
		if condition() {
			return
		}
		<-ticker.C
	}

	t.Fatalf("Condition not met within %v: %s", timeout, msg)
}

// WaitFor waits for a channel to have a value or timeout
func WaitFor(t *testing.T, timeout time.Duration, msg string) chan struct{} {
	t.Helper()
	done := make(chan struct{})
	go func() {
		select {
		case <-time.After(timeout):
			t.Fatalf("Timeout waiting for: %s", msg)
		case <-done:
		}
	}()
	return done
}
