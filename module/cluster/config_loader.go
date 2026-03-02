// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/276793422/NemesisBot/module/path"
	"github.com/caarlos0/env/v11"
)

// AppConfig contains cluster application configuration (loaded from config.cluster.json)
// This is separate from ClusterConfig which contains runtime state (peers.toml)
type AppConfig struct {
	Enabled           bool `json:"enabled" env:"CLUSTER_ENABLED"`
	Port              int `json:"port" env:"CLUSTER_UDP_PORT"`
	RPCPort           int `json:"rpc_port" env:"CLUSTER_RPC_PORT"`
	BroadcastInterval int `json:"broadcast_interval" env:"CLUSTER_BROADCAST_INTERVAL"`
}

// DefaultAppConfig returns the default cluster application configuration
func DefaultAppConfig() *AppConfig {
	return &AppConfig{
		Enabled:           false,
		Port:              49100,
		RPCPort:           49200,
		BroadcastInterval: 30,
	}
}

// LoadAppConfig loads cluster application configuration from workspace/config/config.cluster.json
// If the file doesn't exist, loads from embedded default config
func LoadAppConfig(workspace string) (*AppConfig, error) {
	configPath := path.ResolveClusterConfigPathInWorkspace(workspace)

	// Try to load user config first
	if _, err := os.Stat(configPath); err == nil {
		// User config exists, load it
		data, err := os.ReadFile(configPath)
		if err != nil {
			return nil, fmt.Errorf("failed to read config file: %w", err)
		}

		var cfg AppConfig
		if err := json.Unmarshal(data, &cfg); err != nil {
			return nil, fmt.Errorf("failed to parse JSON: %w", err)
		}

		// Override with environment variables if set
		if err := env.Parse(&cfg); err != nil {
			return nil, fmt.Errorf("failed to parse environment variables: %w", err)
		}

		return &cfg, nil
	}

	// User config doesn't exist, load embedded default
	// Default config (hardcoded, will be embedded from config/config.cluster.default.json at build time)
	defaultConfigJSON := []byte(`{
  "enabled": false,
  "port": 49100,
  "rpc_port": 49200,
  "broadcast_interval": 30
}`)

	var cfg AppConfig
	if err := json.Unmarshal(defaultConfigJSON, &cfg); err != nil {
		return nil, fmt.Errorf("failed to parse embedded default config: %w", err)
	}

	// Override with environment variables if set
	if err := env.Parse(&cfg); err != nil {
		return nil, fmt.Errorf("failed to parse environment variables: %w", err)
	}

	return &cfg, nil
}

// SaveAppConfig saves cluster application configuration to workspace/config/config.cluster.json
func SaveAppConfig(workspace string, cfg *AppConfig) error {
	configPath := path.ResolveClusterConfigPathInWorkspace(workspace)

	// Marshal to JSON
	data, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal JSON: %w", err)
	}

	// Ensure directory exists
	configDir := filepath.Dir(configPath)
	if err := os.MkdirAll(configDir, 0755); err != nil {
		return fmt.Errorf("failed to create config directory: %w", err)
	}

	// Write file
	if err := os.WriteFile(configPath, data, 0644); err != nil {
		return fmt.Errorf("failed to write config file: %w", err)
	}

	return nil
}
