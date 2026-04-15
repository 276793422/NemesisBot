// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package clamav

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// ScanHook integrates virus scanning into the security middleware flow.
// It is called by SecurityPlugin before/during tool execution to check for malware.
type ScanHook struct {
	scanner *Scanner
}

// NewScanHook creates a new scan hook for middleware integration
func NewScanHook(scanner *Scanner) *ScanHook {
	return &ScanHook{scanner: scanner}
}

// ScanToolInvocation determines if a tool invocation needs virus scanning
// and performs the scan if needed. Returns (clean, error).
func (h *ScanHook) ScanToolInvocation(ctx context.Context, toolName string, args map[string]interface{}) (bool, error) {
	if h.scanner == nil || !h.scanner.config.Enabled {
		return true, nil
	}

	// Determine what to scan based on tool type
	switch toolName {
	case "write_file", "edit_file", "append_file":
		if !h.scanner.config.ScanOnWrite {
			return true, nil
		}
		return h.scanFileWriteArgs(ctx, toolName, args)

	case "download":
		if !h.scanner.config.ScanOnDownload {
			return true, nil
		}
		return h.scanDownloadArgs(ctx, args)

	case "exec", "execute_command":
		if !h.scanner.config.ScanOnExec {
			return true, nil
		}
		return h.scanExecArgs(ctx, args)

	default:
		return true, nil
	}
}

// ScanFilePath scans a specific file path and returns whether it's clean
func (h *ScanHook) ScanFilePath(ctx context.Context, filePath string) (bool, *ScanResult, error) {
	// Check if file exists
	if _, err := os.Stat(filePath); os.IsNotExist(err) {
		// File doesn't exist yet, nothing to scan
		return true, nil, nil
	}

	// Check if we should scan this file type
	if !h.scanner.ShouldScanFile(filePath) {
		return true, nil, nil
	}

	result, err := h.scanner.ScanFile(ctx, filePath)
	if err != nil {
		return false, nil, fmt.Errorf("virus scan failed: %w", err)
	}

	if result.Infected {
		return false, result, nil
	}

	return true, result, nil
}

// ScanDownloadedFile scans a downloaded file after it's been saved
func (h *ScanHook) ScanDownloadedFile(ctx context.Context, savePath string) (bool, *ScanResult, error) {
	// Wait for the file to be fully written
	if _, err := os.Stat(savePath); err != nil {
		return true, nil, nil // File not found, skip scan
	}

	result, err := h.scanner.ScanFile(ctx, savePath)
	if err != nil {
		logger.ErrorCF("clamav", "Download scan failed", map[string]interface{}{
			"path":  savePath,
			"error": err.Error(),
		})
		// Don't block on scan failure, but log it
		return true, nil, fmt.Errorf("virus scan failed: %w", err)
	}

	if result.Infected {
		logger.WarnCF("clamav", "Downloaded file is infected", map[string]interface{}{
			"path":   savePath,
			"virus":  result.Virus,
		})
		// Attempt to remove the infected file
		os.Remove(savePath)
		return false, result, nil
	}

	return true, result, nil
}

// scanFileWriteArgs scans content being written to a file
func (h *ScanHook) scanFileWriteArgs(ctx context.Context, toolName string, args map[string]interface{}) (bool, error) {
	// Get the file path
	pathStr, ok := args["path"].(string)
	if !ok || pathStr == "" {
		return true, nil
	}

	// For write_file, scan the content if provided
	content, hasContent := args["content"].(string)
	if hasContent && content != "" {
		result, err := h.scanner.ScanContentBytes(ctx, []byte(content))
		if err != nil {
			logger.ErrorCF("clamav", "Content scan error", map[string]interface{}{
				"path":  pathStr,
				"error": err.Error(),
			})
			// Don't block on scan failure
			return true, nil
		}
		if result.Infected {
			return false, fmt.Errorf("virus detected in content: %s (virus: %s)", pathStr, result.Virus)
		}
	}

	// For edit_file, also scan the new text
	if toolName == "edit_file" {
		newText, hasNewText := args["new_text"].(string)
		if hasNewText && newText != "" {
			result, err := h.scanner.ScanContentBytes(ctx, []byte(newText))
			if err != nil {
				return true, nil
			}
			if result.Infected {
				return false, fmt.Errorf("virus detected in edit content: %s (virus: %s)", pathStr, result.Virus)
			}
		}
	}

	return true, nil
}

// scanDownloadArgs handles download tool scanning
func (h *ScanHook) scanDownloadArgs(ctx context.Context, args map[string]interface{}) (bool, error) {
	savePath, ok := args["save_path"].(string)
	if !ok || savePath == "" {
		return true, nil
	}

	// The file may not exist yet (download hasn't happened)
	// This hook is called pre-execution, so we just note the path
	// Post-execution scanning should be done in the result handler
	clean, _, err := h.ScanFilePath(ctx, savePath)
	return clean, err
}

// scanExecArgs handles exec tool scanning
func (h *ScanHook) scanExecArgs(ctx context.Context, args map[string]interface{}) (bool, error) {
	command, ok := args["command"].(string)
	if !ok || command == "" {
		return true, nil
	}

	// Extract executable path from command
	execPath := extractExecutablePath(command)
	if execPath == "" {
		return true, nil
	}

	// Resolve to absolute path
	absPath, err := filepath.Abs(execPath)
	if err != nil {
		return true, nil // Can't resolve, skip scan
	}

	// Check if file exists
	if _, err := os.Stat(absPath); os.IsNotExist(err) {
		return true, nil // File doesn't exist, skip
	}

	clean, result, err := h.ScanFilePath(ctx, absPath)
	if err != nil {
		return true, nil // Scan failed, don't block
	}

	if !clean && result != nil {
		return false, fmt.Errorf("executable is infected: %s (virus: %s)", absPath, result.Virus)
	}

	return true, nil
}

// extractExecutablePath tries to extract an executable path from a command string
func extractExecutablePath(command string) string {
	// Remove surrounding quotes
	cmd := strings.Trim(command, "\"'")

	// Split by space to get the first token (the executable)
	parts := strings.Fields(cmd)
	if len(parts) == 0 {
		return ""
	}

	execPath := parts[0]

	// On Windows, handle common patterns
	if runtime.GOOS == "windows" {
		// Handle "C:\path\to\exe" or just "exe"
		if strings.Contains(execPath, "\\") || strings.Contains(execPath, "/") {
			return execPath
		}
		// Bare command name, try to find in PATH
		return "" // Let the OS handle it
	}

	// On Unix, handle paths
	if strings.Contains(execPath, "/") {
		return execPath
	}

	return ""
}

// GetScanner returns the underlying scanner for direct access
func (h *ScanHook) GetScanner() *Scanner {
	return h.scanner
}

// HealthCheck verifies the scanner backend is accessible
func (h *ScanHook) HealthCheck(ctx context.Context) error {
	if h.scanner == nil {
		return fmt.Errorf("scanner not initialized")
	}

	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()

	return h.scanner.Ping(ctx)
}

// FormatScanResult formats a scan result for audit logging
func FormatScanResult(result *ScanResult) string {
	if result == nil {
		return "no scan performed"
	}
	if result.Infected {
		return fmt.Sprintf("INFECTED: %s (virus: %s)", result.Path, result.Virus)
	}
	return fmt.Sprintf("CLEAN: %s", result.Path)
}
