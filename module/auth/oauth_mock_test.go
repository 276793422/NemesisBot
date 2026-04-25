// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package auth

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/path"
)

// TestExchangeCodeForTokens tests the exchangeCodeForTokens function with mock server
func TestExchangeCodeForTokens(t *testing.T) {
	t.Run("successful token exchange", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.Method != "POST" {
				t.Errorf("Expected POST, got %s", r.Method)
			}
			if r.URL.Path != "/oauth/token" {
				t.Errorf("Expected /oauth/token, got %s", r.URL.Path)
			}

			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(map[string]interface{}{
				"access_token":  "test_access_token",
				"refresh_token": "test_refresh_token",
				"expires_in":    3600,
				"id_token":      "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJjaGF0Z3B0X2FjY291bnRfaWQiOiJhY2NvdW50XzEyMyJ9.signature",
			})
		}))
		defer server.Close()

		cfg := OAuthProviderConfig{
			Issuer:   server.URL,
			ClientID: "test_client",
		}

		cred, err := exchangeCodeForTokens(cfg, "test_code", "test_verifier", "http://localhost:8080/callback")
		if err != nil {
			t.Fatalf("exchangeCodeForTokens() error = %v", err)
		}
		if cred.AccessToken != "test_access_token" {
			t.Errorf("AccessToken = %s, want test_access_token", cred.AccessToken)
		}
		if cred.RefreshToken != "test_refresh_token" {
			t.Errorf("RefreshToken = %s, want test_refresh_token", cred.RefreshToken)
		}
		if cred.AccountID != "account_123" {
			t.Errorf("AccountID = %s, want account_123", cred.AccountID)
		}
		if cred.AuthMethod != "oauth" {
			t.Errorf("AuthMethod = %s, want oauth", cred.AuthMethod)
		}
		if cred.ExpiresAt.IsZero() {
			t.Error("ExpiresAt should be set")
		}
	})

	t.Run("server returns error", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(http.StatusBadRequest)
			fmt.Fprint(w, "invalid grant")
		}))
		defer server.Close()

		cfg := OAuthProviderConfig{
			Issuer:   server.URL,
			ClientID: "test_client",
		}

		_, err := exchangeCodeForTokens(cfg, "bad_code", "verifier", "http://localhost:8080/callback")
		if err == nil {
			t.Fatal("Expected error for bad request")
		}
		if !strings.Contains(err.Error(), "token exchange failed") {
			t.Errorf("Error should mention token exchange failed, got: %v", err)
		}
	})

	t.Run("server returns no access token", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(map[string]interface{}{
				"refresh_token": "some_token",
			})
		}))
		defer server.Close()

		cfg := OAuthProviderConfig{
			Issuer:   server.URL,
			ClientID: "test_client",
		}

		_, err := exchangeCodeForTokens(cfg, "code", "verifier", "http://localhost:8080/callback")
		if err == nil {
			t.Fatal("Expected error for no access token")
		}
		if !strings.Contains(err.Error(), "no access token") {
			t.Errorf("Error should mention no access token, got: %v", err)
		}
	})

	t.Run("server returns invalid JSON", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			fmt.Fprint(w, "invalid json")
		}))
		defer server.Close()

		cfg := OAuthProviderConfig{
			Issuer:   server.URL,
			ClientID: "test_client",
		}

		_, err := exchangeCodeForTokens(cfg, "code", "verifier", "http://localhost:8080/callback")
		if err == nil {
			t.Fatal("Expected error for invalid JSON")
		}
	})

	t.Run("connection refused", func(t *testing.T) {
		cfg := OAuthProviderConfig{
			Issuer:   "http://localhost:1",
			ClientID: "test_client",
		}

		_, err := exchangeCodeForTokens(cfg, "code", "verifier", "http://localhost:8080/callback")
		if err == nil {
			t.Fatal("Expected connection error")
		}
	})
}

// TestRefreshAccessTokenWithMock tests RefreshAccessToken with mock server
func TestRefreshAccessTokenWithMock(t *testing.T) {
	t.Run("successful refresh", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.Method != "POST" {
				t.Errorf("Expected POST, got %s", r.Method)
			}
			if r.URL.Path != "/oauth/token" {
				t.Errorf("Expected /oauth/token, got %s", r.URL.Path)
			}

			// Verify form values
			if err := r.ParseForm(); err != nil {
				t.Fatalf("Failed to parse form: %v", err)
			}
			if r.FormValue("grant_type") != "refresh_token" {
				t.Errorf("grant_type = %s, want refresh_token", r.FormValue("grant_type"))
			}
			if r.FormValue("refresh_token") != "valid_refresh_token" {
				t.Errorf("refresh_token = %s, want valid_refresh_token", r.FormValue("refresh_token"))
			}

			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(map[string]interface{}{
				"access_token":  "new_access_token",
				"refresh_token": "new_refresh_token",
				"expires_in":    7200,
			})
		}))
		defer server.Close()

		cfg := OAuthProviderConfig{
			Issuer:   server.URL,
			ClientID: "test_client",
		}

		cred := &AuthCredential{
			AccessToken:  "old_access_token",
			RefreshToken: "valid_refresh_token",
			Provider:     "openai",
			AuthMethod:   "oauth",
			AccountID:    "account_123",
		}

		refreshed, err := RefreshAccessToken(cred, cfg)
		if err != nil {
			t.Fatalf("RefreshAccessToken() error = %v", err)
		}
		if refreshed.AccessToken != "new_access_token" {
			t.Errorf("AccessToken = %s, want new_access_token", refreshed.AccessToken)
		}
		if refreshed.RefreshToken != "new_refresh_token" {
			t.Errorf("RefreshToken = %s, want new_refresh_token", refreshed.RefreshToken)
		}
		// Should preserve account ID from original
		if refreshed.AccountID != "account_123" {
			t.Errorf("AccountID = %s, want account_123", refreshed.AccountID)
		}
	})

	t.Run("server returns new token without refresh_token", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(map[string]interface{}{
				"access_token": "new_access_token",
				"expires_in":   3600,
			})
		}))
		defer server.Close()

		cfg := OAuthProviderConfig{
			Issuer:   server.URL,
			ClientID: "test_client",
		}

		cred := &AuthCredential{
			RefreshToken: "original_refresh",
			Provider:     "openai",
		}

		refreshed, err := RefreshAccessToken(cred, cfg)
		if err != nil {
			t.Fatalf("RefreshAccessToken() error = %v", err)
		}
		// Should preserve original refresh token
		if refreshed.RefreshToken != "original_refresh" {
			t.Errorf("Should preserve original RefreshToken, got %s", refreshed.RefreshToken)
		}
	})

	t.Run("server returns error status", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(http.StatusUnauthorized)
			fmt.Fprint(w, "invalid refresh token")
		}))
		defer server.Close()

		cfg := OAuthProviderConfig{
			Issuer:   server.URL,
			ClientID: "test_client",
		}

		cred := &AuthCredential{
			RefreshToken: "bad_token",
			Provider:     "openai",
		}

		_, err := RefreshAccessToken(cred, cfg)
		if err == nil {
			t.Fatal("Expected error for unauthorized")
		}
		if !strings.Contains(err.Error(), "token refresh failed") {
			t.Errorf("Error should mention token refresh failed, got: %v", err)
		}
	})
}

// TestPollDeviceCode tests pollDeviceCode with mock server
func TestPollDeviceCode(t *testing.T) {
	t.Run("successful poll", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			if r.URL.Path == "/api/accounts/deviceauth/token" {
				// First call: poll device code
				json.NewEncoder(w).Encode(map[string]interface{}{
					"authorization_code": "test_auth_code",
					"code_challenge":     "test_challenge",
					"code_verifier":      "test_verifier",
				})
			} else if r.URL.Path == "/oauth/token" {
				// Second call: exchange code for tokens
				json.NewEncoder(w).Encode(map[string]interface{}{
					"access_token":  "test_access_token",
					"refresh_token": "test_refresh_token",
					"expires_in":    3600,
				})
			} else {
				w.WriteHeader(http.StatusNotFound)
			}
		}))
		defer server.Close()

		// Override the server URL for token exchange
		cfg := OAuthProviderConfig{
			Issuer:   server.URL,
			ClientID: "test_client",
		}

		cred, err := pollDeviceCode(cfg, "device_auth_123", "USER-CODE")
		if err != nil {
			t.Fatalf("pollDeviceCode() error = %v", err)
		}
		if cred == nil {
			t.Fatal("Expected credential, got nil")
		}
		if cred.AccessToken != "test_access_token" {
			t.Errorf("AccessToken = %s, want test_access_token", cred.AccessToken)
		}
	})

	t.Run("pending response", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(http.StatusBadRequest)
			fmt.Fprint(w, "pending")
		}))
		defer server.Close()

		cfg := OAuthProviderConfig{
			Issuer:   server.URL,
			ClientID: "test_client",
		}

		cred, err := pollDeviceCode(cfg, "device_auth_123", "USER-CODE")
		if err == nil {
			t.Fatal("Expected error for pending status")
		}
		if cred != nil {
			t.Error("Expected nil credential for pending status")
		}
	})

	t.Run("network error", func(t *testing.T) {
		cfg := OAuthProviderConfig{
			Issuer:   "http://localhost:1",
			ClientID: "test_client",
		}

		cred, err := pollDeviceCode(cfg, "device_auth_123", "USER-CODE")
		if err == nil {
			t.Fatal("Expected network error")
		}
		if cred != nil {
			t.Error("Expected nil credential on network error")
		}
	})
}

// TestExtractAccountIDExtended tests extractAccountID with more cases
func TestExtractAccountIDExtended(t *testing.T) {
	// Helper to create a JWT with specific claims
	makeJWT := func(claims map[string]interface{}) string {
		payload, _ := json.Marshal(claims)
		// Base64URL encode
		encoded := base64URLEncode(payload)
		return "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9." + encoded + ".signature"
	}

	tests := []struct {
		name   string
		claims map[string]interface{}
		want   string
	}{
		{
			name:   "chatgpt_account_id direct",
			claims: map[string]interface{}{"chatgpt_account_id": "acct_123"},
			want:   "acct_123",
		},
		{
			name:   "https://api.openai.com/auth.chatgpt_account_id",
			claims: map[string]interface{}{"https://api.openai.com/auth.chatgpt_account_id": "acct_456"},
			want:   "acct_456",
		},
		{
			name: "https://api.openai.com/auth nested",
			claims: map[string]interface{}{
				"https://api.openai.com/auth": map[string]interface{}{
					"chatgpt_account_id": "acct_789",
				},
			},
			want: "acct_789",
		},
		{
			name: "organizations array",
			claims: map[string]interface{}{
				"organizations": []interface{}{
					map[string]interface{}{"id": "org_123"},
				},
			},
			want: "org_123",
		},
		{
			name:   "no account id",
			claims: map[string]interface{}{"sub": "user_123"},
			want:   "",
		},
		{
			name:   "empty chatgpt_account_id",
			claims: map[string]interface{}{"chatgpt_account_id": ""},
			want:   "",
		},
		{
			name:   "chatgpt_account_id takes priority over organizations",
			claims: map[string]interface{}{"chatgpt_account_id": "acct_priority", "organizations": []interface{}{map[string]interface{}{"id": "org_fallback"}}},
			want:   "acct_priority",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			token := makeJWT(tt.claims)
			got := extractAccountID(token)
			if got != tt.want {
				t.Errorf("extractAccountID() = %q, want %q", got, tt.want)
			}
		})
	}
}

// base64URLEncode helper for creating test JWTs
func base64URLEncode(data []byte) string {
	encoded := base64URLEncodeRaw(data)
	return encoded
}

func base64URLEncodeRaw(data []byte) string {
	// Use standard base64 then convert to URL-safe
	s := base64URLEncodeStd(data)
	s = strings.ReplaceAll(s, "+", "-")
	s = strings.ReplaceAll(s, "/", "_")
	s = strings.TrimRight(s, "=")
	return s
}

func base64URLEncodeStd(data []byte) string {
	return fmt.Sprintf("%s", base64Encode(data))
}

func base64Encode(data []byte) string {
	// Simple base64 encoding
	const base64Table = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
	result := make([]byte, 0, (len(data)+2)/3*4)
	for i := 0; i < len(data); i += 3 {
		var n uint32
		remaining := len(data) - i
		if remaining >= 3 {
			n = uint32(data[i])<<16 | uint32(data[i+1])<<8 | uint32(data[i+2])
			result = append(result, base64Table[n>>18&0x3F], base64Table[n>>12&0x3F], base64Table[n>>6&0x3F], base64Table[n&0x3F])
		} else if remaining == 2 {
			n = uint32(data[i])<<16 | uint32(data[i+1])<<8
			result = append(result, base64Table[n>>18&0x3F], base64Table[n>>12&0x3F], base64Table[n>>6&0x3F], '=')
		} else {
			n = uint32(data[i]) << 16
			result = append(result, base64Table[n>>18&0x3F], base64Table[n>>12&0x3F], '=', '=')
		}
	}
	return string(result)
}

// TestOAuthProviderConfig tests OAuthProviderConfig defaults
func TestOAuthProviderConfigDefaults(t *testing.T) {
	cfg := OpenAIOAuthConfig()

	if cfg.Issuer == "" {
		t.Error("Issuer should not be empty")
	}
	if cfg.ClientID == "" {
		t.Error("ClientID should not be empty")
	}
	if cfg.Scopes == "" {
		t.Error("Scopes should not be empty")
	}
	if cfg.Port <= 0 {
		t.Error("Port should be positive")
	}
}

// TestPKCECodes tests PKCECodes validation
func TestPKCECodesValidation(t *testing.T) {
	pkce, err := GeneratePKCE()
	if err != nil {
		t.Fatalf("GeneratePKCE() error = %v", err)
	}

	if len(pkce.CodeVerifier) < 32 {
		t.Errorf("CodeVerifier should be at least 32 chars, got %d", len(pkce.CodeVerifier))
	}
	if len(pkce.CodeChallenge) < 32 {
		t.Errorf("CodeChallenge should be at least 32 chars, got %d", len(pkce.CodeChallenge))
	}
}

// TestAuthCredentialExpiry tests credential expiry edge cases
func TestAuthCredentialExpiry(t *testing.T) {
	t.Run("expires exactly now", func(t *testing.T) {
		cred := &AuthCredential{
			ExpiresAt: time.Now(),
		}
		// May or may not be expired depending on timing
		// Just ensure no panic
		_ = cred.IsExpired()
		_ = cred.NeedsRefresh()
	})

	t.Run("expires in exactly 5 minutes", func(t *testing.T) {
		cred := &AuthCredential{
			ExpiresAt: time.Now().Add(5 * time.Minute),
		}
		// Edge case: exactly at the boundary
		_ = cred.NeedsRefresh()
	})
}

// TestAuthStoreEdgeCases tests edge cases in auth store
func TestAuthStoreEdgeCases(t *testing.T) {
	origHome := os.Getenv("NEMESISBOT_HOME")
	tempDir := t.TempDir()
	os.Setenv("NEMESISBOT_HOME", tempDir)
	defer func() {
		if origHome != "" {
			os.Setenv("NEMESISBOT_HOME", origHome)
		} else {
			os.Unsetenv("NEMESISBOT_HOME")
		}
	}()

	t.Run("load store from empty directory", func(t *testing.T) {
		store, err := LoadStore()
		if err != nil {
			t.Fatalf("LoadStore() error = %v", err)
		}
		if store == nil {
			t.Fatal("LoadStore() should return non-nil store")
		}
		if store.Credentials == nil {
			t.Error("Credentials should be initialized")
		}
	})

	t.Run("load store from corrupted file", func(t *testing.T) {
		authPath := path.DefaultPathManager().AuthPath()

		// Write corrupted data
		corruptDir := filepath.Dir(authPath)
		os.MkdirAll(corruptDir, 0755)
		os.WriteFile(authPath, []byte("not json"), 0600)

		_, err := LoadStore()
		if err == nil {
			t.Error("Expected error for corrupted file")
		}
		// Cleanup
		os.Remove(authPath)
	})

	t.Run("save and load with multiple providers", func(t *testing.T) {

		store := &AuthStore{
			Credentials: map[string]*AuthCredential{
				"openai": {
					AccessToken:  "openai_token",
					RefreshToken: "openai_refresh",
					Provider:     "openai",
					AuthMethod:   "oauth",
					ExpiresAt:    time.Now().Add(1 * time.Hour),
				},
				"anthropic": {
					AccessToken: "anthropic_key",
					Provider:    "anthropic",
					AuthMethod:  "token",
				},
				"custom": {
					AccessToken: "custom_key",
					Provider:    "custom",
					AuthMethod:  "token",
				},
			},
		}

		err := SaveStore(store)
		if err != nil {
			t.Fatalf("SaveStore() error = %v", err)
		}

		loaded, err := LoadStore()
		if err != nil {
			t.Fatalf("LoadStore() error = %v", err)
		}

		if len(loaded.Credentials) != 3 {
			t.Errorf("Expected 3 credentials, got %d", len(loaded.Credentials))
		}

		for _, provider := range []string{"openai", "anthropic", "custom"} {
			cred, ok := loaded.Credentials[provider]
			if !ok {
				t.Errorf("Missing credential for %s", provider)
				continue
			}
			if cred.Provider != provider {
				t.Errorf("Provider = %s, want %s", cred.Provider, provider)
			}
		}
	})
}
