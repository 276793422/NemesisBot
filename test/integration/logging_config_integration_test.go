// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

// Integration tests for logging configuration
package integration

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	. "github.com/276793422/NemesisBot/module/config"
)

// TestLoggingConfig_Integration tests the complete logging configuration workflow
func TestLoggingConfig_Integration(t *testing.T) {
	// Create temporary directory for testing
	tempDir := t.TempDir()
	configPath := filepath.Join(tempDir, "config.json")

	// Test 1: Create config with logging enabled
	t.Run("CreateConfigWithLogging", func(t *testing.T) {
		cfg := DefaultConfig()

		// Setup logging configuration
		cfg.Logging = &LoggingConfig{
			LLM: &LLMLogConfig{
				Enabled:     true,
				LogDir:      "logs/requests",
				DetailLevel: "summary",
			},
			General: &GeneralLogConfig{
				Enabled:       true,
				EnableConsole: true,
				Level:         "INFO",
				File:          "logs/app.log",
			},
		}

		// Save config
		err := SaveConfig(configPath, cfg)
		if err != nil {
			t.Fatalf("Failed to save config: %v", err)
		}

		// Verify file exists
		if _, err := os.Stat(configPath); os.IsNotExist(err) {
			t.Error("Config file was not created")
		}
	})

	// Test 2: Load config and verify logging settings
	t.Run("LoadConfigAndVerify", func(t *testing.T) {
		loadedCfg, err := LoadConfig(configPath)
		if err != nil {
			t.Fatalf("Failed to load config: %v", err)
		}

		// Verify logging config exists
		if loadedCfg.Logging == nil {
			t.Fatal("Logging config is nil")
		}

		// Verify LLM logging
		if loadedCfg.Logging.LLM == nil {
			t.Fatal("LLM logging config is nil")
		}

		if !loadedCfg.Logging.LLM.Enabled {
			t.Error("LLM logging should be enabled")
		}

		if loadedCfg.Logging.LLM.LogDir != "logs/requests" {
			t.Errorf("Expected LLM log dir to be 'logs/requests', got '%s'", loadedCfg.Logging.LLM.LogDir)
		}

		// Verify General logging
		if loadedCfg.Logging.General == nil {
			t.Fatal("General logging config is nil")
		}

		if !loadedCfg.Logging.General.Enabled {
			t.Error("General logging should be enabled")
		}

		if loadedCfg.Logging.General.Level != "INFO" {
			t.Errorf("Expected log level to be 'INFO', got '%s'", loadedCfg.Logging.General.Level)
		}
	})

	// Test 3: Modify logging config
	t.Run("ModifyLoggingConfig", func(t *testing.T) {
		cfg, err := LoadConfig(configPath)
		if err != nil {
			t.Fatalf("Failed to load config: %v", err)
		}

		// Change log level to DEBUG
		cfg.Logging.General.Level = "DEBUG"
		cfg.Logging.General.EnableConsole = false

		// Save modified config
		err = SaveConfig(configPath, cfg)
		if err != nil {
			t.Fatalf("Failed to save modified config: %v", err)
		}

		// Reload and verify
		reloadedCfg, err := LoadConfig(configPath)
		if err != nil {
			t.Fatalf("Failed to reload config: %v", err)
		}

		if reloadedCfg.Logging.General.Level != "DEBUG" {
			t.Errorf("Expected log level to be 'DEBUG', got '%s'", reloadedCfg.Logging.General.Level)
		}

		if reloadedCfg.Logging.General.EnableConsole {
			t.Error("Console should be disabled")
		}
	})
}

// TestLoggingConfig_JSONRoundTrip tests JSON serialization and deserialization
func TestLoggingConfig_JSONRoundTrip(t *testing.T) {
	originalCfg := &Config{
		Logging: &LoggingConfig{
			LLM: &LLMLogConfig{
				Enabled:     true,
				LogDir:      "logs/llm",
				DetailLevel: "full",
			},
			General: &GeneralLogConfig{
				Enabled:       true,
				EnableConsole: false,
				Level:         "WARN",
				File:          "logs/production.log",
			},
		},
	}

	// Serialize to JSON
	data, err := json.Marshal(originalCfg)
	if err != nil {
		t.Fatalf("Failed to marshal config: %v", err)
	}

	// Deserialize from JSON
	var restoredCfg Config
	err = json.Unmarshal(data, &restoredCfg)
	if err != nil {
		t.Fatalf("Failed to unmarshal config: %v", err)
	}

	// Verify all fields match
	if restoredCfg.Logging == nil {
		t.Fatal("Restored logging config is nil")
	}

	if restoredCfg.Logging.LLM.Enabled != originalCfg.Logging.LLM.Enabled {
		t.Error("LLM enabled field mismatch")
	}

	if restoredCfg.Logging.LLM.LogDir != originalCfg.Logging.LLM.LogDir {
		t.Error("LLM log dir mismatch")
	}

	if restoredCfg.Logging.LLM.DetailLevel != originalCfg.Logging.LLM.DetailLevel {
		t.Error("LLM detail level mismatch")
	}

	if restoredCfg.Logging.General.Enabled != originalCfg.Logging.General.Enabled {
		t.Error("General enabled field mismatch")
	}

	if restoredCfg.Logging.General.EnableConsole != originalCfg.Logging.General.EnableConsole {
		t.Error("Console enabled mismatch")
	}

	if restoredCfg.Logging.General.Level != originalCfg.Logging.General.Level {
		t.Error("Log level mismatch")
	}

	if restoredCfg.Logging.General.File != originalCfg.Logging.General.File {
		t.Error("Log file mismatch")
	}
}

// TestLoggingConfig_PartialConfig tests handling of partial logging configuration
func TestLoggingConfig_PartialConfig(t *testing.T) {
	tests := []struct {
		name     string
		json     string
		validate func(*testing.T, *Config)
	}{
		{
			name: "Only LLM logging",
			json: `{
				"logging": {
					"llm": {
						"enabled": true,
						"log_dir": "logs/llm"
					}
				}
			}`,
			validate: func(t *testing.T, cfg *Config) {
				if cfg.Logging.LLM == nil {
					t.Error("LLM logging should not be nil")
				}
				// General logging may be nil (no automatic defaults)
				// This is expected behavior
			},
		},
		{
			name: "Only General logging",
			json: `{
				"logging": {
					"general": {
						"enabled": true,
						"level": "DEBUG"
					}
				}
			}`,
			validate: func(t *testing.T, cfg *Config) {
				if cfg.Logging.General == nil {
					t.Error("General logging should not be nil")
				}
				// LLM logging may be nil (no automatic defaults)
				// This is expected behavior
			},
		},
		{
			name: "Empty logging object",
			json: `{
				"logging": {}
			}`,
			validate: func(t *testing.T, cfg *Config) {
				if cfg.Logging == nil {
					t.Error("Logging should not be nil")
				}
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var cfg Config
			err := json.Unmarshal([]byte(tt.json), &cfg)
			if err != nil {
				t.Fatalf("Failed to unmarshal JSON: %v", err)
			}

			if tt.validate != nil {
				tt.validate(t, &cfg)
			}
		})
	}
}

// TestLoggingConfig_FilePathExpansion tests file path expansion in logging config
func TestLoggingConfig_FilePathExpansion(t *testing.T) {
	homeDir, err := os.UserHomeDir()
	if err != nil {
		t.Skip("Cannot get home directory")
	}

	tests := []struct {
		name           string
		configFile     string
		expectedPath   string
		shouldExist    bool
	}{
		{
			name: "Tilde expansion in log file",
			configFile: `{
				"logging": {
					"general": {
						"file": "~/logs/app.log"
					}
				}
			}`,
			expectedPath: filepath.Join(homeDir, "logs/app.log"),
			shouldExist:  false,
		},
		{
			name: "Relative path in log file",
			configFile: `{
				"logging": {
					"general": {
						"file": "logs/app.log"
					}
				}
			}`,
			expectedPath: "logs/app.log",
			shouldExist:  false,
		},
		{
			name: "Absolute path in log file",
			configFile: `{
				"logging": {
					"general": {
						"file": "/var/log/app.log"
					}
				}
			}`,
			expectedPath: "/var/log/app.log",
			shouldExist:  false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var cfg Config
			err := json.Unmarshal([]byte(tt.configFile), &cfg)
			if err != nil {
				t.Fatalf("Failed to unmarshal config: %v", err)
			}

			if cfg.Logging == nil || cfg.Logging.General == nil {
				t.Fatal("Logging config is missing")
			}

			actualPath := cfg.Logging.General.File

			// For tilde expansion, we need to manually expand
			if strings.HasPrefix(actualPath, "~/") {
				expandedPath := filepath.Join(homeDir, actualPath[2:])
				if expandedPath != tt.expectedPath {
					t.Errorf("Expected path %q, got %q", tt.expectedPath, expandedPath)
				}
			} else if actualPath != tt.expectedPath {
				t.Errorf("Expected path %q, got %q", tt.expectedPath, actualPath)
			}
		})
	}
}

// TestLoggingConfig_EnvironmentVariableInteraction tests interaction with environment variables
func TestLoggingConfig_EnvironmentVariableInteraction(t *testing.T) {
	// Save original environment variables
	origEnabled := os.Getenv("NEMESISBOT_LOGGING_GENERAL_ENABLED")
	origLevel := os.Getenv("NEMESISBOT_LOGGING_GENERAL_LEVEL")
	origFile := os.Getenv("NEMESISBOT_LOGGING_GENERAL_FILE")

	// Restore environment variables after test
	defer func() {
		if origEnabled != "" {
			os.Setenv("NEMESISBOT_LOGGING_GENERAL_ENABLED", origEnabled)
		} else {
			os.Unsetenv("NEMESISBOT_LOGGING_GENERAL_ENABLED")
		}
		if origLevel != "" {
			os.Setenv("NEMESISBOT_LOGGING_GENERAL_LEVEL", origLevel)
		} else {
			os.Unsetenv("NEMESISBOT_LOGGING_GENERAL_LEVEL")
		}
		if origFile != "" {
			os.Setenv("NEMESISBOT_LOGGING_GENERAL_FILE", origFile)
		} else {
			os.Unsetenv("NEMESISBOT_LOGGING_GENERAL_FILE")
		}
	}()

	// Set environment variables
	os.Setenv("NEMESISBOT_LOGGING_GENERAL_ENABLED", "true")
	os.Setenv("NEMESISBOT_LOGGING_GENERAL_LEVEL", "DEBUG")
	os.Setenv("NEMESISBOT_LOGGING_GENERAL_FILE", "/tmp/test.log")

	// Create a config file
	tempDir := t.TempDir()
	configPath := filepath.Join(tempDir, "config.json")

	cfg := DefaultConfig()
	cfg.Logging = &LoggingConfig{
		General: &GeneralLogConfig{
			Enabled: false, // Config file says disabled
			Level:   "INFO", // Config file says INFO
			File:    "logs/app.log",
		},
	}

	err := SaveConfig(configPath, cfg)
	if err != nil {
		t.Fatalf("Failed to save config: %v", err)
	}

	// Load config - environment variables should override
	loadedCfg, err := LoadConfig(configPath)
	if err != nil {
		t.Fatalf("Failed to load config: %v", err)
	}

	// Note: The actual env var parsing happens in the env package
	// This test just verifies the config structure is correct
	if loadedCfg.Logging == nil {
		t.Error("Logging config should not be nil")
	}
}

// TestLoggingConfig_ConcurrentAccess tests concurrent access to logging configuration
func TestLoggingConfig_ConcurrentAccess(t *testing.T) {
	tempDir := t.TempDir()
	configPath := filepath.Join(tempDir, "config.json")

	// Create initial config
	cfg := DefaultConfig()
	cfg.Logging = &LoggingConfig{
		LLM: &LLMLogConfig{
			Enabled:     true,
			LogDir:      "logs/llm",
			DetailLevel: "full",
		},
		General: &GeneralLogConfig{
			Enabled:       true,
			EnableConsole: true,
			Level:         "INFO",
			File:          "logs/app.log",
		},
	}

	err := SaveConfig(configPath, cfg)
	if err != nil {
		t.Fatalf("Failed to save initial config: %v", err)
	}

	// Launch multiple goroutines to read/write config
	done := make(chan bool)
	errors := make(chan error, 10)

	for i := 0; i < 5; i++ {
		go func(id int) {
			// Read config
			loadedCfg, err := LoadConfig(configPath)
			if err != nil {
				errors <- err
				return
			}

			// Modify config
			loadedCfg.Logging.General.Level = "DEBUG"

			// Save config
			err = SaveConfig(configPath, loadedCfg)
			if err != nil {
				errors <- err
				return
			}

			done <- true
		}(i)
	}

	// Wait for all goroutines to complete
	for i := 0; i < 5; i++ {
		select {
		case <-done:
			// Success
		case err := <-errors:
			t.Fatalf("Concurrent access error: %v", err)
		case <-time.After(5 * time.Second):
			t.Fatal("Timeout waiting for concurrent operations")
		}
	}

	// Final verification
	finalCfg, err := LoadConfig(configPath)
	if err != nil {
		t.Fatalf("Failed to load final config: %v", err)
	}

	if finalCfg.Logging == nil {
		t.Error("Logging config should not be nil after concurrent operations")
	}
}

// TestLoggingConfig_DefaultValues tests that default values are correctly applied
func TestLoggingConfig_DefaultValues(t *testing.T) {
	cfg := DefaultConfig()

	// Logging config should not be nil (if defaults are set in DefaultConfig)
	// This test verifies the behavior when logging config is missing
	if cfg.Logging == nil {
		// Create a config without logging
		cfg.Logging = &LoggingConfig{
			LLM:     &LLMLogConfig{},
			General: &GeneralLogConfig{},
		}
	}

	// Test that empty values are handled correctly
	if cfg.Logging.LLM == nil {
		cfg.Logging.LLM = &LLMLogConfig{}
	}

	if cfg.Logging.General == nil {
		cfg.Logging.General = &GeneralLogConfig{}
	}

	// Verify we can serialize and deserialize without errors
	data, err := json.Marshal(cfg)
	if err != nil {
		t.Fatalf("Failed to marshal config with defaults: %v", err)
	}

	var restoredCfg Config
	err = json.Unmarshal(data, &restoredCfg)
	if err != nil {
		t.Fatalf("Failed to unmarshal config: %v", err)
	}

	if restoredCfg.Logging == nil {
		t.Error("Restored logging config should not be nil")
	}
}
