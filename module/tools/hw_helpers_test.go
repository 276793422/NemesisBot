// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"testing"
)

// ==================== I2C Helper Functions ====================

func TestIsValidBusID(t *testing.T) {
	tests := []struct {
		input string
		want  bool
	}{
		{"1", true},
		{"0", true},
		{"99", true},
		{"", false},
		{"abc", false},
		{"1a", false},
		{"../secret", false},
		{"1; rm -rf", false},
		{" 1", false},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			got := isValidBusID(tt.input)
			if got != tt.want {
				t.Errorf("isValidBusID(%q) = %v, want %v", tt.input, got, tt.want)
			}
		})
	}
}

func TestParseI2CAddress(t *testing.T) {
	tests := []struct {
		name      string
		args      map[string]interface{}
		wantAddr  int
		wantError bool
	}{
		{
			name:      "valid address",
			args:      map[string]interface{}{"address": float64(0x48)},
			wantAddr:  0x48,
			wantError: false,
		},
		{
			name:      "missing address",
			args:      map[string]interface{}{},
			wantAddr:  0,
			wantError: true,
		},
		{
			name:      "address too low",
			args:      map[string]interface{}{"address": float64(0x02)},
			wantAddr:  0,
			wantError: true,
		},
		{
			name:      "address too high",
			args:      map[string]interface{}{"address": float64(0x78)},
			wantAddr:  0,
			wantError: true,
		},
		{
			name:      "boundary low (0x03)",
			args:      map[string]interface{}{"address": float64(0x03)},
			wantAddr:  0x03,
			wantError: false,
		},
		{
			name:      "boundary high (0x77)",
			args:      map[string]interface{}{"address": float64(0x77)},
			wantAddr:  0x77,
			wantError: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			addr, result := parseI2CAddress(tt.args)
			if tt.wantError {
				if result == nil || !result.IsError {
					t.Error("Expected error result")
				}
			} else {
				if result != nil {
					t.Errorf("Expected nil result, got error: %v", result)
				}
				if addr != tt.wantAddr {
					t.Errorf("Expected address %d, got %d", tt.wantAddr, addr)
				}
			}
		})
	}
}

func TestParseI2CBus(t *testing.T) {
	tests := []struct {
		name      string
		args      map[string]interface{}
		wantBus   string
		wantError bool
	}{
		{
			name:      "valid bus",
			args:      map[string]interface{}{"bus": "1"},
			wantBus:   "1",
			wantError: false,
		},
		{
			name:      "missing bus",
			args:      map[string]interface{}{},
			wantBus:   "",
			wantError: true,
		},
		{
			name:      "empty bus",
			args:      map[string]interface{}{"bus": ""},
			wantBus:   "",
			wantError: true,
		},
		{
			name:      "invalid bus (non-numeric)",
			args:      map[string]interface{}{"bus": "abc"},
			wantBus:   "",
			wantError: true,
		},
		{
			name:      "invalid bus (path injection)",
			args:      map[string]interface{}{"bus": "../etc"},
			wantBus:   "",
			wantError: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			bus, result := parseI2CBus(tt.args)
			if tt.wantError {
				if result == nil || !result.IsError {
					t.Error("Expected error result")
				}
			} else {
				if result != nil {
					t.Errorf("Expected nil result, got error: %v", result)
				}
				if bus != tt.wantBus {
					t.Errorf("Expected bus %q, got %q", tt.wantBus, bus)
				}
			}
		})
	}
}

// ==================== I2C Detect (no platform check, direct call) ====================

func TestI2CTool_Detect(t *testing.T) {
	tool := &I2CTool{}
	result := tool.detect()
	// On Windows, /dev/i2c-* won't exist, so should return "No I2C buses found"
	if result == nil {
		t.Fatal("Expected non-nil result")
	}
	if !result.Silent {
		t.Error("Detect result should be silent")
	}
}

// ==================== SPI Helper Functions ====================

func TestParseSPIArgs(t *testing.T) {
	tests := []struct {
		name        string
		args        map[string]interface{}
		wantDev     string
		wantSpeed   uint32
		wantMode    uint8
		wantBits    uint8
		wantErr     bool
	}{
		{
			name:      "valid device only",
			args:      map[string]interface{}{"device": "2.0"},
			wantDev:   "2.0",
			wantSpeed: 1000000,
			wantMode:  0,
			wantBits:  8,
			wantErr:   false,
		},
		{
			name:      "missing device",
			args:      map[string]interface{}{},
			wantDev:   "",
			wantErr:   true,
		},
		{
			name:      "invalid device format",
			args:      map[string]interface{}{"device": "abc"},
			wantDev:   "",
			wantErr:   true,
		},
		{
			name:      "custom speed",
			args:      map[string]interface{}{"device": "1.0", "speed": float64(500000)},
			wantDev:   "1.0",
			wantSpeed: 500000,
			wantMode:  0,
			wantBits:  8,
			wantErr:   false,
		},
		{
			name:      "speed too low",
			args:      map[string]interface{}{"device": "1.0", "speed": float64(0)},
			wantDev:   "",
			wantErr:   true,
		},
		{
			name:      "speed too high",
			args:      map[string]interface{}{"device": "1.0", "speed": float64(200000000)},
			wantDev:   "",
			wantErr:   true,
		},
		{
			name:      "custom mode",
			args:      map[string]interface{}{"device": "1.0", "mode": float64(3)},
			wantDev:   "1.0",
			wantSpeed: 1000000,
			wantMode:  3,
			wantBits:  8,
			wantErr:   false,
		},
		{
			name:      "invalid mode",
			args:      map[string]interface{}{"device": "1.0", "mode": float64(5)},
			wantDev:   "",
			wantErr:   true,
		},
		{
			name:      "custom bits",
			args:      map[string]interface{}{"device": "1.0", "bits": float64(16)},
			wantDev:   "1.0",
			wantSpeed: 1000000,
			wantMode:  0,
			wantBits:  16,
			wantErr:   false,
		},
		{
			name:      "bits too low",
			args:      map[string]interface{}{"device": "1.0", "bits": float64(0)},
			wantDev:   "",
			wantErr:   true,
		},
		{
			name:      "bits too high",
			args:      map[string]interface{}{"device": "1.0", "bits": float64(33)},
			wantDev:   "",
			wantErr:   true,
		},
		{
			name:      "all params",
			args:      map[string]interface{}{"device": "0.1", "speed": float64(2000000), "mode": float64(2), "bits": float64(16)},
			wantDev:   "0.1",
			wantSpeed: 2000000,
			wantMode:  2,
			wantBits:  16,
			wantErr:   false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			dev, speed, mode, bits, errMsg := parseSPIArgs(tt.args)
			if tt.wantErr {
				if errMsg == "" {
					t.Error("Expected error message")
				}
			} else {
				if errMsg != "" {
					t.Errorf("Expected no error, got: %s", errMsg)
				}
				if dev != tt.wantDev {
					t.Errorf("Expected device %q, got %q", tt.wantDev, dev)
				}
				if speed != tt.wantSpeed {
					t.Errorf("Expected speed %d, got %d", tt.wantSpeed, speed)
				}
				if mode != tt.wantMode {
					t.Errorf("Expected mode %d, got %d", tt.wantMode, mode)
				}
				if bits != tt.wantBits {
					t.Errorf("Expected bits %d, got %d", tt.wantBits, bits)
				}
			}
		})
	}
}

// ==================== SPITool List (direct call) ====================

func TestSPITool_List(t *testing.T) {
	tool := &SPITool{}
	result := tool.list()
	// On Windows, /dev/spidev* won't exist
	if result == nil {
		t.Fatal("Expected non-nil result")
	}
	if !result.Silent {
		t.Error("List result should be silent")
	}
}
