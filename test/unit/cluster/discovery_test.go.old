// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster_test

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/discovery"
)

// TestNewAnnounceMessage tests creating an announce message
func TestNewAnnounceMessage(t *testing.T) {
	msg := discovery.NewAnnounceMessage(
		"bot-test",
		"Test Bot",
		"192.168.1.1:49200",
		[]string{"code", "test"},
	)

	if msg.Type != discovery.MessageTypeAnnounce {
		t.Errorf("Expected type announce, got %s", msg.Type)
	}

	if msg.NodeID != "bot-test" {
		t.Errorf("Expected node_id bot-test, got %s", msg.NodeID)
	}

	if msg.Address != "192.168.1.1:49200" {
		t.Errorf("Expected address 192.168.1.1:49200, got %s", msg.Address)
	}

	if len(msg.Capabilities) != 2 {
		t.Errorf("Expected 2 capabilities, got %d", len(msg.Capabilities))
	}
}

// TestNewByeMessage tests creating a bye message
func TestNewByeMessage(t *testing.T) {
	msg := discovery.NewByeMessage("bot-test")

	if msg.Type != discovery.MessageTypeBye {
		t.Errorf("Expected type bye, got %s", msg.Type)
	}

	if msg.NodeID != "bot-test" {
		t.Errorf("Expected node_id bot-test, got %s", msg.NodeID)
	}
}

// TestDiscoveryMessageValidate tests message validation
func TestDiscoveryMessageValidate(t *testing.T) {
	tests := []struct {
		name    string
		msg     *discovery.DiscoveryMessage
		wantErr bool
	}{
		{
			name: "valid announce",
			msg: discovery.NewAnnounceMessage(
				"bot-test",
				"Test Bot",
				"192.168.1.1:49200",
				[]string{"test"},
			),
			wantErr: false,
		},
		{
			name: "wrong version",
			msg: &discovery.DiscoveryMessage{
				Version: "0.0",
				Type:    discovery.MessageTypeAnnounce,
				NodeID:  "bot-test",
				Name:    "Test",
				Address: "192.168.1.1:49200",
			},
			wantErr: true,
		},
		{
			name: "missing node_id",
			msg: &discovery.DiscoveryMessage{
				Version: "1.0",
				Type:    discovery.MessageTypeAnnounce,
				NodeID:  "",
				Name:    "Test",
				Address: "192.168.1.1:49200",
			},
			wantErr: true,
		},
		{
			name: "announce missing name",
			msg: &discovery.DiscoveryMessage{
				Version: "1.0",
				Type:    discovery.MessageTypeAnnounce,
				NodeID:  "bot-test",
				Name:    "",
				Address: "192.168.1.1:49200",
			},
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := tt.msg.Validate()
			if (err != nil) != tt.wantErr {
				t.Errorf("Validate() error = %v, wantErr %v", err, tt.wantErr)
			}
		})
	}
}

// TestDiscoveryMessageIsExpired tests expiration check
func TestDiscoveryMessageIsExpired(t *testing.T) {
	// Recent message - not expired
	recentMsg := discovery.NewAnnounceMessage(
		"bot-test",
		"Test Bot",
		"192.168.1.1:49200",
		[]string{},
	)

	if recentMsg.IsExpired() {
		t.Error("Recent message should not be expired")
	}

	// Old message - expired
	oldMsg := &discovery.DiscoveryMessage{
		Version:   "1.0",
		Type:      discovery.MessageTypeAnnounce,
		NodeID:    "bot-test",
		Name:      "Old Bot",
		Address:   "192.168.1.1:49200",
		Timestamp: time.Now().Unix() - 200, // 200 seconds ago
	}

	if !oldMsg.IsExpired() {
		t.Error("Old message should be expired")
	}
}

// TestDiscoveryMessageBytes tests message serialization
func TestDiscoveryMessageBytes(t *testing.T) {
	msg := discovery.NewAnnounceMessage(
		"bot-test",
		"Test Bot",
		"192.168.1.1:49200",
		[]string{"test"},
	)

	data, err := msg.Bytes()
	if err != nil {
		t.Fatalf("Bytes() error = %v", err)
	}

	if len(data) == 0 {
		t.Error("Bytes() returned empty data")
	}

	// Verify it's valid JSON
	if !json.Valid(data) {
		t.Error("Serialized data is not valid JSON")
	}

	// Verify it contains expected fields
	dataStr := string(data)
	if !strings.Contains(dataStr, "\"node_id\"") {
		t.Error("Serialized data missing node_id field")
	}
}
