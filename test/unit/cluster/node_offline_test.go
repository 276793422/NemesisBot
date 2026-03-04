// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster_test

import (
	"net"
	"testing"

	"github.com/276793422/NemesisBot/module/cluster"
)

// TestGetAllLocalIPsInRestrictedNetwork tests GetAllLocalIPs in restricted network environments
func TestGetAllLocalIPsInRestrictedNetwork(t *testing.T) {
	// This test verifies that GetAllLocalIPs works correctly
	// even in environments with limited or no network

	ips, err := cluster.GetAllLocalIPs()
	if err != nil {
		t.Fatalf("GetAllLocalIPs should never return error, got: %v", err)
	}

	t.Logf("GetAllLocalIPs returned %d IP(s)", len(ips))

	for i, ip := range ips {
		t.Logf("  [%d] %s", i, ip)

		// Verify each IP is valid
		parsedIP := net.ParseIP(ip)
		if parsedIP == nil {
			t.Errorf("Invalid IP at index %d: %s", i, ip)
		}

		// Should not be loopback
		if parsedIP.IsLoopback() {
			t.Errorf("Should not include loopback: %s", ip)
		}

		// Should not be link-local
		if parsedIP.IsLinkLocalUnicast() {
			t.Errorf("Should not include link-local: %s", ip)
		}
	}

	if len(ips) == 0 {
		t.Log("No IPs found (might be OK in isolated environment)")
	}
}

// TestGetAllLocalIPsDoesNotPanic tests that GetAllLocalIPs handles edge cases gracefully
func TestGetAllLocalIPsDoesNotPanic(t *testing.T) {
	// This test ensures the function doesn't panic even in weird network states
	defer func() {
		if r := recover(); r != nil {
			t.Errorf("GetAllLocalIPs panicked: %v", r)
		}
	}()

	ips, err := cluster.GetAllLocalIPs()
	if err != nil {
		// It's OK to return error, just don't panic
		t.Logf("GetAllLocalIPs returned error (acceptable): %v", err)
		return
	}

	t.Logf("GetAllLocalIPs returned %d IP(s)", len(ips))
	for i, ip := range ips {
		t.Logf("  [%d] %s", i, ip)
	}

	if len(ips) == 0 {
		t.Log("GetAllLocalIPs returned empty list (might be OK in some environments)")
	}
}

// TestGenerateNodeIDWithNoNetwork tests node ID generation in limited network environments
func TestGenerateNodeIDWithNoNetwork(t *testing.T) {
	// GenerateNodeID should work regardless of network state
	nodeID, err := cluster.GenerateNodeID()
	if err != nil {
		t.Fatalf("GenerateNodeID failed: %v", err)
	}

	if nodeID == "" {
		t.Fatal("GenerateNodeID returned empty string")
	}

	// Node ID should start with "bot-"
	if len(nodeID) < 5 || nodeID[:4] != "bot-" {
		t.Errorf("Node ID has unexpected format: %s", nodeID)
	}

	t.Logf("Generated node ID: %s", nodeID)
}
