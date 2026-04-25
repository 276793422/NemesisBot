package vector_test

import (
	"testing"

	"github.com/276793422/NemesisBot/module/memory/vector"
)

func TestLoadPlugin_NonexistentPath(t *testing.T) {
	_, err := vector.LoadPlugin("/nonexistent/path/to/plugin.dll")
	if err == nil {
		t.Error("expected error for nonexistent plugin path")
	}
}

func TestLoadPlugin_InvalidPath(t *testing.T) {
	_, err := vector.LoadPlugin("not_a_real_file.so")
	if err == nil {
		t.Error("expected error for invalid plugin path")
	}
}
