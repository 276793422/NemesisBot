// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package clamav

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// ManagerConfig holds all configuration for the ClamAV integration
type ManagerConfig struct {
	// Enabled controls whether ClamAV integration is active
	Enabled bool
	// ClamAVPath is the directory containing ClamAV executables
	ClamAVPath string
	// DataDir is the working directory for ClamAV (database, config, temp)
	DataDir string
	// Address is the TCP address for clamd (default: 127.0.0.1:3310)
	Address string
	// Scanner configuration
	Scanner *ScannerConfig
	// UpdateInterval is the virus database update interval (0 = manual only)
	UpdateInterval string
}

// Manager is the top-level ClamAV integration manager.
// It manages the daemon lifecycle, scanner, updater, and provides the scan hook.
type Manager struct {
	config   *ManagerConfig
	daemon   *Daemon
	scanner  *Scanner
	updater  *Updater
	hook     *ScanHook
	mu       sync.RWMutex
	started  bool
}

// NewManager creates a new ClamAV integration manager
func NewManager(cfg *ManagerConfig) *Manager {
	return &Manager{
		config: cfg,
	}
}

// Start initializes and starts all ClamAV components
func (m *Manager) Start(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.started {
		return fmt.Errorf("ClamAV manager already started")
	}

	if !m.config.Enabled {
		logger.InfoC("clamav", "ClamAV integration is disabled")
		return nil
	}

	// Auto-detect ClamAV path if not set
	clamavPath := m.config.ClamAVPath
	if clamavPath == "" {
		clamavPath = DetectClamAVPath()
		if clamavPath == "" {
			return fmt.Errorf("ClamAV installation not found; set clamav_path in config or install ClamAV")
		}
		logger.InfoCF("clamav", "Auto-detected ClamAV", map[string]interface{}{
			"path": clamavPath,
		})
	}

	// Resolve paths to absolute for reliable config generation
	if absPath, err := filepath.Abs(clamavPath); err == nil {
		clamavPath = absPath
	}

	// Setup data directories
	dataDir := m.config.DataDir
	if dataDir == "" {
		dataDir = filepath.Join(os.TempDir(), "nemesisbot-clamav")
	}
	// Resolve dataDir to absolute path
	if absDir, err := filepath.Abs(dataDir); err == nil {
		dataDir = absDir
	}
	dbDir := filepath.Join(dataDir, "database")
	configDir := filepath.Join(dataDir, "config")
	tempDir := filepath.Join(dataDir, "temp")

	for _, dir := range []string{dbDir, configDir, tempDir} {
		if err := os.MkdirAll(dir, 0755); err != nil {
			return fmt.Errorf("failed to create directory %s: %w", dir, err)
		}
	}

	address := m.config.Address
	if address == "" {
		address = "127.0.0.1:3310"
	}

	// Generate configs
	clamdConf := filepath.Join(configDir, "clamd.conf")
	freshclamConf := filepath.Join(configDir, "freshclam.conf")

	daemonCfg := &DaemonConfig{
		ClamAVPath:  clamavPath,
		ConfigFile:  clamdConf,
		DatabaseDir: dbDir,
		ListenAddr:  address,
		TempDir:     tempDir,
	}

	if err := GenerateClamdConfig(daemonCfg); err != nil {
		return fmt.Errorf("failed to generate clamd.conf: %w", err)
	}

	if err := GenerateFreshclamConfig(dbDir, freshclamConf); err != nil {
		return fmt.Errorf("failed to generate freshclam.conf: %w", err)
	}

	// Download virus database BEFORE starting clamd.
	// clamd refuses to start without a valid database (main.cvd).
	// freshclam runs independently and does not require clamd.
	updateInterval := parseDurationString(m.config.UpdateInterval)
	m.updater = NewUpdater(&UpdaterConfig{
		ClamAVPath:     clamavPath,
		DatabaseDir:    dbDir,
		ConfigFile:     freshclamConf,
		UpdateInterval: updateInterval,
	})

	if m.updater.IsDatabaseStale(24 * time.Hour) {
		logger.InfoC("clamav", "Downloading virus database before starting clamd")
		updateCtx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
		if err := m.updater.Update(updateCtx); err != nil {
			cancel()
			logger.WarnCF("clamav", "Initial database download failed", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			cancel()
			logger.InfoC("clamav", "Virus database downloaded successfully")
		}
	}

	// Start daemon (now database should be available)
	m.daemon = NewDaemon(daemonCfg)
	if err := m.daemon.Start(ctx); err != nil {
		return fmt.Errorf("failed to start ClamAV daemon: %w", err)
	}

	// Create scanner
	scannerCfg := m.config.Scanner
	if scannerCfg == nil {
		scannerCfg = DefaultScannerConfig()
	}
	scannerCfg.Address = address

	m.scanner = NewScannerWithClient(m.daemon.Client(), scannerCfg)
	m.hook = NewScanHook(m.scanner)

	// Start auto-update goroutine
	if updateInterval > 0 {
		go m.updater.StartAutoUpdate(ctx)
	}

	m.started = true
	logger.InfoCF("clamav", "ClamAV manager started", map[string]interface{}{
		"path":    clamavPath,
		"address": address,
		"dataDir": dataDir,
	})

	return nil
}

// Stop shuts down all ClamAV components
func (m *Manager) Stop() error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if !m.started {
		return nil
	}

	var errs []error

	if m.updater != nil {
		m.updater.Stop()
	}

	if m.daemon != nil {
		if err := m.daemon.Stop(); err != nil {
			errs = append(errs, err)
		}
	}

	m.started = false

	if len(errs) > 0 {
		return fmt.Errorf("errors during shutdown: %v", errs)
	}

	logger.InfoC("clamav", "ClamAV manager stopped")
	return nil
}

// Hook returns the scan hook for middleware integration
func (m *Manager) Hook() *ScanHook {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.hook
}

// Scanner returns the virus scanner
func (m *Manager) Scanner() *Scanner {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.scanner
}

// IsRunning returns whether the manager is started and the daemon is responsive
func (m *Manager) IsRunning() bool {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.started && m.daemon != nil && m.daemon.IsReady()
}

// GetStats returns scanning statistics
func (m *Manager) GetStats() map[string]interface{} {
	m.mu.RLock()
	defer m.mu.RUnlock()

	stats := map[string]interface{}{
		"enabled": m.config.Enabled,
		"started": m.started,
	}

	if m.scanner != nil {
		stats["scanner"] = m.scanner.GetStats()
	}

	if m.updater != nil {
		stats["last_update"] = m.updater.LastUpdate()
	}

	return stats
}

// parseDurationString parses a duration string (e.g., "24h", "1h30m")
func parseDurationString(s string) time.Duration {
	if s == "" {
		return 0
	}
	d, err := time.ParseDuration(s)
	if err != nil {
		return 0
	}
	return d
}
