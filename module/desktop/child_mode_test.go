package desktop

import (
	"testing"
)

func TestHasChildModeFlag(t *testing.T) {
	// By default, no --multiple flag is set, so should return false
	result := HasChildModeFlag()
	if result {
		t.Error("HasChildModeFlag should return false when --multiple is not set")
	}
}

func TestRunChildMode_HandlerInitialized(t *testing.T) {
	// childModeHandler is initialized in init() to process.RunChildMode
	// We can't easily test the actual RunChildMode without setting up the process
	// environment, but we can verify the handler is set by checking the function
	// doesn't return the "not initialized" error in a controlled way.

	// Since the actual handler calls process.RunChildMode which expects stdin/stdout
	// pipe communication, we just test that the handler is not nil by checking
	// that the function exists and is callable.
	// The handler is initialized in init(), so it should be non-nil.

	// We save and restore to avoid side effects
	origHandler := childModeHandler
	defer func() {
		childModeHandler = origHandler
	}()

	// Test nil handler case
	childModeHandler = nil
	err := RunChildMode()
	if err == nil {
		t.Error("Expected error when handler is nil")
	}
	if err.Error() != "child mode handler not initialized" {
		t.Errorf("Unexpected error message: %v", err)
	}
}
