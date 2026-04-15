// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package scanner

import (
	"context"
	"encoding/json"
	"time"

	"github.com/276793422/NemesisBot/module/config"
)

// Install status constants.
const (
	InstallStatusPending   = "pending"
	InstallStatusInstalled = "installed"
	InstallStatusFailed    = "failed"
)

// Database status constants.
const (
	DBStatusMissing = "missing"
	DBStatusReady   = "ready"
	DBStatusStale   = "stale"
)

// VirusScanner is the universal interface for virus scanning engines.
// Each engine (ClamAV, YARA, Windows Defender, etc.) implements this interface.
type VirusScanner interface {
	// Name returns the engine identifier (e.g. "clamav").
	Name() string

	// GetInfo returns engine metadata (version, address, etc.).
	GetInfo(ctx context.Context) (*EngineInfo, error)

	// Lifecycle
	Download(ctx context.Context, dir string) error
	Validate(dir string) error
	Setup(config json.RawMessage) error
	Start(ctx context.Context) error
	Stop() error
	IsReady() bool

	// Scanning
	ScanFile(ctx context.Context, path string) (*ScanResult, error)
	ScanContent(ctx context.Context, data []byte) (*ScanResult, error)
	ScanDirectory(ctx context.Context, path string) ([]*ScanResult, error)

	// Database
	GetDatabaseStatus(ctx context.Context) (*DatabaseStatus, error)
	UpdateDatabase(ctx context.Context) error

	// GetStats returns engine-specific statistics.
	GetStats() map[string]interface{}
}

// EngineInfo holds metadata about a scanner engine.
type EngineInfo struct {
	Name      string `json:"name"`
	Version   string `json:"version,omitempty"`
	Address   string `json:"address,omitempty"`
	Ready     bool   `json:"ready"`
	StartTime string `json:"start_time,omitempty"`
}

// ScanResult represents the outcome of a single scan operation.
type ScanResult struct {
	Path     string `json:"path,omitempty"`
	Infected bool   `json:"infected"`
	Virus    string `json:"virus,omitempty"`
	Raw      string `json:"raw,omitempty"`
	Engine   string `json:"engine"`
	Duration string `json:"duration,omitempty"`
}

// Clean returns true when the scanned subject is malware-free.
func (r *ScanResult) Clean() bool {
	return !r.Infected
}

// ScanChainResult aggregates results from multiple engines in the scan chain.
type ScanChainResult struct {
	Clean    bool           `json:"clean"`
	Blocked  bool           `json:"blocked"`
	Engine   string         `json:"engine,omitempty"`
	Virus    string         `json:"virus,omitempty"`
	Path     string         `json:"path,omitempty"`
	Results  []*ScanResult  `json:"results,omitempty"`
	Duration time.Duration  `json:"-"`
}

// DatabaseStatus reports the state of a scanner's virus definitions.
type DatabaseStatus struct {
	Available  bool      `json:"available"`
	Version    string    `json:"version,omitempty"`
	LastUpdate time.Time `json:"last_update,omitempty"`
	Path       string    `json:"path,omitempty"`
	SizeBytes  int64     `json:"size_bytes,omitempty"`
}

// InstallableEngine extends VirusScanner with install/database detection capabilities.
// Only engines that can be downloaded and installed need to implement this interface.
type InstallableEngine interface {
	VirusScanner

	// TargetExecutables returns the executable names to look for on the current OS.
	TargetExecutables() []string

	// DetectInstallPath recursively searches dir for target executables,
	// returning the directory containing the first match.
	DetectInstallPath(dir string) (string, error)

	// DatabaseFileName returns the primary database file name (e.g. "main.cvd").
	DatabaseFileName() string

	// GetEngineState returns a pointer to the engine's state for external read/write.
	GetEngineState() *config.EngineState
}
