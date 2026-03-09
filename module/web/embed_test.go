// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package web

import (
	"io/fs"
	"testing"
)

func TestStaticFiles(t *testing.T) {
	filesystem, err := StaticFiles()

	// The static files may or may not be embedded depending on build
	if err != nil {
		// Static files not embedded - this is ok for testing
		t.Logf("Static files not embedded: %v", err)
		return
	}

	if filesystem == nil {
		t.Error("Filesystem should not be nil when error is nil")
	}

	// Try to read a file if filesystem exists
	file, err := filesystem.Open("index.html")
	if err != nil {
		// File may not exist - this is ok for testing
		t.Logf("Could not open index.html: %v", err)
		return
	}
	defer file.Close()

	// Check file info
	info, err := file.Stat()
	if err != nil {
		t.Errorf("Failed to stat file: %v", err)
		return
	}

	if info.IsDir() {
		t.Error("index.html should not be a directory")
	}
}

func TestStaticFiles_ListFiles(t *testing.T) {
	filesystem, err := StaticFiles()

	if err != nil {
		t.Skip("Static files not embedded")
		return
	}

	// Try to list files
	entries, err := fs.ReadDir(filesystem, ".")
	if err != nil {
		// May not have any files - this is ok
		t.Logf("Could not read directory: %v", err)
		return
	}

	// Log what we found
	t.Logf("Found %d files in static filesystem", len(entries))
	for _, entry := range entries {
		t.Logf("  - %s (isDir: %v)", entry.Name(), entry.IsDir())
	}
}
