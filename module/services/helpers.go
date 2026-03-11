package services

import (
	"os"
	"path/filepath"

	"github.com/276793422/NemesisBot/module/path"
)

// GetConfigPath returns the path to the configuration file
// This is a wrapper around path.PathManager to avoid import cycle
func GetConfigPath() string {
	// Check if local mode is enabled
	if path.LocalMode || path.DetectLocal() {
		return filepath.Join(".nemesisbot", "config.json")
	}

	// Use default home directory
	homeDir, err := path.ResolveHomeDir()
	if err != nil {
		// Fallback to home directory
		homeDir, _ = os.UserHomeDir()
	}

	return filepath.Join(homeDir, ".nemesisbot", "config.json")
}

// ShouldSkipHeartbeatForBootstrap checks if BOOTSTRAP.md exists
// If it exists, heartbeat LLM call should be skipped
func ShouldSkipHeartbeatForBootstrap(workspace string) bool {
	bootstrapPath := filepath.Join(workspace, "BOOTSTRAP.md")
	if _, err := os.Stat(bootstrapPath); err == nil {
		return true
	}
	return false
}
