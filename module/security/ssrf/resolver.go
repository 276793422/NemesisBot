// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package ssrf provides URL parsing and DNS resolution utilities for SSRF protection

package ssrf

import (
	"fmt"
	"net"
	"net/url"
	"strings"
)

// ParseURL parses and validates a URL string.
// Returns an error if the URL is malformed or uses a dangerous scheme.
func ParseURL(rawURL string) (*url.URL, error) {
	if rawURL == "" {
		return nil, fmt.Errorf("empty URL")
	}

	// Add scheme if missing to help parsing
	toParse := rawURL
	if !strings.Contains(rawURL, "://") {
		toParse = "http://" + rawURL
	}

	parsed, err := url.Parse(toParse)
	if err != nil {
		return nil, fmt.Errorf("failed to parse URL: %w", err)
	}

	// Validate host is present
	if parsed.Hostname() == "" {
		return nil, fmt.Errorf("URL has no host")
	}

	// Validate scheme (only http/https after normalization)
	scheme := strings.ToLower(parsed.Scheme)
	if scheme != "http" && scheme != "https" {
		return nil, fmt.Errorf("unsupported scheme %q, only http and https are allowed", scheme)
	}

	// Block URLs with credentials embedded (user:pass@host)
	if parsed.User != nil {
		return nil, fmt.Errorf("URLs with embedded credentials are not allowed")
	}

	// Check for suspicious fragment or query manipulation
	host := parsed.Hostname()

	// Reject hosts with control characters
	if containsControlChars(host) {
		return nil, fmt.Errorf("hostname contains control characters")
	}

	// Reject hosts with @ sign (could indicate credential injection)
	if strings.Contains(host, "@") {
		return nil, fmt.Errorf("invalid hostname")
	}

	// Reject bracket-only or empty host
	if host == "[" || host == "]" || host == "[]" {
		return nil, fmt.Errorf("invalid hostname")
	}

	// Validate port if present
	portStr := parsed.Port()
	if portStr != "" {
		port := 0
		if _, err := fmt.Sscanf(portStr, "%d", &port); err != nil || port < 0 || port > 65535 {
			return nil, fmt.Errorf("invalid port %q", portStr)
		}
	}

	return parsed, nil
}

// ResolveHost performs DNS resolution on a hostname and returns all IP addresses.
// The host can be a hostname or an IP address (returned as-is).
func ResolveHost(host string) ([]net.IP, error) {
	// Strip bracket notation for IPv6 URLs: [::1] -> ::1
	cleanHost := host
	if strings.HasPrefix(cleanHost, "[") && strings.HasSuffix(cleanHost, "]") {
		cleanHost = cleanHost[1 : len(cleanHost)-1]
	}

	// If it's already an IP, return it directly
	if ip := net.ParseIP(cleanHost); ip != nil {
		return []net.IP{ip}, nil
	}

	// DNS lookup
	ips, err := net.LookupIP(cleanHost)
	if err != nil {
		return nil, fmt.Errorf("DNS lookup failed for %q: %w", cleanHost, err)
	}

	if len(ips) == 0 {
		return nil, fmt.Errorf("no IP addresses resolved for %q", cleanHost)
	}

	return ips, nil
}

// IsPrivateIP checks if an IP address is in a private range.
// Covers:
//   - RFC 1918: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
//   - RFC 4193: fc00::/7 (IPv6 unique local)
//   - RFC 5735: 192.0.2.0/24 (TEST-NET-1), 198.51.100.0/24 (TEST-NET-2),
//     203.0.113.0/24 (TEST-NET-3)
//   - RFC 3927: 169.254.0.0/16 (link-local)
func IsPrivateIP(ip net.IP) bool {
	// IPv4 private ranges
	privateIPv4Ranges := []struct {
		network string
	}{
		{"10.0.0.0/8"},       // RFC 1918 Class A
		{"172.16.0.0/12"},    // RFC 1918 Class B
		{"192.168.0.0/16"},   // RFC 1918 Class C
		{"192.0.2.0/24"},     // RFC 5735 TEST-NET-1
		{"198.51.100.0/24"},  // RFC 5735 TEST-NET-2
		{"203.0.113.0/24"},   // RFC 5735 TEST-NET-3
		{"100.64.0.0/10"},    // RFC 6598 Carrier-grade NAT
		{"192.0.0.0/24"},     // RFC 6890 IETF Protocol Assignments
		{"192.88.99.0/24"},   // RFC 3068 6to4 Relay
		{"198.18.0.0/15"},    // RFC 2544 Benchmarking
	}

	// IPv6 private ranges
	privateIPv6Ranges := []struct {
		network string
	}{
		{"fc00::/7"},     // RFC 4193 Unique Local
		{"64:ff9b::/96"}, // RFC 6052 IPv4/IPv6 Translation
	}

	if ip.To4() != nil {
		for _, r := range privateIPv4Ranges {
			_, cidr, _ := net.ParseCIDR(r.network)
			if cidr.Contains(ip) {
				return true
			}
		}
	} else {
		for _, r := range privateIPv6Ranges {
			_, cidr, _ := net.ParseCIDR(r.network)
			if cidr.Contains(ip) {
				return true
			}
		}
	}

	return false
}

// IsLoopbackIP checks if an IP address is a loopback address.
//   - IPv4: 127.0.0.0/8
//   - IPv6: ::1
func IsLoopbackIP(ip net.IP) bool {
	if ip.To4() != nil {
		_, cidr, _ := net.ParseCIDR("127.0.0.0/8")
		return cidr.Contains(ip)
	}
	return ip.Equal(net.ParseIP("::1"))
}

// IsMetadataIP checks if an IP address is a cloud metadata endpoint.
//   - 169.254.169.254 (AWS, GCP, Azure, and most cloud providers)
//   - fd00:ec2::254 (AWS IPv6 metadata)
func IsMetadataIP(ip net.IP) bool {
	metadataEndpoints := []string{
		"169.254.169.254/32",
		"fd00:ec2::254/128",
	}

	for _, ep := range metadataEndpoints {
		_, cidr, _ := net.ParseCIDR(ep)
		if cidr.Contains(ip) {
			return true
		}
	}
	return false
}

// IsLinkLocalIP checks if an IP is link-local.
//   - IPv4: 169.254.0.0/16
//   - IPv6: fe80::/10
func IsLinkLocalIP(ip net.IP) bool {
	if ip.To4() != nil {
		_, cidr, _ := net.ParseCIDR("169.254.0.0/16")
		return cidr.Contains(ip)
	}
	_, cidr, _ := net.ParseCIDR("fe80::/10")
	return cidr.Contains(ip)
}

// IsReservedIP checks if an IP is in other reserved ranges.
//   - 0.0.0.0/8 (Current network)
//   - 224.0.0.0/4 (Multicast)
//   - 240.0.0.0/4 (Future use / Class E)
//   - 255.255.255.255/32 (Broadcast)
//   - :: (unspecified IPv6)
//   - ff00::/8 (IPv6 multicast)
func IsReservedIP(ip net.IP) bool {
	// Check unspecified addresses
	if ip.IsUnspecified() {
		return true
	}

	if ip.To4() != nil {
		reservedIPv4 := []string{
			"0.0.0.0/8",         // Current network
			"224.0.0.0/4",       // Multicast
			"240.0.0.0/4",       // Future use / Class E
			"255.255.255.255/32", // Broadcast
		}
		for _, r := range reservedIPv4 {
			_, cidr, _ := net.ParseCIDR(r)
			if cidr.Contains(ip) {
				return true
			}
		}
	} else {
		reservedIPv6 := []string{
			"ff00::/8", // IPv6 multicast
		}
		for _, r := range reservedIPv6 {
			_, cidr, _ := net.ParseCIDR(r)
			if cidr.Contains(ip) {
				return true
			}
		}
	}

	return false
}

// containsControlChars checks if a string contains ASCII control characters
func containsControlChars(s string) bool {
	for _, r := range s {
		if r < 0x20 || r == 0x7f {
			return true
		}
	}
	return false
}
