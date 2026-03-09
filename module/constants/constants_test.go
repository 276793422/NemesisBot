package constants

import "testing"

func TestIsInternalChannel(t *testing.T) {
	tests := []struct {
		name    string
		channel string
		want    bool
	}{
		{"CLI channel", "cli", true},
		{"System channel", "system", true},
		{"Subagent channel", "subagent", true},
		{"Telegram channel", "telegram", false},
		{"Discord channel", "discord", false},
		{"RPC channel", "rpc", false},
		{"Empty string", "", false},
		{"Web channel", "web", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := IsInternalChannel(tt.channel); got != tt.want {
				t.Errorf("IsInternalChannel(%v) = %v, want %v", tt.channel, got, tt.want)
			}
		})
	}
}

func TestInternalChannelsConsistency(t *testing.T) {
	// Test that known internal channels are consistently identified
	internalChannels := []string{"cli", "system", "subagent"}

	for _, ch := range internalChannels {
		if !IsInternalChannel(ch) {
			t.Errorf("Channel %v should be marked as internal", ch)
		}
	}
}
