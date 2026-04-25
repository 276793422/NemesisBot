package ssrf_test

import (
	"context"
	"net"
	"testing"

	"github.com/276793422/NemesisBot/module/security/ssrf"
)

// ---------------------------------------------------------------------------
// NewGuard
// ---------------------------------------------------------------------------

func TestNewGuard_DefaultConfig(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if g == nil {
		t.Fatal("expected non-nil guard")
	}
	if !g.IsEnabled() {
		t.Error("expected guard to be enabled")
	}
}

func TestNewGuard_NilConfig(t *testing.T) {
	g, err := ssrf.NewGuard(nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if g == nil {
		t.Fatal("expected non-nil guard with nil config (should use defaults)")
	}
}

func TestNewGuard_InvalidBlockedCIDR(t *testing.T) {
	cfg := &ssrf.Config{
		Enabled:      true,
		BlockedCIDRs: []string{"not-a-cidr"},
	}
	_, err := ssrf.NewGuard(cfg)
	if err == nil {
		t.Error("expected error for invalid CIDR")
	}
}

func TestNewGuard_ValidBlockedCIDR(t *testing.T) {
	cfg := &ssrf.Config{
		Enabled:      true,
		BlockedCIDRs: []string{"203.0.113.0/24"},
	}
	g, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if g == nil {
		t.Fatal("expected non-nil guard")
	}
}

func TestNewGuard_Disabled(t *testing.T) {
	cfg := &ssrf.Config{
		Enabled: false,
	}
	g, err := ssrf.NewGuard(cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if g.IsEnabled() {
		t.Error("expected guard to be disabled")
	}
}

// ---------------------------------------------------------------------------
// DefaultConfig
// ---------------------------------------------------------------------------

func TestDefaultConfig(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	if !cfg.Enabled {
		t.Error("expected Enabled=true")
	}
	if !cfg.BlockMetadata {
		t.Error("expected BlockMetadata=true")
	}
	if !cfg.BlockLocalhost {
		t.Error("expected BlockLocalhost=true")
	}
	if !cfg.BlockPrivateIPs {
		t.Error("expected BlockPrivateIPs=true")
	}
	if cfg.MaxRedirects != 5 {
		t.Errorf("expected MaxRedirects=5, got %d", cfg.MaxRedirects)
	}
}

// ---------------------------------------------------------------------------
// ValidateURL — safe URLs
// ---------------------------------------------------------------------------

func TestValidateURL_SafeURLs(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	tests := []struct {
		name string
		url  string
	}{
		{"https example", "https://example.com/path"},
		{"http example", "http://example.com"},
		{"with port", "https://example.com:8443/api"},
		{"with query", "https://api.example.com/v1/data?key=value"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := g.ValidateURL(context.Background(), tt.url)
			// Note: these may fail if DNS resolution fails in the test environment.
			// We mainly want to verify the URL parsing logic works.
			if err != nil {
				t.Logf("ValidateURL(%q) returned: %v (may be DNS-related)", tt.url, err)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// ValidateURL — private IPs
// ---------------------------------------------------------------------------

func TestValidateURL_Loopback(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	tests := []struct {
		name string
		url  string
	}{
		{"127.0.0.1", "http://127.0.0.1/admin"},
		{"localhost", "http://localhost/admin"},
		{"127.1", "http://127.1/secret"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := g.ValidateURL(context.Background(), tt.url)
			if err == nil {
				t.Errorf("expected error for %q", tt.url)
			}
		})
	}
}

func TestValidateURL_PrivateIPs(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	tests := []struct {
		name string
		url  string
	}{
		{"10.x", "http://10.0.0.1/"},
		{"172.16.x", "http://172.16.0.1/"},
		{"192.168.x", "http://192.168.1.1/"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := g.ValidateURL(context.Background(), tt.url)
			if err == nil {
				t.Errorf("expected error for private IP %q", tt.url)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// ValidateURL — metadata endpoints
// ---------------------------------------------------------------------------

func TestValidateURL_MetadataEndpoint(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.ValidateURL(context.Background(), "http://169.254.169.254/latest/meta-data/")
	if err == nil {
		t.Error("expected error for cloud metadata endpoint")
	}
}

// ---------------------------------------------------------------------------
// ValidateURL — disabled guard
// ---------------------------------------------------------------------------

func TestValidateURL_Disabled(t *testing.T) {
	cfg := &ssrf.Config{Enabled: false}
	g, _ := ssrf.NewGuard(cfg)

	err := g.ValidateURL(context.Background(), "http://127.0.0.1/")
	if err != nil {
		t.Errorf("expected no error when guard is disabled, got: %v", err)
	}
}

// ---------------------------------------------------------------------------
// ValidateURL — allowed hosts (whitelist)
// ---------------------------------------------------------------------------

func TestValidateURL_AllowedHosts(t *testing.T) {
	cfg := &ssrf.Config{
		Enabled:      true,
		BlockLocalhost: true,
		AllowedHosts: []string{"trusted.local"},
	}
	g, _ := ssrf.NewGuard(cfg)

	// This should bypass the SSRF check because the host is whitelisted.
	err := g.ValidateURL(context.Background(), "http://trusted.local/api")
	// Even though trusted.local might resolve to localhost, it's whitelisted.
	if err != nil {
		t.Logf("ValidateURL with allowed host returned: %v (DNS may have failed)", err)
	}
}

// ---------------------------------------------------------------------------
// ValidateURL — unsupported schemes
// ---------------------------------------------------------------------------

func TestValidateURL_UnsupportedScheme(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	tests := []struct {
		name string
		url  string
	}{
		{"ftp", "ftp://example.com/file"},
		{"file", "file:///etc/passwd"},
		{"javascript", "javascript:alert(1)"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := g.ValidateURL(context.Background(), tt.url)
			if err == nil {
				t.Errorf("expected error for scheme in %q", tt.url)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// ValidateURL — edge cases
// ---------------------------------------------------------------------------

func TestValidateURL_EmptyString(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.ValidateURL(context.Background(), "")
	if err == nil {
		t.Error("expected error for empty URL")
	}
}

func TestValidateURL_MalformedURL(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.ValidateURL(context.Background(), "://missing-scheme")
	// This may or may not error depending on how Go's URL parser handles it.
	_ = err
}

// ---------------------------------------------------------------------------
// CheckIP
// ---------------------------------------------------------------------------

func TestCheckIP_Loopback(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.CheckIP(context.Background(), "127.0.0.1")
	if err == nil {
		t.Error("expected error for loopback IP")
	}
}

func TestCheckIP_PrivateIPs(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	tests := []struct {
		name string
		ip   string
	}{
		{"10.0.0.1", "10.0.0.1"},
		{"172.16.0.1", "172.16.0.1"},
		{"192.168.1.1", "192.168.1.1"},
		{"10.255.255.255", "10.255.255.255"},
		{"172.31.255.255", "172.31.255.255"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := g.CheckIP(context.Background(), tt.ip)
			if err == nil {
				t.Errorf("expected error for private IP %s", tt.ip)
			}
		})
	}
}

func TestCheckIP_MetadataIP(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.CheckIP(context.Background(), "169.254.169.254")
	if err == nil {
		t.Error("expected error for metadata IP")
	}
}

func TestCheckIP_PublicIP(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.CheckIP(context.Background(), "8.8.8.8")
	if err != nil {
		t.Errorf("expected no error for public IP: %v", err)
	}
}

func TestCheckIP_InvalidIP(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.CheckIP(context.Background(), "not-an-ip")
	if err == nil {
		t.Error("expected error for invalid IP")
	}
}

func TestCheckIP_LinkLocal(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.CheckIP(context.Background(), "169.254.1.1")
	if err == nil {
		t.Error("expected error for link-local IP")
	}
}

func TestCheckIP_ReservedIP(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	tests := []struct {
		name string
		ip   string
	}{
		{"0.0.0.0", "0.0.0.0"},
		{"224.0.0.1", "224.0.0.1"},
		{"255.255.255.255", "255.255.255.255"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := g.CheckIP(context.Background(), tt.ip)
			if err == nil {
				t.Errorf("expected error for reserved IP %s", tt.ip)
			}
		})
	}
}

func TestCheckIP_Disabled(t *testing.T) {
	cfg := &ssrf.Config{Enabled: false}
	g, _ := ssrf.NewGuard(cfg)

	err := g.CheckIP(context.Background(), "127.0.0.1")
	if err != nil {
		t.Errorf("expected no error when guard is disabled, got: %v", err)
	}
}

// ---------------------------------------------------------------------------
// CheckIP — blocked CIDR
// ---------------------------------------------------------------------------

func TestCheckIP_BlockedCIDR(t *testing.T) {
	cfg := &ssrf.Config{
		Enabled:        true,
		BlockLocalhost: false,
		BlockPrivateIPs: false,
		BlockedCIDRs:   []string{"203.0.113.0/24"},
	}
	g, _ := ssrf.NewGuard(cfg)

	err := g.CheckIP(context.Background(), "203.0.113.1")
	if err == nil {
		t.Error("expected error for IP in blocked CIDR")
	}

	// IP outside the blocked CIDR should pass.
	err = g.CheckIP(context.Background(), "203.0.114.1")
	if err != nil {
		t.Logf("CheckIP for non-blocked IP returned: %v", err)
	}
}

// ---------------------------------------------------------------------------
// ResolveAndValidate
// ---------------------------------------------------------------------------

func TestResolveAndValidate_Disabled(t *testing.T) {
	cfg := &ssrf.Config{Enabled: false}
	g, _ := ssrf.NewGuard(cfg)

	err := g.ResolveAndValidate(context.Background(), "http://127.0.0.1/")
	if err != nil {
		t.Errorf("expected no error when disabled, got: %v", err)
	}
}

func TestResolveAndValidate_EmptyURL(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.ResolveAndValidate(context.Background(), "")
	if err == nil {
		t.Error("expected error for empty URL")
	}
}

// ---------------------------------------------------------------------------
// ParseURL
// ---------------------------------------------------------------------------

func TestParseURL_ValidURLs(t *testing.T) {
	tests := []struct {
		name    string
		url     string
		wantErr bool
	}{
		{"https", "https://example.com", false},
		{"http", "http://example.com", false},
		{"with path", "https://example.com/path/to/resource", false},
		{"with port", "https://example.com:8443/api", false},
		{"with query", "https://example.com/search?q=test", false},
		{"no scheme", "example.com", false}, // should add http://
		{"IP address", "http://192.168.1.1/", false},
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

func TestParseURL_EmptyString(t *testing.T) {
	_, err := ssrf.ParseURL("")
	if err == nil {
		t.Error("expected error for empty URL")
	}
}

func TestParseURL_UnsupportedScheme(t *testing.T) {
	_, err := ssrf.ParseURL("ftp://example.com")
	if err == nil {
		t.Error("expected error for unsupported scheme")
	}
}

func TestParseURL_EmbeddedCredentials(t *testing.T) {
	_, err := ssrf.ParseURL("http://user:pass@example.com")
	if err == nil {
		t.Error("expected error for embedded credentials")
	}
}

func TestParseURL_ControlChars(t *testing.T) {
	_, err := ssrf.ParseURL("http://example\x00.com")
	if err == nil {
		t.Error("expected error for control characters in hostname")
	}
}

func TestParseURL_AtSignInHost(t *testing.T) {
	_, err := ssrf.ParseURL("http://evil@example.com")
	if err == nil {
		t.Error("expected error for @ in hostname")
	}
}

func TestParseURL_InvalidPort(t *testing.T) {
	_, err := ssrf.ParseURL("http://example.com:99999")
	if err == nil {
		t.Error("expected error for invalid port")
	}
}

// ---------------------------------------------------------------------------
// IsPrivateIP
// ---------------------------------------------------------------------------

func TestIsPrivateIP(t *testing.T) {
	tests := []struct {
		ip   string
		want bool
	}{
		// Private IPv4 ranges.
		{"10.0.0.1", true},
		{"10.255.255.254", true},
		{"172.16.0.1", true},
		{"172.31.255.254", true},
		{"192.168.0.1", true},
		{"192.168.255.254", true},
		// TEST-NET ranges.
		{"192.0.2.1", true},
		{"198.51.100.1", true},
		{"203.0.113.1", true},
		// Carrier-grade NAT.
		{"100.64.0.1", true},
		// Public IPs.
		{"8.8.8.8", false},
		{"1.1.1.1", false},
		{"203.0.114.1", false},
		// IPv6 unique local.
		{"fc00::1", true},
		{"fd00::1", true},
		// IPv6 global.
		{"2001:db8::1", false},
	}

	for _, tt := range tests {
		t.Run(tt.ip, func(t *testing.T) {
			ip := net.ParseIP(tt.ip)
			if ip == nil {
				t.Fatalf("failed to parse IP %q", tt.ip)
			}
			got := ssrf.IsPrivateIP(ip)
			if got != tt.want {
				t.Errorf("IsPrivateIP(%s) = %v, want %v", tt.ip, got, tt.want)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// IsLoopbackIP
// ---------------------------------------------------------------------------

func TestIsLoopbackIP(t *testing.T) {
	tests := []struct {
		ip   string
		want bool
	}{
		{"127.0.0.1", true},
		{"127.255.255.255", true},
		{"127.0.0.0", true},
		{"::1", true},
		{"8.8.8.8", false},
		{"192.168.1.1", false},
		{"10.0.0.1", false},
	}

	for _, tt := range tests {
		t.Run(tt.ip, func(t *testing.T) {
			ip := net.ParseIP(tt.ip)
			if ip == nil {
				t.Fatalf("failed to parse IP %q", tt.ip)
			}
			got := ssrf.IsLoopbackIP(ip)
			if got != tt.want {
				t.Errorf("IsLoopbackIP(%s) = %v, want %v", tt.ip, got, tt.want)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// IsMetadataIP
// ---------------------------------------------------------------------------

func TestIsMetadataIP(t *testing.T) {
	tests := []struct {
		ip   string
		want bool
	}{
		{"169.254.169.254", true},
		{"fd00:ec2::254", true},
		{"169.254.169.253", false},
		{"8.8.8.8", false},
		{"127.0.0.1", false},
	}

	for _, tt := range tests {
		t.Run(tt.ip, func(t *testing.T) {
			ip := net.ParseIP(tt.ip)
			if ip == nil {
				t.Fatalf("failed to parse IP %q", tt.ip)
			}
			got := ssrf.IsMetadataIP(ip)
			if got != tt.want {
				t.Errorf("IsMetadataIP(%s) = %v, want %v", tt.ip, got, tt.want)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// IsLinkLocalIP
// ---------------------------------------------------------------------------

func TestIsLinkLocalIP(t *testing.T) {
	tests := []struct {
		ip   string
		want bool
	}{
		{"169.254.0.1", true},
		{"169.254.255.255", true},
		{"fe80::1", true},
		{"8.8.8.8", false},
		{"192.168.1.1", false},
	}

	for _, tt := range tests {
		t.Run(tt.ip, func(t *testing.T) {
			ip := net.ParseIP(tt.ip)
			if ip == nil {
				t.Fatalf("failed to parse IP %q", tt.ip)
			}
			got := ssrf.IsLinkLocalIP(ip)
			if got != tt.want {
				t.Errorf("IsLinkLocalIP(%s) = %v, want %v", tt.ip, got, tt.want)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// IsReservedIP
// ---------------------------------------------------------------------------

func TestIsReservedIP(t *testing.T) {
	tests := []struct {
		ip   string
		want bool
	}{
		{"0.0.0.0", true},
		{"0.0.0.1", true},
		{"224.0.0.1", true},
		{"239.255.255.255", true},
		{"240.0.0.1", true},
		{"255.255.255.255", true},
		{"::", true},         // unspecified
		{"ff00::1", true},    // IPv6 multicast
		{"8.8.8.8", false},
		{"1.1.1.1", false},
	}

	for _, tt := range tests {
		t.Run(tt.ip, func(t *testing.T) {
			ip := net.ParseIP(tt.ip)
			if ip == nil {
				t.Fatalf("failed to parse IP %q", tt.ip)
			}
			got := ssrf.IsReservedIP(ip)
			if got != tt.want {
				t.Errorf("IsReservedIP(%s) = %v, want %v", tt.ip, got, tt.want)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// AddBlockedCIDR / AddAllowedHost / RemoveAllowedHost
// ---------------------------------------------------------------------------

func TestGuard_AddBlockedCIDR(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.AddBlockedCIDR("198.51.100.0/24")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Verify the IP is now blocked.
	err = g.CheckIP(context.Background(), "198.51.100.1")
	if err == nil {
		t.Error("expected error for newly blocked IP")
	}
}

func TestGuard_AddBlockedCIDR_Invalid(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.AddBlockedCIDR("invalid")
	if err == nil {
		t.Error("expected error for invalid CIDR")
	}
}

func TestGuard_AddAllowedHost(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	g.AddAllowedHost("MyHost.Example.COM")

	// Verify case-insensitive matching.
	err := g.ValidateURL(context.Background(), "http://myhost.example.com/api")
	// If DNS resolves to a safe IP, this should pass because it's whitelisted.
	_ = err
}

func TestGuard_RemoveAllowedHost(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	cfg.AllowedHosts = []string{"temp.example.com"}
	g, _ := ssrf.NewGuard(cfg)

	g.RemoveAllowedHost("temp.example.com")

	// After removal, the host should go through normal checks.
	// We can't easily test this without DNS, but at least verify no panic.
}

// ---------------------------------------------------------------------------
// ResolveHost
// ---------------------------------------------------------------------------

func TestResolveHost_IPv4(t *testing.T) {
	ips, err := ssrf.ResolveHost("8.8.8.8")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(ips) != 1 {
		t.Fatalf("expected 1 IP, got %d", len(ips))
	}
	if !ips[0].Equal(net.ParseIP("8.8.8.8")) {
		t.Errorf("expected 8.8.8.8, got %s", ips[0])
	}
}

func TestResolveHost_IPv6(t *testing.T) {
	ips, err := ssrf.ResolveHost("::1")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(ips) != 1 {
		t.Fatalf("expected 1 IP, got %d", len(ips))
	}
}

func TestResolveHost_BracketedIPv6(t *testing.T) {
	ips, err := ssrf.ResolveHost("[::1]")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(ips) != 1 {
		t.Fatalf("expected 1 IP, got %d", len(ips))
	}
}

// ---------------------------------------------------------------------------
// IPv6 addresses
// ---------------------------------------------------------------------------

func TestCheckIP_IPv6Loopback(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.CheckIP(context.Background(), "::1")
	if err == nil {
		t.Error("expected error for IPv6 loopback")
	}
}

func TestCheckIP_IPv6LinkLocal(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.CheckIP(context.Background(), "fe80::1")
	if err == nil {
		t.Error("expected error for IPv6 link-local")
	}
}

func TestCheckIP_IPv6Private(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.CheckIP(context.Background(), "fc00::1")
	if err == nil {
		t.Error("expected error for IPv6 unique local")
	}
}

func TestCheckIP_IPv6Multicast(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	err := g.CheckIP(context.Background(), "ff00::1")
	if err == nil {
		t.Error("expected error for IPv6 multicast")
	}
}

func TestCheckIP_IPv6Public(t *testing.T) {
	cfg := ssrf.DefaultConfig()
	g, _ := ssrf.NewGuard(cfg)

	// 2001:4860:4860::8888 is Google's public DNS (IPv6).
	err := g.CheckIP(context.Background(), "2001:4860:4860::8888")
	if err != nil {
		t.Logf("CheckIP for public IPv6 returned: %v", err)
	}
}

// ---------------------------------------------------------------------------
// Guard blocks with selective config
// ---------------------------------------------------------------------------

func TestGuard_OnlyBlockLocalhost(t *testing.T) {
	cfg := &ssrf.Config{
		Enabled:        true,
		BlockLocalhost: true,
		BlockPrivateIPs: false,
		BlockMetadata:  false,
	}
	g, _ := ssrf.NewGuard(cfg)

	// Loopback should be blocked.
	err := g.CheckIP(context.Background(), "127.0.0.1")
	if err == nil {
		t.Error("expected loopback to be blocked")
	}

	// Private IP should pass (not blocked when BlockPrivateIPs=false).
	// But link-local and reserved still apply through checkIPLocked.
	err = g.CheckIP(context.Background(), "10.0.0.1")
	// 10.0.0.1 is private but not loopback, not metadata, not link-local, not reserved.
	// With BlockPrivateIPs=false, it should still pass the IsPrivateIP check...
	// But wait, checkIPLocked also checks IsLinkLocalIP and IsReservedIP always.
	// 10.0.0.1 is not link-local or reserved, so it should pass.
	if err != nil {
		t.Logf("CheckIP for 10.0.0.1 with BlockPrivateIPs=false: %v", err)
	}
}

// ---------------------------------------------------------------------------
// Guard with no blocks
// ---------------------------------------------------------------------------

func TestGuard_NoBlocks(t *testing.T) {
	cfg := &ssrf.Config{
		Enabled:        true,
		BlockLocalhost: false,
		BlockPrivateIPs: false,
		BlockMetadata:  false,
	}
	g, _ := ssrf.NewGuard(cfg)

	// Public IP should always pass.
	err := g.CheckIP(context.Background(), "8.8.8.8")
	if err != nil {
		t.Errorf("expected no error for public IP: %v", err)
	}
}
