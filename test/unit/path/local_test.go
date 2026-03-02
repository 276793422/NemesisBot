// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package path_test

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/path"
)

// TestDetectLocal tests the DetectLocal function
func TestDetectLocal(t *testing.T) {
	// Save current directory
	cwd, _ := os.Getwd()
	defer os.Chdir(cwd)

	// Test in a directory without .nemesisbot
	tempDir, err := os.MkdirTemp("", "test-nemesis-*")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(tempDir)

	os.Chdir(tempDir)
	if path.DetectLocal() {
		t.Error("DetectLocal() returned true in empty directory")
	}

	// Create .nemesisbot directory
	if err := os.Mkdir(".nemesisbot", 0755); err != nil {
		t.Fatal(err)
	}

	if !path.DetectLocal() {
		t.Error("DetectLocal() returned false when .nemesisbot exists")
	}
}

// TestLocalModePriority tests that LocalMode has highest priority
func TestLocalModePriority(t *testing.T) {
	// Save original state
	origLocal := path.LocalMode
	origHome := os.Getenv(path.EnvHome)
	defer func() {
		path.LocalMode = origLocal
		if origHome == "" {
			os.Unsetenv(path.EnvHome)
		} else {
			os.Setenv(path.EnvHome, origHome)
		}
	}()

	// Test 1: LocalMode overrides environment variable
	path.LocalMode = true
	os.Setenv(path.EnvHome, "/should/be/ignored")

	home, err := path.ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir() error = %v", err)
	}

	// Should be current directory's .nemesisbot, not the env var
	cwd, _ := os.Getwd()
	// Use filepath.Join for proper path handling
	expected := filepath.Join(cwd, ".nemesisbot")
	if home != expected {
		t.Errorf("ResolveHomeDir() with LocalMode = %q, want %q", home, expected)
	}

	// Test 2: Environment variable works when LocalMode is false
	path.LocalMode = false
	os.Setenv(path.EnvHome, "/custom/home")

	home, err = path.ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir() error = %v", err)
	}

	expected = "/custom/home"
	if home != expected {
		t.Errorf("ResolveHomeDir() with env var = %q, want %q", home, expected)
	}
}

// TestAutoDetectionOrder tests the priority order
func TestAutoDetectionOrder(t *testing.T) {
	// Save original state
	origLocal := path.LocalMode
	origHome := os.Getenv(path.EnvHome)
	origCwd, _ := os.Getwd()
	defer func() {
		path.LocalMode = origLocal
		os.Chdir(origCwd)
		if origHome == "" {
			os.Unsetenv(path.EnvHome)
		} else {
			os.Setenv(path.EnvHome, origHome)
		}
	}()

	// Create a temporary directory for testing
	tempDir, err := os.MkdirTemp("", "test-nemesis-*")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(tempDir)
	os.Chdir(tempDir)

	// Test 1: No .nemesisbot, no env var -> use default
	path.LocalMode = false
	os.Unsetenv(path.EnvHome)

	home, err := path.ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir() error = %v", err)
	}

	userHome, _ := os.UserHomeDir()
	expected := filepath.Join(userHome, ".nemesisbot")
	if home != expected {
		t.Errorf("ResolveHomeDir() default = %q, want %q", home, expected)
	}

	// Test 2: .nemesisbot exists in current directory -> use it
	if err := os.Mkdir(".nemesisbot", 0755); err != nil {
		t.Fatal(err)
	}

	home, err = path.ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir() error = %v", err)
	}

	expected = filepath.Join(tempDir, ".nemesisbot")
	if home != expected {
		t.Errorf("ResolveHomeDir() with auto-detect = %q, want %q", home, expected)
	}

	// Test 3: Environment variable overrides auto-detection
	os.Setenv(path.EnvHome, "/env/override")

	home, err = path.ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir() error = %v", err)
	}

	expected = "/env/override"
	if home != expected {
		t.Errorf("ResolveHomeDir() env override = %q, want %q", home, expected)
	}

	// Test 4: LocalMode overrides everything
	path.LocalMode = true

	home, err = path.ResolveHomeDir()
	if err != nil {
		t.Fatalf("ResolveHomeDir() error = %v", err)
	}

	expected = filepath.Join(tempDir, ".nemesisbot")
	if home != expected {
		t.Errorf("ResolveHomeDir() LocalMode = %q, want %q", home, expected)
	}
}
