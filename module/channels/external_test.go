// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
)

func createTestScript(t *testing.T, name, content string) string {
	t.Helper()
	dir := t.TempDir()
	path := filepath.Join(dir, name)
	if err := os.WriteFile(path, []byte(content), 0755); err != nil {
		t.Fatalf("failed to create test script: %v", err)
	}
	return path
}

func TestExternalChannel_InvalidConfig(t *testing.T) {
	msgBus := bus.NewMessageBus()

	tests := []struct {
		name    string
		cfg     config.ExternalConfig
		errMsg  string
	}{
		{
			name: "empty input exe",
			cfg: config.ExternalConfig{
				InputEXE:  "",
				OutputEXE: "something",
			},
			errMsg: "both input_exe and output_exe must be specified",
		},
		{
			name: "empty output exe",
			cfg: config.ExternalConfig{
				InputEXE:  "something",
				OutputEXE: "",
			},
			errMsg: "both input_exe and output_exe must be specified",
		},
		{
			name: "nonexistent input exe",
			cfg: config.ExternalConfig{
				InputEXE:  "nonexistent_input.exe",
				OutputEXE: "nonexistent_output.exe",
			},
			errMsg: "input exe not found",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := NewExternalChannel(&tt.cfg, msgBus)
			if err == nil {
				t.Error("expected error")
			} else if tt.errMsg != "" && !contains(err.Error(), tt.errMsg) {
				t.Errorf("error = %q, want substring %q", err.Error(), tt.errMsg)
			}
		})
	}
}

func TestExternalChannel_StartStop(t *testing.T) {
	// Create simple scripts that just exit
	inputScript := createTestScript(t, "input.bat", "@echo off\nexit 0\n")
	outputScript := createTestScript(t, "output.bat", "@echo off\nexit 0\n")

	msgBus := bus.NewMessageBus()
	cfg := &config.ExternalConfig{
		InputEXE:  inputScript,
		OutputEXE: outputScript,
		ChatID:    "test-chat",
	}

	ch, err := NewExternalChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}
	if !ch.IsRunning() {
		t.Error("should be running")
	}

	if err := ch.Stop(ctx); err != nil {
		t.Fatalf("stop failed: %v", err)
	}
	if ch.IsRunning() {
		t.Error("should not be running after stop")
	}
}

func TestExternalChannel_SendNotRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &config.ExternalConfig{
		InputEXE:  "dummy",
		OutputEXE: "dummy",
		ChatID:    "test-chat",
	}
	// Create directly without NewExternalChannel to skip validation
	ch := &ExternalChannel{
		BaseChannel: NewBaseChannel("external", cfg, msgBus, nil),
		config:      cfg,
		stopped:     make(chan struct{}),
	}

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "test-chat",
		Content: "hello",
	})
	if err == nil {
		t.Error("expected error when not running")
	}
}

func TestExternalChannel_SendWrongChatID(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &config.ExternalConfig{
		InputEXE:  "dummy",
		OutputEXE: "dummy",
		ChatID:    "correct-chat",
	}
	ch := &ExternalChannel{
		BaseChannel: NewBaseChannel("external", cfg, msgBus, nil),
		config:      cfg,
		stopped:     make(chan struct{}),
	}
	ch.running.Store(true)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "wrong-chat",
		Content: "hello",
	})
	if err == nil {
		t.Error("expected error for wrong chat ID")
	}
	if !contains(err.Error(), "invalid chat ID") {
		t.Errorf("error = %v", err)
	}
}

func TestExternalChannel_StopNotRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &config.ExternalConfig{
		InputEXE:  "dummy",
		OutputEXE: "dummy",
		ChatID:    "test",
	}
	ch := &ExternalChannel{
		BaseChannel: NewBaseChannel("external", cfg, msgBus, nil),
		config:      cfg,
		stopped:     make(chan struct{}),
	}

	err := ch.Stop(context.Background())
	if err != nil {
		t.Errorf("stop when not running should not error: %v", err)
	}
}

func TestExternalChannel_IsRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &config.ExternalConfig{
		InputEXE:  "dummy",
		OutputEXE: "dummy",
		ChatID:    "test",
	}
	ch := &ExternalChannel{
		BaseChannel: NewBaseChannel("external", cfg, msgBus, nil),
		config:      cfg,
		stopped:     make(chan struct{}),
	}

	if ch.IsRunning() {
		t.Error("should not be running initially")
	}

	ch.running.Store(true)
	if !ch.IsRunning() {
		t.Error("should be running after setting flag")
	}
}
