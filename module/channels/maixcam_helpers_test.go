// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"encoding/json"
	"testing"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
)

func TestMaixCamMessageParsing(t *testing.T) {
	jsonStr := `{
		"type": "person_detected",
		"tips": "detected person",
		"timestamp": 1700000000.5,
		"data": {
			"class_name": "person",
			"score": 0.95,
			"x": 100,
			"y": 200,
			"w": 50,
			"h": 80
		}
	}`

	var msg MaixCamMessage
	if err := json.Unmarshal([]byte(jsonStr), &msg); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}

	if msg.Type != "person_detected" {
		t.Errorf("type = %q, want 'person_detected'", msg.Type)
	}
	if msg.Tips != "detected person" {
		t.Errorf("tips = %q", msg.Tips)
	}
	if msg.Timestamp != 1700000000.5 {
		t.Errorf("timestamp = %f", msg.Timestamp)
	}
	if msg.Data["class_name"] != "person" {
		t.Errorf("class_name = %v", msg.Data["class_name"])
	}
}

func TestMaixCamMessageHeartbeat(t *testing.T) {
	jsonStr := `{"type": "heartbeat", "tips": "", "timestamp": 1234.0, "data": {}}`
	var msg MaixCamMessage
	if err := json.Unmarshal([]byte(jsonStr), &msg); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}
	if msg.Type != "heartbeat" {
		t.Errorf("type = %q", msg.Type)
	}
}

func TestNewMaixCamChannel_Valid(t *testing.T) {
	cfg := config.MaixCamConfig{
		Enabled:   true,
		Host:      "127.0.0.1",
		Port:      0, // let OS pick
		AllowFrom: []string{},
	}
	msgBus := bus.NewMessageBus()

	ch, err := NewMaixCamChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if ch == nil {
		t.Fatal("channel is nil")
	}
}

func TestMaixCamChannel_Name(t *testing.T) {
	cfg := config.MaixCamConfig{
		Host: "127.0.0.1",
		Port: 9999,
	}
	msgBus := bus.NewMessageBus()
	ch, _ := NewMaixCamChannel(cfg, msgBus)

	if ch.Name() != "maixcam" {
		t.Errorf("name = %q, want 'maixcam'", ch.Name())
	}
}

func TestMaixCamChannel_IsAllowed(t *testing.T) {
	tests := []struct {
		name      string
		allowList []string
		senderID  string
		allowed   bool
	}{
		{"empty list allows all", []string{}, "anyone", true},
		{"match in list", []string{"device1"}, "device1", true},
		{"not in list", []string{"device1"}, "device2", false},
		{"nil list allows all", nil, "anyone", true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := config.MaixCamConfig{
				Host:      "127.0.0.1",
				Port:      9999,
				AllowFrom: tt.allowList,
			}
			msgBus := bus.NewMessageBus()
			ch, _ := NewMaixCamChannel(cfg, msgBus)
			if ch.IsAllowed(tt.senderID) != tt.allowed {
				t.Errorf("IsAllowed(%q) = %v, want %v", tt.senderID, !tt.allowed, tt.allowed)
			}
		})
	}
}

func TestMaixCamMessageSerialization(t *testing.T) {
	msg := MaixCamMessage{
		Type:      "status",
		Tips:      "ok",
		Timestamp: 12345.0,
		Data:      map[string]interface{}{"cpu": 50.0},
	}
	data, err := json.Marshal(msg)
	if err != nil {
		t.Fatalf("failed to marshal: %v", err)
	}

	var parsed MaixCamMessage
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}
	if parsed.Type != "status" {
		t.Errorf("type = %q", parsed.Type)
	}
}
