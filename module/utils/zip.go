// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package utils

import (
	"archive/zip"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
)

// ExtractZipFile extracts a ZIP file to the specified directory.
// It creates all necessary directories and extracts files with their permissions preserved.
func ExtractZipFile(zipPath, destDir string) error {
	// Open the ZIP file
	r, err := zip.OpenReader(zipPath)
	if err != nil {
		return fmt.Errorf("failed to open zip file: %w", err)
	}
	defer r.Close()

	// Ensure destination directory exists
	if err := os.MkdirAll(destDir, 0o755); err != nil {
		return fmt.Errorf("failed to create destination directory: %w", err)
	}

	// Extract each file
	for _, f := range r.File {
		err := extractFile(f, destDir)
		if err != nil {
			return fmt.Errorf("failed to extract %s: %w", f.Name, err)
		}
	}

	return nil
}

// extractFile extracts a single file from the ZIP archive
func extractFile(f *zip.File, destDir string) error {
	// Construct the full path for the file
	path := filepath.Join(destDir, f.Name)

	// Security check: ensure the path is within the destination directory
	if !isPathWithinDir(path, destDir) {
		return fmt.Errorf("invalid file path: %s (zip slip)", f.Name)
	}

	if f.Mode().IsDir() {
		// Create directory
		return os.MkdirAll(path, f.Mode())
	}

	// Create parent directory if it doesn't exist
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return fmt.Errorf("failed to create directory: %w", err)
	}

	// Create the file
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, f.Mode())
	if err != nil {
		return fmt.Errorf("failed to create file: %w", err)
	}
	defer file.Close()

	// Open the file in the ZIP archive
	rc, err := f.Open()
	if err != nil {
		return fmt.Errorf("failed to open file in zip: %w", err)
	}
	defer rc.Close()

	// Copy the file content
	_, err = io.Copy(file, rc)
	if err != nil {
		return fmt.Errorf("failed to write file: %w", err)
	}

	return nil
}

// IsPathWithinDir checks if the path is within the base directory (security check).
// Exported for use by other packages that need zip-slip style validation.
func IsPathWithinDir(path, baseDir string) bool {
	return isPathWithinDir(path, baseDir)
}

// isPathWithinDir checks if the path is within the base directory (security check)
func isPathWithinDir(path, baseDir string) bool {
	absPath, err := filepath.Abs(path)
	if err != nil {
		return false
	}

	absBaseDir, err := filepath.Abs(baseDir)
	if err != nil {
		return false
	}

	rel, err := filepath.Rel(absBaseDir, absPath)
	if err != nil {
		return false
	}

	// Check if the relative path starts with ".." which would indicate path traversal
	return !filepath.IsAbs(rel) && !strings.HasPrefix(rel, "..")
}
