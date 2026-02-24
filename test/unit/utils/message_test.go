// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package utils_test

import (
	"testing"

	. "github.com/276793422/NemesisBot/module/utils"
)

func TestSplitMessage_Basic(t *testing.T) {
	message := "Hello World"
	parts := SplitMessage(message, 1000)

	if len(parts) != 1 {
		t.Errorf("Expected 1 part, got %d", len(parts))
	}
	if parts[0] != message {
		t.Errorf("Message should not be split")
	}
}

func TestSplitMessage_LongMessage(t *testing.T) {
	message := string(make([]byte, 3000))
	parts := SplitMessage(message, 1000)

	// We expect at least 3 parts, maybe more depending on implementation
	if len(parts) < 3 {
		t.Errorf("Expected at least 3 parts, got %d", len(parts))
	}

	// Verify total content preserved
	recombined := ""
	for _, part := range parts {
		recombined += part
	}
	if recombined != message {
		t.Error("Recombined message doesn't match original")
	}
}
