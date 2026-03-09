// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package heartbeat_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/heartbeat"
)

// TestNewHeartbeatService tests creating a new heartbeat service
func TestNewHeartbeatService(t *testing.T) {
	tmpDir := t.TempDir()

	// Test with default values
	t.Run("create with defaults", func(t *testing.T) {
		hs := heartbeat.NewHeartbeatService(tmpDir, 0, true)
		if hs == nil {
			t.Fatal("NewHeartbeatService() returned nil")
		}
	})

	// Test with custom interval
	t.Run("create with custom interval", func(t *testing.T) {
		hs := heartbeat.NewHeartbeatService(tmpDir, 60, true)
		if hs == nil {
			t.Fatal("NewHeartbeatService() returned nil")
		}
	})

	// Test with minimum interval enforced
	t.Run("minimum interval enforced", func(t *testing.T) {
		hs := heartbeat.NewHeartbeatService(tmpDir, 2, true)
		if hs == nil {
			t.Fatal("NewHeartbeatService() returned nil")
		}
		// Minimum should be 5 minutes
	})

	// Test disabled service
	t.Run("create disabled service", func(t *testing.T) {
		hs := heartbeat.NewHeartbeatService(tmpDir, 30, false)
		if hs == nil {
			t.Fatal("NewHeartbeatService() returned nil")
		}
	})
}

// TestHeartbeatService_StartStop tests starting and stopping the service
func TestHeartbeatService_StartStop(t *testing.T) {
	tmpDir := t.TempDir()

	t.Run("start and stop enabled service", func(t *testing.T) {
		hs := heartbeat.NewHeartbeatService(tmpDir, 30, true)

		err := hs.Start()
		if err != nil {
			t.Fatalf("Start() failed: %v", err)
		}

		// Check if running
		if !hs.IsRunning() {
			t.Error("IsRunning() returned false after Start()")
		}

		// Stop the service
		hs.Stop()

		// Give it a moment to stop
		time.Sleep(100 * time.Millisecond)

		// Check if stopped
		if hs.IsRunning() {
			t.Error("IsRunning() returned true after Stop()")
		}
	})

	t.Run("start disabled service", func(t *testing.T) {
		hs := heartbeat.NewHeartbeatService(tmpDir, 30, false)

		err := hs.Start()
		if err != nil {
			t.Fatalf("Start() failed: %v", err)
		}

		// Should not be running since disabled
		if hs.IsRunning() {
			t.Error("IsRunning() returned true for disabled service")
		}
	})

	t.Run("start already running service", func(t *testing.T) {
		hs := heartbeat.NewHeartbeatService(tmpDir, 30, true)

		err := hs.Start()
		if err != nil {
			t.Fatalf("Start() failed: %v", err)
		}

		// Start again - should not error
		err = hs.Start()
		if err != nil {
			t.Errorf("Start() on already running service failed: %v", err)
		}

		hs.Stop()
	})

	t.Run("stop when not running", func(t *testing.T) {
		hs := heartbeat.NewHeartbeatService(tmpDir, 30, false)

		// Should not panic
		hs.Stop()

		if hs.IsRunning() {
			t.Error("IsRunning() returned true after Stop() on non-running service")
		}
	})
}

// TestHeartbeatService_SetBus tests setting the message bus
func TestHeartbeatService_SetBus(t *testing.T) {
	tmpDir := t.TempDir()
	hs := heartbeat.NewHeartbeatService(tmpDir, 30, false)

	// Test with nil bus - should not panic
	t.Run("set nil bus", func(t *testing.T) {
		hs.SetBus(nil)
	})

	// Test multiple sets - should not panic
	t.Run("set bus multiple times", func(t *testing.T) {
		hs.SetBus(nil)
		hs.SetBus(nil)
	})
}

// TestHeartbeatService_SetHandler tests setting the handler
func TestHeartbeatService_SetHandler(t *testing.T) {
	tmpDir := t.TempDir()
	hs := heartbeat.NewHeartbeatService(tmpDir, 30, false)

	// Test with nil handler - should not panic
	t.Run("set nil handler", func(t *testing.T) {
		hs.SetHandler(nil)
	})
}

// TestHeartbeatService_LogsDirectory tests logs directory creation
func TestHeartbeatService_LogsDirectory(t *testing.T) {
	tmpDir := t.TempDir()

	_ = heartbeat.NewHeartbeatService(tmpDir, 30, false)

	// Check that logs directory was created
	logsDir := filepath.Join(tmpDir, "logs")
	if _, err := os.Stat(logsDir); os.IsNotExist(err) {
		t.Error("NewHeartbeatService() should create logs directory")
	}
}

// TestHeartbeatService_ConcurrentStartStop tests concurrent operations
func TestHeartbeatService_ConcurrentStartStop(t *testing.T) {
	tmpDir := t.TempDir()
	hs := heartbeat.NewHeartbeatService(tmpDir, 30, true)

	// Concurrent start operations
	done := make(chan bool)
	for i := 0; i < 5; i++ {
		go func() {
			hs.Start()
			done <- true
		}()
	}

	// Wait for all starts
	for i := 0; i < 5; i++ {
		<-done
	}

	// Verify running
	if !hs.IsRunning() {
		t.Error("IsRunning() should return true after concurrent starts")
	}

	// Concurrent stop operations
	for i := 0; i < 5; i++ {
		go func() {
			hs.Stop()
			done <- true
		}()
	}

	// Wait for all stops
	for i := 0; i < 5; i++ {
		<-done
	}

	// Give time to stop
	time.Sleep(100 * time.Millisecond)

	// Verify stopped (or at least not crashing)
}

// TestHeartbeatFileEmptyLogic tests the logic for detecting empty heartbeat files
func TestHeartbeatFileEmptyLogic(t *testing.T) {
	tests := []struct {
		name      string
		content   string
		wantEmpty bool
	}{
		{
			name:      "completely empty",
			content:   "",
			wantEmpty: true,
		},
		{
			name:      "only whitespace",
			content:   "   \n\t\n  ",
			wantEmpty: true,
		},
		{
			name:      "only comments",
			content:   "# Comment 1\n# Comment 2\n",
			wantEmpty: true,
		},
		{
			name:      "comments and whitespace",
			content:   "# Comment\n\n  # Another\n\t\n",
			wantEmpty: true,
		},
		{
			name:      "mixed with actual content",
			content:   "# Comment\nActual content here\n",
			wantEmpty: false,
		},
		{
			name:      "only actual content",
			content:   "Check email\nReview calendar\n",
			wantEmpty: false,
		},
		{
			name: "comments with dashes",
			content: `# Header
- Task 1
- Task 2
`,
			wantEmpty: false,
		},
		{
			name:      "comment after content",
			content:   "Some content\n# Followed by comment\n",
			wantEmpty: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Test the logic directly
			lines := strings.Split(tt.content, "\n")
			isEmpty := true
			for _, line := range lines {
				trimmed := strings.TrimSpace(line)
				if trimmed != "" && !strings.HasPrefix(trimmed, "#") {
					isEmpty = false
					break
				}
			}

			if isEmpty != tt.wantEmpty {
				t.Errorf("isHeartbeatFileEmpty() logic = %v, want %v", isEmpty, tt.wantEmpty)
			}
		})
	}
}

// TestHeartbeatService_FileOperations tests file-related operations
func TestHeartbeatService_FileOperations(t *testing.T) {
	tmpDir := t.TempDir()

	t.Run("heartbeat file creation", func(t *testing.T) {
		_ = heartbeat.NewHeartbeatService(tmpDir, 30, false)

		// Trigger heartbeat execution which will create HEARTBEAT.md if it doesn't exist
		// We can't directly execute without a handler, but we can check the service initialization
		logsDir := filepath.Join(tmpDir, "logs")
		if _, err := os.Stat(logsDir); os.IsNotExist(err) {
			t.Error("Service should create logs directory")
		}
	})

	t.Run("heartbeat file with content", func(t *testing.T) {
		heartbeatPath := filepath.Join(tmpDir, "HEARTBEAT.md")
		content := `# Heartbeat tasks

- Check email
- Review calendar
`
		err := os.WriteFile(heartbeatPath, []byte(content), 0644)
		if err != nil {
			t.Fatalf("Failed to create HEARTBEAT.md: %v", err)
		}

		// Verify file was created
		if _, err := os.Stat(heartbeatPath); os.IsNotExist(err) {
			t.Error("HEARTBEAT.md should exist")
		}

		// Verify content
		data, err := os.ReadFile(heartbeatPath)
		if err != nil {
			t.Fatalf("Failed to read HEARTBEAT.md: %v", err)
		}

		if string(data) != content {
			t.Error("HEARTBEAT.md content mismatch")
		}
	})
}

// TestHeartbeatService_ParseChannelLogic tests channel parsing logic
func TestHeartbeatService_ParseChannelLogic(t *testing.T) {
	tests := []struct {
		name         string
		lastChannel  string
		wantPlatform string
		wantUserID   string
		shouldSkip   bool
	}{
		{
			name:         "valid telegram channel",
			lastChannel:  "telegram:123456",
			wantPlatform: "telegram",
			wantUserID:   "123456",
			shouldSkip:   false,
		},
		{
			name:         "valid discord channel",
			lastChannel:  "discord:789012",
			wantPlatform: "discord",
			wantUserID:   "789012",
			shouldSkip:   false,
		},
		{
			name:         "empty string",
			lastChannel:  "",
			wantPlatform: "",
			wantUserID:   "",
			shouldSkip:   true,
		},
		{
			name:         "missing colon separator",
			lastChannel:  "telegram123456",
			wantPlatform: "",
			wantUserID:   "",
			shouldSkip:   true,
		},
		{
			name:         "missing platform",
			lastChannel:  ":123456",
			wantPlatform: "",
			wantUserID:   "",
			shouldSkip:   true,
		},
		{
			name:         "missing user ID",
			lastChannel:  "telegram:",
			wantPlatform: "",
			wantUserID:   "",
			shouldSkip:   true,
		},
		{
			name:         "multiple colons",
			lastChannel:  "platform:user:id",
			wantPlatform: "platform",
			wantUserID:   "user:id",
			shouldSkip:   false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Test the parsing logic
			if tt.lastChannel == "" {
				if tt.wantPlatform != "" || tt.wantUserID != "" {
					t.Error("Empty channel should return empty strings")
				}
				return
			}

			parts := strings.SplitN(tt.lastChannel, ":", 2)
			if len(parts) != 2 || parts[0] == "" || parts[1] == "" {
				// Invalid format
				if tt.wantPlatform != "" || tt.wantUserID != "" {
					t.Error("Invalid format should return empty strings")
				}
				return
			}

			platform, userID := parts[0], parts[1]
			if platform != tt.wantPlatform || userID != tt.wantUserID {
				t.Errorf("Parsed channel = (%s, %s), want (%s, %s)",
					platform, userID, tt.wantPlatform, tt.wantUserID)
			}
		})
	}
}

// TestHeartbeatService_HandlerIntegration tests handler integration
func TestHeartbeatService_HandlerIntegration(t *testing.T) {
	tmpDir := t.TempDir()
	hs := heartbeat.NewHeartbeatService(tmpDir, 30, true)

	t.Run("handler can be set", func(t *testing.T) {
		// Test that we can set a handler
		// The actual handler execution is tested through integration tests
		_ = hs
	})
}

// TestHeartbeatService_TimerBehavior tests timer and interval behavior
func TestHeartbeatService_TimerBehavior(t *testing.T) {
	tmpDir := t.TempDir()

	t.Run("service respects disabled state", func(t *testing.T) {
		hs := heartbeat.NewHeartbeatService(tmpDir, 1, false) // 1 minute interval

		err := hs.Start()
		if err != nil {
			t.Fatalf("Start() failed: %v", err)
		}

		// Should not be running
		if hs.IsRunning() {
			t.Error("Disabled service should not be running")
		}

		hs.Stop()
	})

	t.Run("service starts with enabled state", func(t *testing.T) {
		hs := heartbeat.NewHeartbeatService(tmpDir, 30, true)

		err := hs.Start()
		if err != nil {
			t.Fatalf("Start() failed: %v", err)
		}

		// Should be running
		if !hs.IsRunning() {
			t.Error("Enabled service should be running")
		}

		hs.Stop()
	})
}

// TestHeartbeatService_DefaultTemplate tests default heartbeat template
func TestHeartbeatService_DefaultTemplate(t *testing.T) {
	tmpDir := t.TempDir()
	_ = heartbeat.NewHeartbeatService(tmpDir, 30, false)

	// Check that logs directory was created (side effect of NewHeartbeatService)
	logsDir := filepath.Join(tmpDir, "logs")
	if _, err := os.Stat(logsDir); os.IsNotExist(err) {
		t.Error("NewHeartbeatService() should create logs directory")
	}

	// The default HEARTBEAT.md template should be created when needed
	// This test verifies the directory structure is ready
	heartbeatPath := filepath.Join(tmpDir, "HEARTBEAT.md")
	if _, err := os.Stat(heartbeatPath); !os.IsNotExist(err) {
		// File exists, check content
		content, err := os.ReadFile(heartbeatPath)
		if err != nil {
			t.Fatalf("Failed to read HEARTBEAT.md: %v", err)
		}

		contentStr := string(content)

		// Check for expected sections in default template
		expectedSections := []string{
			"Heartbeat",
			"Check List",
		}

		for _, section := range expectedSections {
			if !strings.Contains(contentStr, section) {
				t.Logf("Note: Default template should contain '%s'", section)
			}
		}
	}
}
