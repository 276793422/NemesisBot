// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Module - Embedded Static Files

package web

import (
	"embed"
	"errors"
	"io/fs"
)

// staticFiles holds the embedded static files (all: prefix for recursive embedding)
//
//go:embed all:static
var staticFiles embed.FS

// StaticFiles returns the static files filesystem.
// Returns an error if the static directory is not properly embedded.
func StaticFiles() (fs.FS, error) {
	// Create a sub-filesystem that removes the "static/" prefix
	sub, err := fs.Sub(staticFiles, "static")
	if err != nil {
		return nil, errors.New("static files not embedded: ensure static directory exists")
	}
	return sub, nil
}
