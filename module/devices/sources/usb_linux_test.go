//go:build linux

// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package sources

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/devices/events"
)

func TestParseUSBEvent_ValidAddEvent(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM":       "usb",
		"DEVTYPE":         "usb_device",
		"ACTION":          "add",
		"ID_VENDOR":       "TestVendor",
		"ID_MODEL":        "TestProduct",
		"ID_SERIAL_SHORT": "SN12345",
		"BUSNUM":          "1",
		"DEVNUM":          "2",
		"ID_USB_CLASS":    "03",
		"DEVPATH":         "/devices/pci0000:00/0000:00:14.0/usb1/1-2",
	}

	ev := parseUSBEvent("add", props)
	if ev == nil {
		t.Fatal("parseUSBEvent() returned nil for valid event")
	}

	if ev.Action != events.ActionAdd {
		t.Errorf("Action = %v, want %v", ev.Action, events.ActionAdd)
	}
	if ev.Kind != events.KindUSB {
		t.Errorf("Kind = %v, want %v", ev.Kind, events.KindUSB)
	}
	if ev.Vendor != "TestVendor" {
		t.Errorf("Vendor = %v, want TestVendor", ev.Vendor)
	}
	if ev.Product != "TestProduct" {
		t.Errorf("Product = %v, want TestProduct", ev.Product)
	}
	if ev.Serial != "SN12345" {
		t.Errorf("Serial = %v, want SN12345", ev.Serial)
	}
	if ev.DeviceID != "1:2" {
		t.Errorf("DeviceID = %v, want 1:2", ev.DeviceID)
	}
	if ev.Capabilities != "HID (Keyboard/Mouse/Gamepad)" {
		t.Errorf("Capabilities = %v, want HID", ev.Capabilities)
	}
}

func TestParseUSBEvent_ValidRemoveEvent(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM": "usb",
		"DEVTYPE":   "usb_device",
		"ACTION":    "remove",
		"ID_VENDOR": "VendorX",
		"ID_MODEL":  "ProductY",
		"BUSNUM":    "2",
		"DEVNUM":    "3",
	}

	ev := parseUSBEvent("remove", props)
	if ev == nil {
		t.Fatal("parseUSBEvent() returned nil for valid event")
	}

	if ev.Action != events.ActionRemove {
		t.Errorf("Action = %v, want %v", ev.Action, events.ActionRemove)
	}
	if ev.Vendor != "VendorX" {
		t.Errorf("Vendor = %v, want VendorX", ev.Vendor)
	}
}

func TestParseUSBEvent_SkipInterfaceEvents(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM": "usb",
		"DEVTYPE":   "usb_interface", // Should be skipped
		"ACTION":    "add",
		"ID_VENDOR": "TestVendor",
		"ID_MODEL":  "TestProduct",
	}

	ev := parseUSBEvent("add", props)
	if ev != nil {
		t.Error("parseUSBEvent() should return nil for interface events")
	}
}

func TestParseUSBEvent_SkipOtherSubsystems(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM": "pci", // Not USB
		"DEVTYPE":   "usb_device",
		"ACTION":    "add",
	}

	ev := parseUSBEvent("add", props)
	if ev != nil {
		t.Error("parseUSBEvent() should return nil for non-USB subsystems")
	}
}

func TestParseUSBEvent_SkipInvalidDevType(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM": "usb",
		"DEVTYPE":   "invalid_type", // Should be skipped
		"ACTION":    "add",
	}

	ev := parseUSBEvent("add", props)
	if ev != nil {
		t.Error("parseUSBEvent() should return nil for invalid DEVTYPE")
	}
}

func TestParseUSBEvent_NoDevType(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM": "usb",
		// No DEVTYPE - should be accepted
		"ACTION":    "add",
		"ID_VENDOR": "TestVendor",
		"ID_MODEL":  "TestProduct",
	}

	ev := parseUSBEvent("add", props)
	if ev == nil {
		t.Error("parseUSBEvent() should accept events without DEVTYPE")
	}
	if ev.Vendor != "TestVendor" {
		t.Errorf("Vendor = %v, want TestVendor", ev.Vendor)
	}
}

func TestParseUSBEvent_UnknownVendor(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM": "usb",
		"DEVTYPE":   "usb_device",
		"ACTION":    "add",
		// No ID_VENDOR or ID_VENDOR_ID
	}

	ev := parseUSBEvent("add", props)
	if ev == nil {
		t.Fatal("parseUSBEvent() returned nil")
	}
	if ev.Vendor != "Unknown Vendor" {
		t.Errorf("Vendor = %v, want 'Unknown Vendor'", ev.Vendor)
	}
}

func TestParseUSBEvent_VendorIDFallback(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM":    "usb",
		"DEVTYPE":      "usb_device",
		"ACTION":       "add",
		"ID_VENDOR_ID": "1234",
		// No ID_VENDOR
	}

	ev := parseUSBEvent("add", props)
	if ev == nil {
		t.Fatal("parseUSBEvent() returned nil")
	}
	if ev.Vendor != "1234" {
		t.Errorf("Vendor = %v, want '1234'", ev.Vendor)
	}
}

func TestParseUSBEvent_UnknownProduct(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM": "usb",
		"DEVTYPE":   "usb_device",
		"ACTION":    "add",
		"ID_VENDOR": "TestVendor",
		// No ID_MODEL or ID_MODEL_ID
	}

	ev := parseUSBEvent("add", props)
	if ev == nil {
		t.Fatal("parseUSBEvent() returned nil")
	}
	if ev.Product != "Unknown Device" {
		t.Errorf("Product = %v, want 'Unknown Device'", ev.Product)
	}
}

func TestParseUSBEvent_ProductIDFallback(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM":   "usb",
		"DEVTYPE":     "usb_device",
		"ACTION":      "add",
		"ID_VENDOR":   "TestVendor",
		"ID_MODEL_ID": "5678",
		// No ID_MODEL
	}

	ev := parseUSBEvent("add", props)
	if ev == nil {
		t.Fatal("parseUSBEvent() returned nil")
	}
	if ev.Product != "5678" {
		t.Errorf("Product = %v, want '5678'", ev.Product)
	}
}

func TestParseUSBEvent_InvalidAction(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM": "usb",
		"DEVTYPE":   "usb_device",
		"ACTION":    "change", // Invalid action
		"ID_VENDOR": "TestVendor",
	}

	ev := parseUSBEvent("change", props)
	if ev != nil {
		t.Error("parseUSBEvent() should return nil for invalid actions")
	}
}

func TestParseUSBEvent_DeviceIDPriority(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM": "usb",
		"DEVTYPE":   "usb_device",
		"ACTION":    "add",
		"ID_VENDOR": "TestVendor",
		"ID_MODEL":  "TestProduct",
		"BUSNUM":    "1",
		"DEVNUM":    "2",
		"DEVPATH":   "/devices/usb1/1-2",
	}

	ev := parseUSBEvent("add", props)
	if ev == nil {
		t.Fatal("parseUSBEvent() returned nil")
	}
	// BUSNUM:DEVNUM should take priority over DEVPATH
	if ev.DeviceID != "1:2" {
		t.Errorf("DeviceID = %v, want '1:2'", ev.DeviceID)
	}
}

func TestParseUSBEvent_NoBusDev(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM": "usb",
		"DEVTYPE":   "usb_device",
		"ACTION":    "add",
		"ID_VENDOR": "TestVendor",
		"ID_MODEL":  "TestProduct",
		"DEVPATH":   "/devices/usb1/1-2",
		// No BUSNUM or DEVNUM
	}

	ev := parseUSBEvent("add", props)
	if ev == nil {
		t.Fatal("parseUSBEvent() returned nil")
	}
	// Should fall back to DEVPATH
	if ev.DeviceID != "/devices/usb1/1-2" {
		t.Errorf("DeviceID = %v, want DEVPATH", ev.DeviceID)
	}
}

func TestParseUSBEvent_AllUSBClasses(t *testing.T) {
	classes := map[string]string{
		"00": "Interface Definition (by interface)",
		"01": "Audio",
		"02": "CDC Communication (Network Card/Modem)",
		"03": "HID (Keyboard/Mouse/Gamepad)",
		"05": "Physical Interface",
		"06": "Image (Scanner/Camera)",
		"07": "Printer",
		"08": "Mass Storage (USB Flash Drive/Hard Disk)",
		"09": "USB Hub",
		"0a": "CDC Data",
		"0b": "Smart Card",
		"0e": "Video (Camera)",
		"dc": "Diagnostic Device",
		"e0": "Wireless Controller (Bluetooth)",
		"ef": "Miscellaneous",
		"fe": "Application Specific",
		"ff": "Vendor Specific",
	}

	for class, expectedCap := range classes {
		t.Run(class, func(t *testing.T) {
			props := map[string]string{
				"SUBSYSTEM":    "usb",
				"DEVTYPE":      "usb_device",
				"ACTION":       "add",
				"ID_VENDOR":    "TestVendor",
				"ID_MODEL":     "TestProduct",
				"ID_USB_CLASS": class,
			}

			ev := parseUSBEvent("add", props)
			if ev == nil {
				t.Fatal("parseUSBEvent() returned nil")
			}
			if ev.Capabilities != expectedCap {
				t.Errorf("Capabilities = %v, want %v", ev.Capabilities, expectedCap)
			}
		})
	}
}

func TestParseUSBEvent_UnknownUSBClass(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM":    "usb",
		"DEVTYPE":      "usb_device",
		"ACTION":       "add",
		"ID_VENDOR":    "TestVendor",
		"ID_MODEL":     "TestProduct",
		"ID_USB_CLASS": "99", // Unknown class
	}

	ev := parseUSBEvent("add", props)
	if ev == nil {
		t.Fatal("parseUSBEvent() returned nil")
	}
	if ev.Capabilities != "USB Device" {
		t.Errorf("Capabilities = %v, want 'USB Device'", ev.Capabilities)
	}
}

func TestParseUSBEvent_NoUSBClass(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM": "usb",
		"DEVTYPE":   "usb_device",
		"ACTION":    "add",
		"ID_VENDOR": "TestVendor",
		"ID_MODEL":  "TestProduct",
		// No ID_USB_CLASS
	}

	ev := parseUSBEvent("add", props)
	if ev == nil {
		t.Fatal("parseUSBEvent() returned nil")
	}
	if ev.Capabilities != "USB Device" {
		t.Errorf("Capabilities = %v, want 'USB Device'", ev.Capabilities)
	}
}

func TestParseUSBEvent_RawProperties(t *testing.T) {
	props := map[string]string{
		"SUBSYSTEM":       "usb",
		"DEVTYPE":         "usb_device",
		"ACTION":          "add",
		"ID_VENDOR":       "TestVendor",
		"ID_MODEL":        "TestProduct",
		"BUSNUM":          "1",
		"DEVNUM":          "2",
		"CUSTOM_PROPERTY": "custom_value",
	}

	ev := parseUSBEvent("add", props)
	if ev == nil {
		t.Fatal("parseUSBEvent() returned nil")
	}
	if ev.Raw == nil {
		t.Fatal("Raw properties should not be nil")
	}
	if ev.Raw["CUSTOM_PROPERTY"] != "custom_value" {
		t.Errorf("Raw properties not preserved")
	}
}

func TestUSBMonitor_ConcurrentStartStop(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping concurrent test in short mode")
	}

	monitor := NewUSBMonitor()
	ctx := context.Background()

	done := make(chan bool)

	// Try multiple concurrent starts/stops
	for i := 0; i < 3; i++ {
		go func() {
			ch, err := monitor.Start(ctx)
			if err != nil {
				t.Logf("Start() error (may be expected if udevadm not available): %v", err)
			}
			if ch != nil {
				// Let it run briefly
				time.Sleep(10 * time.Millisecond)
			}
			done <- true
		}()
	}

	for i := 0; i < 3; i++ {
		go func() {
			monitor.Stop()
			done <- true
		}()
	}

	// Wait for completion
	for i := 0; i < 6; i++ {
		<-done
	}

	// Final cleanup
	monitor.Stop()
}

func TestUSBMonitor_Integration(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping integration test in short mode")
	}

	// This test requires udevadm to be installed
	monitor := NewUSBMonitor()
	ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()

	ch, err := monitor.Start(ctx)
	if err != nil {
		// udevadm might not be available - that's OK for tests
		t.Skipf("udevadm not available: %v", err)
	}
	if ch == nil {
		t.Fatal("Start() returned nil channel")
	}

	eventCount := 0
	done := make(chan bool)

	go func() {
		for range ch {
			eventCount++
		}
		done <- true
	}()

	<-ctx.Done()
	monitor.Stop()
	<-done

	// We don't expect actual USB events during the test,
	// but the monitor should run without errors
	t.Logf("Received %d USB events", eventCount)
}

func BenchmarkParseUSBEvent(b *testing.B) {
	props := map[string]string{
		"SUBSYSTEM":       "usb",
		"DEVTYPE":         "usb_device",
		"ACTION":          "add",
		"ID_VENDOR":       "TestVendor",
		"ID_MODEL":        "TestProduct",
		"ID_SERIAL_SHORT": "SN12345",
		"BUSNUM":          "1",
		"DEVNUM":          "2",
		"ID_USB_CLASS":    "03",
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		parseUSBEvent("add", props)
	}
}
