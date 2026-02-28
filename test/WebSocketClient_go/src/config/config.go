package config

import (
	"encoding/json"
	"fmt"
	"os"
	"time"
)

type Config struct {
	Server     ServerConfig     `json:"server"`
	Reconnect  ReconnectConfig  `json:"reconnect"`
	Heartbeat  HeartbeatConfig  `json:"heartbeat"`
	Logging    LoggingConfig    `json:"logging"`
	Statistics StatisticsConfig `json:"statistics"`
	UI         UIConfig         `json:"ui"`
}

type ServerConfig struct {
	URL   string `json:"url"`
	Token string `json:"token"`
}

type ReconnectConfig struct {
	Enabled         bool          `json:"enabled"`
	InitialDelaySec int           `json:"initial_delay_sec"`
	MaxDelaySec     int           `json:"max_delay_sec"`
	DelayMultiplier float64       `json:"delay_multiplier"`
	MaxAttempts     int           `json:"max_attempts"`
}

type HeartbeatConfig struct {
	Enabled    bool `json:"enabled"`
	IntervalSec int `json:"interval_sec"`
}

type LoggingConfig struct {
	Enabled bool   `json:"enabled"`
	File    string `json:"file"`
	Level   string `json:"level"`
}

type StatisticsConfig struct {
	Enabled bool `json:"enabled"`
}

type UIConfig struct {
	Color         bool   `json:"color"`
	ShowTimestamp bool   `json:"show_timestamp"`
	PromptStyle   string `json:"prompt_style"`
}

func LoadOrCreateDefault() *Config {
	cfg := GetDefaultConfig()

	// Try to load from file
	data, err := os.ReadFile("config.json")
	if err == nil {
		if err := json.Unmarshal(data, cfg); err != nil {
			fmt.Printf("Warning: failed to parse config file, using defaults: %v\n", err)
		}
	} else {
		// Create default config file
		if data, err := json.MarshalIndent(cfg, "", "  "); err == nil {
			_ = os.WriteFile("config.json", data, 0644)
		}
	}

	return cfg
}

func GetDefaultConfig() *Config {
	return &Config{
		Server: ServerConfig{
			URL:   "ws://127.0.0.1:49001/ws",
			Token: "",
		},
		Reconnect: ReconnectConfig{
			Enabled:         true,
			InitialDelaySec: 2,
			MaxDelaySec:     30,
			DelayMultiplier: 1.5,
			MaxAttempts:     0,
		},
		Heartbeat: HeartbeatConfig{
			Enabled:    true,
			IntervalSec: 30,
		},
		Logging: LoggingConfig{
			Enabled: true,
			File:    "websocket_client.log",
			Level:   "info",
		},
		Statistics: StatisticsConfig{
			Enabled: true,
		},
		UI: UIConfig{
			Color:         true,
			ShowTimestamp: true,
			PromptStyle:   "simple",
		},
	}
}

func (c *Config) GetURL() string {
	if c.Server.Token == "" {
		return c.Server.URL
	}
	return fmt.Sprintf("%s?token=%s", c.Server.URL, c.Server.Token)
}

func LogToFile(cfg *Config, message string) {
	if !cfg.Logging.Enabled || cfg.Logging.File == "" {
		return
	}

	f, err := os.OpenFile(cfg.Logging.File, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0644)
	if err != nil {
		return
	}
	defer f.Close()

	fmt.Fprintf(f, "[%s] %s\n", time.Now().Format(time.RFC3339), message)
}
