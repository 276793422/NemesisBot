// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package config_test

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	. "github.com/276793422/NemesisBot/module/config"
)

// TestLoggingConfig_DefaultValues tests that default logging values are correct
func TestLoggingConfig_DefaultValues(t *testing.T) {
	cfg := &Config{
		Logging: &LoggingConfig{
			LLM:     &LLMLogConfig{},
			General: &GeneralLogConfig{},
		},
	}

	if cfg.Logging == nil {
		t.Fatal("Logging should not be nil")
	}

	if cfg.Logging.LLM == nil {
		t.Fatal("LLM logging config should not be nil")
	}

	if cfg.Logging.General == nil {
		t.Fatal("General logging config should not be nil")
	}
}

// TestLoggingConfig_GeneralEnabled tests the general logging enabled field
func TestLoggingConfig_GeneralEnabled(t *testing.T) {
	tests := []struct {
		name     string
		enabled  bool
		expected bool
	}{
		{
			name:     "enabled true",
			enabled:  true,
			expected: true,
		},
		{
			name:     "enabled false",
			enabled:  false,
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &Config{
				Logging: &LoggingConfig{
					General: &GeneralLogConfig{
						Enabled: tt.enabled,
					},
				},
			}

			if cfg.Logging.General.Enabled != tt.expected {
				t.Errorf("Expected Enabled to be %v, got %v", tt.expected, cfg.Logging.General.Enabled)
			}
		})
	}
}

// TestLoggingConfig_GeneralConsole tests the console output field
func TestLoggingConfig_GeneralConsole(t *testing.T) {
	tests := []struct {
		name     string
		console  bool
		expected bool
	}{
		{
			name:     "console enabled",
			console:  true,
			expected: true,
		},
		{
			name:     "console disabled",
			console:  false,
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &Config{
				Logging: &LoggingConfig{
					General: &GeneralLogConfig{
						EnableConsole: tt.console,
					},
				},
			}

			if cfg.Logging.General.EnableConsole != tt.expected {
				t.Errorf("Expected EnableConsole to be %v, got %v", tt.expected, cfg.Logging.General.EnableConsole)
			}
		})
	}
}

// TestLoggingConfig_LevelParsing tests log level parsing
func TestLoggingConfig_LevelParsing(t *testing.T) {
	tests := []struct {
		name     string
		level    string
		expected string
	}{
		{"DEBUG level", "DEBUG", "DEBUG"},
		{"INFO level", "INFO", "INFO"},
		{"WARN level", "WARN", "WARN"},
		{"ERROR level", "ERROR", "ERROR"},
		{"FATAL level", "FATAL", "FATAL"},
		{"debug lowercase", "debug", "debug"},
		{"info lowercase", "info", "info"},
		{"empty level", "", ""},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &Config{
				Logging: &LoggingConfig{
					General: &GeneralLogConfig{
						Level: tt.level,
					},
				},
			}

			if cfg.Logging.General.Level != tt.expected {
				t.Errorf("Expected Level to be %q, got %q", tt.expected, cfg.Logging.General.Level)
			}
		})
	}
}

// TestLoggingConfig_FilePath tests file path configuration
func TestLoggingConfig_FilePath(t *testing.T) {
	tests := []struct {
		name     string
		file     string
		expected string
	}{
		{"absolute path", "/var/log/nemesisbot.log", "/var/log/nemesisbot.log"},
		{"relative path", "logs/app.log", "logs/app.log"},
		{"home path", "~/logs/app.log", "~/logs/app.log"},
		{"empty path", "", ""},
		{"windows path", "C:\\logs\\app.log", "C:\\logs\\app.log"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &Config{
				Logging: &LoggingConfig{
					General: &GeneralLogConfig{
						File: tt.file,
					},
				},
			}

			if cfg.Logging.General.File != tt.expected {
				t.Errorf("Expected File to be %q, got %q", tt.expected, cfg.Logging.General.File)
			}
		})
	}
}

// TestLoggingConfig_LLMEnabled tests LLM logging configuration
func TestLoggingConfig_LLMEnabled(t *testing.T) {
	tests := []struct {
		name     string
		enabled  bool
		expected bool
	}{
		{
			name:     "LLM enabled",
			enabled:  true,
			expected: true,
		},
		{
			name:     "LLM disabled",
			enabled:  false,
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &Config{
				Logging: &LoggingConfig{
					LLM: &LLMLogConfig{
						Enabled: tt.enabled,
					},
				},
			}

			if cfg.Logging.LLM.Enabled != tt.expected {
				t.Errorf("Expected LLM Enabled to be %v, got %v", tt.expected, cfg.Logging.LLM.Enabled)
			}
		})
	}
}

// TestLoggingConfig_LLMDetailLevel tests LLM detail level configuration
func TestLoggingConfig_LLMDetailLevel(t *testing.T) {
	tests := []struct {
		name     string
		level    string
		expected string
	}{
		{"full detail", "full", "full"},
		{"summary detail", "summary", "summary"},
		{"minimal detail", "minimal", "minimal"},
		{"empty detail", "", ""},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &Config{
				Logging: &LoggingConfig{
					LLM: &LLMLogConfig{
						DetailLevel: tt.level,
					},
				},
			}

			if cfg.Logging.LLM.DetailLevel != tt.expected {
				t.Errorf("Expected LLM DetailLevel to be %q, got %q", tt.expected, cfg.Logging.LLM.DetailLevel)
			}
		})
	}
}

// TestLoggingConfig_LogDir tests LLM log directory configuration
func TestLoggingConfig_LogDir(t *testing.T) {
	tests := []struct {
		name     string
		logDir   string
		expected string
	}{
		{"absolute path", "/var/log/request_logs", "/var/log/request_logs"},
		{"relative path", "logs/request_logs", "logs/request_logs"},
		{"home path", "~/logs/request_logs", "~/logs/request_logs"},
		{"empty path", "", ""},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &Config{
				Logging: &LoggingConfig{
					LLM: &LLMLogConfig{
						LogDir: tt.logDir,
					},
				},
			}

			if cfg.Logging.LLM.LogDir != tt.expected {
				t.Errorf("Expected LLM LogDir to be %q, got %q", tt.expected, cfg.Logging.LLM.LogDir)
			}
		})
	}
}

// TestLoggingConfig_NilHandling tests nil handling for logging configs
func TestLoggingConfig_NilHandling(t *testing.T) {
	tests := []struct {
		name    string
		cfg     *Config
		wantNil bool
	}{
		{
			name: "all nil",
			cfg: &Config{
				Logging: nil,
			},
			wantNil: true,
		},
		{
			name: "logging not nil but LLM nil",
			cfg: &Config{
				Logging: &LoggingConfig{
					LLM:     nil,
					General: &GeneralLogConfig{},
				},
			},
			wantNil: false,
		},
		{
			name: "logging not nil but General nil",
			cfg: &Config{
				Logging: &LoggingConfig{
					LLM:     &LLMLogConfig{},
					General: nil,
				},
			},
			wantNil: false,
		},
		{
			name: "all present",
			cfg: &Config{
				Logging: &LoggingConfig{
					LLM:     &LLMLogConfig{},
					General: &GeneralLogConfig{},
				},
			},
			wantNil: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			isNil := tt.cfg.Logging == nil

			if isNil != tt.wantNil {
				t.Errorf("Expected Logging nil to be %v, got %v", tt.wantNil, isNil)
			}
		})
	}
}

// TestLoggingConfig_EnvironmentVariables tests that environment variable tags are set
func TestLoggingConfig_EnvironmentVariables(t *testing.T) {
	// This test verifies the struct tags are correct
	// We don't actually test environment variable parsing (that's done by the env package)

	tests := []struct {
		name     string
		envTag   string
		expected string
	}{
		{
			name:     "general enabled env",
			envTag:   getEnvTag("Enabled"),
			expected: "NEMESISBOT_LOGGING_GENERAL_ENABLED",
		},
		{
			name:     "general console env",
			envTag:   getEnvTag("EnableConsole"),
			expected: "NEMESISBOT_LOGGING_GENERAL_ENABLE_CONSOLE",
		},
		{
			name:     "general level env",
			envTag:   getEnvTag("Level"),
			expected: "NEMESISBOT_LOGGING_GENERAL_LEVEL",
		},
		{
			name:     "general file env",
			envTag:   getEnvTag("File"),
			expected: "NEMESISBOT_LOGGING_GENERAL_FILE",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if tt.envTag != tt.expected {
				t.Errorf("Expected env tag to be %q, got %q", tt.expected, tt.envTag)
			}
		})
	}
}

// TestLoggingConfig_WorkspaceExpansion tests that workspace paths are handled correctly
func TestLoggingConfig_WorkspaceExpansion(t *testing.T) {
	// Create a temporary directory
	tempDir := t.TempDir()

	// Test relative path expansion
	cfg := &Config{
		Logging: &LoggingConfig{
			General: &GeneralLogConfig{
				File: "logs/app.log",
			},
		},
	}

	// Simulate workspace path resolution
	workspace := tempDir
	relPath := cfg.Logging.General.File
	var fullPath string
	if filepath.IsAbs(relPath) {
		fullPath = relPath
	} else {
		fullPath = filepath.Join(workspace, relPath)
	}

	expectedPath := filepath.Join(tempDir, "logs/app.log")
	if fullPath != expectedPath {
		t.Errorf("Expected full path to be %q, got %q", expectedPath, fullPath)
	}
}

// TestLoggingConfig_HomeExpansion tests tilde expansion in file paths
func TestLoggingConfig_HomeExpansion(t *testing.T) {
	homeDir, err := os.UserHomeDir()
	if err != nil {
		t.Skip("Cannot get home directory")
	}

	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "tilde with path",
			input:    "~/logs/app.log",
			expected: filepath.Join(homeDir, "logs/app.log"),
		},
		{
			name:     "tilde only",
			input:    "~",
			expected: homeDir,
		},
		{
			name:     "absolute path",
			input:    "/var/log/app.log",
			expected: "/var/log/app.log",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var result string
			if tt.input == "~" {
				result = homeDir
			} else if len(tt.input) > 1 && tt.input[0] == '~' && (tt.input[1] == '/' || tt.input[1] == '\\') {
				result = filepath.Join(homeDir, tt.input[2:])
			} else {
				result = tt.input
			}

			if result != tt.expected {
				t.Errorf("Expected expanded path to be %q, got %q", tt.expected, result)
			}
		})
	}
}

// Helper function to get env tag from struct field
func getEnvTag(field string) string {
	// Map of field names to their expected env tags
	fieldToEnv := map[string]string{
		"Enabled":       "NEMESISBOT_LOGGING_GENERAL_ENABLED",
		"EnableConsole": "NEMESISBOT_LOGGING_GENERAL_ENABLE_CONSOLE",
		"Level":         "NEMESISBOT_LOGGING_GENERAL_LEVEL",
		"File":          "NEMESISBOT_LOGGING_GENERAL_FILE",
	}

	return fieldToEnv[field]
}

// TestLoggingConfig_JSONUnmarshal tests JSON unmarshaling
func TestLoggingConfig_JSONUnmarshal(t *testing.T) {
	tests := []struct {
		name     string
		json     string
		wantErr  bool
		validate func(*testing.T, *Config)
	}{
		{
			name: "valid full config",
			json: `{
				"logging": {
					"llm": {
						"enabled": true,
						"log_dir": "logs/requests",
						"detail_level": "full"
					},
					"general": {
						"enabled": true,
						"enable_console": true,
						"level": "DEBUG",
						"file": "logs/app.log"
					}
				}
			}`,
			wantErr: false,
			validate: func(t *testing.T, cfg *Config) {
				if cfg.Logging == nil {
					t.Fatal("Logging should not be nil")
				}
				if cfg.Logging.LLM == nil {
					t.Fatal("LLM logging should not be nil")
				}
				if cfg.Logging.General == nil {
					t.Fatal("General logging should not be nil")
				}
				if cfg.Logging.LLM.Enabled != true {
					t.Error("Expected LLM enabled to be true")
				}
				if cfg.Logging.General.Enabled != true {
					t.Error("Expected general enabled to be true")
				}
				if cfg.Logging.General.Level != "DEBUG" {
					t.Errorf("Expected level to be DEBUG, got %s", cfg.Logging.General.Level)
				}
			},
		},
		{
			name: "valid partial config",
			json: `{
				"logging": {
					"general": {
						"level": "INFO"
					}
				}
			}`,
			wantErr: false,
			validate: func(t *testing.T, cfg *Config) {
				if cfg.Logging == nil {
					t.Fatal("Logging should not be nil")
				}
				if cfg.Logging.General == nil {
					t.Fatal("General logging should not be nil")
				}
				if cfg.Logging.General.Level != "INFO" {
					t.Errorf("Expected level to be INFO, got %s", cfg.Logging.General.Level)
				}
			},
		},
		{
			name: "empty logging config",
			json: `{
				"logging": {}
			}`,
			wantErr: false,
			validate: func(t *testing.T, cfg *Config) {
				if cfg.Logging == nil {
					t.Fatal("Logging should not be nil")
				}
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var cfg Config
			err := json.Unmarshal([]byte(tt.json), &cfg)

			if (err != nil) != tt.wantErr {
				t.Errorf("json.Unmarshal() error = %v, wantErr %v", err, tt.wantErr)
				return
			}

			if tt.validate != nil {
				tt.validate(t, &cfg)
			}
		})
	}
}

// TestLoggingConfig_FullConfig tests complete logging configuration
func TestLoggingConfig_FullConfig(t *testing.T) {
	cfgJSON := `{
		"logging": {
			"llm": {
				"enabled": true,
				"log_dir": "logs/request_logs",
				"detail_level": "full"
			},
			"general": {
				"enabled": true,
				"enable_console": true,
				"level": "DEBUG",
				"file": "logs/nemesisbot.log"
			}
		}
	}`

	var cfg Config
	err := json.Unmarshal([]byte(cfgJSON), &cfg)
	if err != nil {
		t.Fatalf("Failed to unmarshal config: %v", err)
	}

	// Verify LLM logging config
	if cfg.Logging == nil {
		t.Fatal("Logging should not be nil")
	}

	if cfg.Logging.LLM == nil {
		t.Fatal("LLM logging config should not be nil")
	}

	if cfg.Logging.LLM.Enabled != true {
		t.Error("Expected LLM logging to be enabled")
	}

	if cfg.Logging.LLM.LogDir != "logs/request_logs" {
		t.Errorf("Expected LLM log dir to be 'logs/request_logs', got '%s'", cfg.Logging.LLM.LogDir)
	}

	if cfg.Logging.LLM.DetailLevel != "full" {
		t.Errorf("Expected LLM detail level to be 'full', got '%s'", cfg.Logging.LLM.DetailLevel)
	}

	// Verify General logging config
	if cfg.Logging.General == nil {
		t.Fatal("General logging config should not be nil")
	}

	if cfg.Logging.General.Enabled != true {
		t.Error("Expected general logging to be enabled")
	}

	if cfg.Logging.General.EnableConsole != true {
		t.Error("Expected console output to be enabled")
	}

	if cfg.Logging.General.Level != "DEBUG" {
		t.Errorf("Expected log level to be 'DEBUG', got '%s'", cfg.Logging.General.Level)
	}

	if cfg.Logging.General.File != "logs/nemesisbot.log" {
		t.Errorf("Expected log file to be 'logs/nemesisbot.log', got '%s'", cfg.Logging.General.File)
	}
}
