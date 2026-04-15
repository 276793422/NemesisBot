// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package clamav

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// UpdaterConfig configures the virus database updater
type UpdaterConfig struct {
	// ClamAVPath is the directory containing ClamAV executables
	ClamAVPath string
	// DatabaseDir is the directory to store virus definitions
	DatabaseDir string
	// ConfigFile is the path to freshclam.conf
	ConfigFile string
	// UpdateInterval is how often to check for updates (0 = manual only)
	UpdateInterval time.Duration
	// MirrorURLs is a list of custom mirror URLs (empty = use defaults)
	MirrorURLs []string
}

// Updater manages virus database updates via freshclam
type Updater struct {
	config     *UpdaterConfig
	lastUpdate time.Time
	stopCh     chan struct{}
}

// NewUpdater creates a new virus database updater
func NewUpdater(cfg *UpdaterConfig) *Updater {
	return &Updater{
		config: cfg,
		stopCh: make(chan struct{}),
	}
}

// Update runs a virus database update
func (u *Updater) Update(ctx context.Context) error {
	freshclamExe := u.findExecutable("freshclam")
	if _, err := os.Stat(freshclamExe); err != nil {
		return fmt.Errorf("freshclam not found at %s: %w", freshclamExe, err)
	}

	// Ensure database directory exists
	if u.config.DatabaseDir != "" {
		if err := os.MkdirAll(u.config.DatabaseDir, 0755); err != nil {
			return fmt.Errorf("failed to create database directory: %w", err)
		}
	}

	args := []string{}
	if u.config.ConfigFile != "" {
		args = append(args, "--config-file", u.config.ConfigFile)
	}
	if u.config.DatabaseDir != "" {
		args = append(args, "--datadir", u.config.DatabaseDir)
	}

	cmd := exec.CommandContext(ctx, freshclamExe, args...)
	cmd.Dir = u.config.ClamAVPath

	// Set environment for ClamAV DLLs (Windows)
	if runtime.GOOS == "windows" {
		cmd.Env = append(os.Environ(),
			"PATH="+u.config.ClamAVPath+";"+os.Getenv("PATH"),
		)
	}

	output, err := cmd.CombinedOutput()
	outputStr := strings.TrimSpace(string(output))

	if err != nil {
		logger.ErrorCF("clamav", "Virus database update failed", map[string]interface{}{
			"error":  err.Error(),
			"output": outputStr,
		})
		return fmt.Errorf("freshclam failed: %w\n%s", err, outputStr)
	}

	u.lastUpdate = time.Now()
	logger.InfoCF("clamav", "Virus database updated", map[string]interface{}{
		"output": outputStr,
	})

	return nil
}

// StartAutoUpdate starts periodic database updates
func (u *Updater) StartAutoUpdate(ctx context.Context) {
	if u.config.UpdateInterval == 0 {
		return
	}

	ticker := time.NewTicker(u.config.UpdateInterval)
	defer ticker.Stop()

	logger.InfoCF("clamav", "Auto-update started", map[string]interface{}{
		"interval": u.config.UpdateInterval.String(),
	})

	for {
		select {
		case <-ctx.Done():
			return
		case <-u.stopCh:
			return
		case <-ticker.C:
			updateCtx, cancel := context.WithTimeout(ctx, 5*time.Minute)
			if err := u.Update(updateCtx); err != nil {
				logger.ErrorCF("clamav", "Auto-update failed", map[string]interface{}{
					"error": err.Error(),
				})
			}
			cancel()
		}
	}
}

// Stop stops the auto-update goroutine
func (u *Updater) Stop() {
	close(u.stopCh)
}

// LastUpdate returns the time of the last successful update
func (u *Updater) LastUpdate() time.Time {
	return u.lastUpdate
}

// IsDatabaseStale checks if the database is older than the given duration
func (u *Updater) IsDatabaseStale(maxAge time.Duration) bool {
	if u.lastUpdate.IsZero() {
		// Check file modification times
		if u.config.DatabaseDir != "" {
			mainCVD := filepath.Join(u.config.DatabaseDir, "main.cvd")
			dailyCVD := filepath.Join(u.config.DatabaseDir, "daily.cvd")
			for _, f := range []string{mainCVD, dailyCVD} {
				info, err := os.Stat(f)
				if err != nil {
					continue
				}
				if time.Since(info.ModTime()) > maxAge {
					return true
				}
			}
		}
		return true
	}
	return time.Since(u.lastUpdate) > maxAge
}

// findExecutable finds the full path to a ClamAV executable
func (u *Updater) findExecutable(name string) string {
	if runtime.GOOS == "windows" {
		return filepath.Join(u.config.ClamAVPath, name+".exe")
	}
	return filepath.Join(u.config.ClamAVPath, name)
}
