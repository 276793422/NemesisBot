// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package devices

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/devices/events"
	"github.com/276793422/NemesisBot/module/state"
)

// mockEventSource is a test implementation of EventSource
type mockEventSource struct {
	kind       events.Kind
	eventChan  chan *events.DeviceEvent
	startCount int
	stopCount  int
	shouldFail bool
}

func newMockEventSource(kind events.Kind) *mockEventSource {
	return &mockEventSource{
		kind:      kind,
		eventChan: make(chan *events.DeviceEvent, 10),
	}
}

func (m *mockEventSource) Kind() events.Kind {
	return m.kind
}

func (m *mockEventSource) Start(ctx context.Context) (<-chan *events.DeviceEvent, error) {
	m.startCount++
	if m.shouldFail {
		return nil, errors.New("mock start error")
	}
	return m.eventChan, nil
}

func (m *mockEventSource) Stop() error {
	m.stopCount++
	close(m.eventChan)
	return nil
}

func (m *mockEventSource) sendEvent(ev *events.DeviceEvent) {
	select {
	case m.eventChan <- ev:
	default:
	}
}

func (m *mockEventSource) setShouldFail(fail bool) {
	m.shouldFail = fail
}

// Test NewService
func TestNewService(t *testing.T) {
	t.Run("disabled service", func(t *testing.T) {
		cfg := Config{
			Enabled:    false,
			MonitorUSB: false,
		}
		stateMgr := state.NewManager("/tmp/test")

		svc := NewService(cfg, stateMgr)

		if svc == nil {
			t.Fatal("NewService() should not return nil")
		}

		if svc.enabled {
			t.Error("Service should be disabled")
		}

		if len(svc.sources) != 0 {
			t.Errorf("Expected 0 sources, got %d", len(svc.sources))
		}
	})

	t.Run("enabled service without USB monitoring", func(t *testing.T) {
		cfg := Config{
			Enabled:    true,
			MonitorUSB: false,
		}
		stateMgr := state.NewManager("/tmp/test")

		svc := NewService(cfg, stateMgr)

		if !svc.enabled {
			t.Error("Service should be enabled")
		}

		if len(svc.sources) != 0 {
			t.Errorf("Expected 0 sources, got %d", len(svc.sources))
		}
	})

	t.Run("enabled service with USB monitoring", func(t *testing.T) {
		cfg := Config{
			Enabled:    true,
			MonitorUSB: true,
		}
		stateMgr := state.NewManager("/tmp/test")

		svc := NewService(cfg, stateMgr)

		if !svc.enabled {
			t.Error("Service should be enabled")
		}

		// Note: On non-Linux systems, USB monitor might not be available
		// So we just check that the service was created successfully
		if svc == nil {
			t.Fatal("NewService() should not return nil")
		}
	})
}

// Test SetBus
func TestSetBus(t *testing.T) {
	cfg := Config{Enabled: false}
	stateMgr := state.NewManager("/tmp/test")
	svc := NewService(cfg, stateMgr)

	msgBus := bus.NewMessageBus()
	svc.SetBus(msgBus)

	if svc.bus != msgBus {
		t.Error("Bus should be set correctly")
	}
}

// Test Start and Stop
func TestStartStop(t *testing.T) {
	t.Run("start disabled service", func(t *testing.T) {
		cfg := Config{Enabled: false}
		stateMgr := state.NewManager("/tmp/test")
		svc := NewService(cfg, stateMgr)

		ctx := context.Background()
		err := svc.Start(ctx)

		if err != nil {
			t.Errorf("Start() should not error for disabled service, got: %v", err)
		}

		if svc.ctx != nil || svc.cancel != nil {
			t.Error("Context should not be created for disabled service")
		}
	})

	t.Run("start service with no sources", func(t *testing.T) {
		cfg := Config{Enabled: true, MonitorUSB: false}
		stateMgr := state.NewManager("/tmp/test")
		svc := NewService(cfg, stateMgr)

		ctx := context.Background()
		err := svc.Start(ctx)

		if err != nil {
			t.Errorf("Start() should not error when no sources, got: %v", err)
		}
	})

	t.Run("start and stop service", func(t *testing.T) {
		cfg := Config{Enabled: true, MonitorUSB: false}
		stateMgr := state.NewManager("/tmp/test")
		svc := NewService(cfg, stateMgr)

		// Add a mock source
		mockSrc := newMockEventSource(events.KindUSB)
		svc.sources = append(svc.sources, mockSrc)

		ctx := context.Background()
		err := svc.Start(ctx)

		if err != nil {
			t.Fatalf("Start() failed: %v", err)
		}

		if mockSrc.startCount != 1 {
			t.Errorf("Expected source to be started once, got %d", mockSrc.startCount)
		}

		// Stop the service
		svc.Stop()

		if mockSrc.stopCount != 1 {
			t.Errorf("Expected source to be stopped once, got %d", mockSrc.stopCount)
		}

		if svc.cancel != nil {
			t.Error("Cancel should be nil after stop")
		}
	})

	t.Run("stop when not running", func(t *testing.T) {
		cfg := Config{Enabled: false}
		stateMgr := state.NewManager("/tmp/test")
		svc := NewService(cfg, stateMgr)

		// Should not panic
		svc.Stop()
	})

	t.Run("start service with failing source", func(t *testing.T) {
		cfg := Config{Enabled: true, MonitorUSB: false}
		stateMgr := state.NewManager("/tmp/test")
		svc := NewService(cfg, stateMgr)

		// Add a mock source that fails to start
		mockSrc := newMockEventSource(events.KindUSB)
		mockSrc.setShouldFail(true)
		svc.sources = append(svc.sources, mockSrc)

		ctx := context.Background()
		err := svc.Start(ctx)

		// Should not error even if one source fails
		if err != nil {
			t.Errorf("Start() should not error when some sources fail, got: %v", err)
		}

		// Source should have been attempted to start
		if mockSrc.startCount != 1 {
			t.Errorf("Expected source to be attempted to start once, got %d", mockSrc.startCount)
		}
	})

	t.Run("start service with multiple sources, one failing", func(t *testing.T) {
		cfg := Config{Enabled: true, MonitorUSB: false}
		stateMgr := state.NewManager("/tmp/test")
		svc := NewService(cfg, stateMgr)

		// Add a working mock source
		workingSrc := newMockEventSource(events.KindUSB)
		svc.sources = append(svc.sources, workingSrc)

		// Add a failing mock source
		failingSrc := newMockEventSource(events.KindBluetooth)
		failingSrc.setShouldFail(true)
		svc.sources = append(svc.sources, failingSrc)

		ctx := context.Background()
		err := svc.Start(ctx)

		// Should not error even if one source fails
		if err != nil {
			t.Errorf("Start() should not error when some sources fail, got: %v", err)
		}

		// Both sources should have been attempted to start
		if workingSrc.startCount != 1 {
			t.Errorf("Expected working source to be started once, got %d", workingSrc.startCount)
		}
		if failingSrc.startCount != 1 {
			t.Errorf("Expected failing source to be attempted to start once, got %d", failingSrc.startCount)
		}

		// Stop the service
		svc.Stop()

		if workingSrc.stopCount != 1 {
			t.Errorf("Expected working source to be stopped once, got %d", workingSrc.stopCount)
		}
		if failingSrc.stopCount != 1 {
			t.Errorf("Expected failing source to be stopped once, got %d", failingSrc.stopCount)
		}
	})
}

// Test event handling
func TestEventHandling(t *testing.T) {
	t.Run("handle device events", func(t *testing.T) {
		tempDir := t.TempDir()
		cfg := Config{Enabled: true, MonitorUSB: false}
		stateMgr := state.NewManager(tempDir)
		svc := NewService(cfg, stateMgr)

		// Set up message bus
		msgBus := bus.NewMessageBus()
		svc.SetBus(msgBus)

		// Subscribe to outbound messages
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()

		receivedMessages := make(chan bus.OutboundMessage, 10)
		go func() {
			for {
				msg, ok := msgBus.SubscribeOutbound(ctx)
				if !ok {
					return
				}
				receivedMessages <- msg
			}
		}()

		// Set last channel
		stateMgr.SetLastChannel("test_platform:test_user")

		// Add mock source
		mockSrc := newMockEventSource(events.KindUSB)
		svc.sources = append(svc.sources, mockSrc)

		// Start service
		ctx2 := context.Background()
		if err := svc.Start(ctx2); err != nil {
			t.Fatalf("Start() failed: %v", err)
		}
		defer svc.Stop()

		// Send a test event
		testEvent := &events.DeviceEvent{
			Action:   events.ActionAdd,
			Kind:     events.KindUSB,
			DeviceID: "1-2",
			Vendor:   "Test Vendor",
			Product:  "Test Product",
			Serial:   "ABC123",
		}
		mockSrc.sendEvent(testEvent)

		// Wait for message
		select {
		case msg := <-receivedMessages:
			if msg.Channel != "test_platform" {
				t.Errorf("Expected channel 'test_platform', got '%s'", msg.Channel)
			}
			if msg.ChatID != "test_user" {
				t.Errorf("Expected chatID 'test_user', got '%s'", msg.ChatID)
			}
			if !strings.Contains(msg.Content, "Test Vendor") {
				t.Error("Message should contain vendor name")
			}
			if !strings.Contains(msg.Content, "Test Product") {
				t.Error("Message should contain product name")
			}
		case <-time.After(2 * time.Second):
			t.Error("Timeout waiting for message")
		}
	})

	t.Run("skip events when no last channel", func(t *testing.T) {
		tempDir := t.TempDir()
		cfg := Config{Enabled: true, MonitorUSB: false}
		stateMgr := state.NewManager(tempDir)
		svc := NewService(cfg, stateMgr)

		// Set up message bus
		msgBus := bus.NewMessageBus()
		svc.SetBus(msgBus)

		// Subscribe to outbound messages
		ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
		defer cancel()

		receivedCount := 0
		done := make(chan bool)
		go func() {
			for {
				_, ok := msgBus.SubscribeOutbound(ctx)
				if !ok {
					return
				}
				receivedCount++
				done <- true
			}
		}()

		// Add mock source
		mockSrc := newMockEventSource(events.KindUSB)
		svc.sources = append(svc.sources, mockSrc)

		// Start service
		ctx2 := context.Background()
		if err := svc.Start(ctx2); err != nil {
			t.Fatalf("Start() failed: %v", err)
		}
		defer svc.Stop()

		// Send a test event (should be skipped due to no last channel)
		testEvent := &events.DeviceEvent{
			Action:   events.ActionAdd,
			Kind:     events.KindUSB,
			DeviceID: "1-2",
			Vendor:   "Test Vendor",
			Product:  "Test Product",
		}
		mockSrc.sendEvent(testEvent)

		// Should not receive any message
		select {
		case <-done:
			t.Error("Should not receive message when no last channel is set")
		case <-time.After(100 * time.Millisecond):
			// Expected - no message should be sent
		}
	})

	t.Run("skip nil events", func(t *testing.T) {
		tempDir := t.TempDir()
		cfg := Config{Enabled: true, MonitorUSB: false}
		stateMgr := state.NewManager(tempDir)
		svc := NewService(cfg, stateMgr)

		// Set up message bus
		msgBus := bus.NewMessageBus()
		svc.SetBus(msgBus)

		// Set last channel
		stateMgr.SetLastChannel("test_platform:test_user")

		// Add mock source
		mockSrc := newMockEventSource(events.KindUSB)
		svc.sources = append(svc.sources, mockSrc)

		// Start service
		ctx2 := context.Background()
		if err := svc.Start(ctx2); err != nil {
			t.Fatalf("Start() failed: %v", err)
		}
		defer svc.Stop()

		// Send nil event (should be skipped)
		// This tests the nil check in handleEvents
		go mockSrc.sendEvent(nil)

		// Wait a bit for the goroutine to process the nil event
		time.Sleep(10 * time.Millisecond)
	})

	t.Run("skip internal channels", func(t *testing.T) {
		tempDir := t.TempDir()
		cfg := Config{Enabled: true, MonitorUSB: false}
		stateMgr := state.NewManager(tempDir)
		svc := NewService(cfg, stateMgr)

		// Set up message bus
		msgBus := bus.NewMessageBus()
		svc.SetBus(msgBus)

		// Subscribe to outbound messages
		ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
		defer cancel()

		receivedCount := 0
		done := make(chan bool)
		go func() {
			for {
				_, ok := msgBus.SubscribeOutbound(ctx)
				if !ok {
					return
				}
				receivedCount++
				done <- true
			}
		}()

		// Set last channel to internal channel
		stateMgr.SetLastChannel("cli:test_user")

		// Add mock source
		mockSrc := newMockEventSource(events.KindUSB)
		svc.sources = append(svc.sources, mockSrc)

		// Start service
		ctx2 := context.Background()
		if err := svc.Start(ctx2); err != nil {
			t.Fatalf("Start() failed: %v", err)
		}
		defer svc.Stop()

		// Send a test event
		testEvent := &events.DeviceEvent{
			Action:   events.ActionAdd,
			Kind:     events.KindUSB,
			DeviceID: "1-2",
			Vendor:   "Test Vendor",
			Product:  "Test Product",
		}
		mockSrc.sendEvent(testEvent)

		// Should not receive any message for internal channels
		select {
		case <-done:
			t.Error("Should not receive message for internal channels")
		case <-time.After(100 * time.Millisecond):
			// Expected - no message should be sent
		}
	})
}

// Test sendNotification edge cases
func TestSendNotification(t *testing.T) {
	t.Run("no message bus", func(t *testing.T) {
		tempDir := t.TempDir()
		cfg := Config{Enabled: true, MonitorUSB: false}
		stateMgr := state.NewManager(tempDir)
		svc := NewService(cfg, stateMgr)

		// Don't set message bus (it should be nil)

		// Create a test event
		testEvent := &events.DeviceEvent{
			Action:   events.ActionAdd,
			Kind:     events.KindUSB,
			DeviceID: "1-2",
			Vendor:   "Test Vendor",
			Product:  "Test Product",
		}

		// Should not panic and should return early
		svc.sendNotification(testEvent)
	})

	t.Run("no last channel", func(t *testing.T) {
		tempDir := t.TempDir()
		cfg := Config{Enabled: true, MonitorUSB: false}
		stateMgr := state.NewManager(tempDir)
		svc := NewService(cfg, stateMgr)

		// Set up message bus
		msgBus := bus.NewMessageBus()
		svc.SetBus(msgBus)

		// Don't set last channel (it should be empty)

		// Create a test event
		testEvent := &events.DeviceEvent{
			Action:   events.ActionAdd,
			Kind:     events.KindUSB,
			DeviceID: "1-2",
			Vendor:   "Test Vendor",
			Product:  "Test Product",
		}

		// Should not panic and should return early
		svc.sendNotification(testEvent)
	})

	t.Run("internal channel", func(t *testing.T) {
		tempDir := t.TempDir()
		cfg := Config{Enabled: true, MonitorUSB: false}
		stateMgr := state.NewManager(tempDir)
		svc := NewService(cfg, stateMgr)

		// Set up message bus
		msgBus := bus.NewMessageBus()
		svc.SetBus(msgBus)

		// Set last channel to internal channel
		stateMgr.SetLastChannel("cli:test_user")

		// Create a test event
		testEvent := &events.DeviceEvent{
			Action:   events.ActionAdd,
			Kind:     events.KindUSB,
			DeviceID: "1-2",
			Vendor:   "Test Vendor",
			Product:  "Test Product",
		}

		// Should not panic and should return early for internal channels
		svc.sendNotification(testEvent)
	})

	t.Run("invalid channel format", func(t *testing.T) {
		tempDir := t.TempDir()
		cfg := Config{Enabled: true, MonitorUSB: false}
		stateMgr := state.NewManager(tempDir)
		svc := NewService(cfg, stateMgr)

		// Set up message bus
		msgBus := bus.NewMessageBus()
		svc.SetBus(msgBus)

		// Set last channel with invalid format
		stateMgr.SetLastChannel("invalid_format")

		// Create a test event
		testEvent := &events.DeviceEvent{
			Action:   events.ActionAdd,
			Kind:     events.KindUSB,
			DeviceID: "1-2",
			Vendor:   "Test Vendor",
			Product:  "Test Product",
		}

		// Should not panic and should return early for invalid format
		svc.sendNotification(testEvent)
	})

	t.Run("valid channel", func(t *testing.T) {
		tempDir := t.TempDir()
		cfg := Config{Enabled: true, MonitorUSB: false}
		stateMgr := state.NewManager(tempDir)
		svc := NewService(cfg, stateMgr)

		// Set up message bus
		msgBus := bus.NewMessageBus()
		svc.SetBus(msgBus)

		// Subscribe to outbound messages
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()

		receivedMessages := make(chan bus.OutboundMessage, 10)
		go func() {
			for {
				msg, ok := msgBus.SubscribeOutbound(ctx)
				if !ok {
					return
				}
				receivedMessages <- msg
			}
		}()

		// Set last channel
		stateMgr.SetLastChannel("discord:user123")

		// Create a test event
		testEvent := &events.DeviceEvent{
			Action:   events.ActionAdd,
			Kind:     events.KindUSB,
			DeviceID: "1-2",
			Vendor:   "Test Vendor",
			Product:  "Test Product",
		}

		// Should send notification
		svc.sendNotification(testEvent)

		// Wait for message
		select {
		case msg := <-receivedMessages:
			if msg.Channel != "discord" {
				t.Errorf("Expected channel 'discord', got '%s'", msg.Channel)
			}
			if msg.ChatID != "user123" {
				t.Errorf("Expected chatID 'user123', got '%s'", msg.ChatID)
			}
			if !strings.Contains(msg.Content, "Test Vendor") {
				t.Error("Message should contain vendor name")
			}
		case <-time.After(2 * time.Second):
			t.Error("Timeout waiting for message")
		}
	})
}

// Test parseLastChannel
func TestParseLastChannel(t *testing.T) {
	tests := []struct {
		name             string
		lastChannel      string
		expectedPlatform string
		expectedUserID   string
	}{
		{
			name:             "valid channel",
			lastChannel:      "discord:user123",
			expectedPlatform: "discord",
			expectedUserID:   "user123",
		},
		{
			name:             "empty channel",
			lastChannel:      "",
			expectedPlatform: "",
			expectedUserID:   "",
		},
		{
			name:             "missing separator",
			lastChannel:      "discord",
			expectedPlatform: "",
			expectedUserID:   "",
		},
		{
			name:             "empty platform",
			lastChannel:      ":user123",
			expectedPlatform: "",
			expectedUserID:   "",
		},
		{
			name:             "empty user ID",
			lastChannel:      "discord:",
			expectedPlatform: "",
			expectedUserID:   "",
		},
		{
			name:             "multiple separators",
			lastChannel:      "discord:channel:user",
			expectedPlatform: "discord",
			expectedUserID:   "channel:user",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			platform, userID := parseLastChannel(tt.lastChannel)
			if platform != tt.expectedPlatform {
				t.Errorf("Expected platform '%s', got '%s'", tt.expectedPlatform, platform)
			}
			if userID != tt.expectedUserID {
				t.Errorf("Expected userID '%s', got '%s'", tt.expectedUserID, userID)
			}
		})
	}
}

// Test concurrent access
func TestConcurrentAccess(t *testing.T) {
	tempDir := t.TempDir()
	cfg := Config{Enabled: true, MonitorUSB: false}
	stateMgr := state.NewManager(tempDir)
	svc := NewService(cfg, stateMgr)

	msgBus := bus.NewMessageBus()
	svc.SetBus(msgBus)

	// Test concurrent SetBus calls
	done := make(chan bool)
	for i := 0; i < 10; i++ {
		go func() {
			svc.SetBus(msgBus)
			done <- true
		}()
	}

	// Wait for all goroutines
	for i := 0; i < 10; i++ {
		<-done
	}

	// Test concurrent Start/Stop
	for i := 0; i < 5; i++ {
		go func() {
			ctx := context.Background()
			_ = svc.Start(ctx)
			time.Sleep(10 * time.Millisecond)
			svc.Stop()
			done <- true
		}()
	}

	for i := 0; i < 5; i++ {
		<-done
	}
}

// Test DeviceEvent FormatMessage
func TestDeviceEventFormatMessage(t *testing.T) {
	tests := []struct {
		name  string
		event *events.DeviceEvent
		check func(string) bool
	}{
		{
			name: "add event",
			event: &events.DeviceEvent{
				Action:       events.ActionAdd,
				Kind:         events.KindUSB,
				Vendor:       "Test Vendor",
				Product:      "Test Product",
				Serial:       "ABC123",
				Capabilities: "Keyboard",
			},
			check: func(msg string) bool {
				return strings.Contains(msg, "Connected") &&
					strings.Contains(msg, "usb") &&
					strings.Contains(msg, "Test Vendor") &&
					strings.Contains(msg, "Test Product") &&
					strings.Contains(msg, "ABC123") &&
					strings.Contains(msg, "Keyboard")
			},
		},
		{
			name: "remove event",
			event: &events.DeviceEvent{
				Action:  events.ActionRemove,
				Kind:    events.KindUSB,
				Vendor:  "Test Vendor",
				Product: "Test Product",
			},
			check: func(msg string) bool {
				return strings.Contains(msg, "Disconnected") &&
					strings.Contains(msg, "Test Vendor")
			},
		},
		{
			name: "event with serial",
			event: &events.DeviceEvent{
				Action:  events.ActionAdd,
				Kind:    events.KindUSB,
				Vendor:  "Vendor",
				Product: "Product",
				Serial:  "SERIAL123",
			},
			check: func(msg string) bool {
				return strings.Contains(msg, "Serial:") &&
					strings.Contains(msg, "SERIAL123")
			},
		},
		{
			name: "event without capabilities",
			event: &events.DeviceEvent{
				Action:  events.ActionAdd,
				Kind:    events.KindUSB,
				Vendor:  "Vendor",
				Product: "Product",
			},
			check: func(msg string) bool {
				return !strings.Contains(msg, "Capabilities:")
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			msg := tt.event.FormatMessage()
			if !tt.check(msg) {
				t.Errorf("FormatMessage() check failed for message: %s", msg)
			}
		})
	}
}

// Benchmark tests
func BenchmarkParseLastChannel(b *testing.B) {
	lastChannel := "discord:user123"
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_, _ = parseLastChannel(lastChannel)
	}
}

func BenchmarkDeviceEventFormatMessage(b *testing.B) {
	event := &events.DeviceEvent{
		Action:       events.ActionAdd,
		Kind:         events.KindUSB,
		Vendor:       "Test Vendor",
		Product:      "Test Product",
		Serial:       "ABC123",
		Capabilities: "Keyboard, Mouse",
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_ = event.FormatMessage()
	}
}
