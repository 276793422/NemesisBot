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

// StaticConfig represents the static cluster configuration (peers.toml)
// This file is created during onboard and contains the current node's information
// Users can manually edit this file to add known peers
type StaticConfig struct {
	Cluster ClusterMeta  `toml:"cluster"`
	Node    NodeInfo     `toml:"node"`
	Peers   []PeerConfig `toml:"peers"`
}

// ClusterMeta contains cluster metadata
type ClusterMeta struct {
	ID            string    `toml:"id"`
	AutoDiscovery bool      `toml:"auto_discovery"`
	LastUpdated   time.Time `toml:"last_updated"`
	RPCAuthToken  string    `toml:"rpc_auth_token"` // RPC authentication token
}

// NodeInfo contains information about the current node
type NodeInfo struct {
	ID           string   `toml:"id"`
	Name         string   `toml:"name"`
	Address      string   `toml:"address"`
	Role         string   `toml:"role"`     // Cluster role: manager, coordinator, worker, observer, standby
	Category     string   `toml:"category"` // Business category: design, development, testing, ops, deployment, analysis, general
	Tags         []string `toml:"tags"`     // Custom tags for flexible classification
	Capabilities []string `toml:"capabilities"`
}

// PeerConfig represents a peer node configuration
type PeerConfig struct {
	ID           string     `toml:"id"`
	Name         string     `toml:"name"`
	Address      string     `toml:"address"`   // Deprecated: Primary address for backward compatibility
	Addresses    []string   `toml:"addresses"` // List of all IP addresses
	RPCPort      int        `toml:"rpc_port"`  // RPC port number
	Role         string     `toml:"role"`      // Cluster role: manager, coordinator, worker, observer, standby
	Category     string     `toml:"category"`  // Business category: design, development, testing, ops, deployment, analysis, general
	Tags         []string   `toml:"tags"`      // Custom tags for flexible classification
	Capabilities []string   `toml:"capabilities"`
	Priority     int        `toml:"priority"`
	Enabled      bool       `toml:"enabled"`
	Status       PeerStatus `toml:"status"`
}

// PeerStatus contains runtime status of a peer
type PeerStatus struct {
	State           string    `toml:"state"`
	LastSeen        time.Time `toml:"last_seen"`
	Uptime          string    `toml:"uptime"` // Human-readable uptime
	TasksCompleted  int       `toml:"tasks_completed"`
	SuccessRate     float64   `toml:"success_rate"`
	AvgResponseTime int       `toml:"avg_response_time"` // milliseconds
	LastError       string    `toml:"last_error"`
}

// DynamicState represents the dynamic cluster state (state.toml)
// This file is automatically managed by the cluster module
// It contains runtime information about discovered peers
type DynamicState struct {
	Cluster    ClusterMeta  `toml:"cluster"`
	LocalNode  NodeInfo     `toml:"local_node"`
	Discovered []PeerConfig `toml:"discovered"`
	LastSync   time.Time    `toml:"last_sync"`
}

// LoadStaticConfig loads the static cluster configuration from peers.toml
func LoadStaticConfig(configPath string) (*StaticConfig, error) {
	// Check if file exists
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		return nil, fmt.Errorf("static config file not found: %s", configPath)
	}

	// Read file
	data, err := os.ReadFile(configPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read config file: %w", err)
	}

	// Parse TOML
	var config StaticConfig
	if err := toml.Unmarshal(data, &config); err != nil {
		return nil, fmt.Errorf("failed to parse TOML: %w", err)
	}

	return &config, nil
}

// SaveStaticConfig saves the static cluster configuration to peers.toml
func SaveStaticConfig(configPath string, config *StaticConfig) error {
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

// LoadDynamicState loads the dynamic cluster state from state.toml
func LoadDynamicState(statePath string) (*DynamicState, error) {
	// Check if file exists
	if _, err := os.Stat(statePath); os.IsNotExist(err) {
		// Return empty state if file doesn't exist
		return &DynamicState{
			Cluster: ClusterMeta{
				ID:            "auto-discovered",
				AutoDiscovery: true,
				LastUpdated:   time.Now(),
			},
			Discovered: []PeerConfig{},
			LastSync:   time.Now(),
		}, nil
	}

	// Read file
	data, err := os.ReadFile(statePath)
	if err != nil {
		return nil, fmt.Errorf("failed to read state file: %w", err)
	}

	// Parse TOML
	var state DynamicState
	if err := toml.Unmarshal(data, &state); err != nil {
		return nil, fmt.Errorf("failed to parse TOML: %w", err)
	}

	return &state, nil
}

// SaveDynamicState saves the dynamic cluster state to state.toml
func SaveDynamicState(statePath string, state *DynamicState) error {
	// Update last sync time
	state.LastSync = time.Now()
	state.Cluster.LastUpdated = time.Now()

	// Marshal to TOML
	data, err := toml.Marshal(state)
	if err != nil {
		return fmt.Errorf("failed to marshal TOML: %w", err)
	}

	// Ensure directory exists
	dir := filepath.Dir(statePath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("failed to create state directory: %w", err)
	}

	// Write to temporary file first
	tmpPath := statePath + ".tmp"
	if err := os.WriteFile(tmpPath, data, 0644); err != nil {
		return fmt.Errorf("failed to write state file: %w", err)
	}

	// Atomic rename
	if err := os.Rename(tmpPath, statePath); err != nil {
		os.Remove(tmpPath) // Clean up temp file
		return fmt.Errorf("failed to save state file: %w", err)
	}

	return nil
}

// CreateStaticConfig creates a default static configuration for the current node
func CreateStaticConfig(nodeID, nodeName, address string) *StaticConfig {
	return &StaticConfig{
		Cluster: ClusterMeta{
			ID:            "manual",
			AutoDiscovery: true,
			LastUpdated:   time.Now(),
		},
		Node: NodeInfo{
			ID:           nodeID,
			Name:         nodeName,
			Address:      address,
			Role:         "worker",
			Category:     "general",
			Tags:         []string{},
			Capabilities: []string{},
		},
		Peers: []PeerConfig{},
	}
}

// Legacy aliases for backward compatibility
type ClusterConfig = StaticConfig

// LoadConfig loads the cluster configuration (legacy alias)
func LoadConfig(configPath string) (*StaticConfig, error) {
	return LoadStaticConfig(configPath)
}

// SaveConfig saves the cluster configuration (legacy alias)
func SaveConfig(configPath string, config *StaticConfig) error {
	return SaveStaticConfig(configPath, config)
}

// DefaultConfig creates a default cluster configuration (legacy alias)
func DefaultConfig(nodeID string) *StaticConfig {
	return CreateStaticConfig(nodeID, "Bot "+nodeID, "")
}

// LoadOrCreateConfig loads existing config or creates a default one (legacy alias)
func LoadOrCreateConfig(configPath string, nodeID string) (*StaticConfig, error) {
	config, err := LoadStaticConfig(configPath)
	if err == nil {
		return config, nil
	}

	if os.IsNotExist(err) {
		return DefaultConfig(nodeID), nil
	}

	return nil, err
}
