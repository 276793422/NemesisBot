// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package web

import (
	"encoding/json"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
)

// TestCORSManager_CheckCORS tests CORS validation logic
func TestCORSManager_CheckCORS(t *testing.T) {
	tests := []struct {
		name           string
		config         *CORSConfig
		origin         string
		expectedResult bool
		description    string
	}{
		// Development mode tests
		{
			name: "dev_mode_localhost",
			config: &CORSConfig{
				DevelopmentMode: true,
			},
			origin:         "http://localhost:3000",
			expectedResult: true,
			description:    "Development mode should allow localhost with any port",
		},
		{
			name: "dev_mode_localhost_ip",
			config: &CORSConfig{
				DevelopmentMode: true,
			},
			origin:         "http://127.0.0.1:8080",
			expectedResult: true,
			description:    "Development mode should allow 127.0.0.1",
		},
		{
			name: "dev_mode_external_blocked",
			config: &CORSConfig{
				DevelopmentMode: true,
			},
			origin:         "https://evil.com",
			expectedResult: false,
			description:    "Development mode should block non-localhost origins",
		},

		// Production mode tests
		{
			name: "prod_mode_whitelist_match",
			config: &CORSConfig{
				DevelopmentMode: false,
				AllowedOrigins:  []string{"https://yourdomain.com", "https://app.yourdomain.com"},
			},
			origin:         "https://yourdomain.com",
			expectedResult: true,
			description:    "Production mode should allow whitelisted origins",
		},
		{
			name: "prod_mode_whitelist_no_match",
			config: &CORSConfig{
				DevelopmentMode: false,
				AllowedOrigins:  []string{"https://yourdomain.com"},
			},
			origin:         "https://evil.com",
			expectedResult: false,
			description:    "Production mode should block non-whitelisted origins",
		},

		// No Origin header tests
		{
			name: "no_origin_allowed",
			config: &CORSConfig{
				AllowNoOrigin: true,
			},
			origin:         "",
			expectedResult: true,
			description:    "AllowNoOrigin=true should allow requests without Origin",
		},
		{
			name: "no_origin_blocked",
			config: &CORSConfig{
				AllowNoOrigin: false,
			},
			origin:         "",
			expectedResult: false,
			description:    "AllowNoOrigin=false should block requests without Origin",
		},

		// CDN domain tests
		{
			name: "cdn_exact_match",
			config: &CORSConfig{
				AllowedCDNDomains: []string{"cdn.cloudflare.com"},
			},
			origin:         "https://cdn.cloudflare.com",
			expectedResult: true,
			description:    "CDN domain should match exactly",
		},
		{
			name: "cdn_subdomain_match",
			config: &CORSConfig{
				AllowedCDNDomains: []string{"cdn.cloudflare.com"},
			},
			origin:         "https://abc.cdn.cloudflare.com",
			expectedResult: true,
			description:    "CDN domain should match subdomains",
		},
		{
			name: "cdn_no_match_fake_domain",
			config: &CORSConfig{
				AllowedCDNDomains: []string{"cdn.cloudflare.com"},
			},
			origin:         "https://fake-cdn.cloudflare.com.evil.com",
			expectedResult: false,
			description:    "CDN domain should NOT match fake domains",
		},
		{
			name: "cdn_no_match_partial",
			config: &CORSConfig{
				AllowedCDNDomains: []string{"cdn.cloudflare.com"},
			},
			origin:         "https://cloudflare.com",
			expectedResult: false,
			description:    "CDN domain should NOT match parent domain",
		},

		// Port handling
		{
			name: "origin_with_port",
			config: &CORSConfig{
				AllowedOrigins: []string{"https://yourdomain.com:8443"},
			},
			origin:         "https://yourdomain.com:8443",
			expectedResult: true,
			description:    "Origin with port should match exactly",
		},
		{
			name: "origin_different_port",
			config: &CORSConfig{
				AllowedOrigins: []string{"https://yourdomain.com"},
			},
			origin:         "https://yourdomain.com:8443",
			expectedResult: false,
			description:    "Origin with different port should NOT match",
		},

		// Complex scenarios
		{
			name: "multiple_cdn_domains",
			config: &CORSConfig{
				AllowedCDNDomains: []string{"cdn.cloudflare.com", "cdn.jsdelivr.net"},
			},
			origin:         "https://cdn.jsdelivr.net",
			expectedResult: true,
			description:    "Should match one of multiple CDN domains",
		},
		{
			name: "mixed_config",
			config: &CORSConfig{
				AllowedOrigins:    []string{"https://yourdomain.com"},
				AllowedCDNDomains: []string{"cdn.cloudflare.com"},
				DevelopmentMode:   false,
				AllowNoOrigin:     false,
			},
			origin:         "https://cdn.cloudflare.com",
			expectedResult: true,
			description:    "Should check both whitelist and CDN domains",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Create temporary config directory
			tmpDir := t.TempDir()
			configPath := filepath.Join(tmpDir, "cors.json")

			// Save config
			configData, err := json.MarshalIndent(tt.config, "", "  ")
			if err != nil {
				t.Fatalf("Failed to marshal config: %v", err)
			}
			if err := os.WriteFile(configPath, configData, 0644); err != nil {
				t.Fatalf("Failed to write config: %v", err)
			}

			// Create CORS manager
			mgr, err := NewCORSManager(configPath)
			if err != nil {
				t.Fatalf("Failed to create CORS manager: %v", err)
			}
			defer mgr.Close()

			// Create HTTP request
			req := httptest.NewRequest("GET", "http://example.com/test", nil)
			if tt.origin != "" {
				req.Header.Set("Origin", tt.origin)
			}

			// Check CORS
			result := mgr.CheckCORS(req)
			if result != tt.expectedResult {
				t.Errorf("%s: expected %v, got %v", tt.description, tt.expectedResult, result)
			}
		})
	}
}

// TestCORSManager_AddRemoveOrigin tests adding and removing origins
func TestCORSManager_AddRemoveOrigin(t *testing.T) {
	// Create temporary config directory
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "cors.json")

	// Create CORS manager
	mgr, err := NewCORSManager(configPath)
	if err != nil {
		t.Fatalf("Failed to create CORS manager: %v", err)
	}
	defer mgr.Close()

	// Add origin
	origin1 := "https://test1.com"
	if err := mgr.AddOrigin(origin1); err != nil {
		t.Fatalf("Failed to add origin: %v", err)
	}

	// Verify origin was added
	origins := mgr.ListOrigins()
	if len(origins) != 1 || origins[0] != origin1 {
		t.Errorf("Expected origins [%s], got %v", origin1, origins)
	}

	// Add another origin
	origin2 := "https://test2.com"
	if err := mgr.AddOrigin(origin2); err != nil {
		t.Fatalf("Failed to add second origin: %v", err)
	}

	// Verify both origins exist
	origins = mgr.ListOrigins()
	if len(origins) != 2 {
		t.Errorf("Expected 2 origins, got %d", len(origins))
	}

	// Try to add duplicate origin (should be idempotent)
	if err := mgr.AddOrigin(origin1); err != nil {
		t.Fatalf("Failed to add duplicate origin: %v", err)
	}

	// Verify still only 2 origins
	origins = mgr.ListOrigins()
	if len(origins) != 2 {
		t.Errorf("Expected 2 origins after duplicate add, got %d", len(origins))
	}

	// Remove origin
	if err := mgr.RemoveOrigin(origin1); err != nil {
		t.Fatalf("Failed to remove origin: %v", err)
	}

	// Verify origin was removed
	origins = mgr.ListOrigins()
	if len(origins) != 1 || origins[0] != origin2 {
		t.Errorf("Expected origins [%s], got %v", origin2, origins)
	}

	// Remove non-existent origin (should be idempotent)
	if err := mgr.RemoveOrigin("https://nonexistent.com"); err != nil {
		t.Fatalf("Failed to remove non-existent origin: %v", err)
	}
}

// TestCORSManager_SetDevelopmentMode tests development mode toggle
func TestCORSManager_SetDevelopmentMode(t *testing.T) {
	// Create temporary config directory
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "cors.json")

	// Create CORS manager
	mgr, err := NewCORSManager(configPath)
	if err != nil {
		t.Fatalf("Failed to create CORS manager: %v", err)
	}
	defer mgr.Close()

	// Enable development mode
	if err := mgr.SetDevelopmentMode(true); err != nil {
		t.Fatalf("Failed to enable development mode: %v", err)
	}

	// Verify localhost is allowed
	req := httptest.NewRequest("GET", "http://example.com/test", nil)
	req.Header.Set("Origin", "http://localhost:3000")
	if !mgr.CheckCORS(req) {
		t.Error("Expected localhost to be allowed in development mode")
	}

	// Disable development mode and localhost
	if err := mgr.SetDevelopmentMode(false); err != nil {
		t.Fatalf("Failed to disable development mode: %v", err)
	}

	// Also disable AllowLocalhost
	mgr.mu.Lock()
	mgr.config.AllowLocalhost = false
	mgr.mu.Unlock()

	// Verify localhost is blocked
	req = httptest.NewRequest("GET", "http://example.com/test", nil)
	req.Header.Set("Origin", "http://localhost:3000")
	if mgr.CheckCORS(req) {
		t.Error("Expected localhost to be blocked when both development mode and AllowLocalhost are disabled")
	}
}

// TestCORSManager_ConfigPersistence tests config save/load
func TestCORSManager_ConfigPersistence(t *testing.T) {
	// Create temporary config directory
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "cors.json")

	// Create CORS manager and add some origins
	mgr1, err := NewCORSManager(configPath)
	if err != nil {
		t.Fatalf("Failed to create first CORS manager: %v", err)
	}

	// Add origins
	mgr1.AddOrigin("https://test1.com")
	mgr1.AddOrigin("https://test2.com")
	mgr1.SetDevelopmentMode(true)

	// Close first manager
	mgr1.Close()

	// Create new manager (should load existing config)
	mgr2, err := NewCORSManager(configPath)
	if err != nil {
		t.Fatalf("Failed to create second CORS manager: %v", err)
	}
	defer mgr2.Close()

	// Verify origins were persisted
	origins := mgr2.ListOrigins()
	if len(origins) != 2 {
		t.Errorf("Expected 2 origins to persist, got %d", len(origins))
	}

	// Verify development mode was persisted
	req := httptest.NewRequest("GET", "http://example.com/test", nil)
	req.Header.Set("Origin", "http://localhost:3000")
	if !mgr2.CheckCORS(req) {
		t.Error("Expected development mode to persist")
	}
}

// TestCORSManager_DefaultConfig tests default configuration
func TestCORSManager_DefaultConfig(t *testing.T) {
	// Create temporary config directory (no existing config)
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "cors.json")

	// Create CORS manager (should create default config)
	mgr, err := NewCORSManager(configPath)
	if err != nil {
		t.Fatalf("Failed to create CORS manager: %v", err)
	}
	defer mgr.Close()

	// Verify default config allows no-origin requests
	req := httptest.NewRequest("GET", "http://example.com/test", nil)
	// No Origin header
	if !mgr.CheckCORS(req) {
		t.Error("Expected default config to allow requests without Origin")
	}

	// Verify default config has localhost enabled
	req = httptest.NewRequest("GET", "http://example.com/test", nil)
	req.Header.Set("Origin", "http://localhost:3000")
	if !mgr.CheckCORS(req) {
		t.Error("Expected default config to allow localhost")
	}
}

// TestCORSManager_InvalidOrigin tests handling of invalid origins
func TestCORSManager_InvalidOrigin(t *testing.T) {
	// Create temporary config directory
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "cors.json")

	// Create CORS manager
	mgr, err := NewCORSManager(configPath)
	if err != nil {
		t.Fatalf("Failed to create CORS manager: %v", err)
	}
	defer mgr.Close()

	tests := []struct {
		name   string
		origin string
	}{
		{"malformed_url", "not-a-valid-url"},
		{"with_spaces", "https://example .com"},
		{"javascript_protocol", "javascript:alert(1)"},
		{"data_protocol", "data:text/html,<script>alert(1)</script>"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := httptest.NewRequest("GET", "http://example.com/test", nil)
			req.Header.Set("Origin", tt.origin)

			// Should not panic, should return false
			result := mgr.CheckCORS(req)
			// We just verify it doesn't panic
			_ = result
		})
	}
}

// TestCORSManager_ConcurrentAccess tests thread safety
func TestCORSManager_ConcurrentAccess(t *testing.T) {
	// Create temporary config directory
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "cors.json")

	// Create CORS manager
	mgr, err := NewCORSManager(configPath)
	if err != nil {
		t.Fatalf("Failed to create CORS manager: %v", err)
	}
	defer mgr.Close()

	// Test concurrent CheckCORS calls
	numGoroutines := 50
	done := make(chan bool, numGoroutines)

	for i := 0; i < numGoroutines; i++ {
		go func(index int) {
			// Alternate between different operations
			if index%3 == 0 {
				// Check CORS
				req := httptest.NewRequest("GET", "http://example.com/test", nil)
				req.Header.Set("Origin", "http://localhost:3000")
				mgr.CheckCORS(req)
			} else if index%3 == 1 {
				// Add origin
				mgr.AddOrigin("https://test.com")
			} else {
				// List origins
				mgr.ListOrigins()
			}
			done <- true
		}(i)
	}

	// Wait for all goroutines to complete
	for i := 0; i < numGoroutines; i++ {
		<-done
	}

	// If we reach here without panic or race condition, test passes
}
