package config

import (
	"encoding/json"
	"fmt"
	"os"
	"time"
)

// Config holds the minimal configuration for the DLL WebSocket client.
type Config struct {
	Server    ServerConfig    `json:"server"`
	Reconnect ReconnectConfig `json:"reconnect"`
	Heartbeat HeartbeatConfig `json:"heartbeat"`
}

type ServerConfig struct {
	URL   string `json:"url"`
	Token string `json:"token"`
}

type ReconnectConfig struct {
	Enabled         bool    `json:"enabled"`
	InitialDelaySec int     `json:"initial_delay_sec"`
	MaxDelaySec     int     `json:"max_delay_sec"`
	DelayMultiplier float64 `json:"delay_multiplier"`
	MaxAttempts     int     `json:"max_attempts"`
}

type HeartbeatConfig struct {
	Enabled     bool `json:"enabled"`
	IntervalSec int  `json:"interval_sec"`
}

// NewConfig creates a Config from the given URL and token with sensible defaults.
func NewConfig(url, token string) *Config {
	return &Config{
		Server: ServerConfig{
			URL:   url,
			Token: token,
		},
		Reconnect: ReconnectConfig{
			Enabled:         true,
			InitialDelaySec: 2,
			MaxDelaySec:     30,
			DelayMultiplier: 1.5,
			MaxAttempts:     0,
		},
		Heartbeat: HeartbeatConfig{
			Enabled:     true,
			IntervalSec: 30,
		},
	}
}

// LoadOrCreateDefault loads config from config.json in the current directory,
// or creates one with default values if the file doesn't exist.
func LoadOrCreateDefault() *Config {
	cfg := NewConfig("ws://127.0.0.1:49001/ws", "")

	data, err := os.ReadFile("config.json")
	if err == nil {
		if err := json.Unmarshal(data, cfg); err != nil {
			fmt.Printf("Warning: failed to parse config file, using defaults: %v\n", err)
		}
	} else {
		if data, err := json.MarshalIndent(cfg, "", "  "); err == nil {
			_ = os.WriteFile("config.json", data, 0644)
		}
	}

	return cfg
}

// GetURL returns the WebSocket URL with optional token query parameter.
func (c *Config) GetURL() string {
	if c.Server.Token == "" {
		return c.Server.URL
	}
	return c.Server.URL + "?token=" + c.Server.Token
}

// InitialDelay returns the initial reconnect delay as a Duration.
func (c *Config) InitialDelay() time.Duration {
	return time.Duration(c.Reconnect.InitialDelaySec) * time.Second
}

// MaxDelay returns the max reconnect delay as a Duration.
func (c *Config) MaxDelay() time.Duration {
	return time.Duration(c.Reconnect.MaxDelaySec) * time.Second
}

// HeartbeatInterval returns the heartbeat interval as a Duration.
func (c *Config) HeartbeatInterval() time.Duration {
	return time.Duration(c.Heartbeat.IntervalSec) * time.Second
}
