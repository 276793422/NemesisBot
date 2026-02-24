// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package routing_test

import (
	"testing"

	. "github.com/276793422/NemesisBot/module/routing"
)

func TestNormalizeAgentID_Basic(t *testing.T) {
	result := NormalizeAgentID("My-Agent")
	if result != "my-agent" {
		t.Errorf("Expected 'my-agent', got '%s'", result)
	}
}

func TestNormalizeAgentID_Empty(t *testing.T) {
	result := NormalizeAgentID("")
	if result != "main" {
		t.Errorf("Expected 'main' for empty, got '%s'", result)
	}
}
