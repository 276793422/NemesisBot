// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package routing

import (
	"testing"

	"github.com/276793422/NemesisBot/module/config"
)

func TestNewRouteResolver(t *testing.T) {
	cfg := &config.Config{
		Bindings: []config.AgentBinding{},
	}

	resolver := NewRouteResolver(cfg)
	if resolver == nil {
		t.Fatal("NewRouteResolver() returned nil")
	}
	if resolver.cfg != cfg {
		t.Error("NewRouteResolver() did not set config")
	}
}

func TestRouteResolverResolveRoute(t *testing.T) {
	tests := []struct {
		name         string
		bindings     []config.AgentBinding
		agents       []config.AgentConfig
		input        RouteInput
		expectedAgent string
		expectedMatch string
	}{
		{
			name: "peer binding has highest priority",
			bindings: []config.AgentBinding{
				{
					Match: config.BindingMatch{
						Channel:   "discord",
						AccountID: "default",
						Peer: &config.PeerMatch{
							Kind: "direct",
							ID:   "user123",
						},
					},
					AgentID: "peer-agent",
				},
				{
					Match: config.BindingMatch{
						Channel:   "discord",
						AccountID: "default",
						GuildID:   "guild123",
					},
					AgentID: "guild-agent",
				},
			},
			agents: []config.AgentConfig{
				{ID: "main", Default: true},
				{ID: "peer-agent"},
			},
			input: RouteInput{
				Channel:   "discord",
				AccountID: "default",
				Peer: &RoutePeer{
					Kind: "direct",
					ID:   "user123",
				},
				GuildID: "guild123",
			},
			expectedAgent: "peer-agent",
			expectedMatch: "binding.peer",
		},
		{
			name: "parent peer binding has second priority",
			bindings: []config.AgentBinding{
				{
					Match: config.BindingMatch{
						Channel:   "discord",
						AccountID: "default",
						GuildID:   "guild123",
					},
					AgentID: "guild-agent",
				},
				{
					Match: config.BindingMatch{
						Channel:   "discord",
						AccountID: "default",
						Peer: &config.PeerMatch{
							Kind: "channel",
							ID:   "parent123",
						},
					},
					AgentID: "parent-agent",
				},
			},
			agents: []config.AgentConfig{
				{ID: "main", Default: true},
				{ID: "parent-agent"},
			},
			input: RouteInput{
				Channel:     "discord",
				AccountID:   "default",
				ParentPeer: &RoutePeer{
					Kind: "channel",
					ID:   "parent123",
				},
				GuildID: "guild123",
			},
			expectedAgent: "parent-agent",
			expectedMatch: "binding.peer.parent",
		},
		{
			name: "guild binding has third priority",
			bindings: []config.AgentBinding{
				{
					Match: config.BindingMatch{
						Channel:   "discord",
						AccountID: "default",
						GuildID:   "guild123",
					},
					AgentID: "guild-agent",
				},
				{
					Match: config.BindingMatch{
						Channel:   "discord",
						AccountID: "default",
					},
					AgentID: "account-agent",
				},
			},
			agents: []config.AgentConfig{
				{ID: "main", Default: true},
				{ID: "guild-agent"},
			},
			input: RouteInput{
				Channel:   "discord",
				AccountID: "default",
				GuildID:   "guild123",
			},
			expectedAgent: "guild-agent",
			expectedMatch: "binding.guild",
		},
		{
			name: "team binding has fourth priority",
			bindings: []config.AgentBinding{
				{
					Match: config.BindingMatch{
						Channel:   "slack",
						AccountID: "default",
						TeamID:    "team123",
					},
					AgentID: "team-agent",
				},
				{
					Match: config.BindingMatch{
						Channel:   "slack",
						AccountID: "default",
					},
					AgentID: "account-agent",
				},
			},
			agents: []config.AgentConfig{
				{ID: "main", Default: true},
				{ID: "team-agent"},
			},
			input: RouteInput{
				Channel:   "slack",
				AccountID: "default",
				TeamID:    "team123",
			},
			expectedAgent: "team-agent",
			expectedMatch: "binding.team",
		},
		{
			name: "account binding has fifth priority",
			bindings: []config.AgentBinding{
				{
					Match: config.BindingMatch{
						Channel:   "discord",
						AccountID: "account1",
					},
					AgentID: "account-agent",
				},
				{
					Match: config.BindingMatch{
						Channel:   "discord",
						AccountID: "*",
					},
					AgentID: "wildcard-agent",
				},
			},
			agents: []config.AgentConfig{
				{ID: "main", Default: true},
				{ID: "account-agent"},
			},
			input: RouteInput{
				Channel:   "discord",
				AccountID: "account1",
			},
			expectedAgent: "account-agent",
			expectedMatch: "binding.account",
		},
		{
			name: "channel wildcard binding has sixth priority",
			bindings: []config.AgentBinding{
				{
					Match: config.BindingMatch{
						Channel:   "discord",
						AccountID: "*",
					},
					AgentID: "wildcard-agent",
				},
			},
			agents: []config.AgentConfig{
				{ID: "main", Default: true},
				{ID: "wildcard-agent"},
			},
			input: RouteInput{
				Channel:   "discord",
				AccountID: "default",
			},
			expectedAgent: "wildcard-agent",
			expectedMatch: "binding.channel",
		},
		{
			name:   "default agent when no bindings match",
			bindings: []config.AgentBinding{},
			agents: []config.AgentConfig{
				{ID: "main", Default: true},
			},
			input: RouteInput{
				Channel:   "discord",
				AccountID: "default",
			},
			expectedAgent: "main",
			expectedMatch: "default",
		},
		{
			name: "first agent when no default specified",
			bindings: []config.AgentBinding{},
			agents: []config.AgentConfig{
				{ID: "first-agent"},
				{ID: "second-agent"},
			},
			input: RouteInput{
				Channel:   "discord",
				AccountID: "default",
			},
			expectedAgent: "first-agent",
			expectedMatch: "default",
		},
		{
			name: "case insensitive channel matching",
			bindings: []config.AgentBinding{
				{
					Match: config.BindingMatch{
						Channel:   "DISCORD",
						AccountID: "*",
					},
					AgentID: "discord-agent",
				},
			},
			agents: []config.AgentConfig{
				{ID: "main", Default: true},
				{ID: "discord-agent"},
			},
			input: RouteInput{
				Channel:   "discord",
				AccountID: "default",
			},
			expectedAgent: "discord-agent",
			expectedMatch: "binding.channel",
		},
		{
			name: "unknown agent falls back to default",
			bindings: []config.AgentBinding{
				{
					Match: config.BindingMatch{
						Channel:   "discord",
						AccountID: "*",
					},
					AgentID: "nonexistent-agent",
				},
			},
			agents: []config.AgentConfig{
				{ID: "main", Default: true},
			},
			input: RouteInput{
				Channel:   "discord",
				AccountID: "default",
			},
			expectedAgent: "main",
			expectedMatch: "binding.channel", // Binding matched, but agent didn't exist so fell back
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &config.Config{
				Bindings: tt.bindings,
				Agents: config.AgentsConfig{
					List: tt.agents,
				},
			}
			resolver := NewRouteResolver(cfg)
			result := resolver.ResolveRoute(tt.input)

			if result.AgentID != tt.expectedAgent {
				t.Errorf("ResolveRoute() AgentID = %q, want %q", result.AgentID, tt.expectedAgent)
			}
			if result.MatchedBy != tt.expectedMatch {
				t.Errorf("ResolveRoute() MatchedBy = %q, want %q", result.MatchedBy, tt.expectedMatch)
			}
			if result.Channel != tt.input.Channel {
				t.Errorf("ResolveRoute() Channel = %q, want %q", result.Channel, tt.input.Channel)
			}
		})
	}
}

func TestRouteResolverFilterBindings(t *testing.T) {
	cfg := &config.Config{
		Bindings: []config.AgentBinding{
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "account1",
				},
				AgentID: "agent1",
			},
			{
				Match: config.BindingMatch{
					Channel:   "slack",
					AccountID: "account1",
				},
				AgentID: "agent2",
			},
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "account2",
				},
				AgentID: "agent3",
			},
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "*",
				},
				AgentID: "agent4",
			},
		},
	}

	resolver := NewRouteResolver(cfg)

	t.Run("filter by channel and account", func(t *testing.T) {
		result := resolver.filterBindings("discord", "account1")
		if len(result) != 2 { // Wildcard also matches
			t.Errorf("filterBindings() returned %d bindings, want 2", len(result))
		}
		if len(result) > 0 && result[0].AgentID != "agent1" {
			t.Errorf("filterBindings()[0].AgentID = %q, want agent1", result[0].AgentID)
		}
	})

	t.Run("filter with wildcard account", func(t *testing.T) {
		result := resolver.filterBindings("discord", "default")
		if len(result) != 1 {
			t.Errorf("filterBindings() returned %d bindings, want 1", len(result))
		}
		if len(result) > 0 && result[0].AgentID != "agent4" {
			t.Errorf("filterBindings()[0].AgentID = %q, want agent4", result[0].AgentID)
		}
	})

	t.Run("no matches", func(t *testing.T) {
		result := resolver.filterBindings("telegram", "account1")
		if len(result) != 0 {
			t.Errorf("filterBindings() returned %d bindings, want 0", len(result))
		}
	})
}

func TestMatchesAccountID(t *testing.T) {
	tests := []struct {
		name         string
		matchAccountID string
		actual       string
		expected     bool
	}{
		{
			name:         "exact match",
			matchAccountID: "account1",
			actual:       "account1",
			expected:     true,
		},
		{
			name:         "case insensitive",
			matchAccountID: "Account1",
			actual:       "account1",
			expected:     true,
		},
		{
			name:         "wildcard matches any",
			matchAccountID: "*",
			actual:       "any-account",
			expected:     true,
		},
		{
			name:         "empty match uses default",
			matchAccountID: "",
			actual:       "default",
			expected:     true,
		},
		{
			name:         "empty match with non-default",
			matchAccountID: "",
			actual:       "account1",
			expected:     false,
		},
		{
			name:         "no match",
			matchAccountID: "account1",
			actual:       "account2",
			expected:     false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := matchesAccountID(tt.matchAccountID, tt.actual)
			if result != tt.expected {
				t.Errorf("matchesAccountID(%q, %q) = %v, want %v", tt.matchAccountID, tt.actual, result, tt.expected)
			}
		})
	}
}

func TestRouteResolverResolveDefaultAgentID(t *testing.T) {
	tests := []struct {
		name     string
		agents   []config.AgentConfig
		expected string
	}{
		{
			name:     "no agents returns default",
			agents:   []config.AgentConfig{},
			expected: DefaultAgentID,
		},
		{
			name: "marked default agent",
			agents: []config.AgentConfig{
				{ID: "main", Default: true},
				{ID: "other"},
			},
			expected: "main",
		},
		{
			name: "first agent when no default",
			agents: []config.AgentConfig{
				{ID: "first"},
				{ID: "second"},
			},
			expected: "first",
		},
		{
			name: "empty agent ID uses default",
			agents: []config.AgentConfig{
				{ID: "", Default: true},
				{ID: "fallback"},
			},
			expected: "main", // First agent with Default=true has empty ID, so falls through to first non-empty agent or default
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &config.Config{
				Agents: config.AgentsConfig{
					List: tt.agents,
				},
			}
			resolver := NewRouteResolver(cfg)
			result := resolver.resolveDefaultAgentID()
			if result != tt.expected {
				t.Errorf("resolveDefaultAgentID() = %q, want %q", result, tt.expected)
			}
		})
	}
}

func TestRouteResolverPickAgentID(t *testing.T) {
	tests := []struct {
		name         string
		agents       []config.AgentConfig
		agentID      string
		expected     string
	}{
		{
			name:    "existing agent",
			agents:  []config.AgentConfig{{ID: "agent1"}},
			agentID: "agent1",
			expected: "agent1",
		},
		{
			name:    "nonexistent agent falls back to default",
			agents:  []config.AgentConfig{{ID: "main", Default: true}},
			agentID: "nonexistent",
			expected: "main",
		},
		{
			name:    "empty agent ID uses default",
			agents:  []config.AgentConfig{{ID: "main", Default: true}},
			agentID: "",
			expected: "main",
		},
		{
			name:    "no agents returns normalized input",
			agents:  []config.AgentConfig{},
			agentID: "test-agent",
			expected: "test-agent",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := &config.Config{
				Agents: config.AgentsConfig{
					List: tt.agents,
				},
			}
			resolver := NewRouteResolver(cfg)
			result := resolver.pickAgentID(tt.agentID)
			if result != tt.expected {
				t.Errorf("pickAgentID(%q) = %q, want %q", tt.agentID, result, tt.expected)
			}
		})
	}
}

func TestRouteResolverFindPeerMatch(t *testing.T) {
	cfg := &config.Config{
		Bindings: []config.AgentBinding{
			{
				Match: config.BindingMatch{
					Peer: &config.PeerMatch{
						Kind: "direct",
						ID:   "user123",
					},
				},
				AgentID: "agent1",
			},
			{
				Match: config.BindingMatch{
					Peer: &config.PeerMatch{
						Kind: "channel",
						ID:   "guild123",
					},
				},
				AgentID: "agent2",
			},
			{
				Match: config.BindingMatch{
					Peer: &config.PeerMatch{
						Kind: "direct",
						ID:   "",
					},
				},
				AgentID: "agent3",
			},
			{
				Match: config.BindingMatch{
					Peer: &config.PeerMatch{
						Kind: "",
						ID:   "user123",
					},
				},
				AgentID: "agent4",
			},
		},
	}

	resolver := NewRouteResolver(cfg)

	t.Run("matching peer", func(t *testing.T) {
		peer := &RoutePeer{Kind: "direct", ID: "user123"}
		result := resolver.findPeerMatch(cfg.Bindings, peer)
		if result == nil {
			t.Fatal("findPeerMatch() returned nil")
		}
		if result.AgentID != "agent1" {
			t.Errorf("findPeerMatch().AgentID = %q, want agent1", result.AgentID)
		}
	})

	t.Run("no match", func(t *testing.T) {
		peer := &RoutePeer{Kind: "direct", ID: "user999"}
		result := resolver.findPeerMatch(cfg.Bindings, peer)
		if result != nil {
			t.Errorf("findPeerMatch() returned non-nil: %v", result)
		}
	})

	t.Run("case insensitive kind", func(t *testing.T) {
		peer := &RoutePeer{Kind: "DIRECT", ID: "user123"}
		result := resolver.findPeerMatch(cfg.Bindings, peer)
		if result == nil {
			t.Fatal("findPeerMatch() returned nil")
		}
	})

	t.Run("peer with empty ID after trim", func(t *testing.T) {
		peer := &RoutePeer{Kind: "direct", ID: "   "}
		result := resolver.findPeerMatch(cfg.Bindings, peer)
		if result != nil {
			t.Errorf("findPeerMatch() should not match peer with empty ID after trim")
		}
	})

	t.Run("peer with empty kind after trim", func(t *testing.T) {
		peer := &RoutePeer{Kind: "   ", ID: "user123"}
		result := resolver.findPeerMatch(cfg.Bindings, peer)
		if result != nil {
			t.Errorf("findPeerMatch() should not match peer with empty kind after trim")
		}
	})

	t.Run("peer binding with empty ID in config", func(t *testing.T) {
		peer := &RoutePeer{Kind: "direct", ID: "user123"}
		result := resolver.findPeerMatch(cfg.Bindings, peer)
		// Should not match agent3 or agent4
		if result != nil && result.AgentID == "agent3" {
			t.Errorf("findPeerMatch() should not match binding with empty peer ID")
		}
	})
}

func TestRouteResolverFindGuildMatch(t *testing.T) {
	cfg := &config.Config{
		Bindings: []config.AgentBinding{
			{
				Match: config.BindingMatch{
					Channel: "discord",
					GuildID: "guild123",
				},
				AgentID: "agent1",
			},
			{
				Match: config.BindingMatch{
					Channel: "discord",
					GuildID: "guild456",
				},
				AgentID: "agent2",
			},
			{
				Match: config.BindingMatch{
					Channel: "discord",
					GuildID: "   ",
				},
				AgentID: "agent3",
			},
		},
	}

	resolver := NewRouteResolver(cfg)

	t.Run("matching guild", func(t *testing.T) {
		result := resolver.findGuildMatch(cfg.Bindings, "guild123")
		if result == nil {
			t.Fatal("findGuildMatch() returned nil")
		}
		if result.AgentID != "agent1" {
			t.Errorf("findGuildMatch().AgentID = %q, want agent1", result.AgentID)
		}
	})

	t.Run("no match", func(t *testing.T) {
		result := resolver.findGuildMatch(cfg.Bindings, "guild999")
		if result != nil {
			t.Errorf("findGuildMatch() returned non-nil: %v", result)
		}
	})

	t.Run("empty guild ID in config", func(t *testing.T) {
		result := resolver.findGuildMatch(cfg.Bindings, "")
		if result != nil {
			t.Errorf("findGuildMatch() should not match binding with empty guild ID")
		}
	})

	t.Run("whitespace guild ID in config", func(t *testing.T) {
		result := resolver.findGuildMatch(cfg.Bindings, "   ")
		if result != nil {
			t.Errorf("findGuildMatch() should not match binding with whitespace guild ID")
		}
	})

	t.Run("empty guild ID input", func(t *testing.T) {
		result := resolver.findGuildMatch(cfg.Bindings, "")
		if result != nil {
			t.Errorf("findGuildMatch() should not match for empty input guild ID")
		}
	})
}

func TestRouteResolverFindTeamMatch(t *testing.T) {
	cfg := &config.Config{
		Bindings: []config.AgentBinding{
			{
				Match: config.BindingMatch{
					Channel: "slack",
					TeamID:  "team123",
				},
				AgentID: "agent1",
			},
			{
				Match: config.BindingMatch{
					Channel: "slack",
					TeamID:  "team456",
				},
				AgentID: "agent2",
			},
			{
				Match: config.BindingMatch{
					Channel: "slack",
					TeamID:  "   ",
				},
				AgentID: "agent3",
			},
		},
	}

	resolver := NewRouteResolver(cfg)

	t.Run("matching team", func(t *testing.T) {
		result := resolver.findTeamMatch(cfg.Bindings, "team123")
		if result == nil {
			t.Fatal("findTeamMatch() returned nil")
		}
		if result.AgentID != "agent1" {
			t.Errorf("findTeamMatch().AgentID = %q, want agent1", result.AgentID)
		}
	})

	t.Run("no match", func(t *testing.T) {
		result := resolver.findTeamMatch(cfg.Bindings, "team999")
		if result != nil {
			t.Errorf("findTeamMatch() returned non-nil: %v", result)
		}
	})

	t.Run("empty team ID in config", func(t *testing.T) {
		result := resolver.findTeamMatch(cfg.Bindings, "")
		if result != nil {
			t.Errorf("findTeamMatch() should not match binding with empty team ID")
		}
	})

	t.Run("whitespace team ID in config", func(t *testing.T) {
		result := resolver.findTeamMatch(cfg.Bindings, "   ")
		if result != nil {
			t.Errorf("findTeamMatch() should not match binding with whitespace team ID")
		}
	})

	t.Run("empty team ID input", func(t *testing.T) {
		result := resolver.findTeamMatch(cfg.Bindings, "")
		if result != nil {
			t.Errorf("findTeamMatch() should not match for empty input team ID")
		}
	})
}

func TestRouteResolverFindAccountMatch(t *testing.T) {
	cfg := &config.Config{
		Bindings: []config.AgentBinding{
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "account1",
				},
				AgentID: "agent1",
			},
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "account2",
				},
				AgentID: "agent2",
			},
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "*",
				},
				AgentID: "agent3",
			},
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "account4",
					Peer: &config.PeerMatch{
						Kind: "direct",
						ID:   "user123",
					},
				},
				AgentID: "agent4",
			},
		},
	}

	resolver := NewRouteResolver(cfg)

	t.Run("returns first account match", func(t *testing.T) {
		result := resolver.findAccountMatch(cfg.Bindings)
		if result == nil {
			t.Fatal("findAccountMatch() returned nil")
		}
		if result.AgentID != "agent1" {
			t.Errorf("findAccountMatch().AgentID = %q, want agent1", result.AgentID)
		}
	})

	t.Run("skip wildcard account", func(t *testing.T) {
		result := resolver.findAccountMatch(cfg.Bindings)
		if result != nil && result.AgentID == "agent3" {
			t.Errorf("findAccountMatch() should not match wildcard account")
		}
	})

	t.Run("skip binding with peer", func(t *testing.T) {
		result := resolver.findAccountMatch(cfg.Bindings)
		if result != nil && result.AgentID == "agent4" {
			t.Errorf("findAccountMatch() should not match binding with peer")
		}
	})

	t.Run("no account match when all have peers", func(t *testing.T) {
		bindings := []config.AgentBinding{
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "account1",
					Peer: &config.PeerMatch{
						Kind: "direct",
						ID:   "user123",
					},
				},
				AgentID: "agent1",
			},
		}
		result := resolver.findAccountMatch(bindings)
		if result != nil {
			t.Errorf("findAccountMatch() should not match when all bindings have peers")
		}
	})

	t.Run("no account match when all have guilds", func(t *testing.T) {
		bindings := []config.AgentBinding{
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "account1",
					GuildID:   "guild123",
				},
				AgentID: "agent1",
			},
		}
		result := resolver.findAccountMatch(bindings)
		if result != nil {
			t.Errorf("findAccountMatch() should not match when all bindings have guilds")
		}
	})
}

func TestRouteResolverFindChannelWildcardMatch(t *testing.T) {
	cfg := &config.Config{
		Bindings: []config.AgentBinding{
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "*",
				},
				AgentID: "agent1",
			},
			{
				Match: config.BindingMatch{
					Channel:   "slack",
					AccountID: "*",
				},
				AgentID: "agent2",
			},
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "*",
					Peer: &config.PeerMatch{
						Kind: "direct",
						ID:   "user123",
					},
				},
				AgentID: "agent3",
			},
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "*",
					GuildID:   "guild123",
				},
				AgentID: "agent4",
			},
		},
	}

	resolver := NewRouteResolver(cfg)

	t.Run("returns first channel wildcard match", func(t *testing.T) {
		result := resolver.findChannelWildcardMatch(cfg.Bindings)
		if result == nil {
			t.Fatal("findChannelWildcardMatch() returned nil")
		}
		if result.AgentID != "agent1" {
			t.Errorf("findChannelWildcardMatch().AgentID = %q, want agent1", result.AgentID)
		}
	})

	t.Run("skip binding with peer", func(t *testing.T) {
		result := resolver.findChannelWildcardMatch(cfg.Bindings)
		if result != nil && result.AgentID == "agent3" {
			t.Errorf("findChannelWildcardMatch() should not match binding with peer")
		}
	})

	t.Run("skip binding with guild", func(t *testing.T) {
		result := resolver.findChannelWildcardMatch(cfg.Bindings)
		if result != nil && result.AgentID == "agent4" {
			t.Errorf("findChannelWildcardMatch() should not match binding with guild")
		}
	})

	t.Run("no wildcard match when all have peers", func(t *testing.T) {
		bindings := []config.AgentBinding{
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "*",
					Peer: &config.PeerMatch{
						Kind: "direct",
						ID:   "user123",
					},
				},
				AgentID: "agent1",
			},
		}
		result := resolver.findChannelWildcardMatch(bindings)
		if result != nil {
			t.Errorf("findChannelWildcardMatch() should not match when all bindings have peers")
		}
	})

	t.Run("no wildcard match when all have guilds", func(t *testing.T) {
		bindings := []config.AgentBinding{
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "*",
					GuildID:   "guild123",
				},
				AgentID: "agent1",
			},
		}
		result := resolver.findChannelWildcardMatch(bindings)
		if result != nil {
			t.Errorf("findChannelWildcardMatch() should not match when all bindings have guilds")
		}
	})
}

func TestRouteResolverConcurrent(t *testing.T) {
	cfg := &config.Config{
		Bindings: []config.AgentBinding{
			{
				Match: config.BindingMatch{
					Channel:   "discord",
					AccountID: "*",
				},
				AgentID: "agent1",
			},
		},
		Agents: config.AgentsConfig{
			List: []config.AgentConfig{{ID: "main", Default: true}},
		},
	}

	resolver := NewRouteResolver(cfg)
	done := make(chan bool)

	for i := 0; i < 100; i++ {
		go func() {
			input := RouteInput{
				Channel:   "discord",
				AccountID: "default",
			}
			resolver.ResolveRoute(input)
			done <- true
		}()
	}

	for i := 0; i < 100; i++ {
		<-done
	}
}
