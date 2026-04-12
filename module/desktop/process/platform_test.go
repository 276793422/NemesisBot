//go:build !cross_compile

package process

import (
	"testing"
)

func TestGetPlatform(t *testing.T) {
	p := GetPlatform()
	// On Windows CI/dev machines, should return PlatformWindows
	// This test just verifies the function returns a valid platform
	if p != PlatformWindows && p != PlatformLinux && p != PlatformmacOS {
		t.Errorf("GetPlatform returned invalid platform: %d", p)
	}
}

func TestGetPlatformExecutor(t *testing.T) {
	executor := GetPlatformExecutor(nil)
	if executor == nil {
		t.Error("GetPlatformExecutor returned nil")
	}
}

func TestGetPlatformExecutorWithConfig(t *testing.T) {
	cfg := &ExecutorConfig{HideWindow: true}
	executor := GetPlatformExecutor(cfg)
	if executor == nil {
		t.Error("GetPlatformExecutor with config returned nil")
	}
}
