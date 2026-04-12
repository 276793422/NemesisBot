//go:build !cross_compile

package process

import (
	"errors"
	"testing"
)

func TestNewProcessManager(t *testing.T) {
	pm := NewProcessManager()
	if pm == nil {
		t.Fatal("NewProcessManager returned nil")
	}
	if pm.children == nil {
		t.Error("children map should be initialized")
	}
}

func TestProcessManagerStartStop(t *testing.T) {
	pm := NewProcessManager()

	if err := pm.Start(); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	if err := pm.Stop(); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}
}

func TestProcessManagerStopWithoutStart(t *testing.T) {
	pm := NewProcessManager()

	// Stop without Start should not panic
	if err := pm.Stop(); err != nil {
		t.Fatalf("Stop without Start failed: %v", err)
	}
}

func TestProcessManagerSpawnChildPopupNotSupported(t *testing.T) {
	if PopupSupported {
		t.Skip("PopupSupported is true, skipping non-production test")
	}

	pm := NewProcessManager()
	defer pm.Stop()

	_, _, err := pm.SpawnChild("test", nil)
	if err == nil {
		t.Error("Expected error when PopupSupported is false")
	}
	if !errors.Is(err, ErrPopupNotSupported) {
		t.Errorf("Expected ErrPopupNotSupported, got: %v", err)
	}
}

func TestProcessManagerGetChildNotFound(t *testing.T) {
	pm := NewProcessManager()
	defer pm.Stop()

	child, ok := pm.GetChild("nonexistent")
	if ok {
		t.Error("Expected GetChild to return false for unknown ID")
	}
	if child != nil {
		t.Error("Expected nil child for unknown ID")
	}
}

func TestProcessManagerTerminateChildNotFound(t *testing.T) {
	pm := NewProcessManager()
	defer pm.Stop()

	err := pm.TerminateChild("nonexistent")
	if err == nil {
		t.Error("Expected error when terminating unknown child")
	}
}

func TestProcessManagerDoubleStop(t *testing.T) {
	pm := NewProcessManager()

	if err := pm.Start(); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	if err := pm.Stop(); err != nil {
		t.Fatalf("First Stop failed: %v", err)
	}

	// Double stop should not panic
	if err := pm.Stop(); err != nil {
		t.Fatalf("Second Stop failed: %v", err)
	}
}
