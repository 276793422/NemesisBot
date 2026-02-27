// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// External Channel Unit Tests

package channels_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	. "github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
)

// TestNewExternalChannelSuccess tests successful creation of external channel
func TestNewExternalChannelSuccess(t *testing.T) {
	// Create mock executables for testing
	tmpDir := t.TempDir()
	inputEXE := filepath.Join(tmpDir, "input.exe")
	if err := os.WriteFile(inputEXE, []byte(""), 0755); err != nil {
		t.Fatalf("Failed to create input exe: %v", err)
	}

	outputEXE := filepath.Join(tmpDir, "output.exe")
	if err := os.WriteFile(outputEXE, []byte(""), 0755); err != nil {
		t.Fatalf("Failed to create output exe: %v", err)
	}

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: outputEXE,
		ChatID:    "external:test",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()

	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	if channel == nil {
		t.Fatal("Expected channel to be created, got nil")
	}

	if channel.Name() != "external" {
		t.Errorf("Expected channel name 'external', got: %s", channel.Name())
	}
}

// TestNewExternalChannelMissingInputEXE tests error when input exe is missing
func TestNewExternalChannelMissingInputEXE(t *testing.T) {
	outputEXE := createMockExecutable(t, "output")
	defer os.Remove(outputEXE)

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  "",
		OutputEXE: outputEXE,
		ChatID:    "external:test",
	}

	testBus := bus.NewMessageBus()

	_, err := NewExternalChannel(cfg, testBus)
	if err == nil {
		t.Error("Expected error when input exe is empty, got nil")
	}
}

// TestNewExternalChannelMissingOutputEXE tests error when output exe is missing
func TestNewExternalChannelMissingOutputEXE(t *testing.T) {
	inputEXE := createMockExecutable(t, "input")
	defer os.Remove(inputEXE)

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: "",
		ChatID:    "external:test",
	}

	testBus := bus.NewMessageBus()

	_, err := NewExternalChannel(cfg, testBus)
	if err == nil {
		t.Error("Expected error when output exe is empty, got nil")
	}
}

// TestNewExternalChannelNonExistentInputEXE tests error when input exe doesn't exist
func TestNewExternalChannelNonExistentInputEXE(t *testing.T) {
	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  "non_existent_input.exe",
		OutputEXE: "non_existent_output.exe",
		ChatID:    "external:test",
	}

	testBus := bus.NewMessageBus()

	_, err := NewExternalChannel(cfg, testBus)
	if err == nil {
		t.Error("Expected error when input exe doesn't exist, got nil")
	}
}

// TestExternalChannelIsRunningInitialState tests initial running state
func TestExternalChannelIsRunningInitialState(t *testing.T) {
	tmpDir := t.TempDir()
	inputEXE := filepath.Join(tmpDir, "input.exe")
	if err := os.WriteFile(inputEXE, []byte(""), 0755); err != nil {
		t.Fatalf("Failed to create input exe: %v", err)
	}

	outputEXE := filepath.Join(tmpDir, "output.exe")
	if err := os.WriteFile(outputEXE, []byte(""), 0755); err != nil {
		t.Fatalf("Failed to create output exe: %v", err)
	}

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: outputEXE,
		ChatID:    "external:test",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()

	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	if channel.IsRunning() {
		t.Error("Expected channel to not be running initially")
	}
}

// TestExternalChannelSetWebChannel tests setting web channel reference
func TestExternalChannelSetWebChannel(t *testing.T) {
	tmpDir := t.TempDir()
	inputEXE := filepath.Join(tmpDir, "input.exe")
	if err := os.WriteFile(inputEXE, []byte(""), 0755); err != nil {
		t.Fatalf("Failed to create input exe: %v", err)
	}

	outputEXE := filepath.Join(tmpDir, "output.exe")
	if err := os.WriteFile(outputEXE, []byte(""), 0755); err != nil {
		t.Fatalf("Failed to create output exe: %v", err)
	}

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: outputEXE,
		ChatID:    "external:test",
		SyncToWeb: true,
	}

	testBus := bus.NewMessageBus()

	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	// Create a mock web channel (just for testing the setter)
	var webChannel Channel = &mockChannel{}

	// This should not panic
	channel.SetWebChannel(&webChannel)
}

// TestExternalChannelStartAndStop tests starting and stopping the channel
func TestExternalChannelStartAndStop(t *testing.T) {
	t.Skip("Skipping process execution test - requires actual executables")

	inputEXE := createMockExecutable(t, "input-echo")
	writeMockEchoExecutable(t, inputEXE)
	defer os.Remove(inputEXE)

	outputEXE := createMockExecutable(t, "output-sink")
	writeMockSinkExecutable(t, outputEXE)
	defer os.Remove(outputEXE)

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: outputEXE,
		ChatID:    "external:test",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()

	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Start the channel
	err = channel.Start(ctx)
	if err != nil {
		t.Fatalf("Failed to start external channel: %v", err)
	}

	if !channel.IsRunning() {
		t.Error("Expected channel to be running after Start()")
	}

	// Give it a moment to start up
	time.Sleep(100 * time.Millisecond)

	// Stop the channel
	err = channel.Stop(ctx)
	if err != nil {
		t.Errorf("Failed to stop external channel: %v", err)
	}

	if channel.IsRunning() {
		t.Error("Expected channel to not be running after Stop()")
	}
}

// TestExternalChannelSendBeforeStart tests sending before channel is started
func TestExternalChannelSendBeforeStart(t *testing.T) {
	tmpDir := t.TempDir()
	inputEXE := filepath.Join(tmpDir, "input.exe")
	if err := os.WriteFile(inputEXE, []byte(""), 0755); err != nil {
		t.Fatalf("Failed to create input exe: %v", err)
	}

	outputEXE := filepath.Join(tmpDir, "output.exe")
	if err := os.WriteFile(outputEXE, []byte(""), 0755); err != nil {
		t.Fatalf("Failed to create output exe: %v", err)
	}

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: outputEXE,
		ChatID:    "external:test",
	}

	testBus := bus.NewMessageBus()

	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	ctx := context.Background()

	msg := bus.OutboundMessage{
		Channel: "external",
		ChatID:  "external:test",
		Content: "test message",
	}

	err = channel.Send(ctx, msg)
	if err == nil {
		t.Error("Expected error when sending before channel is started, got nil")
	}
}

// TestExternalChannelSendWrongChatID tests sending with wrong chat ID
func TestExternalChannelSendWrongChatID(t *testing.T) {
	t.Skip("Skipping process execution test - requires actual executables")

	inputEXE := createMockExecutable(t, "input-echo")
	writeMockEchoExecutable(t, inputEXE)
	defer os.Remove(inputEXE)

	outputEXE := createMockExecutable(t, "output-sink")
	writeMockSinkExecutable(t, outputEXE)
	defer os.Remove(outputEXE)

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: outputEXE,
		ChatID:    "external:test",
	}

	testBus := bus.NewMessageBus()

	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err = channel.Start(ctx)
	if err != nil {
		t.Fatalf("Failed to start external channel: %v", err)
	}
	defer channel.Stop(ctx)

	time.Sleep(100 * time.Millisecond)

	msg := bus.OutboundMessage{
		Channel: "external",
		ChatID:  "external:wrong",
		Content: "test message",
	}

	err = channel.Send(ctx, msg)
	if err == nil {
		t.Error("Expected error when sending with wrong chat ID, got nil")
	}
}

// Helper functions

// createMockExecutable creates a temporary empty file for testing
func createMockExecutable(t *testing.T, name string) string {
	t.Helper()
	tmpDir := t.TempDir()
	return filepath.Join(tmpDir, name)
}

// writeMockEchoExecutable creates a simple echo program for testing input EXE
func writeMockEchoExecutable(t *testing.T, path string) {
	t.Helper()
	// For Windows, we'll create a batch file without .exe extension
	// The test will need to handle this appropriately
	content := `@echo off
set /p line=
echo %line%
`
	batPath := path + ".bat"
	if err := os.WriteFile(batPath, []byte(content), 0755); err != nil {
		t.Fatalf("Failed to write mock input exe: %v", err)
	}
}

// writeMockSinkExecutable creates a simple sink program for testing output EXE
func writeMockSinkExecutable(t *testing.T, path string) {
	t.Helper()
	// For Windows, we'll create a batch file without .exe extension
	content := `@echo off
:loop
set /p line=
if "%line%"=="" goto loop
echo Received: %line% > nul
goto loop
`
	batPath := path + ".bat"
	if err := os.WriteFile(batPath, []byte(content), 0755); err != nil {
		t.Fatalf("Failed to write mock output exe: %v", err)
	}
}

// mockChannel is a minimal implementation of Channel interface for testing
type mockChannel struct{}

func (m *mockChannel) Name() string {
	return "mock"
}

func (m *mockChannel) Start(ctx context.Context) error {
	return nil
}

func (m *mockChannel) Stop(ctx context.Context) error {
	return nil
}

func (m *mockChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	return nil
}

func (m *mockChannel) IsRunning() bool {
	return false
}

func (m *mockChannel) IsAllowed(senderID string) bool {
	return true
}
