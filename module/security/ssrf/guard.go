// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package ssrf provides Server-Side Request Forgery protection for outbound requests

package ssrf

import (
	"context"
	"fmt"
	"net"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// Config holds SSRF guard configuration
type Config struct {
	Enabled       bool     // master switch
	BlockedCIDRs  []string // additional CIDRs to block
	AllowedHosts  []string // whitelist hosts that bypass checks
	BlockMetadata bool     // block cloud metadata endpoints (169.254.169.254)
	BlockLocalhost  bool   // block localhost/loopback
	BlockPrivateIPs bool   // block RFC 1918/4193 private ranges
	MaxRedirects  int      // max HTTP redirects to follow
}

// DefaultConfig returns a secure-by-default configuration
func DefaultConfig() *Config {
	return &Config{
		Enabled:         true,
		BlockMetadata:   true,
		BlockLocalhost:  true,
		BlockPrivateIPs: true,
		MaxRedirects:    5,
		BlockedCIDRs:    []string{},
		AllowedHosts:    []string{},
	}
}

// Guard provides SSRF protection by validating URLs and IPs before requests
type Guard struct {
	config      *Config
	blockedNets []*net.IPNet
	allowedSet  map[string]bool
	mu          sync.RWMutex
}

// NewGuard creates a new SSRF guard with the given configuration
func NewGuard(cfg *Config) (*Guard, error) {
	if cfg == nil {
		cfg = DefaultConfig()
	}

	g := &Guard{
		config:     cfg,
		allowedSet: make(map[string]bool),
	}

	// Build allowed hosts set
	for _, host := range cfg.AllowedHosts {
		g.allowedSet[strings.ToLower(host)] = true
	}

	// Parse blocked CIDRs
	for _, cidr := range cfg.BlockedCIDRs {
		_, ipNet, err := net.ParseCIDR(cidr)
		if err != nil {
			return nil, fmt.Errorf("invalid blocked CIDR %q: %w", cidr, err)
		}
		g.blockedNets = append(g.blockedNets, ipNet)
	}

	return g, nil
}

// ValidateURL validates a URL before making a request.
// It parses the URL, resolves the host via DNS, and checks all resolved IPs
// against private/reserved ranges.
func (g *Guard) ValidateURL(ctx context.Context, rawURL string) error {
	if !g.config.Enabled {
		return nil
	}

	g.mu.RLock()
	defer g.mu.RUnlock()

	// Parse the URL
	parsed, err := ParseURL(rawURL)
	if err != nil {
		return fmt.Errorf("ssrf: invalid URL: %w", err)
	}

	// Check if host is in allowed whitelist
	host := strings.ToLower(parsed.Hostname())
	if g.allowedSet[host] {
		logger.DebugC("ssrf", fmt.Sprintf("Host %q is whitelisted", host))
		return nil
	}

	// Check scheme
	scheme := strings.ToLower(parsed.Scheme)
	if scheme != "http" && scheme != "https" {
		return fmt.Errorf("ssrf: unsupported scheme %q (only http and https are allowed)", scheme)
	}

	// Resolve and validate
	return g.resolveAndValidateLocked(ctx, parsed)
}

// CheckIP checks if a single IP address is private or reserved.
// Returns an error if the IP is in a blocked range.
func (g *Guard) CheckIP(ctx context.Context, ipStr string) error {
	if !g.config.Enabled {
		return nil
	}

	g.mu.RLock()
	defer g.mu.RUnlock()

	ip := net.ParseIP(ipStr)
	if ip == nil {
		return fmt.Errorf("ssrf: invalid IP address %q", ipStr)
	}

	return g.checkIPLocked(ip)
}

// ResolveAndValidate resolves DNS for the URL's host and validates all resulting IPs.
// This is useful when you want to resolve DNS separately from making the actual request.
func (g *Guard) ResolveAndValidate(ctx context.Context, rawURL string) error {
	if !g.config.Enabled {
		return nil
	}

	g.mu.RLock()
	defer g.mu.RUnlock()

	parsed, err := ParseURL(rawURL)
	if err != nil {
		return fmt.Errorf("ssrf: invalid URL: %w", err)
	}

	// Check whitelist
	host := strings.ToLower(parsed.Hostname())
	if g.allowedSet[host] {
		return nil
	}

	return g.resolveAndValidateLocked(ctx, parsed)
}

// resolveAndValidateLocked performs DNS resolution and IP checks.
// Caller must hold at least a read lock on g.mu.
func (g *Guard) resolveAndValidateLocked(ctx context.Context, parsed *url.URL) error {
	host := parsed.Hostname()

	// Check for localhost hostname strings
	if g.config.BlockLocalhost {
		lowerHost := strings.ToLower(host)
		if lowerHost == "localhost" || strings.HasSuffix(lowerHost, ".localhost") ||
			lowerHost == "localhost.localdomain" {
			return fmt.Errorf("ssrf: localhost hostname %q is blocked", host)
		}
	}

	// Try to parse as IP directly first
	ip := net.ParseIP(host)
	if ip != nil {
		return g.checkIPLocked(ip)
	}

	// Resolve via DNS with timeout
	resolveCtx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()

	ips, err := ResolveHost(host)
	if err != nil {
		select {
		case <-resolveCtx.Done():
			return fmt.Errorf("ssrf: DNS resolution timeout for host %q", host)
		default:
			return fmt.Errorf("ssrf: DNS resolution failed for host %q: %w", host, err)
		}
	}

	if len(ips) == 0 {
		return fmt.Errorf("ssrf: no IP addresses found for host %q", host)
	}

	// Check all resolved IPs
	for _, resolvedIP := range ips {
		if err := g.checkIPLocked(resolvedIP); err != nil {
			logger.WarnCF("ssrf", fmt.Sprintf("Blocked IP %s for host %q", resolvedIP, host), map[string]interface{}{
				"ip":   resolvedIP.String(),
				"host": host,
				"err":  err.Error(),
			})
			return fmt.Errorf("ssrf: host %q resolves to blocked IP %s: %w", host, resolvedIP, err)
		}
	}

	logger.DebugCF("ssrf", fmt.Sprintf("URL passed validation (host=%s)", host), map[string]interface{}{
		"host": host,
		"ips":  fmt.Sprintf("%v", ips),
	})
	return nil
}

// checkIPLocked checks a single IP against all blocking rules.
// Caller must hold at least a read lock on g.mu.
func (g *Guard) checkIPLocked(ip net.IP) error {
	// Block localhost/loopback (127.0.0.0/8, ::1)
	if g.config.BlockLocalhost && IsLoopbackIP(ip) {
		return fmt.Errorf("loopback IP %s is blocked", ip)
	}

	// Block cloud metadata endpoints
	if g.config.BlockMetadata && IsMetadataIP(ip) {
		return fmt.Errorf("cloud metadata IP %s is blocked", ip)
	}

	// Block private IPs (RFC 1918, RFC 4193, etc.)
	if g.config.BlockPrivateIPs && IsPrivateIP(ip) {
		return fmt.Errorf("private IP %s is blocked", ip)
	}

	// Check link-local
	if IsLinkLocalIP(ip) {
		return fmt.Errorf("link-local IP %s is blocked", ip)
	}

	// Check other reserved ranges
	if IsReservedIP(ip) {
		return fmt.Errorf("reserved IP %s is blocked", ip)
	}

	// Check user-configured blocked CIDRs
	for _, blockedNet := range g.blockedNets {
		if blockedNet.Contains(ip) {
			return fmt.Errorf("IP %s is in blocked CIDR %s", ip, blockedNet)
		}
	}

	return nil
}

// IsEnabled returns whether the SSRF guard is enabled
func (g *Guard) IsEnabled() bool {
	return g.config.Enabled
}

// AddBlockedCIDR dynamically adds a CIDR to the block list.
// Thread-safe.
func (g *Guard) AddBlockedCIDR(cidr string) error {
	_, ipNet, err := net.ParseCIDR(cidr)
	if err != nil {
		return fmt.Errorf("invalid CIDR %q: %w", cidr, err)
	}

	g.mu.Lock()
	defer g.mu.Unlock()
	g.blockedNets = append(g.blockedNets, ipNet)
	return nil
}

// AddAllowedHost dynamically adds a host to the whitelist.
// Thread-safe.
func (g *Guard) AddAllowedHost(host string) {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.allowedSet[strings.ToLower(host)] = true
}

// RemoveAllowedHost removes a host from the whitelist.
// Thread-safe.
func (g *Guard) RemoveAllowedHost(host string) {
	g.mu.Lock()
	defer g.mu.Unlock()
	delete(g.allowedSet, strings.ToLower(host))
}
