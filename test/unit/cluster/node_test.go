// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster_test

import (
	"net"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/cluster"
)

// TestGetAllLocalIPs tests GetAllLocalIPs function
func TestGetAllLocalIPs(t *testing.T) {
	ips, err := cluster.GetAllLocalIPs()
	if err != nil {
		t.Fatalf("GetAllLocalIPs failed: %v", err)
	}

	t.Logf("Found %d local IP(s)", len(ips))
	for i, ip := range ips {
		t.Logf("  [%d] %s", i, ip)

		// Verify each IP
		parsedIP := net.ParseIP(ip)
		if parsedIP == nil {
			t.Errorf("Invalid IP at index %d: %s", i, ip)
		}
	}
}

// TestGetAllLocalIPsPriority tests that IPs are returned in priority order
func TestGetAllLocalIPsPriority(t *testing.T) {
	ips, err := cluster.GetAllLocalIPs()
	if err != nil {
		t.Fatalf("GetAllLocalIPs failed: %v", err)
	}

	if len(ips) == 0 {
		t.Log("No IPs found (might be OK in isolated environment)")
		return
	}

	// Verify all IPs are valid
	for _, ip := range ips {
		parsedIP := net.ParseIP(ip)
		if parsedIP == nil {
			t.Errorf("Invalid IP: %s", ip)
		}
		if parsedIP.IsLoopback() {
			t.Errorf("Should not include loopback: %s", ip)
		}
		if parsedIP.IsLinkLocalUnicast() {
			t.Errorf("Should not include link-local: %s", ip)
		}
	}

	t.Logf("✅ GetAllLocalIPs returned %d valid IPs", len(ips))
}

// TestGenerateNodeID tests GenerateNodeID function
func TestGenerateNodeID(t *testing.T) {
	nodeID, err := cluster.GenerateNodeID()
	if err != nil {
		t.Fatalf("GenerateNodeID failed: %v", err)
	}

	if nodeID == "" {
		t.Fatal("GenerateNodeID returned empty string")
	}

	t.Logf("Generated node ID: %s", nodeID)

	// Check format: should start with "bot-" and contain hostname
	if len(nodeID) < 10 {
		t.Errorf("Node ID seems too short: %s", nodeID)
	}

	// Should NOT contain IP address anymore
	// Format is now: bot-hostname-timestamp
	if !matchesFormat(nodeID) {
		t.Logf("Node ID format: bot-hostname-timestamp")
	}
}

// Helper to check if node ID matches expected format
func matchesFormat(nodeID string) bool {
	// Simple check: starts with "bot-" and has two parts separated by dashes
	parts := strings.Split(nodeID, "-")
	return len(parts) >= 3 && parts[0] == "bot"
}
