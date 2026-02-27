// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// External Channel Integration Tests

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

// TestIntegrationExternalChannelFullLifecycle tests complete lifecycle: create, start, send, stop
func TestIntegrationExternalChannelFullLifecycle(t *testing.T) {
	t.Skip("Skipping integration test - requires actual executables")

	// Create mock executables
	tmpDir := t.TempDir()
	inputEXE := filepath.Join(tmpDir, "integration-input.exe")
	writeMockEchoExecutable(t, inputEXE)

	outputEXE := filepath.Join(tmpDir, "integration-output.exe")
	writeMockSinkExecutable(t, outputEXE)

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: outputEXE,
		ChatID:    "external:integration-test",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()

	// Create channel
	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Start channel
	t.Log("Starting external channel...")
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}

	if !channel.IsRunning() {
		t.Fatal("Channel should be running after Start()")
	}

	// Wait for processes to initialize
	time.Sleep(500 * time.Millisecond)

	// Subscribe to outbound messages to verify messages flow through
	outboundReceived := make(chan bus.OutboundMessage, 10)
	go func() {
		for {
			msg, ok := testBus.SubscribeOutbound(ctx)
			if !ok {
				return
			}
			if msg.Channel == "external" {
				outboundReceived <- msg
			}
		}
	}()

	// Send a message
	t.Log("Sending test message...")
	testMsg := bus.OutboundMessage{
		Channel: "external",
		ChatID:  "external:integration-test",
		Content: "Hello from integration test!",
	}

	if err := channel.Send(ctx, testMsg); err != nil {
		t.Errorf("Failed to send message: %v", err)
	}

	// Wait for message to be processed
	select {
	case <-outboundReceived:
		t.Log("Message successfully sent through channel")
	case <-time.After(2 * time.Second):
		t.Error("Timeout waiting for outbound message")
	}

	// Stop channel
	t.Log("Stopping external channel...")
	if err := channel.Stop(ctx); err != nil {
		t.Errorf("Failed to stop channel: %v", err)
	}

	if channel.IsRunning() {
		t.Error("Channel should not be running after Stop()")
	}

	t.Log("Integration test completed successfully")
}

// TestIntegrationExternalChannelWithWebSync tests channel with web sync enabled
func TestIntegrationExternalChannelWithWebSync(t *testing.T) {
	t.Skip("Skipping integration test - requires actual executables")

	tmpDir := t.TempDir()
	inputEXE := filepath.Join(tmpDir, "sync-input.exe")
	writeMockEchoExecutable(t, inputEXE)

	outputEXE := filepath.Join(tmpDir, "sync-output.exe")
	writeMockSinkExecutable(t, outputEXE)

	cfg := &config.ExternalConfig{
		Enabled:     true,
		InputEXE:    inputEXE,
		OutputEXE:   outputEXE,
		ChatID:      "external:sync-test",
		SyncToWeb:   true,
		WebSessionID: "",
	}

	testBus := bus.NewMessageBus()

	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	// Set a mock web channel
	var webChannel Channel = &mockChannel{}
	channel.SetWebChannel(&webChannel)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	time.Sleep(200 * time.Millisecond)

	// Send message - should sync to web channel
	msg := bus.OutboundMessage{
		Channel: "external",
		ChatID:  "external:sync-test",
		Content: "Test sync to web",
	}

	if err := channel.Send(ctx, msg); err != nil {
		t.Errorf("Failed to send message: %v", err)
	}

	t.Log("Web sync test completed")
}

// TestIntegrationExternalChannelMultipleStartStop tests multiple start/stop cycles
func TestIntegrationExternalChannelMultipleStartStop(t *testing.T) {
	t.Skip("Skipping integration test - requires actual executables")

	tmpDir := t.TempDir()
	inputEXE := filepath.Join(tmpDir, "multi-input.exe")
	writeMockEchoExecutable(t, inputEXE)

	outputEXE := filepath.Join(tmpDir, "multi-output.exe")
	writeMockSinkExecutable(t, outputEXE)

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: outputEXE,
		ChatID:    "external:multi-test",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()

	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	// Perform multiple start/stop cycles
	for i := 0; i < 3; i++ {
		t.Logf("Cycle %d: Starting channel...", i+1)

		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)

		if err := channel.Start(ctx); err != nil {
			cancel()
			t.Errorf("Cycle %d: Failed to start channel: %v", i+1, err)
			continue
		}

		if !channel.IsRunning() {
			cancel()
			t.Errorf("Cycle %d: Channel should be running", i+1)
			continue
		}

		time.Sleep(100 * time.Millisecond)

		t.Logf("Cycle %d: Stopping channel...", i+1)
		if err := channel.Stop(ctx); err != nil {
			cancel()
			t.Errorf("Cycle %d: Failed to stop channel: %v", i+1, err)
			continue
		}

		if channel.IsRunning() {
			cancel()
			t.Errorf("Cycle %d: Channel should not be running", i+1)
			continue
		}

		cancel()
		t.Logf("Cycle %d: Completed successfully", i+1)
	}
}

// TestIntegrationExternalChannelConcurrentMessages tests sending multiple messages concurrently
func TestIntegrationExternalChannelConcurrentMessages(t *testing.T) {
	t.Skip("Skipping integration test - requires actual executables")

	tmpDir := t.TempDir()
	inputEXE := filepath.Join(tmpDir, "concurrent-input.exe")
	writeMockEchoExecutable(t, inputEXE)

	outputEXE := filepath.Join(tmpDir, "concurrent-output.exe")
	writeMockSinkExecutable(t, outputEXE)

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: outputEXE,
		ChatID:    "external:concurrent-test",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()

	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	time.Sleep(200 * time.Millisecond)

	// Send multiple messages concurrently
	numMessages := 10
	done := make(chan bool, numMessages)

	for i := 0; i < numMessages; i++ {
		go func(msgNum int) {
			msg := bus.OutboundMessage{
				Channel: "external",
				ChatID:  "external:concurrent-test",
				Content: "Concurrent message",
			}

			if err := channel.Send(ctx, msg); err != nil {
				t.Errorf("Failed to send message %d: %v", msgNum, err)
			}
			done <- true
		}(i)
	}

	// Wait for all messages to be sent
	for i := 0; i < numMessages; i++ {
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Errorf("Timeout waiting for message %d to be sent", i)
		}
	}

	t.Logf("Successfully sent %d concurrent messages", numMessages)
}

// TestIntegrationExternalChannelMessageBusIntegration tests integration with message bus
func TestIntegrationExternalChannelMessageBusIntegration(t *testing.T) {
	t.Skip("Skipping integration test - requires actual executables")

	tmpDir := t.TempDir()
	inputEXE := filepath.Join(tmpDir, "bus-input.exe")
	writeMockEchoExecutable(t, inputEXE)

	outputEXE := filepath.Join(tmpDir, "bus-output.exe")
	writeMockSinkExecutable(t, outputEXE)

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: outputEXE,
		ChatID:    "external:bus-test",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()

	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	time.Sleep(200 * time.Millisecond)

	// Send an outbound message (simulating AI response)
	outboundMsg := bus.OutboundMessage{
		Channel: "external",
		ChatID:  "external:bus-test",
		Content: "Test message bus integration",
	}

	if err := channel.Send(ctx, outboundMsg); err != nil {
		t.Errorf("Failed to send outbound message: %v", err)
	}

	t.Log("Message bus integration test completed")
}

// TestIntegrationExternalChannelErrorHandling tests error handling scenarios
func TestIntegrationExternalChannelErrorHandling(t *testing.T) {
	t.Skip("Skipping integration test - requires actual executables")

	tmpDir := t.TempDir()
	inputEXE := filepath.Join(tmpDir, "error-input.exe")
	writeMockEchoExecutable(t, inputEXE)

	outputEXE := filepath.Join(tmpDir, "error-output.exe")
	writeMockSinkExecutable(t, outputEXE)

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: outputEXE,
		ChatID:    "external:error-test",
	}

	testBus := bus.NewMessageBus()

	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	time.Sleep(200 * time.Millisecond)

	// Test sending with wrong chat ID
	wrongMsg := bus.OutboundMessage{
		Channel: "external",
		ChatID:  "external:wrong-chat-id",
		Content: "This should fail",
	}

	err = channel.Send(ctx, wrongMsg)
	if err == nil {
		t.Error("Expected error when sending with wrong chat ID")
	}

	// Test sending to correct chat ID
	correctMsg := bus.OutboundMessage{
		Channel: "external",
		ChatID:  "external:error-test",
		Content: "This should succeed",
	}

	err = channel.Send(ctx, correctMsg)
	if err != nil {
		t.Errorf("Failed to send message with correct chat ID: %v", err)
	}

	t.Log("Error handling test completed")
}

// TestIntegrationExternalChannelContextCancellation tests behavior with cancelled context
func TestIntegrationExternalChannelContextCancellation(t *testing.T) {
	t.Skip("Skipping integration test - requires actual executables")

	tmpDir := t.TempDir()
	inputEXE := filepath.Join(tmpDir, "cancel-input.exe")
	writeMockEchoExecutable(t, inputEXE)

	outputEXE := filepath.Join(tmpDir, "cancel-output.exe")
	writeMockSinkExecutable(t, outputEXE)

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: outputEXE,
		ChatID:    "external:cancel-test",
	}

	testBus := bus.NewMessageBus()

	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	// Start with a context that gets cancelled quickly
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	if err := channel.Start(ctx); err != nil && err != context.DeadlineExceeded {
		t.Logf("Channel start with cancelled context returned: %v", err)
	}

	// Channel should handle context cancellation gracefully
	_ = channel.Stop(context.Background())

	t.Log("Context cancellation test completed")
}

// TestIntegrationExternalChannelLongRunning tests long-running operation
func TestIntegrationExternalChannelLongRunning(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping long-running test in short mode")
	}
	t.Skip("Skipping integration test - requires actual executables")

	tmpDir := t.TempDir()
	inputEXE := filepath.Join(tmpDir, "long-input.exe")
	writeMockEchoExecutable(t, inputEXE)

	outputEXE := filepath.Join(tmpDir, "long-output.exe")
	writeMockSinkExecutable(t, outputEXE)

	cfg := &config.ExternalConfig{
		Enabled:   true,
		InputEXE:  inputEXE,
		OutputEXE: outputEXE,
		ChatID:    "external:long-test",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()

	channel, err := NewExternalChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create external channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}

	// Send messages periodically for 5 seconds
	ticker := time.NewTicker(500 * time.Millisecond)
	stopTicker := make(chan bool)
	msgCount := 0

	go func() {
		for {
			select {
			case <-ticker.C:
				msg := bus.OutboundMessage{
					Channel: "external",
					ChatID:  "external:long-test",
					Content: "Long running test message",
				}
				if err := channel.Send(ctx, msg); err != nil {
					t.Logf("Failed to send message: %v", err)
				}
				msgCount++
			case <-stopTicker:
				ticker.Stop()
				return
			}
		}
	}()

	// Let it run for 5 seconds
	time.Sleep(5 * time.Second)
	close(stopTicker)

	// Stop the channel
	if err := channel.Stop(ctx); err != nil {
		t.Errorf("Failed to stop channel: %v", err)
	}

	t.Logf("Long-running test completed. Sent %d messages successfully", msgCount)
}

// Helper functions

// writeMockEchoExecutable creates a simple echo program for testing input EXE
func writeMockEchoExecutable(t *testing.T, path string) {
	t.Helper()
	// For Windows, we'll create a batch file without .exe extension
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
