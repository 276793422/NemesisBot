// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package clamav

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// ScannerConfig configures the virus scanner
type ScannerConfig struct {
	// Enabled controls whether virus scanning is active
	Enabled bool
	// Address is the clamd TCP address (used when connecting to an existing daemon)
	Address string
	// ScanOnWrite controls whether files are scanned before write operations
	ScanOnWrite bool
	// ScanOnDownload controls whether downloaded files are scanned
	ScanOnDownload bool
	// ScanOnExec controls whether executables are scanned before execution
	ScanOnExec bool
	// MaxFileSize is the maximum file size to scan in bytes (0 = unlimited)
	MaxFileSize int64
	// Timeout is the per-scan operation timeout
	Timeout time.Duration
}

// DefaultScannerConfig returns a default scanner configuration
func DefaultScannerConfig() *ScannerConfig {
	return &ScannerConfig{
		Enabled:        true,
		Address:        "127.0.0.1:3310",
		ScanOnWrite:    true,
		ScanOnDownload: true,
		ScanOnExec:     true,
		MaxFileSize:    50 * 1024 * 1024, // 50 MB
		Timeout:        60 * time.Second,
	}
}

// Scanner provides high-level virus scanning operations
type Scanner struct {
	client *Client
	config *ScannerConfig
	mu     sync.RWMutex
	stats  ScanStats
}

// ScanStats tracks scanning statistics
type ScanStats struct {
	TotalScans   int64
	CleanScans   int64
	InfectedScans int64
	Errors       int64
	TotalBytes   int64
}

// NewScanner creates a new virus scanner
func NewScanner(cfg *ScannerConfig) *Scanner {
	return &Scanner{
		client: NewClientWithTimeout(cfg.Address, cfg.Timeout),
		config: cfg,
	}
}

// NewScannerWithClient creates a scanner with an existing client
func NewScannerWithClient(client *Client, cfg *ScannerConfig) *Scanner {
	return &Scanner{
		client: client,
		config: cfg,
	}
}

// ScanFile scans a file by its path
func (s *Scanner) ScanFile(ctx context.Context, filePath string) (*ScanResult, error) {
	if !s.config.Enabled {
		return &ScanResult{Path: filePath, Raw: "scanning disabled"}, nil
	}

	// Check file size limit
	if s.config.MaxFileSize > 0 {
		info, err := os.Stat(filePath)
		if err != nil {
			return nil, fmt.Errorf("failed to stat file: %w", err)
		}
		if info.Size() > s.config.MaxFileSize {
			s.recordScan(0, false, false)
			return &ScanResult{
				Path:     filePath,
				Raw:      fmt.Sprintf("file too large (%d bytes, max %d)", info.Size(), s.config.MaxFileSize),
			}, nil
		}
	}

	result, err := s.client.ScanFile(ctx, filePath)
	if err != nil {
		s.recordScan(0, false, true)
		return nil, fmt.Errorf("scan failed for %s: %w", filePath, err)
	}

	s.recordScan(0, result.Infected, false)

	if result.Infected {
		logger.WarnCF("clamav", "Virus detected", map[string]interface{}{
			"path":   filePath,
			"virus":  result.Virus,
			"raw":    result.Raw,
		})
	}

	return result, nil
}

// ScanContent scans content from an io.Reader using stream protocol
func (s *Scanner) ScanContent(ctx context.Context, content io.Reader, size int64) (*ScanResult, error) {
	if !s.config.Enabled {
		return &ScanResult{Raw: "scanning disabled"}, nil
	}

	// Check size limit
	if s.config.MaxFileSize > 0 && size > s.config.MaxFileSize {
		s.recordScan(size, false, false)
		return &ScanResult{
			Raw: fmt.Sprintf("content too large (%d bytes, max %d)", size, s.config.MaxFileSize),
		}, nil
	}

	result, err := s.client.ScanStream(ctx, content)
	if err != nil {
		s.recordScan(size, false, true)
		return nil, fmt.Errorf("stream scan failed: %w", err)
	}

	s.recordScan(size, result.Infected, false)

	return result, nil
}

// ScanContentBytes scans a byte slice
func (s *Scanner) ScanContentBytes(ctx context.Context, data []byte) (*ScanResult, error) {
	return s.ScanContent(ctx, strings.NewReader(string(data)), int64(len(data)))
}

// ScanDirectory scans all files in a directory
func (s *Scanner) ScanDirectory(ctx context.Context, dirPath string) ([]*ScanResult, error) {
	if !s.config.Enabled {
		return nil, nil
	}

	results, err := s.client.ContScan(ctx, dirPath)
	if err != nil {
		return nil, fmt.Errorf("directory scan failed for %s: %w", dirPath, err)
	}

	// Update stats
	for _, r := range results {
		s.recordScan(0, r.Infected, false)
		if r.Infected {
			logger.WarnCF("clamav", "Virus detected in directory scan", map[string]interface{}{
				"path":   r.Path,
				"virus":  r.Virus,
				"dir":    dirPath,
			})
		}
	}

	return results, nil
}

// ShouldScan checks if a file operation should trigger a scan based on config
func (s *Scanner) ShouldScan(operation string, filePath string) bool {
	if !s.config.Enabled {
		return false
	}

	switch operation {
	case "write_file", "edit_file", "append_file":
		return s.config.ScanOnWrite
	case "download":
		return s.config.ScanOnDownload
	case "exec", "execute_command":
		return s.config.ScanOnExec
	default:
		return false
	}
}

// ShouldScanFile checks if a specific file should be scanned based on extension
func (s *Scanner) ShouldScanFile(filePath string) bool {
	if !s.config.Enabled {
		return false
	}

	ext := strings.ToLower(filepath.Ext(filePath))

	// Skip known safe file types
	safeExtensions := map[string]bool{
		".txt": true, ".md": true, ".json": true, ".yaml": true, ".yml": true,
		".xml": true, ".csv": true, ".log": true, ".ini": true, ".toml": true,
		".html": true, ".css": true, ".js": true, ".ts": true,
	}

	if safeExtensions[ext] {
		return false
	}

	// Always scan executable types
	execExtensions := map[string]bool{
		".exe": true, ".dll": true, ".bat": true, ".cmd": true, ".ps1": true,
		".sh": true, ".so": true, ".dylib": true, ".msi": true, ".vbs": true,
		".com": true, ".scr": true, ".pif": true, ".jar": true, ".py": true,
	}

	if execExtensions[ext] {
		return true
	}

	// Scan unknown extensions (conservative approach)
	return ext == "" || true
}

// GetStats returns scan statistics
func (s *Scanner) GetStats() ScanStats {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.stats
}

// Ping checks if the scanner backend is available
func (s *Scanner) Ping(ctx context.Context) error {
	return s.client.Ping(ctx)
}

// recordScan updates scan statistics
func (s *Scanner) recordScan(bytes int64, infected bool, isError bool) {
	s.mu.Lock()
	defer s.mu.Unlock()

	s.stats.TotalScans++
	s.stats.TotalBytes += bytes

	if isError {
		s.stats.Errors++
	} else if infected {
		s.stats.InfectedScans++
	} else {
		s.stats.CleanScans++
	}
}
