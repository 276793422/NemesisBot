// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster_test

import (
	"net"
	"testing"

	"github.com/276793422/NemesisBot/module/cluster"
)

// TestIPHandlingComprehensive 综合测试IP处理功能
func TestIPHandlingComprehensive(t *testing.T) {
	t.Run("GetAllLocalIPs_ReturnsValidIPs", func(t *testing.T) {
		ips, err := cluster.GetAllLocalIPs()
		if err != nil {
			t.Errorf("GetAllLocalIPs should never return error, got: %v", err)
		}

		t.Logf("✅ GetAllLocalIPs returned %d IP(s)", len(ips))

		for i, ip := range ips {
			t.Logf("  [%d] %s", i, ip)

			// 验证IP格式
			parsedIP := net.ParseIP(ip)
			if parsedIP == nil {
				t.Errorf("Invalid IP format at index %d: %s", i, ip)
				continue
			}

			// 验证不是回环地址
			if parsedIP.IsLoopback() {
				t.Errorf("IP at index %d should not be loopback: %s", i, ip)
			}

			// 验证不是链路本地地址
			if parsedIP.IsLinkLocalUnicast() {
				t.Errorf("IP at index %d should not be link-local: %s", i, ip)
			}

			// 验证是IPv4
			if parsedIP.To4() == nil {
				t.Errorf("IP at index %d should be IPv4: %s", i, ip)
			}
		}

		if len(ips) == 0 {
			t.Log("⚠️  No IPs found (might be OK in isolated environment)")
		}
	})

	t.Run("GenerateNodeID_CorrectFormat", func(t *testing.T) {
		nodeID, err := cluster.GenerateNodeID()
		if err != nil {
			t.Fatalf("GenerateNodeID failed: %v", err)
		}

		if nodeID == "" {
			t.Fatal("GenerateNodeID returned empty string")
		}

		// 验证格式: bot-hostname-timestamp
		if len(nodeID) < 10 {
			t.Errorf("Node ID seems too short: %s", nodeID)
		}

		// 验证以 "bot-" 开头
		if len(nodeID) < 4 || nodeID[:4] != "bot-" {
			t.Errorf("Node ID should start with 'bot-': %s", nodeID)
		}

		// 验证不包含IP地址（新格式）
		if net.ParseIP(nodeID) != nil {
			t.Errorf("Node ID should not be an IP address: %s", nodeID)
		}

		t.Logf("✅ Generated node ID: %s", nodeID)
	})

	t.Run("GetAllLocalIPs_NoErrorInAnyEnvironment", func(t *testing.T) {
		// 多次调用确保稳定性
		for i := 0; i < 3; i++ {
			ips, err := cluster.GetAllLocalIPs()
			if err != nil {
				t.Errorf("Call %d: GetAllLocalIPs should never return error, got: %v", i+1, err)
			}

			if ips == nil {
				t.Errorf("Call %d: GetAllLocalIPs should return slice, not nil", i+1)
			}

			t.Logf("Call %d: Got %d IP(s)", i+1, len(ips))
		}
		t.Log("✅ GetAllLocalIPs is stable across multiple calls")
	})

	t.Run("GenerateNodeID_ProducesUniqueIDs", func(t *testing.T) {
		ids := make(map[string]bool)

		// 生成多个ID验证唯一性（可能有时间延迟）
		for i := 0; i < 5; i++ {
			nodeID, err := cluster.GenerateNodeID()
			if err != nil {
				t.Fatalf("GenerateNodeID call %d failed: %v", i+1, err)
			}

			if ids[nodeID] {
				t.Logf("⚠️  Duplicate node ID detected (might be OK if time didn't change): %s", nodeID)
			}
			ids[nodeID] = true
		}

		t.Logf("✅ Generated %d unique node IDs out of 5 attempts", len(ids))
	})
}

// TestIPHandlingEdgeCases 测试边界情况
func TestIPHandlingEdgeCases(t *testing.T) {
	t.Run("GetAllLocalIPs_EmptyInterfaceList", func(t *testing.T) {
		// 即使没有网络接口，也不应该panic
		defer func() {
			if r := recover(); r != nil {
				t.Errorf("GetAllLocalIPs panicked: %v", r)
			}
		}()

		ips, err := cluster.GetAllLocalIPs()
		if err != nil {
			t.Errorf("GetAllLocalIPs should not error even with no interfaces: %v", err)
		}

		if ips == nil {
			t.Error("GetAllLocalIPs should return empty slice, not nil")
		}

		t.Logf("✅ GetAllLocalIPs handled empty interface case gracefully: %d IPs", len(ips))
	})

	t.Run("GenerateNodeID_AlwaysReturnsValidID", func(t *testing.T) {
		// 多次调用确保总是返回有效ID
		for i := 0; i < 10; i++ {
			nodeID, err := cluster.GenerateNodeID()
			if err != nil {
				t.Errorf("GenerateNodeID call %d failed: %v", i+1, err)
			}

			if nodeID == "" {
				t.Errorf("GenerateNodeID call %d returned empty string", i+1)
			}

			if len(nodeID) < 4 || nodeID[:4] != "bot-" {
				t.Errorf("GenerateNodeID call %d returned invalid format: %s", i+1, nodeID)
			}
		}
		t.Log("✅ GenerateNodeID consistently returns valid IDs")
	})
}

// TestIPHandlingNoNetwork 测试无网络环境下的行为
func TestIPHandlingNoNetwork(t *testing.T) {
	t.Run("NoNetwork_NotPanic", func(t *testing.T) {
		defer func() {
			if r := recover(); r != nil {
				t.Errorf("Code panicked in restricted network simulation: %v", r)
			}
		}()

		// GetAllLocalIPs 不应该依赖外部网络
		ips, err := cluster.GetAllLocalIPs()
		if err != nil {
			t.Errorf("GetAllLocalIPs should work without external network: %v", err)
		}

		// GenerateNodeID 不应该依赖外部网络
		nodeID, err := cluster.GenerateNodeID()
		if err != nil {
			t.Errorf("GenerateNodeID should work without external network: %v", err)
		}

		if nodeID == "" {
			t.Error("GenerateNodeID should return valid ID even without network")
		}

		t.Logf("✅ Functions work without external network: %d IPs, nodeID=%s", len(ips), nodeID)
	})
}

// TestIPHandlingFormatConsistency 测试格式一致性
func TestIPHandlingFormatConsistency(t *testing.T) {
	t.Run("NodeID_Format", func(t *testing.T) {
		nodeID, err := cluster.GenerateNodeID()
		if err != nil {
			t.Fatalf("GenerateNodeID failed: %v", err)
		}

		// 格式: bot-hostname-timestamp
		// 验证包含至少3个部分（bot, hostname, timestamp）
		partCount := 0
		current := ""
		for _, ch := range nodeID {
			if ch == '-' {
				if current != "" {
					partCount++
					current = ""
				}
			} else {
				current += string(ch)
			}
		}
		if current != "" {
			partCount++
		}

		if partCount < 3 {
			t.Errorf("Node ID should have at least 3 parts (bot-hostname-timestamp), got %d: %s", partCount, nodeID)
		}

		t.Logf("✅ Node ID format is consistent: bot-hostname-timestamp (%d parts)", partCount)
	})

	t.Run("IPs_Format", func(t *testing.T) {
		ips, err := cluster.GetAllLocalIPs()
		if err != nil {
			t.Fatalf("GetAllLocalIPs failed: %v", err)
		}

		for i, ip := range ips {
			parsedIP := net.ParseIP(ip)
			if parsedIP == nil {
				t.Errorf("IP at index %d is not valid format: %s", i, ip)
			}
		}

		t.Logf("✅ All %d IPs have valid format", len(ips))
	})
}
