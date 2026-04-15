// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package clamav

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
)

// GenerateClamdConfig generates a minimal clamd.conf for TCP mode
func GenerateClamdConfig(cfg *DaemonConfig) error {
	if cfg.ConfigFile == "" {
		return fmt.Errorf("config file path is required")
	}

	// Ensure parent directory exists
	if err := os.MkdirAll(filepath.Dir(cfg.ConfigFile), 0755); err != nil {
		return fmt.Errorf("failed to create config directory: %w", err)
	}

	var lines []string

	// TCP listening configuration
	host := "127.0.0.1"
	port := "3310"
	if cfg.ListenAddr != "" {
		parts := strings.SplitN(cfg.ListenAddr, ":", 2)
		if len(parts) == 2 {
			host = parts[0]
			port = parts[1]
		}
	}

	lines = append(lines,
		"# Auto-generated clamd.conf for NemesisBot",
		"TCPSocket "+port,
		"TCPAddr "+host,
		"",
	)

	// Database directory
	if cfg.DatabaseDir != "" {
		dbPath := cfg.DatabaseDir
		// Convert to forward slashes for ClamAV config
		dbPath = filepath.ToSlash(dbPath)
		lines = append(lines, "DatabaseDirectory "+dbPath)
	}

	// Temporary directory
	if cfg.TempDir != "" {
		tmpPath := filepath.ToSlash(cfg.TempDir)
		lines = append(lines, "TemporaryDirectory "+tmpPath)
	}

	// Logging
	lines = append(lines,
		"",
		"# Logging",
		"LogTime yes",
		"LogRotate yes",
		"LogFileMaxSize 10M",
	)

	// Scan options
	lines = append(lines,
		"",
		"# Scan options",
		"ScanPE yes",
		"ScanELF yes",
		"ScanOLE2 yes",
		"ScanPDF yes",
		"ScanSWF yes",
		"ScanXMLDOCS yes",
		"ScanHWP3 yes",
		"ScanMail yes",
		"ScanArchive yes",
		"MaxScanSize 100M",
		"MaxFileSize 50M",
	)

	// Platform-specific settings
	if runtime.GOOS == "windows" {
		lines = append(lines,
			"",
			"# Windows-specific",
			"FollowDirectorySymlinks no",
			"FollowFileSymlinks no",
		)
	}

	content := strings.Join(lines, "\n") + "\n"
	if err := os.WriteFile(cfg.ConfigFile, []byte(content), 0644); err != nil {
		return fmt.Errorf("failed to write clamd.conf: %w", err)
	}

	return nil
}

// GenerateFreshclamConfig generates a minimal freshclam.conf
func GenerateFreshclamConfig(dbDir string, configFile string) error {
	if configFile == "" {
		return fmt.Errorf("config file path is required")
	}

	if err := os.MkdirAll(filepath.Dir(configFile), 0755); err != nil {
		return fmt.Errorf("failed to create config directory: %w", err)
	}

	var lines []string

	lines = append(lines,
		"# Auto-generated freshclam.conf for NemesisBot",
	)

	if dbDir != "" {
		lines = append(lines, "DatabaseDirectory "+filepath.ToSlash(dbDir))
	}

	lines = append(lines,
		"",
		"# Database mirror (ClamAV official)",
		"DatabaseMirror database.clamav.net",
		"",
		"# Update settings",
		"Checks 24",
		"LogTime yes",
		"LogRotate yes",
	)

	// Ensure database directory exists
	if dbDir != "" {
		if err := os.MkdirAll(dbDir, 0755); err != nil {
			return fmt.Errorf("failed to create database directory: %w", err)
		}
	}

	content := strings.Join(lines, "\n") + "\n"
	if err := os.WriteFile(configFile, []byte(content), 0646); err != nil {
		return fmt.Errorf("failed to write freshclam.conf: %w", err)
	}

	return nil
}

// DetectClamAVPath attempts to find ClamAV installation on the system
func DetectClamAVPath() string {
	// Check common installation locations
	candidates := []string{}

	switch runtime.GOOS {
	case "windows":
		// Common Windows locations
		candidates = append(candidates,
			`C:\Program Files\ClamAV`,
			`C:\ClamAV`,
		)
		// Check PATH
		if p := findInPath("clamd.exe"); p != "" {
			candidates = append([]string{filepath.Dir(p)}, candidates...)
		}
	case "linux":
		candidates = append(candidates,
			"/usr/bin",
			"/usr/local/bin",
			"/usr/sbin",
		)
	case "darwin":
		candidates = append(candidates,
			"/usr/local/bin",
			"/opt/homebrew/bin",
			"/usr/bin",
		)
	}

	for _, dir := range candidates {
		exeName := "clamd"
		if runtime.GOOS == "windows" {
			exeName = "clamd.exe"
		}
		if _, err := os.Stat(filepath.Join(dir, exeName)); err == nil {
			return dir
		}
	}

	return ""
}

// findInPath finds an executable in the system PATH
func findInPath(name string) string {
	pathEnv := os.Getenv("PATH")
	sep := ":"
	if runtime.GOOS == "windows" {
		sep = ";"
	}

	for _, dir := range strings.Split(pathEnv, sep) {
		candidate := filepath.Join(dir, name)
		if _, err := os.Stat(candidate); err == nil {
			return candidate
		}
	}
	return ""
}
