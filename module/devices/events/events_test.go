// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package events

import (
	"context"
	"testing"
	"time"
)

// MockEventSource is a mock implementation of EventSource for testing
type MockEventSource struct {
	kind        Kind
	startCalled bool
	stopCalled  bool
	eventChan   chan *DeviceEvent
	returnError bool
}

func (m *MockEventSource) Kind() Kind {
	return m.kind
}

func (m *MockEventSource) Start(ctx context.Context) (<-chan *DeviceEvent, error) {
	m.startCalled = true
	if m.returnError {
		return nil, context.Canceled
	}
	if m.eventChan == nil {
		m.eventChan = make(chan *DeviceEvent, 10)
	}
	return m.eventChan, nil
}

func (m *MockEventSource) Stop() error {
	m.stopCalled = true
	if m.eventChan != nil {
		select {
		case <-m.eventChan:
			// Channel already closed
		default:
			close(m.eventChan)
		}
		m.eventChan = nil
	}
	return nil
}

func TestEventSource_Interface(t *testing.T) {
	mock := &MockEventSource{
		kind:      KindUSB,
		eventChan: make(chan *DeviceEvent, 1),
	}

	ctx := context.Background()

	// Test Kind
	if mock.Kind() != KindUSB {
		t.Errorf("Kind() = %v, want %v", mock.Kind(), KindUSB)
	}

	// Test Start
	ch, err := mock.Start(ctx)
	if err != nil {
		t.Fatalf("Start() error = %v", err)
	}
	if !mock.startCalled {
		t.Error("Start() was not called")
	}
	if ch == nil {
		t.Error("Start() returned nil channel")
	}

	// Test Stop
	err = mock.Stop()
	if err != nil {
		t.Fatalf("Stop() error = %v", err)
	}
	if !mock.stopCalled {
		t.Error("Stop() was not called")
	}
}

func TestEventSource_StartWithError(t *testing.T) {
	mock := &MockEventSource{
		kind:        KindUSB,
		returnError: true,
	}

	ctx := context.Background()
	ch, err := mock.Start(ctx)
	if err == nil {
		t.Error("Start() should return error")
	}
	if ch != nil {
		t.Error("Start() should return nil channel on error")
	}
}

func TestAction_Constants(t *testing.T) {
	tests := []struct {
		name  string
		action Action
		want  string
	}{
		{"ActionAdd", ActionAdd, "add"},
		{"ActionRemove", ActionRemove, "remove"},
		{"ActionChange", ActionChange, "change"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if string(tt.action) != tt.want {
				t.Errorf("%s = %v, want %v", tt.name, tt.action, tt.want)
			}
		})
	}
}

func TestKind_Constants(t *testing.T) {
	tests := []struct {
		name string
		kind Kind
		want string
	}{
		{"KindUSB", KindUSB, "usb"},
		{"KindBluetooth", KindBluetooth, "bluetooth"},
		{"KindPCI", KindPCI, "pci"},
		{"KindGeneric", KindGeneric, "generic"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if string(tt.kind) != tt.want {
				t.Errorf("%s = %v, want %v", tt.name, tt.kind, tt.want)
			}
		})
	}
}

func TestDeviceEvent_FormatMessage_AddAction(t *testing.T) {
	event := &DeviceEvent{
		Action:       ActionAdd,
		Kind:         KindUSB,
		DeviceID:     "1-2",
		Vendor:       "TestVendor",
		Product:      "TestProduct",
		Serial:       "SN12345",
		Capabilities: "Camera, Microphone",
		Raw:          map[string]string{"key": "value"},
	}

	msg := event.FormatMessage()

	// Check for expected content
	expectedContent := []string{
		"🔌", "Device Connected",
		"Type: usb",
		"Device: TestVendor TestProduct",
		"Capabilities: Camera, Microphone",
		"Serial: SN12345",
	}

	for _, content := range expectedContent {
		if !contains(msg, content) {
			t.Errorf("FormatMessage() missing expected content: %s\nGot: %s", content, msg)
		}
	}
}

func TestDeviceEvent_FormatMessage_RemoveAction(t *testing.T) {
	event := &DeviceEvent{
		Action:  ActionRemove,
		Kind:    KindBluetooth,
		DeviceID: "bt-adapter-1",
		Vendor:  "BlueTech",
		Product: "BT Adapter",
	}

	msg := event.FormatMessage()

	// Check for expected content
	expectedContent := []string{
		"🔌", "Device Disconnected",
		"Type: bluetooth",
		"Device: BlueTech BT Adapter",
	}

	for _, content := range expectedContent {
		if !contains(msg, content) {
			t.Errorf("FormatMessage() missing expected content: %s\nGot: %s", content, msg)
		}
	}
}

func TestDeviceEvent_FormatMessage_ChangeAction(t *testing.T) {
	event := &DeviceEvent{
		Action:  ActionChange,
		Kind:    KindPCI,
		DeviceID: "pci-0000:00:1f.0",
		Vendor:  "Intel",
		Product: "Audio Device",
	}

	msg := event.FormatMessage()

	// Check for expected content (change action uses same format as add)
	expectedContent := []string{
		"🔌", "Device Connected",
		"Type: pci",
		"Device: Intel Audio Device",
	}

	for _, content := range expectedContent {
		if !contains(msg, content) {
			t.Errorf("FormatMessage() missing expected content: %s\nGot: %s", content, msg)
		}
	}
}

func TestDeviceEvent_FormatMessage_Minimal(t *testing.T) {
	event := &DeviceEvent{
		Action:  ActionAdd,
		Kind:    KindGeneric,
		DeviceID: "generic-1",
		Vendor:  "GenericVendor",
		Product: "GenericProduct",
		// No Serial or Capabilities
	}

	msg := event.FormatMessage()

	// Check basic content
	expectedContent := []string{
		"🔌", "Device Connected",
		"Type: generic",
		"Device: GenericVendor GenericProduct",
	}

	for _, content := range expectedContent {
		if !contains(msg, content) {
			t.Errorf("FormatMessage() missing expected content: %s\nGot: %s", content, msg)
		}
	}

	// Check that optional fields are not present
	unexpectedContent := []string{
		"Capabilities:", "Serial:",
	}

	for _, content := range unexpectedContent {
		if contains(msg, content) {
			t.Errorf("FormatMessage() should not contain: %s\nGot: %s", content, msg)
		}
	}
}

func TestDeviceEvent_RawMap(t *testing.T) {
	rawData := map[string]string{
		"bus_number":    "1",
		"device_number": "2",
		"custom_field":  "custom_value",
	}

	event := &DeviceEvent{
		Action:  ActionAdd,
		Kind:    KindUSB,
		Vendor:  "Test",
		Product: "Device",
		Raw:     rawData,
	}

	// Verify raw map is stored correctly
	if len(event.Raw) != len(rawData) {
		t.Errorf("Raw map length = %d, want %d", len(event.Raw), len(rawData))
	}

	for key, value := range rawData {
		if event.Raw[key] != value {
			t.Errorf("Raw[%s] = %s, want %s", key, event.Raw[key], value)
		}
	}
}

func TestDeviceEvent_AllActions(t *testing.T) {
	actions := []Action{ActionAdd, ActionRemove, ActionChange}

	for _, action := range actions {
		t.Run(string(action), func(t *testing.T) {
			event := &DeviceEvent{
				Action:  action,
				Kind:    KindUSB,
				Vendor:  "Test",
				Product: "Device",
			}

			msg := event.FormatMessage()
			if msg == "" {
				t.Error("FormatMessage() returned empty string")
			}

			// Verify kind is always included
			if !contains(msg, "Type: usb") {
				t.Error("FormatMessage() should include device type")
			}

			// Verify vendor/product is always included
			if !contains(msg, "Device: Test Device") {
				t.Error("FormatMessage() should include vendor and product")
			}
		})
	}
}

func TestDeviceEvent_AllKinds(t *testing.T) {
	kinds := []Kind{KindUSB, KindBluetooth, KindPCI, KindGeneric}

	for _, kind := range kinds {
		t.Run(string(kind), func(t *testing.T) {
			event := &DeviceEvent{
				Action:  ActionAdd,
				Kind:    kind,
				Vendor:  "Test",
				Product: "Device",
			}

			msg := event.FormatMessage()
			if msg == "" {
				t.Error("FormatMessage() returned empty string")
			}

			// Verify kind is included
			expectedType := "Type: " + string(kind)
			if !contains(msg, expectedType) {
				t.Errorf("FormatMessage() should include %s", expectedType)
			}
		})
	}
}

func TestDeviceEvent_EmptyFields(t *testing.T) {
	event := &DeviceEvent{
		Action: ActionAdd,
		Kind:   KindUSB,
		// All other fields empty
	}

	msg := event.FormatMessage()

	// Should still produce valid message with empty vendor/product
	if msg == "" {
		t.Error("FormatMessage() returned empty string")
	}

	// Should include type
	if !contains(msg, "Type: usb") {
		t.Error("FormatMessage() should include device type")
	}

	// Device line should have spaces for empty vendor/product
	if !contains(msg, "Device:  ") {
		t.Error("FormatMessage() should handle empty vendor/product")
	}
}

func TestEventSource_Lifecycle(t *testing.T) {
	mock := &MockEventSource{
		kind:      KindUSB,
		eventChan: make(chan *DeviceEvent, 5),
	}

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	// Start the source
	ch, err := mock.Start(ctx)
	if err != nil {
		t.Fatalf("Start() error = %v", err)
	}

	// Send some events
	events := []*DeviceEvent{
		{Action: ActionAdd, Kind: KindUSB, Vendor: "Vendor1", Product: "Product1"},
		{Action: ActionRemove, Kind: KindUSB, Vendor: "Vendor2", Product: "Product2"},
	}

	go func() {
		for _, event := range events {
			mock.eventChan <- event
		}
	}()

	// Receive events
	receivedCount := 0
	for {
		select {
		case <-ctx.Done():
			if receivedCount != len(events) {
				t.Errorf("Received %d events, want %d", receivedCount, len(events))
			}
			return
		case event := <-ch:
			if event != nil {
				receivedCount++
			}
		}
	}
}

func TestEventSource_ConcurrentStartStop(t *testing.T) {
	mock := &MockEventSource{
		kind:      KindUSB,
		eventChan: make(chan *DeviceEvent, 1),
	}

	ctx := context.Background()

	// Test concurrent start calls
	done := make(chan bool)
	for i := 0; i < 5; i++ {
		go func() {
			mock.Start(ctx)
			done <- true
		}()
	}

	for i := 0; i < 5; i++ {
		<-done
	}

	// Test concurrent stop calls
	for i := 0; i < 5; i++ {
		go func() {
			mock.Stop()
			done <- true
		}()
	}

	for i := 0; i < 5; i++ {
		<-done
	}
}

// Helper function
func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(substr) == 0 ||
		(len(s) > 0 && len(substr) > 0 && findSubstring(s, substr)))
}

func findSubstring(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
