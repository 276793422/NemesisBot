// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package auth

import (
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

// TestAuthCredential_IsExpired_Zero tests that zero time is not expired.
func TestAuthCredential_IsExpired_Zero(t *testing.T) {
	cred := &AuthCredential{ExpiresAt: time.Time{}}
	if cred.IsExpired() {
		t.Error("expected zero ExpiresAt to not be expired")
	}
}

// TestAuthCredential_IsExpired_Future tests that future time is not expired.
func TestAuthCredential_IsExpired_Future(t *testing.T) {
	cred := &AuthCredential{ExpiresAt: time.Now().Add(1 * time.Hour)}
	if cred.IsExpired() {
		t.Error("expected future ExpiresAt to not be expired")
	}
}

// TestAuthCredential_IsExpired_Past tests that past time is expired.
func TestAuthCredential_IsExpired_Past(t *testing.T) {
	cred := &AuthCredential{ExpiresAt: time.Now().Add(-1 * time.Hour)}
	if !cred.IsExpired() {
		t.Error("expected past ExpiresAt to be expired")
	}
}

// TestAuthCredential_NeedsRefresh_Zero tests that zero time doesn't need refresh.
func TestAuthCredential_NeedsRefresh_Zero(t *testing.T) {
	cred := &AuthCredential{ExpiresAt: time.Time{}}
	if cred.NeedsRefresh() {
		t.Error("expected zero ExpiresAt to not need refresh")
	}
}

// TestAuthCredential_NeedsRefresh_Future tests that far future doesn't need refresh.
func TestAuthCredential_NeedsRefresh_Future(t *testing.T) {
	cred := &AuthCredential{ExpiresAt: time.Now().Add(1 * time.Hour)}
	if cred.NeedsRefresh() {
		t.Error("expected far future to not need refresh")
	}
}

// TestAuthCredential_NeedsRefresh_Soon tests that near-future needs refresh.
func TestAuthCredential_NeedsRefresh_Soon(t *testing.T) {
	cred := &AuthCredential{ExpiresAt: time.Now().Add(3 * time.Minute)}
	if !cred.NeedsRefresh() {
		t.Error("expected near-future to need refresh")
	}
}

// TestProviderDisplayName_All tests providerDisplayName for all cases.
func TestProviderDisplayName_All(t *testing.T) {
	tests := []struct {
		provider string
		expected string
	}{
		{"anthropic", "console.anthropic.com"},
		{"openai", "platform.openai.com"},
		{"custom", "custom"},
		{"", ""},
	}
	for _, tt := range tests {
		result := providerDisplayName(tt.provider)
		if result != tt.expected {
			t.Errorf("providerDisplayName(%q) = %q, want %q", tt.provider, result, tt.expected)
		}
	}
}

// TestLoginPasteToken_Success tests LoginPasteToken with valid input.
func TestLoginPasteToken_Success(t *testing.T) {
	input := "sk-test-token-12345\n"
	cred, err := LoginPasteToken("openai", &stringReader{data: input})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if cred.AccessToken != "sk-test-token-12345" {
		t.Errorf("expected token 'sk-test-token-12345', got %q", cred.AccessToken)
	}
	if cred.Provider != "openai" {
		t.Errorf("expected provider 'openai', got %q", cred.Provider)
	}
	if cred.AuthMethod != "token" {
		t.Errorf("expected auth method 'token', got %q", cred.AuthMethod)
	}
}

// TestLoginPasteToken_Empty tests LoginPasteToken with empty input.
func TestLoginPasteToken_Empty(t *testing.T) {
	_, err := LoginPasteToken("openai", &stringReader{data: "\n"})
	if err == nil {
		t.Fatal("expected error for empty token")
	}
}

// TestLoginPasteToken_NoInput tests LoginPasteToken with no input.
func TestLoginPasteToken_NoInput(t *testing.T) {
	_, err := LoginPasteToken("openai", &stringReader{data: ""})
	if err == nil {
		t.Fatal("expected error for no input")
	}
}

// TestLoginPasteToken_WhitespaceOnly tests trimming whitespace.
func TestLoginPasteToken_WhitespaceOnly(t *testing.T) {
	_, err := LoginPasteToken("openai", &stringReader{data: "   \n"})
	if err == nil {
		t.Fatal("expected error for whitespace-only token")
	}
}

// stringReader is a simple io.Reader for testing.
type stringReader struct {
	data string
	pos  int
}

func (r *stringReader) Read(p []byte) (n int, err error) {
	if r.pos >= len(r.data) {
		return 0, err
	}
	n = copy(p, r.data[r.pos:])
	r.pos += n
	return n, nil
}

// TestLoginDeviceCode_MockServer tests the device code flow with a mock server.
func TestLoginDeviceCode_MockServer(t *testing.T) {
	callCount := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/accounts/deviceauth/usercode" {
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(map[string]interface{}{
				"device_auth_id": "device-123",
				"user_code":      "ABCD-1234",
				"interval":       "1",
			})
			return
		}

		if r.URL.Path == "/api/accounts/deviceauth/token" {
			callCount++
			if callCount < 2 {
				// First call: pending
				w.WriteHeader(http.StatusUnauthorized)
				w.Write([]byte(`{"error": "authorization_pending"}`))
				return
			}
			// Second call: success with authorization code
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(map[string]interface{}{
				"authorization_code": "auth-code-123",
				"code_challenge":     "challenge",
				"code_verifier":      "verifier",
			})
			return
		}

		if r.URL.Path == "/oauth/token" {
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(map[string]interface{}{
				"access_token":  "access-token-123",
				"refresh_token": "refresh-token-123",
				"expires_in":    3600,
			})
			return
		}
	}))
	defer server.Close()

	cfg := OAuthProviderConfig{
		Issuer:   server.URL,
		ClientID: "test-client",
		Port:     9999,
	}

	cred, err := LoginDeviceCode(cfg)
	if err != nil {
		t.Fatalf("LoginDeviceCode failed: %v", err)
	}
	if cred == nil {
		t.Fatal("expected non-nil credential")
	}
	if cred.AccessToken != "access-token-123" {
		t.Errorf("expected access token 'access-token-123', got %q", cred.AccessToken)
	}
	if cred.RefreshToken != "refresh-token-123" {
		t.Errorf("expected refresh token 'refresh-token-123', got %q", cred.RefreshToken)
	}
}

// TestLoginDeviceCode_ServerError tests device code with initial server error.
func TestLoginDeviceCode_ServerError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
		w.Write([]byte(`internal server error`))
	}))
	defer server.Close()

	cfg := OAuthProviderConfig{
		Issuer:   server.URL,
		ClientID: "test-client",
		Port:     9999,
	}

	// This will timeout since the server always returns errors
	// Use a context with timeout to avoid long test
	_, err := LoginDeviceCode(cfg)
	if err == nil {
		t.Error("expected error when device code server fails")
	}
}

// TestPollDeviceCode_NonSuccess tests pollDeviceCode with non-200 status.
func TestPollDeviceCode_NonSuccess(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
	}))
	defer server.Close()

	cfg := OAuthProviderConfig{
		Issuer:   server.URL,
		ClientID: "test-client",
	}

	cred, err := pollDeviceCode(cfg, "device-id", "user-code")
	if err == nil {
		t.Error("expected error for non-200 status")
	}
	if cred != nil {
		t.Error("expected nil credential on error")
	}
}

// TestRefreshAccessToken_MockServer tests token refresh with mock server.
func TestRefreshAccessToken_MockServer(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"access_token":  "new-access-token",
			"refresh_token": "new-refresh-token",
			"expires_in":    7200,
		})
	}))
	defer server.Close()

	cfg := OAuthProviderConfig{
		Issuer:   server.URL,
		ClientID: "test-client",
	}

	cred := &AuthCredential{
		AccessToken:  "old-access",
		RefreshToken: "old-refresh",
		Provider:     "test",
		ExpiresAt:    time.Now().Add(-1 * time.Hour),
	}

	refreshed, err := RefreshAccessToken(cred, cfg)
	if err != nil {
		t.Fatalf("RefreshAccessToken failed: %v", err)
	}
	if refreshed.AccessToken != "new-access-token" {
		t.Errorf("expected 'new-access-token', got %q", refreshed.AccessToken)
	}
	if refreshed.RefreshToken != "new-refresh-token" {
		t.Errorf("expected 'new-refresh-token', got %q", refreshed.RefreshToken)
	}
}

// TestRefreshAccessToken_ServerError tests refresh with server error.
func TestRefreshAccessToken_ServerError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusBadRequest)
		w.Write([]byte(`bad request`))
	}))
	defer server.Close()

	cfg := OAuthProviderConfig{
		Issuer:   server.URL,
		ClientID: "test-client",
	}

	cred := &AuthCredential{
		AccessToken:  "old-access",
		RefreshToken: "old-refresh",
		Provider:     "test",
	}

	_, err := RefreshAccessToken(cred, cfg)
	if err == nil {
		t.Error("expected error for server error response")
	}
}

// TestLoginBrowser_MockCallback tests LoginBrowser with a simulated browser callback.
func TestLoginBrowser_MockCallback(t *testing.T) {
	tokenServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/oauth/token" {
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(map[string]interface{}{
				"access_token":  "browser-access-token",
				"refresh_token": "browser-refresh-token",
				"expires_in":    3600,
			})
			return
		}
		w.WriteHeader(http.StatusNotFound)
	}))
	defer tokenServer.Close()

	// Find a free port for the callback server
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("failed to find free port: %v", err)
	}
	port := listener.Addr().(*net.TCPAddr).Port
	listener.Close()

	cfg := OAuthProviderConfig{
		Issuer:   tokenServer.URL,
		ClientID: "test-client",
		Scopes:   "openid profile",
		Port:     port,
	}

	// Start LoginBrowser in a goroutine
	resultCh := make(chan struct {
		cred *AuthCredential
		err  error
	}, 1)

	go func() {
		cred, err := LoginBrowser(cfg)
		resultCh <- struct {
			cred *AuthCredential
			err  error
		}{cred, err}
	}()

	// Wait a bit for the server to start
	time.Sleep(500 * time.Millisecond)

	// Now we need to make a callback request
	// We don't know the state, but we can try to get it from the auth URL
	// Actually, we can't know the state since it's generated internally
	// Instead, let's send a callback with code but wrong state first
	resp, err := http.Get(fmt.Sprintf("http://127.0.0.1:%d/auth/callback?state=wrong&code=test-code", port))
	if err != nil {
		t.Fatalf("failed to make callback request: %v", err)
	}
	resp.Body.Close()

	// Wait for result - should be state mismatch error
	select {
	case result := <-resultCh:
		if result.err == nil {
			t.Error("expected error for state mismatch")
		}
	case <-time.After(5 * time.Second):
		t.Fatal("timed out waiting for LoginBrowser result")
	}
}

// TestLoginBrowser_PortInUse tests LoginBrowser when port is already in use.
func TestLoginBrowser_PortInUse(t *testing.T) {
	tokenServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {}))
	defer tokenServer.Close()

	// Occupy the port
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("failed to find free port: %v", err)
	}
	port := listener.Addr().(*net.TCPAddr).Port
	defer listener.Close()

	cfg := OAuthProviderConfig{
		Issuer:   tokenServer.URL,
		ClientID: "test-client",
		Port:     port, // Port already in use
	}

	_, err = LoginBrowser(cfg)
	if err == nil {
		t.Error("expected error when port is in use")
	}
}

// TestParseTokenResponse_NoAccessToken tests parseTokenResponse without access token.
func TestParseTokenResponse_NoAccessToken(t *testing.T) {
	body := []byte(`{"refresh_token": "refresh", "expires_in": 3600}`)
	_, err := parseTokenResponse(body, "test")
	if err == nil {
		t.Error("expected error for missing access token")
	}
}

// TestParseTokenResponse_WithExpiresIn tests parseTokenResponse with expires_in.
func TestParseTokenResponse_WithExpiresIn(t *testing.T) {
	body := []byte(`{"access_token": "token", "expires_in": 3600}`)
	cred, err := parseTokenResponse(body, "test")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if cred.AccessToken != "token" {
		t.Errorf("expected 'token', got %q", cred.AccessToken)
	}
	if cred.ExpiresAt.IsZero() {
		t.Error("expected non-zero ExpiresAt")
	}
	if cred.AuthMethod != "oauth" {
		t.Errorf("expected 'oauth', got %q", cred.AuthMethod)
	}
}

// TestParseTokenResponse_InvalidJSON tests parseTokenResponse with invalid JSON.
func TestParseTokenResponse_InvalidJSON(t *testing.T) {
	_, err := parseTokenResponse([]byte(`not json`), "test")
	if err == nil {
		t.Error("expected error for invalid JSON")
	}
}
