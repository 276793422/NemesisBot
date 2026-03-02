// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/BurntSushi/toml"
)

// LoadConfig loads the cluster configuration from TOML file
func LoadConfig(configPath string) (*ClusterConfig, error) {
	// Check if file exists
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		return nil, fmt.Errorf("config file not found: %s", configPath)
	}

	// Read file
	data, err := os.ReadFile(configPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read config file: %w", err)
	}

	// Parse TOML
	var config ClusterConfig
	if err := toml.Unmarshal(data, &config); err != nil {
		return nil, fmt.Errorf("failed to parse TOML: %w", err)
	}

	return &config, nil
}

// LoadOrCreateConfig loads existing config or creates a default one
func LoadOrCreateConfig(configPath string, nodeID string) (*ClusterConfig, error) {
	// Try to load existing config
	config, err := LoadConfig(configPath)
	if err == nil {
		return config, nil
	}

	// Config doesn't exist, create default
	if os.IsNotExist(err) {
		return DefaultConfig(nodeID), nil
	}

	return nil, err
}

// SaveConfig saves the cluster configuration to TOML file
func SaveConfig(configPath string, config *ClusterConfig) error {
	// Update last updated time
	config.Cluster.LastUpdated = time.Now()

	// Marshal to TOML
	data, err := toml.Marshal(config)
	if err != nil {
		return fmt.Errorf("failed to marshal TOML: %w", err)
	}

	// Ensure directory exists
	dir := filepath.Dir(configPath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("failed to create config directory: %w", err)
	}

	// Write to temporary file first
	tmpPath := configPath + ".tmp"
	if err := os.WriteFile(tmpPath, data, 0644); err != nil {
		return fmt.Errorf("failed to write config file: %w", err)
	}

	// Atomic rename
	if err := os.Rename(tmpPath, configPath); err != nil {
		os.Remove(tmpPath) // Clean up temp file
		return fmt.Errorf("failed to save config file: %w", err)
	}

	return nil
}

// DefaultConfig creates a default cluster configuration
func DefaultConfig(nodeID string) *ClusterConfig {
	return &ClusterConfig{
		Cluster: ClusterMeta{
			ID:            "auto-discovered",
			AutoDiscovery: true,
			LastUpdated:   time.Now(),
		},
		Node: NodeInfo{
			ID:           nodeID,
			Name:         "Bot " + nodeID,
			Address:      "",
			Role:         "worker",
			Capabilities: []string{},
		},
		Peers: []PeerConfig{},
	}
}
