// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package routing

import (
	"testing"
)

func TestBuildAgentMainSessionKey(t *testing.T) {
	tests := []struct {
		name     string
		agentID  string
		expected string
	}{
		{
			name:     "simple agent ID",
			agentID:  "main",
			expected: "agent:main:main",
		},
		{
			name:     "agent ID with dashes",
			agentID:  "my-agent",
			expected: "agent:my-agent:main",
		},
		{
			name:     "uppercase normalized",
			agentID:  "MyAgent",
			expected: "agent:myagent:main",
		},
		{
			name:     "empty agent ID uses default",
			agentID:  "",
			expected: "agent:main:main",
		},
		{
			name:     "whitespace agent ID uses default",
			agentID:  "  ",
			expected: "agent:main:main",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := BuildAgentMainSessionKey(tt.agentID)
			if result != tt.expected {
				t.Errorf("BuildAgentMainSessionKey(%q) = %q, want %q", tt.agentID, result, tt.expected)
			}
		})
	}
}

func TestBuildAgentPeerSessionKey(t *testing.T) {
	tests := []struct {
		name     string
		params   SessionKeyParams
		expected string
	}{
		{
			name: "DM scope - main",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer: &RoutePeer{
					Kind: "direct",
					ID:   "user123",
				},
				DMScope: DMScopeMain,
			},
			expected: "agent:main:main",
		},
		{
			name: "DM scope - per-peer",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer: &RoutePeer{
					Kind: "direct",
					ID:   "user123",
				},
				DMScope: DMScopePerPeer,
			},
			expected: "agent:main:direct:user123",
		},
		{
			name: "DM scope - per-channel-peer",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer: &RoutePeer{
					Kind: "direct",
					ID:   "user123",
				},
				DMScope: DMScopePerChannelPeer,
			},
			expected: "agent:main:discord:direct:user123",
		},
		{
			name: "DM scope - per-account-channel-peer",
			params: SessionKeyParams{
				AgentID:   "main",
				Channel:   "discord",
				AccountID: "account1",
				Peer: &RoutePeer{
					Kind: "direct",
					ID:   "user123",
				},
				DMScope: DMScopePerAccountChannelPeer,
			},
			expected: "agent:main:discord:account1:direct:user123",
		},
		{
			name: "Group channel",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer: &RoutePeer{
					Kind: "channel",
					ID:   "guild123",
				},
			},
			expected: "agent:main:discord:channel:guild123",
		},
		{
			name: "Unknown peer kind defaults to direct",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer: &RoutePeer{
					Kind: "",
					ID:   "user123",
				},
				DMScope: DMScopePerPeer,
			},
			expected: "agent:main:direct:user123",
		},
		{
			name: "Nil peer defaults to direct",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer:    nil,
			},
			expected: "agent:main:main",
		},
		{
			name: "Group peer with empty ID",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer: &RoutePeer{
					Kind: "group",
					ID:   "",
				},
			},
			expected: "agent:main:discord:group:unknown",
		},
		{
			name: "Identity link resolution",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "telegram",
				Peer: &RoutePeer{
					Kind: "direct",
					ID:   "user123",
				},
				DMScope:       DMScopePerPeer,
				IdentityLinks: map[string][]string{"canonical": {"telegram:user123"}},
			},
			expected: "agent:main:direct:canonical",
		},
		{
			name: "Identity link with scoped ID",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer: &RoutePeer{
					Kind: "direct",
					ID:   "user123",
				},
				DMScope:       DMScopePerPeer,
				IdentityLinks: map[string][]string{"canonical": {"discord:user123"}},
			},
			expected: "agent:main:direct:canonical",
		},
		{
			name: "Uppercase channel normalized",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "DISCORD",
				Peer: &RoutePeer{
					Kind: "channel",
					ID:   "guild123",
				},
			},
			expected: "agent:main:discord:channel:guild123",
		},
		{
			name: "Uppercase peer ID normalized",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer: &RoutePeer{
					Kind: "channel",
					ID:   "GUILD123",
				},
			},
			expected: "agent:main:discord:channel:guild123",
		},
		{
			name: "Empty channel defaults to unknown",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "",
				Peer: &RoutePeer{
					Kind: "channel",
					ID:   "guild123",
				},
			},
			expected: "agent:main:unknown:channel:guild123",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := BuildAgentPeerSessionKey(tt.params)
			if result != tt.expected {
				t.Errorf("BuildAgentPeerSessionKey() = %q, want %q", result, tt.expected)
			}
		})
	}
}

func TestParseAgentSessionKey(t *testing.T) {
	tests := []struct {
		name        string
		sessionKey  string
		expected    *ParsedSessionKey
		expectNil   bool
	}{
		{
			name:       "valid session key",
			sessionKey: "agent:main:main",
			expected: &ParsedSessionKey{
				AgentID: "main",
				Rest:    "main",
			},
		},
		{
			name:       "session key with channel",
			sessionKey: "agent:main:discord:direct:user123",
			expected: &ParsedSessionKey{
				AgentID: "main",
				Rest:    "discord:direct:user123",
			},
		},
		{
			name:       "empty string returns nil",
			sessionKey: "",
			expectNil:  true,
		},
		{
			name:       "whitespace only returns nil",
			sessionKey: "   ",
			expectNil:  true,
		},
		{
			name:       "missing agent prefix returns nil",
			sessionKey: "main:main",
			expectNil:  true,
		},
		{
			name:       "too few parts returns nil",
			sessionKey: "agent:main",
			expectNil:  true,
		},
		{
			name:       "empty agent ID returns nil",
			sessionKey: "agent::main",
			expectNil:  true,
		},
		{
			name:       "empty rest returns nil",
			sessionKey: "agent:main:",
			expectNil:  true,
		},
		{
			name:       "whitespace trimmed",
			sessionKey: "  agent:main:main  ",
			expected: &ParsedSessionKey{
				AgentID: "main",
				Rest:    "main",
			},
		},
		{
			name:       "subagent session key",
			sessionKey: "agent:main:subagent:test:main",
			expected: &ParsedSessionKey{
				AgentID: "main",
				Rest:    "subagent:test:main",
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ParseAgentSessionKey(tt.sessionKey)
			if tt.expectNil {
				if result != nil {
					t.Errorf("ParseAgentSessionKey(%q) = %+v, want nil", tt.sessionKey, result)
				}
			} else {
				if result == nil {
					t.Errorf("ParseAgentSessionKey(%q) = nil, want %+v", tt.sessionKey, tt.expected)
				} else if result.AgentID != tt.expected.AgentID || result.Rest != tt.expected.Rest {
					t.Errorf("ParseAgentSessionKey(%q) = %+v, want %+v", tt.sessionKey, result, tt.expected)
				}
			}
		})
	}
}

func TestIsSubagentSessionKey(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected bool
	}{
		{
			name:     "subagent prefix",
			input:    "subagent:test:main",
			expected: true,
		},
		{
			name:     "uppercase subagent prefix",
			input:    "SUBAGENT:test:main",
			expected: true,
		},
		{
			name:     "agent scoped subagent",
			input:    "agent:main:subagent:test:main",
			expected: true,
		},
		{
			name:     "regular agent session",
			input:    "agent:main:main",
			expected: false,
		},
		{
			name:     "agent with peer",
			input:    "agent:main:discord:direct:user123",
			expected: false,
		},
		{
			name:     "empty string",
			input:    "",
			expected: false,
		},
		{
			name:     "whitespace only",
			input:    "  ",
			expected: false,
		},
		{
			name:     "agent prefix without subagent",
			input:    "agent:test:sub:agent",
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := IsSubagentSessionKey(tt.input)
			if result != tt.expected {
				t.Errorf("IsSubagentSessionKey(%q) = %v, want %v", tt.input, result, tt.expected)
			}
		})
	}
}

func TestResolveLinkedPeerID(t *testing.T) {
	tests := []struct {
		name          string
		identityLinks map[string][]string
		channel       string
		peerID        string
		expected      string
	}{
		{
			name:     "no identity links",
			identityLinks: map[string][]string{},
			channel:  "discord",
			peerID:   "user123",
			expected: "",
		},
		{
			name: "nil identity links",
			identityLinks: nil,
			channel:  "discord",
			peerID:   "user123",
			expected: "",
		},
		{
			name: "exact match",
			identityLinks: map[string][]string{
				"canonical": {"user123"},
			},
			channel:  "discord",
			peerID:   "user123",
			expected: "canonical",
		},
		{
			name: "scoped match",
			identityLinks: map[string][]string{
				"canonical": {"discord:user123"},
			},
			channel:  "discord",
			peerID:   "user123",
			expected: "canonical",
		},
		{
			name: "case insensitive match",
			identityLinks: map[string][]string{
				"canonical": {"DISCORD:USER123"},
			},
			channel:  "discord",
			peerID:   "user123",
			expected: "canonical",
		},
		{
			name: "no match",
			identityLinks: map[string][]string{
				"canonical": {"discord:user456"},
			},
			channel:  "discord",
			peerID:   "user123",
			expected: "",
		},
		{
			name: "empty peer ID",
			identityLinks: map[string][]string{
				"canonical": {"user123"},
			},
			channel:  "discord",
			peerID:   "",
			expected: "",
		},
		{
			name: "empty channel name",
			identityLinks: map[string][]string{
				"canonical": {":user123"},
			},
			channel:  "",
			peerID:   "user123",
			expected: "",
		},
		{
			name: "whitespace peer ID",
			identityLinks: map[string][]string{
				"canonical": {"user123"},
			},
			channel:  "discord",
			peerID:   "  user123  ",
			expected: "canonical",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := resolveLinkedPeerID(tt.identityLinks, tt.channel, tt.peerID)
			if result != tt.expected {
				t.Errorf("resolveLinkedPeerID() = %q, want %q", result, tt.expected)
			}
		})
	}
}

func TestNormalizeChannel(t *testing.T) {
	tests := []struct {
		name     string
		channel  string
		expected string
	}{
		{
			name:     "normal channel",
			channel:  "discord",
			expected: "discord",
		},
		{
			name:     "uppercase converted",
			channel:  "DISCORD",
			expected: "discord",
		},
		{
			name:     "whitespace trimmed",
			channel:  "  discord  ",
			expected: "discord",
		},
		{
			name:     "empty returns unknown",
			channel:  "",
			expected: "unknown",
		},
		{
			name:     "whitespace only returns unknown",
			channel:  "   ",
			expected: "unknown",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := normalizeChannel(tt.channel)
			if result != tt.expected {
				t.Errorf("normalizeChannel(%q) = %q, want %q", tt.channel, result, tt.expected)
			}
		})
	}
}

func TestBuildAgentPeerSessionKeyAdditionalCases(t *testing.T) {
	tests := []struct {
		name     string
		params   SessionKeyParams
		expected string
	}{
		{
			name: "Empty peer ID with per-peer scope",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer: &RoutePeer{
					Kind: "direct",
					ID:   "",
				},
				DMScope: DMScopePerPeer,
			},
			expected: "agent:main:main", // Falls back to main session
		},
		{
			name: "Empty peer ID with per-channel-peer scope",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer: &RoutePeer{
					Kind: "direct",
					ID:   "",
				},
				DMScope: DMScopePerChannelPeer,
			},
			expected: "agent:main:main", // Falls back to main session
		},
		{
			name: "Empty peer ID with per-account-channel-peer scope",
			params: SessionKeyParams{
				AgentID:   "main",
				Channel:   "discord",
				AccountID: "account1",
				Peer: &RoutePeer{
					Kind: "direct",
					ID:   "",
				},
				DMScope: DMScopePerAccountChannelPeer,
			},
			expected: "agent:main:main", // Falls back to main session
		},
		{
			name: "Group peer with empty ID but not direct",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer: &RoutePeer{
					Kind: "channel",
					ID:   "",
				},
			},
			expected: "agent:main:discord:channel:unknown",
		},
		{
			name: "Group peer with nil peer",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				DMScope: DMScopeMain,
			},
			expected: "agent:main:main",
		},
		{
			name: "DM with identity link but empty peer ID",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "telegram",
				Peer: &RoutePeer{
					Kind: "direct",
					ID:   "",
				},
				DMScope:       DMScopePerPeer,
				IdentityLinks: map[string][]string{"canonical": {"telegram:user123"}},
			},
			expected: "agent:main:main", // Empty peer ID, falls back to main
		},
		{
			name: "Direct peer with empty kind",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer: &RoutePeer{
					Kind: "",
					ID:   "user123",
				},
				DMScope: DMScopePerPeer,
			},
			expected: "agent:main:direct:user123",
		},
		{
			name: "Direct peer with spaces in ID",
			params: SessionKeyParams{
				AgentID: "main",
				Channel: "discord",
				Peer: &RoutePeer{
					Kind: "direct",
					ID:   "  user123  ",
				},
				DMScope: DMScopePerPeer,
			},
			expected: "agent:main:direct:user123", // ID should be trimmed
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := BuildAgentPeerSessionKey(tt.params)
			if result != tt.expected {
				t.Errorf("BuildAgentPeerSessionKey() = %q, want %q", result, tt.expected)
			}
		})
	}
}
