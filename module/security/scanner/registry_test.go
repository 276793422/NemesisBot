// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package scanner

import (
	"encoding/json"
	"testing"
)

func TestRegistry_CreateClamAV(t *testing.T) {
	engine, err := CreateEngine("clamav", []byte(`{"address":"127.0.0.1:3310"}`))
	if err != nil {
		t.Fatalf("CreateEngine(clamav) error: %v", err)
	}
	if engine.Name() != "clamav" {
		t.Errorf("Name() = %q, want %q", engine.Name(), "clamav")
	}
}

func TestRegistry_CreateUnknown(t *testing.T) {
	_, err := CreateEngine("unknown_engine", json.RawMessage(`{}`))
	if err == nil {
		t.Error("Expected error for unknown engine")
	}
}

func TestRegistry_AvailableEngines(t *testing.T) {
	engines := AvailableEngines()
	if len(engines) < 1 {
		t.Error("Should have at least one available engine")
	}
	found := false
	for _, e := range engines {
		if e == "clamav" {
			found = true
		}
	}
	if !found {
		t.Error("clamav should be in available engines")
	}
}

func TestRegistry_CreateClamAV_InvalidConfig(t *testing.T) {
	_, err := CreateEngine("clamav", []byte(`{invalid`))
	if err == nil {
		t.Error("Expected error for invalid config")
	}
}

func TestRegistry_CreateClamAV_EmptyConfig(t *testing.T) {
	engine, err := CreateEngine("clamav", nil)
	if err != nil {
		t.Fatalf("CreateEngine(clamav, nil) error: %v", err)
	}
	if engine == nil {
		t.Error("Engine should not be nil")
	}
}
