// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

// TestClusterConfig_RPCAuthToken tests RPC auth token configuration
func TestClusterConfig_RPCAuthToken(t *testing.T) {
	tests := []struct {
		name          string
		configContent string
		expectedToken string
		description   string
	}{
		{
			name: "token_present",
			configContent: `
[cluster]
id = "test-node"
auto_discovery = true
rpc_auth_token = "my-secret-token-123"
`,
			expectedToken: "my-secret-token-123",
			description:   "Should load token from config",
		},
		{
			name: "token_empty",
			configContent: `
[cluster]
id = "test-node"
auto_discovery = true
rpc_auth_token = ""
`,
			expectedToken: "",
			description:   "Empty token should result in empty string",
		},
		{
			name: "token_absent",
			configContent: `
[cluster]
id = "test-node"
auto_discovery = true
`,
			expectedToken: "",
			description:   "Missing token field should result in empty string",
		},
		{
			name: "token_with_special_chars",
			configContent: `
[cluster]
id = "test-node"
rpc_auth_token = "token-with-special!@#$%^&*()"
`,
			expectedToken: "token-with-special!@#$%^&*()",
			description:   "Should preserve special characters in token",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Create temporary directory
			tmpDir := t.TempDir()
			configPath := filepath.Join(tmpDir, "peers.toml")

			// Write config
			if err := os.WriteFile(configPath, []byte(tt.configContent), 0644); err != nil {
				t.Fatalf("Failed to write config: %v", err)
			}

			// Load config
			config, err := LoadStaticConfig(configPath)
			if err != nil {
				t.Fatalf("Failed to load config: %v", err)
			}

			// Verify token
			if config.Cluster.RPCAuthToken != tt.expectedToken {
				t.Errorf("%s: expected token %q, got %q",
					tt.description, tt.expectedToken, config.Cluster.RPCAuthToken)
			}
		})
	}
}

// TestClusterConfig_SaveAndLoad tests saving and loading configuration
func TestClusterConfig_SaveAndLoad(t *testing.T) {
	// Create temporary directory
	tmpDir := t.TempDir()

	// Create initial config
	config := &StaticConfig{
		Cluster: ClusterMeta{
			ID:            "test-node",
			AutoDiscovery: true,
			LastUpdated:   time.Now(),
			RPCAuthToken:  "initial-token",
		},
	}

	// Save config
	configPath := filepath.Join(tmpDir, "peers.toml")
	if err := SaveStaticConfig(configPath, config); err != nil {
		t.Fatalf("Failed to save config: %v", err)
	}

	// Load config back
	loaded, err := LoadStaticConfig(configPath)
	if err != nil {
		t.Fatalf("Failed to load config: %v", err)
	}

	// Verify loaded config
	if loaded.Cluster.ID != config.Cluster.ID {
		t.Errorf("Expected ID %s, got %s", config.Cluster.ID, loaded.Cluster.ID)
	}
	if loaded.Cluster.RPCAuthToken != config.Cluster.RPCAuthToken {
		t.Errorf("Expected token %s, got %s", config.Cluster.RPCAuthToken, loaded.Cluster.RPCAuthToken)
	}

	// Modify and save again
	loaded.Cluster.RPCAuthToken = "updated-token"
	if err := SaveStaticConfig(configPath, loaded); err != nil {
		t.Fatalf("Failed to save updated config: %v", err)
	}

	// Load again
	loaded2, err := LoadStaticConfig(configPath)
	if err != nil {
		t.Fatalf("Failed to load updated config: %v", err)
	}

	// Verify update
	if loaded2.Cluster.RPCAuthToken != "updated-token" {
		t.Errorf("Expected updated token, got %s", loaded2.Cluster.RPCAuthToken)
	}
}

// TestClusterConfig_BackwardCompatibility tests backward compatibility
func TestClusterConfig_BackwardCompatibility(t *testing.T) {
	// Create temporary directory
	tmpDir := t.TempDir()

	// Create config without RPCAuthToken (old format)
	configContent := `
[cluster]
id = "old-node"
auto_discovery = true
`
	configPath := filepath.Join(tmpDir, "peers.toml")
	if err := os.WriteFile(configPath, []byte(configContent), 0644); err != nil {
		t.Fatalf("Failed to write old config: %v", err)
	}

	// Load config
	config, err := LoadStaticConfig(configPath)
	if err != nil {
		t.Fatalf("Failed to load old config: %v", err)
	}

	// Verify token is empty (backward compatible)
	if config.Cluster.RPCAuthToken != "" {
		t.Errorf("Expected empty token for old config, got %s", config.Cluster.RPCAuthToken)
	}

	// Verify other fields are still loaded
	if config.Cluster.ID != "old-node" {
		t.Errorf("Expected ID 'old-node', got %s", config.Cluster.ID)
	}
	if config.Cluster.AutoDiscovery != true {
		t.Errorf("Expected AutoDiscovery true, got %v", config.Cluster.AutoDiscovery)
	}
}
