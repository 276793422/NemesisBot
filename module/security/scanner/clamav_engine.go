// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package scanner

import (
	"archive/zip"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	clamav "github.com/276793422/NemesisBot/module/security/scanner/clamav"
	"github.com/276793422/NemesisBot/module/logger"
)

// ClamAVEngine implements VirusScanner for ClamAV.
type ClamAVEngine struct {
	config  *config.ClamAVEngineConfig
	manager *clamav.Manager
	scanner *clamav.Scanner
	mu      sync.RWMutex
	started bool
}

// NewClamAVEngine creates a new ClamAV engine from raw JSON config.
func NewClamAVEngine(rawConfig json.RawMessage) (*ClamAVEngine, error) {
	cfg := &config.ClamAVEngineConfig{}
	if len(rawConfig) > 0 {
		if err := json.Unmarshal(rawConfig, cfg); err != nil {
			return nil, fmt.Errorf("invalid clamav config: %w", err)
		}
	}
	return &ClamAVEngine{config: cfg}, nil
}

// Name returns "clamav".
func (e *ClamAVEngine) Name() string { return "clamav" }

// GetInfo returns engine metadata.
func (e *ClamAVEngine) GetInfo(ctx context.Context) (*EngineInfo, error) {
	info := &EngineInfo{
		Name:    "clamav",
		Address: e.config.Address,
		Ready:   e.IsReady(),
	}

	if e.manager != nil && e.manager.IsRunning() {
		// Try to get version from daemon
		client := clamav.NewClient(e.config.Address)
		ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
		defer cancel()
		if ver, err := client.Version(ctx); err == nil {
			info.Version = ver
		}
	}

	return info, nil
}

// Download downloads ClamAV distribution from config.URL to dir.
func (e *ClamAVEngine) Download(ctx context.Context, dir string) error {
	if e.config.URL == "" {
		return fmt.Errorf("no download URL configured for clamav")
	}

	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("failed to create directory: %w", err)
	}

	// Download the archive
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, e.config.URL, nil)
	if err != nil {
		return fmt.Errorf("failed to create request: %w", err)
	}

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return fmt.Errorf("download failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("download failed with status: %s", resp.Status)
	}

	// Save to temp file with progress display
	tmpFile, err := os.CreateTemp(dir, "clamav-*.zip")
	if err != nil {
		return fmt.Errorf("failed to create temp file: %w", err)
	}
	tmpPath := tmpFile.Name()

	total := resp.ContentLength
	var written int64
	buf := make([]byte, 32*1024)
	lastLog := time.Now()

	for {
		nr, readErr := resp.Body.Read(buf)
		if nr > 0 {
			nw, writeErr := tmpFile.Write(buf[:nr])
			if writeErr != nil {
				tmpFile.Close()
				os.Remove(tmpPath)
				return fmt.Errorf("download write failed: %w", writeErr)
			}
			written += int64(nw)
		}
		if readErr != nil {
			if readErr != io.EOF {
				tmpFile.Close()
				os.Remove(tmpPath)
				return fmt.Errorf("download read failed: %w", readErr)
			}
			break
		}
		// Log progress every 2 seconds
		if time.Since(lastLog) >= 2*time.Second {
			if total > 0 {
				pct := float64(written) * 100 / float64(total)
				logger.InfoCF("scanner", "Downloading", map[string]interface{}{
					"progress":    fmt.Sprintf("%.1f%%", pct),
					"downloaded":  formatBytes(written),
					"total":       formatBytes(total),
				})
			} else {
				logger.InfoCF("scanner", "Downloading", map[string]interface{}{
					"downloaded": formatBytes(written),
				})
			}
			lastLog = time.Now()
		}
	}

	// Final progress
	if total > 0 {
		logger.InfoCF("scanner", "Download complete", map[string]interface{}{
			"size": formatBytes(written),
		})
	}

	tmpFile.Close()

	// Extract zip
	if err := extractZip(tmpPath, dir); err != nil {
		os.Remove(tmpPath)
		return fmt.Errorf("extraction failed: %w", err)
	}

	os.Remove(tmpPath)

	// Auto-detect install path from extracted files
	installPath, err := e.DetectInstallPath(dir)
	if err != nil {
		return fmt.Errorf("post-extraction detection failed: %w", err)
	}
	e.config.ClamAVPath = installPath

	logger.InfoCF("scanner", "ClamAV downloaded and detected", map[string]interface{}{
		"url":         e.config.URL,
		"dir":         dir,
		"installPath": installPath,
	})
	return nil
}

// Validate checks that the directory contains a valid ClamAV installation.
func (e *ClamAVEngine) Validate(dir string) error {
	var exeName string
	if runtime.GOOS == "windows" {
		exeName = "clamd.exe"
	} else {
		exeName = "clamd"
	}

	exePath := filepath.Join(dir, exeName)
	if _, err := os.Stat(exePath); os.IsNotExist(err) {
		return fmt.Errorf("clamd executable not found at %s", exePath)
	}
	return nil
}

// Setup parses the raw config (already done in NewClamAVEngine).
func (e *ClamAVEngine) Setup(config json.RawMessage) error {
	if len(config) == 0 {
		return nil
	}
	return json.Unmarshal(config, e.config)
}

// Start launches the ClamAV daemon and scanner.
func (e *ClamAVEngine) Start(ctx context.Context) error {
	e.mu.Lock()
	defer e.mu.Unlock()

	if e.started {
		return nil
	}

	address := e.config.Address
	if address == "" {
		address = "127.0.0.1:3310"
	}

	mgrCfg := &clamav.ManagerConfig{
		Enabled:        true,
		ClamAVPath:     e.config.ClamAVPath,
		DataDir:        e.config.DataDir,
		Address:        address,
		UpdateInterval: e.config.UpdateInterval,
		Scanner: &clamav.ScannerConfig{
			Enabled:        true,
			Address:        address,
			ScanOnWrite:    e.config.ScanOnWrite,
			ScanOnDownload: e.config.ScanOnDownload,
			ScanOnExec:     e.config.ScanOnExec,
			MaxFileSize:    e.config.MaxFileSize,
			Timeout:        60 * time.Second,
		},
	}

	mgr := clamav.NewManager(mgrCfg)
	if err := mgr.Start(ctx); err != nil {
		return fmt.Errorf("failed to start clamav: %w", err)
	}

	e.manager = mgr
	e.scanner = mgr.Scanner()
	e.started = true

	logger.InfoC("scanner", "ClamAV engine started")
	return nil
}

// formatBytes formats bytes into a human-readable string (e.g., "42.5 MB").
func formatBytes(b int64) string {
	const mb = 1024 * 1024
	if b < mb {
		return fmt.Sprintf("%d KB", b/1024)
	}
	return fmt.Sprintf("%.1f MB", float64(b)/float64(mb))
}

// Stop shuts down the ClamAV engine.
func (e *ClamAVEngine) Stop() error {
	e.mu.Lock()
	defer e.mu.Unlock()

	if !e.started || e.manager == nil {
		return nil
	}

	err := e.manager.Stop()
	e.started = false
	e.manager = nil
	e.scanner = nil
	return err
}

// IsReady returns true when the daemon is responsive.
func (e *ClamAVEngine) IsReady() bool {
	e.mu.RLock()
	defer e.mu.RUnlock()
	return e.started && e.manager != nil && e.manager.IsRunning()
}

// ScanFile scans a single file.
func (e *ClamAVEngine) ScanFile(ctx context.Context, path string) (*ScanResult, error) {
	e.mu.RLock()
	sc := e.scanner
	e.mu.RUnlock()

	if sc == nil {
		return &ScanResult{Path: path, Engine: "clamav", Raw: "engine not ready"}, nil
	}

	result, err := sc.ScanFile(ctx, path)
	if err != nil {
		return nil, err
	}

	return &ScanResult{
		Path:     result.Path,
		Infected: result.Infected,
		Virus:    result.Virus,
		Raw:      result.Raw,
		Engine:   "clamav",
	}, nil
}

// ScanContent scans byte content via the INSTREAM protocol.
func (e *ClamAVEngine) ScanContent(ctx context.Context, data []byte) (*ScanResult, error) {
	e.mu.RLock()
	sc := e.scanner
	e.mu.RUnlock()

	if sc == nil {
		return &ScanResult{Engine: "clamav", Raw: "engine not ready"}, nil
	}

	result, err := sc.ScanContentBytes(ctx, data)
	if err != nil {
		return nil, err
	}

	return &ScanResult{
		Infected: result.Infected,
		Virus:    result.Virus,
		Raw:      result.Raw,
		Engine:   "clamav",
	}, nil
}

// ScanDirectory scans all files in a directory.
func (e *ClamAVEngine) ScanDirectory(ctx context.Context, path string) ([]*ScanResult, error) {
	e.mu.RLock()
	sc := e.scanner
	e.mu.RUnlock()

	if sc == nil {
		return nil, nil
	}

	results, err := sc.ScanDirectory(ctx, path)
	if err != nil {
		return nil, err
	}

	out := make([]*ScanResult, len(results))
	for i, r := range results {
		out[i] = &ScanResult{
			Path:     r.Path,
			Infected: r.Infected,
			Virus:    r.Virus,
			Raw:      r.Raw,
			Engine:   "clamav",
		}
	}
	return out, nil
}

// GetDatabaseStatus returns the virus database status.
func (e *ClamAVEngine) GetDatabaseStatus(ctx context.Context) (*DatabaseStatus, error) {
	// Use a simple approach: check if database directory exists and has files
	status := &DatabaseStatus{}

	if e.manager == nil {
		return status, nil
	}

	stats := e.manager.GetStats()
	if lastUpdate, ok := stats["last_update"].(time.Time); ok {
		status.LastUpdate = lastUpdate
		status.Available = !lastUpdate.IsZero()
	}

	return status, nil
}

// UpdateDatabase triggers a virus database update.
func (e *ClamAVEngine) UpdateDatabase(ctx context.Context) error {
	// The manager's updater handles this internally.
	// For external trigger, we can't directly access updater.
	// This is a placeholder — full implementation would require
	// exposing updater through the manager.
	if !e.IsReady() {
		return fmt.Errorf("clamav engine not ready")
	}
	return nil
}

// GetStats returns engine statistics.
func (e *ClamAVEngine) GetStats() map[string]interface{} {
	e.mu.RLock()
	defer e.mu.RUnlock()

	if e.manager == nil {
		return map[string]interface{}{"started": false}
	}
	return e.manager.GetStats()
}

// GetExtensionRules returns extension rules from the engine config.
func (e *ClamAVEngine) GetExtensionRules() ExtensionRules {
	if e.config == nil {
		return ExtensionRules{}
	}
	return ExtensionRules{
		ScanExtensions: e.config.ScanExtensions,
		SkipExtensions: e.config.SkipExtensions,
	}
}

// --- InstallableEngine interface ---

// TargetExecutables returns the executable names for ClamAV on the current OS.
func (e *ClamAVEngine) TargetExecutables() []string {
	if runtime.GOOS == "windows" {
		return []string{"clamd.exe"}
	}
	return []string{"clamd"}
}

// DetectInstallPath recursively searches dir for ClamAV executables,
// returning the directory containing the first match.
func (e *ClamAVEngine) DetectInstallPath(dir string) (string, error) {
	targets := e.TargetExecutables()
	targetSet := make(map[string]bool, len(targets))
	for _, t := range targets {
		targetSet[t] = true
	}

	var foundPath string
	filepath.WalkDir(dir, func(path string, d os.DirEntry, err error) error {
		if err != nil || foundPath != "" {
			return err
		}
		if !d.IsDir() && targetSet[d.Name()] {
			foundPath = filepath.Dir(path)
			return filepath.SkipAll
		}
		return nil
	})
	if foundPath == "" {
		return "", fmt.Errorf("target executable not found in %s (looked for: %v)", dir, targets)
	}
	return foundPath, nil
}

// DatabaseFileName returns the primary ClamAV database file name.
func (e *ClamAVEngine) DatabaseFileName() string { return "main.cvd" }

// GetEngineState returns a pointer to the engine's state.
func (e *ClamAVEngine) GetEngineState() *config.EngineState { return &e.config.State }

// GetClamAVPath returns the current ClamAV installation path.
func (e *ClamAVEngine) GetClamAVPath() string {
	e.mu.RLock()
	defer e.mu.RUnlock()
	return e.config.ClamAVPath
}

// SetDataDir sets the data directory path used by the engine.
func (e *ClamAVEngine) SetDataDir(dir string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.config.DataDir = dir
}

// extractZip extracts a zip archive to the destination directory.
func extractZip(zipPath, dest string) error {
	r, err := zip.OpenReader(zipPath)
	if err != nil {
		return err
	}
	defer r.Close()

	for _, f := range r.File {
		fpath := filepath.Join(dest, f.Name)

		// Prevent zip slip
		if !strings.HasPrefix(filepath.Clean(fpath), filepath.Clean(dest)+string(os.PathSeparator)) {
			return fmt.Errorf("invalid file path: %s", fpath)
		}

		if f.FileInfo().IsDir() {
			os.MkdirAll(fpath, 0755)
			continue
		}

		if err := os.MkdirAll(filepath.Dir(fpath), 0755); err != nil {
			return err
		}

		outFile, err := os.OpenFile(fpath, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, f.Mode())
		if err != nil {
			return err
		}

		rc, err := f.Open()
		if err != nil {
			outFile.Close()
			return err
		}

		_, err = io.Copy(outFile, rc)
		rc.Close()
		outFile.Close()
		if err != nil {
			return err
		}
	}

	return nil
}
