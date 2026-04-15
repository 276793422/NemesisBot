// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package scanner

import (
	"encoding/json"
	"fmt"
)

// CreateEngine instantiates a VirusScanner by engine name.
func CreateEngine(name string, config json.RawMessage) (VirusScanner, error) {
	switch name {
	case "clamav":
		return NewClamAVEngine(config)
	default:
		return nil, fmt.Errorf("unknown scanner engine: %s", name)
	}
}

// AvailableEngines lists all built-in engine names.
func AvailableEngines() []string {
	return []string{"clamav"}
}
