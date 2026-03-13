// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package web

import (
	"encoding/json"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// CORSConfig represents CORS configuration
type CORSConfig struct {
	AllowedOrigins    []string `json:"allowed_origins"`
	AllowedMethods    []string `json:"allowed_methods"`
	AllowedHeaders    []string `json:"allowed_headers"`
	AllowCredentials  bool     `json:"allow_credentials"`
	MaxAge            int      `json:"max_age"`
	AllowLocalhost    bool     `json:"allow_localhost"`
	DevelopmentMode   bool     `json:"development_mode"`
	AllowedCDNDomains []string `json:"allowed_cdn_domains"`
	AllowNoOrigin     bool     `json:"allow_no_origin"` // Allow requests without Origin header (e.g., mobile apps, curl)
}

// CORSManager manages CORS configuration and validation
type CORSManager struct {
	config     *CORSConfig
	configPath string
	mu         sync.RWMutex
	stopCh     chan struct{}
}

// NewCORSManager creates a new CORS manager
func NewCORSManager(configPath string) (*CORSManager, error) {
	mgr := &CORSManager{
		configPath: configPath,
		stopCh:     make(chan struct{}),
	}

	// Load or create config
	if err := mgr.loadOrCreateConfig(); err != nil {
		return nil, err
	}

	// Start hot reload
	go mgr.watchConfig()

	return mgr, nil
}

// loadOrCreateConfig loads existing config or creates default
func (m *CORSManager) loadOrCreateConfig() error {
	// Check if file exists
	if _, err := os.Stat(m.configPath); os.IsNotExist(err) {
		// Create default config
		m.config = m.defaultConfig()
		return m.saveConfig()
	}

	// Load existing config
	return m.loadConfig()
}

// defaultConfig returns default CORS configuration
func (m *CORSManager) defaultConfig() *CORSConfig {
	return &CORSConfig{
		AllowedOrigins:    []string{},
		AllowedMethods:    []string{"GET", "POST"},
		AllowedHeaders:    []string{"Content-Type", "Authorization"},
		AllowCredentials:  true,
		MaxAge:            3600,
		AllowLocalhost:    true,
		DevelopmentMode:   false,
		AllowedCDNDomains: []string{},
		AllowNoOrigin:     true, // Default: allow mobile apps, curl, etc.
	}
}

// loadConfig loads configuration from file
func (m *CORSManager) loadConfig() error {
	data, err := os.ReadFile(m.configPath)
	if err != nil {
		return err
	}

	var config CORSConfig
	if err := json.Unmarshal(data, &config); err != nil {
		return err
	}

	m.mu.Lock()
	m.config = &config
	m.mu.Unlock()

	logger.InfoCF("cors", "CORS config loaded", map[string]interface{}{
		"config_path": m.configPath,
	})
	return nil
}

// saveConfig saves configuration to file
func (m *CORSManager) saveConfig() error {
	// Ensure directory exists
	dir := filepath.Dir(m.configPath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	data, err := json.MarshalIndent(m.config, "", "  ")
	if err != nil {
		return err
	}

	// Write to temp file first
	tmpPath := m.configPath + ".tmp"
	if err := os.WriteFile(tmpPath, data, 0644); err != nil {
		return err
	}

	// Atomic rename
	if err := os.Rename(tmpPath, m.configPath); err != nil {
		os.Remove(tmpPath)
		return err
	}

	logger.InfoCF("cors", "CORS config saved", map[string]interface{}{
		"config_path": m.configPath,
	})
	return nil
}

// watchConfig watches for config changes and reloads
func (m *CORSManager) watchConfig() {
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-ticker.C:
			if err := m.loadConfig(); err != nil {
				logger.ErrorCF("cors", "Failed to reload CORS config", map[string]interface{}{
					"error": err.Error(),
				})
			}
		case <-m.stopCh:
			return
		}
	}
}

// CheckCORS validates if the request origin is allowed
func (m *CORSManager) CheckCORS(r *http.Request) bool {
	m.mu.RLock()
	defer m.mu.RUnlock()

	origin := r.Header.Get("Origin")

	// Allow requests without Origin (e.g., mobile apps, curl)
	if origin == "" {
		allowed := m.config.AllowNoOrigin
		if !allowed {
			logger.WarnCF("cors", "Rejected connection without Origin header", map[string]interface{}{
				"remote_addr": r.RemoteAddr,
			})
		}
		return allowed
	}

	// Development mode or AllowLocalhost: allow localhost with any port
	if m.config.DevelopmentMode || m.config.AllowLocalhost {
		if strings.HasPrefix(origin, "http://localhost:") ||
			strings.HasPrefix(origin, "http://127.0.0.1:") {
			logger.DebugCF("cors", "Allowed localhost origin", map[string]interface{}{
				"origin":           origin,
				"development_mode": m.config.DevelopmentMode,
				"allow_localhost":  m.config.AllowLocalhost,
			})
			return true
		}
	}

	// Check exact match in allowed origins
	for _, allowed := range m.config.AllowedOrigins {
		if origin == allowed {
			logger.DebugCF("cors", "Allowed origin", map[string]interface{}{
				"origin": origin,
			})
			return true
		}
	}

	// Check CDN domains (improved matching using URL parsing)
	for _, cdn := range m.config.AllowedCDNDomains {
		// Parse origin URL
		originURL, err := url.Parse(origin)
		if err != nil {
			logger.WarnCF("cors", "Failed to parse origin URL", map[string]interface{}{
				"origin": origin,
				"error":  err.Error(),
			})
			continue
		}

		host := originURL.Hostname()

		// Check exact match or subdomain match
		// e.g., cdn.cloudflare.com matches:
		//   - cdn.cloudflare.com (exact)
		//   - abc.cdn.cloudflare.com (subdomain)
		//   but NOT:
		//   - fake-cdn.cloudflare.com.evil.com
		if host == cdn || strings.HasSuffix(host, "."+cdn) {
			logger.DebugCF("cors", "Allowed CDN origin", map[string]interface{}{
				"origin": origin,
				"cdn":    cdn,
				"host":   host,
			})
			return true
		}
	}

	// Log rejected origin
	logger.WarnCF("cors", "CORS violation", map[string]interface{}{
		"origin":      origin,
		"remote_addr": r.RemoteAddr,
	})

	return false
}

// AddOrigin adds an allowed origin
func (m *CORSManager) AddOrigin(origin string) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	// Check if already exists
	for _, o := range m.config.AllowedOrigins {
		if o == origin {
			return nil
		}
	}

	m.config.AllowedOrigins = append(m.config.AllowedOrigins, origin)
	logger.InfoCF("cors", "Added allowed origin", map[string]interface{}{
		"origin": origin,
	})

	return m.saveConfig()
}

// RemoveOrigin removes an allowed origin
func (m *CORSManager) RemoveOrigin(origin string) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	// Filter out the origin
	newOrigins := []string{}
	for _, o := range m.config.AllowedOrigins {
		if o != origin {
			newOrigins = append(newOrigins, o)
		}
	}

	m.config.AllowedOrigins = newOrigins
	logger.InfoCF("cors", "Removed allowed origin", map[string]interface{}{
		"origin": origin,
	})

	return m.saveConfig()
}

// ListOrigins returns all allowed origins
func (m *CORSManager) ListOrigins() []string {
	m.mu.RLock()
	defer m.mu.RUnlock()

	origins := make([]string, len(m.config.AllowedOrigins))
	copy(origins, m.config.AllowedOrigins)
	return origins
}

// SetDevelopmentMode enables or disables development mode
func (m *CORSManager) SetDevelopmentMode(enabled bool) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	m.config.DevelopmentMode = enabled
	logger.InfoCF("cors", "Development mode changed", map[string]interface{}{
		"enabled": enabled,
	})

	return m.saveConfig()
}

// Close stops the CORS manager
func (m *CORSManager) Close() error {
	close(m.stopCh)
	return nil
}
