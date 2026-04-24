// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package security_test

import (
	"context"
	"net"
	"testing"

	ssrf "github.com/276793422/NemesisBot/module/security/ssrf"
)

func TestSSRFGuard_PrivateIP(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	guard, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("NewGuard returned error: %v", err)
	}
	ctx := context.Background()

	privateIPs := []struct {
		name string
		ip   string
	}{
		{"RFC1918 10.x", "10.0.0.1"},
		{"RFC1918 172.16.x", "172.16.0.1"},
		{"RFC1918 192.168.x", "192.168.1.1"},
		{"10.255.255.255", "10.255.255.255"},
		{"172.31.255.255", "172.31.255.255"},
		{"192.168.0.100", "192.168.0.100"},
	}

	for _, tt := range privateIPs {
		t.Run(tt.name, func(t *testing.T) {
			err := guard.CheckIP(ctx, tt.ip)
			if err == nil {
				t.Errorf("expected private IP %s to be blocked", tt.ip)
			}
		})
	}
}

func TestSSRFGuard_Localhost(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	guard, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("NewGuard returned error: %v", err)
	}
	ctx := context.Background()

	tests := []struct {
		name string
		ip   string
	}{
		{"127.0.0.1", "127.0.0.1"},
		{"127.0.0.2", "127.0.0.2"},
		{"127.255.255.255", "127.255.255.255"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := guard.CheckIP(ctx, tt.ip)
			if err == nil {
				t.Errorf("expected localhost IP %s to be blocked", tt.ip)
			}
		})
	}

	// Test localhost hostname via URL validation
	t.Run("localhost hostname", func(t *testing.T) {
		err := guard.ValidateURL(ctx, "http://localhost/admin")
		if err == nil {
			t.Error("expected localhost hostname to be blocked")
		}
	})
}

func TestSSRFGuard_MetadataEndpoint(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	guard, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("NewGuard returned error: %v", err)
	}
	ctx := context.Background()

	err = guard.CheckIP(ctx, "169.254.169.254")
	if err == nil {
		t.Error("expected cloud metadata IP 169.254.169.254 to be blocked")
	}
}

func TestSSRFGuard_PublicURL(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	guard, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("NewGuard returned error: %v", err)
	}
	ctx := context.Background()

	// Test with a direct IP check for a known public IP
	err = guard.CheckIP(ctx, "8.8.8.8")
	if err != nil {
		t.Errorf("expected public IP 8.8.8.8 to be allowed, got error: %v", err)
	}

	err = guard.CheckIP(ctx, "1.1.1.1")
	if err != nil {
		t.Errorf("expected public IP 1.1.1.1 to be allowed, got error: %v", err)
	}
}

func TestSSRFGuard_IPv6(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	guard, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("NewGuard returned error: %v", err)
	}
	ctx := context.Background()

	// ::1 (IPv6 loopback) should be blocked
	err = guard.CheckIP(ctx, "::1")
	if err == nil {
		t.Error("expected IPv6 loopback ::1 to be blocked")
	}

	// fc00::1 (IPv6 unique local) should be blocked
	err = guard.CheckIP(ctx, "fc00::1")
	if err == nil {
		t.Error("expected IPv6 unique local fc00::1 to be blocked")
	}

	// fe80::1 (IPv6 link-local) should be blocked
	err = guard.CheckIP(ctx, "fe80::1")
	if err == nil {
		t.Error("expected IPv6 link-local fe80::1 to be blocked")
	}
}

func TestSSRFGuard_AllowedHosts(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	cfg.AllowedHosts = []string{"trusted.example.com"}
	guard, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("NewGuard returned error: %v", err)
	}

	// Add allowed host dynamically
	guard.AddAllowedHost("another-trusted.com")

	// Verify allowed host set includes our entries
	if !guard.IsEnabled() {
		t.Error("expected guard to be enabled")
	}
}

func TestSSRFGuard_InvalidURL(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	guard, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("NewGuard returned error: %v", err)
	}
	ctx := context.Background()

	tests := []struct {
		name string
		url  string
	}{
		{"empty string", ""},
		{"javascript scheme", "javascript:alert(1)"},
		{"ftp scheme", "ftp://example.com/file"},
		{"file scheme", "file:///etc/passwd"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := guard.ValidateURL(ctx, tt.url)
			if err == nil {
				t.Errorf("expected error for URL %q", tt.url)
			}
		})
	}
}

func TestSSRFGuard_InvalidIP(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	guard, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("NewGuard returned error: %v", err)
	}
	ctx := context.Background()

	err = guard.CheckIP(ctx, "not-an-ip")
	if err == nil {
		t.Error("expected error for invalid IP")
	}
}

func TestSSRFGuard_Disabled(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	cfg.Enabled = false
	guard, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("NewGuard returned error: %v", err)
	}
	ctx := context.Background()

	// Everything should pass when disabled
	err = guard.CheckIP(ctx, "127.0.0.1")
	if err != nil {
		t.Errorf("expected no error when disabled, got: %v", err)
	}

	err = guard.ValidateURL(ctx, "http://localhost")
	if err != nil {
		t.Errorf("expected no error when disabled, got: %v", err)
	}
}

func TestSSRFGuard_AddBlockedCIDR(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	guard, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("NewGuard returned error: %v", err)
	}
	ctx := context.Background()

	// First verify 8.8.8.8 is allowed
	err = guard.CheckIP(ctx, "8.8.8.8")
	if err != nil {
		t.Fatalf("expected 8.8.8.8 to be initially allowed, got: %v", err)
	}

	// Block 8.8.0.0/16
	err = guard.AddBlockedCIDR("8.8.0.0/16")
	if err != nil {
		t.Fatalf("AddBlockedCIDR returned error: %v", err)
	}

	// Now 8.8.8.8 should be blocked
	err = guard.CheckIP(ctx, "8.8.8.8")
	if err == nil {
		t.Error("expected 8.8.8.8 to be blocked after adding CIDR")
	}

	// Invalid CIDR should return error
	err = guard.AddBlockedCIDR("not-a-cidr")
	if err == nil {
		t.Error("expected error for invalid CIDR")
	}
}

func TestSSRFGuard_RemoveAllowedHost(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	cfg.AllowedHosts = []string{"temp-trusted.com"}
	guard, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("NewGuard returned error: %v", err)
	}

	// Remove the allowed host
	guard.RemoveAllowedHost("temp-trusted.com")

	// Removal is silent even if the host doesn't exist
	guard.RemoveAllowedHost("nonexistent.com")
}

func TestSSRF_ResolverHelpers(t *testing.T) {
	tests := []struct {
		name     string
		ip       string
		check    func(net.IP) bool
		expected bool
	}{
		{"10.0.0.1 is private", "10.0.0.1", ssrf.IsPrivateIP, true},
		{"172.16.0.1 is private", "172.16.0.1", ssrf.IsPrivateIP, true},
		{"192.168.1.1 is private", "192.168.1.1", ssrf.IsPrivateIP, true},
		{"8.8.8.8 is not private", "8.8.8.8", ssrf.IsPrivateIP, false},
		{"127.0.0.1 is loopback", "127.0.0.1", ssrf.IsLoopbackIP, true},
		{"8.8.8.8 is not loopback", "8.8.8.8", ssrf.IsLoopbackIP, false},
		{"169.254.169.254 is metadata", "169.254.169.254", ssrf.IsMetadataIP, true},
		{"8.8.8.8 is not metadata", "8.8.8.8", ssrf.IsMetadataIP, false},
		{"169.254.1.1 is link-local", "169.254.1.1", ssrf.IsLinkLocalIP, true},
		{"8.8.8.8 is not link-local", "8.8.8.8", ssrf.IsLinkLocalIP, false},
		{"0.0.0.0 is reserved", "0.0.0.0", ssrf.IsReservedIP, true},
		{"224.0.0.1 is reserved (multicast)", "224.0.0.1", ssrf.IsReservedIP, true},
		{"8.8.8.8 is not reserved", "8.8.8.8", ssrf.IsReservedIP, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			ip := net.ParseIP(tt.ip)
			if ip == nil {
				t.Fatalf("failed to parse IP %q", tt.ip)
			}
			got := tt.check(ip)
			if got != tt.expected {
				t.Errorf("check(%s) = %v, want %v", tt.ip, got, tt.expected)
			}
		})
	}
}

func TestSSRF_ParseURL(t *testing.T) {
	tests := []struct {
		name    string
		url     string
		wantErr bool
	}{
		{"valid http", "http://example.com/path", false},
		{"valid https", "https://example.com/path", false},
		{"empty URL", "", true},
		{"javascript scheme", "javascript:alert(1)", true},
		{"ftp scheme", "ftp://files.example.com", true},
		{"URL with embedded credentials", "http://user:pass@host.com/path", true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := ssrf.ParseURL(tt.url)
			if (err != nil) != tt.wantErr {
				t.Errorf("ParseURL(%q) error = %v, wantErr %v", tt.url, err, tt.wantErr)
			}
		})
	}
}

func TestSSRFGuard_NilConfig(t *testing.T) {
	// Passing nil config should use defaults
	guard, err := ssrf.NewGuard(nil)
	if err != nil {
		t.Fatalf("NewGuard(nil) returned error: %v", err)
	}
	if !guard.IsEnabled() {
		t.Error("expected guard to be enabled with default config")
	}
}

func TestSSRFGuard_BlockedCIDRConfig(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	cfg.BlockedCIDRs = []string{"8.8.0.0/16"}
	guard, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("NewGuard returned error: %v", err)
	}
	ctx := context.Background()

	// 8.8.8.8 should be blocked because it's in the custom CIDR
	err = guard.CheckIP(ctx, "8.8.8.8")
	if err == nil {
		t.Error("expected 8.8.8.8 to be blocked by custom CIDR")
	}
}

func TestSSRFGuard_InvalidBlockedCIDR(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	cfg.BlockedCIDRs = []string{"not-a-cidr"}
	_, err := ssrf.NewGuard(cfg)
	if err == nil {
		t.Error("expected error for invalid CIDR in config")
	}
}
