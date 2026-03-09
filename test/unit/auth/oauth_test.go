// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package auth_test

import (
	"testing"

	"github.com/276793422/NemesisBot/module/auth"
)

// Test OpenAI OAuth Config
func TestOpenAIOAuthConfig(t *testing.T) {
	cfg := auth.OpenAIOAuthConfig()

	if cfg.Issuer != "https://auth.openai.com" {
		t.Errorf("OpenAIOAuthConfig() Issuer = %v, want https://auth.openai.com", cfg.Issuer)
	}
	if cfg.ClientID != "app_EMoamEEZ73f0CkXaXp7hrann" {
		t.Errorf("OpenAIOAuthConfig() ClientID = %v, want app_EMoamEEZ73f0CkXaXp7hrann", cfg.ClientID)
	}
	if cfg.Scopes != "openid profile email offline_access" {
		t.Errorf("OpenAIOAuthConfig() Scopes = %v, want 'openid profile email offline_access'", cfg.Scopes)
	}
	if cfg.Originator != "codex_cli_rs" {
		t.Errorf("OpenAIOAuthConfig() Originator = %v, want codex_cli_rs", cfg.Originator)
	}
	if cfg.Port != 1455 {
		t.Errorf("OpenAIOAuthConfig() Port = %v, want 1455", cfg.Port)
	}
}

// Test Build Authorize URL
func TestBuildAuthorizeURL(t *testing.T) {
	cfg := auth.OAuthProviderConfig{
		Issuer:     "https://example.com",
		ClientID:   "test_client",
		Scopes:     "openid profile",
		Originator: "test_originator",
		Port:       8080,
	}

	pkce := auth.PKCECodes{
		CodeChallenge: "challenge123",
		CodeVerifier:  "verifier123",
	}

	state := "state456"
	redirectURI := "http://localhost:8080/callback"

	url := auth.BuildAuthorizeURL(cfg, pkce, state, redirectURI)

	if url == "" {
		t.Fatal("BuildAuthorizeURL() returned empty string")
	}

	// Check URL contains required parameters
	requiredParams := []string{
		"response_type=code",
		"client_id=test_client",
		"scope=openid+profile",
		"code_challenge=challenge123",
		"code_challenge_method=S256",
		"state=state456",
		"originator=test_originator",
	}

	for _, param := range requiredParams {
		if !containsString(url, param) {
			t.Errorf("BuildAuthorizeURL() missing parameter: %s\nURL: %s", param, url)
		}
	}

	// Check URL starts with issuer
	if !hasPrefix(url, cfg.Issuer) {
		t.Errorf("BuildAuthorizeURL() URL doesn't start with issuer: %s", url)
	}
}

// Helper functions
func containsString(s, substr string) bool {
	return len(s) >= len(substr) && findSubstringString(s, substr)
}

func findSubstringString(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}

func hasPrefix(s, prefix string) bool {
	return len(s) >= len(prefix) && s[:len(prefix)] == prefix
}
