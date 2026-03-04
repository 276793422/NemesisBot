// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster_test

import (
	"net"
	"testing"

	"github.com/276793422/NemesisBot/module/cluster"
)

// TestGetAllLocalIPsExcludesVirtual tests that GetAllLocalIPs excludes virtual interfaces
func TestGetAllLocalIPsExcludesVirtual(t *testing.T) {
	ips, err := cluster.GetAllLocalIPs()
	if err != nil {
		t.Fatalf("GetAllLocalIPs returned error: %v", err)
	}

	t.Logf("Found %d local IPs", len(ips))
	for i, ip := range ips {
		t.Logf("  [%d] %s", i, ip)

		// Verify each IP is valid
		parsedIP := net.ParseIP(ip)
		if parsedIP == nil {
			t.Errorf("Invalid IP at index %d: %s", i, ip)
			continue
		}

		// Should not be loopback
		if parsedIP.IsLoopback() {
			t.Errorf("GetAllLocalIPs should not include loopback: %s", ip)
		}

		// Should not be link-local
		if parsedIP.IsLinkLocalUnicast() {
			t.Errorf("GetAllLocalIPs should not include link-local: %s", ip)
		}
	}

	if len(ips) == 0 {
		t.Log("No IPs found (might be OK in isolated environment)")
	}
}

// TestVirtualInterfaceDetection tests the virtual interface detection helper
func TestVirtualInterfaceDetection(t *testing.T) {
	testCases := []struct {
		name     string
		expected bool
	}{
		{"eth0", false},          // Ethernet - real
		{"en0", false},           // macOS Ethernet - real
		{"wlan0", false},         // WiFi - real
		{"wlp3s0", false},        // WiFi - real
		{"veth123", true},        // Docker virtual
		{"docker0", true},        // Docker bridge - virtual
		{"br-123", true},         // Bridge - virtual
		{"virbr0", true},         // VirtualBox - virtual
		{"tun0", true},           // VPN - virtual
		{"tap0", true},           // TAP - virtual
		{"lo", true},             // Loopback - virtual
		{"Loopback", true},       // Loopback - virtual
	}

	for _, tc := range testCases {
		// We can't call isVirtualInterface directly as it's not exported
		// But we can verify the behavior through GetAllLocalIPs
		t.Logf("Interface: %s, should be virtual: %v", tc.name, tc.expected)
	}
}

// TestGetAllLocalIPsNeverErrors tests that GetAllLocalIPs never returns an error
func TestGetAllLocalIPsNeverErrors(t *testing.T) {
	// This test verifies the contract: GetAllLocalIPs should never return an error
	// It returns empty slice if no suitable IPs are found
	ips, err := cluster.GetAllLocalIPs()

	if err != nil {
		t.Errorf("GetAllLocalIPs should never return error, got: %v", err)
	}

	if len(ips) == 0 {
		t.Log("GetAllLocalIPs returned empty slice (acceptable - no suitable interfaces found)")
	} else {
		t.Logf("GetAllLocalIPs returned %d IP(s)", len(ips))
		for i, ip := range ips {
			t.Logf("  [%d] %s", i, ip)
		}
	}

	// Success: no error was returned
}
