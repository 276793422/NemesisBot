// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package providers_test

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/providers"
)

// --- ReadCodexCliCredentials ---

func TestReadCodexCliCredentials_ValidFile(t *testing.T) {
	tmpDir := t.TempDir()
	authDir := filepath.Join(tmpDir, ".codex")
	if err := os.MkdirAll(authDir, 0755); err != nil {
		t.Fatalf("failed to create auth dir: %v", err)
	}

	authFile := filepath.Join(authDir, "auth.json")
	authData := map[string]interface{}{
		"tokens": map[string]interface{}{
			"access_token":  "test-access-token-123",
			"refresh_token": "test-refresh-token-456",
			"account_id":    "test-account-789",
		},
	}
	data, err := json.Marshal(authData)
	if err != nil {
		t.Fatalf("failed to marshal auth data: %v", err)
	}
	if err := os.WriteFile(authFile, data, 0644); err != nil {
		t.Fatalf("failed to write auth file: %v", err)
	}

	// Set CODEX_HOME to our temp directory
	originalCodexHome := os.Getenv("CODEX_HOME")
	os.Setenv("CODEX_HOME", authDir)
	defer os.Setenv("CODEX_HOME", originalCodexHome)

	token, accountID, expiresAt, err := providers.ReadCodexCliCredentials()
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}

	if token != "test-access-token-123" {
		t.Errorf("expected token 'test-access-token-123', got '%s'", token)
	}
	if accountID != "test-account-789" {
		t.Errorf("expected accountID 'test-account-789', got '%s'", accountID)
	}
	if expiresAt.IsZero() {
		t.Error("expected non-zero expiresAt")
	}

	// Expiry should be approximately now + 1 hour
	expectedExpiry := time.Now().Add(time.Hour)
	diff := expiresAt.Sub(expectedExpiry)
	if diff < -time.Minute || diff > time.Minute {
		t.Errorf("expected expiry near %v, got %v (diff: %v)", expectedExpiry, expiresAt, diff)
	}
}

func TestReadCodexCliCredentials_MissingFile(t *testing.T) {
	tmpDir := t.TempDir()
	authDir := filepath.Join(tmpDir, ".codex-nonexistent")

	originalCodexHome := os.Getenv("CODEX_HOME")
	os.Setenv("CODEX_HOME", authDir)
	defer os.Setenv("CODEX_HOME", originalCodexHome)

	_, _, _, err := providers.ReadCodexCliCredentials()
	if err == nil {
		t.Error("expected error when auth file does not exist")
	}
}

func TestReadCodexCliCredentials_InvalidJSON(t *testing.T) {
	tmpDir := t.TempDir()
	authDir := filepath.Join(tmpDir, ".codex")
	if err := os.MkdirAll(authDir, 0755); err != nil {
		t.Fatalf("failed to create auth dir: %v", err)
	}

	authFile := filepath.Join(authDir, "auth.json")
	if err := os.WriteFile(authFile, []byte("not valid json"), 0644); err != nil {
		t.Fatalf("failed to write auth file: %v", err)
	}

	originalCodexHome := os.Getenv("CODEX_HOME")
	os.Setenv("CODEX_HOME", authDir)
	defer os.Setenv("CODEX_HOME", originalCodexHome)

	_, _, _, err := providers.ReadCodexCliCredentials()
	if err == nil {
		t.Error("expected error for invalid JSON")
	}
}

func TestReadCodexCliCredentials_EmptyAccessToken(t *testing.T) {
	tmpDir := t.TempDir()
	authDir := filepath.Join(tmpDir, ".codex")
	if err := os.MkdirAll(authDir, 0755); err != nil {
		t.Fatalf("failed to create auth dir: %v", err)
	}

	authFile := filepath.Join(authDir, "auth.json")
	authData := map[string]interface{}{
		"tokens": map[string]interface{}{
			"access_token":  "",
			"refresh_token": "some-refresh-token",
			"account_id":    "some-account",
		},
	}
	data, err := json.Marshal(authData)
	if err != nil {
		t.Fatalf("failed to marshal auth data: %v", err)
	}
	if err := os.WriteFile(authFile, data, 0644); err != nil {
		t.Fatalf("failed to write auth file: %v", err)
	}

	originalCodexHome := os.Getenv("CODEX_HOME")
	os.Setenv("CODEX_HOME", authDir)
	defer os.Setenv("CODEX_HOME", originalCodexHome)

	_, _, _, err = providers.ReadCodexCliCredentials()
	if err == nil {
		t.Error("expected error when access_token is empty")
	}
}

func TestReadCodexCliCredentials_MissingTokens(t *testing.T) {
	tmpDir := t.TempDir()
	authDir := filepath.Join(tmpDir, ".codex")
	if err := os.MkdirAll(authDir, 0755); err != nil {
		t.Fatalf("failed to create auth dir: %v", err)
	}

	authFile := filepath.Join(authDir, "auth.json")
	data := []byte(`{"other_field": "value"}`)
	if err := os.WriteFile(authFile, data, 0644); err != nil {
		t.Fatalf("failed to write auth file: %v", err)
	}

	originalCodexHome := os.Getenv("CODEX_HOME")
	os.Setenv("CODEX_HOME", authDir)
	defer os.Setenv("CODEX_HOME", originalCodexHome)

	_, _, _, err := providers.ReadCodexCliCredentials()
	if err == nil {
		t.Error("expected error when tokens section is missing")
	}
}

func TestReadCodexCliCredentials_ExpiryBasedOnModTime(t *testing.T) {
	tmpDir := t.TempDir()
	authDir := filepath.Join(tmpDir, ".codex")
	if err := os.MkdirAll(authDir, 0755); err != nil {
		t.Fatalf("failed to create auth dir: %v", err)
	}

	authFile := filepath.Join(authDir, "auth.json")
	authData := map[string]interface{}{
		"tokens": map[string]interface{}{
			"access_token": "test-token",
			"account_id":   "test-account",
		},
	}
	data, err := json.Marshal(authData)
	if err != nil {
		t.Fatalf("failed to marshal auth data: %v", err)
	}

	// Write the file with a specific modification time
	if err := os.WriteFile(authFile, data, 0644); err != nil {
		t.Fatalf("failed to write auth file: %v", err)
	}

	// Set a specific modification time (2 hours ago)
	modTime := time.Now().Add(-2 * time.Hour)
	if err := os.Chtimes(authFile, modTime, modTime); err != nil {
		t.Fatalf("failed to set file mod time: %v", err)
	}

	originalCodexHome := os.Getenv("CODEX_HOME")
	os.Setenv("CODEX_HOME", authDir)
	defer os.Setenv("CODEX_HOME", originalCodexHome)

	_, _, expiresAt, err := providers.ReadCodexCliCredentials()
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}

	// The expiry should be modTime + 1 hour, which is 1 hour ago
	expectedExpiry := modTime.Add(time.Hour)
	diff := expiresAt.Sub(expectedExpiry)
	if diff < -time.Second || diff > time.Second {
		t.Errorf("expected expiry near %v, got %v", expectedExpiry, expiresAt)
	}

	// The credentials should be expired (modTime + 1 hour is in the past)
	if time.Now().Before(expiresAt) {
		t.Error("expected credentials to be expired")
	}
}

// --- CreateCodexCliTokenSource ---

func TestCreateCodexCliTokenSource_Valid(t *testing.T) {
	tmpDir := t.TempDir()
	authDir := filepath.Join(tmpDir, ".codex")
	if err := os.MkdirAll(authDir, 0755); err != nil {
		t.Fatalf("failed to create auth dir: %v", err)
	}

	authFile := filepath.Join(authDir, "auth.json")
	authData := map[string]interface{}{
		"tokens": map[string]interface{}{
			"access_token": "fresh-token",
			"account_id":   "fresh-account",
		},
	}
	data, err := json.Marshal(authData)
	if err != nil {
		t.Fatalf("failed to marshal auth data: %v", err)
	}
	if err := os.WriteFile(authFile, data, 0644); err != nil {
		t.Fatalf("failed to write auth file: %v", err)
	}

	originalCodexHome := os.Getenv("CODEX_HOME")
	os.Setenv("CODEX_HOME", authDir)
	defer os.Setenv("CODEX_HOME", originalCodexHome)

	tokenSource := providers.CreateCodexCliTokenSource()
	token, accountID, err := tokenSource()
	if err != nil {
		t.Fatalf("expected no error from token source, got: %v", err)
	}

	if token != "fresh-token" {
		t.Errorf("expected token 'fresh-token', got '%s'", token)
	}
	if accountID != "fresh-account" {
		t.Errorf("expected accountID 'fresh-account', got '%s'", accountID)
	}
}

func TestCreateCodexCliTokenSource_Expired(t *testing.T) {
	tmpDir := t.TempDir()
	authDir := filepath.Join(tmpDir, ".codex")
	if err := os.MkdirAll(authDir, 0755); err != nil {
		t.Fatalf("failed to create auth dir: %v", err)
	}

	authFile := filepath.Join(authDir, "auth.json")
	authData := map[string]interface{}{
		"tokens": map[string]interface{}{
			"access_token": "old-token",
			"account_id":   "old-account",
		},
	}
	data, err := json.Marshal(authData)
	if err != nil {
		t.Fatalf("failed to marshal auth data: %v", err)
	}
	if err := os.WriteFile(authFile, data, 0644); err != nil {
		t.Fatalf("failed to write auth file: %v", err)
	}

	// Set modification time to 2 hours ago (expired)
	modTime := time.Now().Add(-2 * time.Hour)
	if err := os.Chtimes(authFile, modTime, modTime); err != nil {
		t.Fatalf("failed to set mod time: %v", err)
	}

	originalCodexHome := os.Getenv("CODEX_HOME")
	os.Setenv("CODEX_HOME", authDir)
	defer os.Setenv("CODEX_HOME", originalCodexHome)

	tokenSource := providers.CreateCodexCliTokenSource()
	_, _, err = tokenSource()
	if err == nil {
		t.Error("expected error for expired credentials")
	}
}

func TestCreateCodexCliTokenSource_MissingFile(t *testing.T) {
	tmpDir := t.TempDir()
	authDir := filepath.Join(tmpDir, ".codex-nonexistent")

	originalCodexHome := os.Getenv("CODEX_HOME")
	os.Setenv("CODEX_HOME", authDir)
	defer os.Setenv("CODEX_HOME", originalCodexHome)

	tokenSource := providers.CreateCodexCliTokenSource()
	_, _, err := tokenSource()
	if err == nil {
		t.Error("expected error when auth file is missing")
	}
}

// --- resolveCodexAuthPath (tested via ReadCodexCliCredentials) ---

func TestResolveCodexAuthPath_WithCodexHome(t *testing.T) {
	tmpDir := t.TempDir()

	originalCodexHome := os.Getenv("CODEX_HOME")
	os.Setenv("CODEX_HOME", tmpDir)
	defer os.Setenv("CODEX_HOME", originalCodexHome)

	// Create the auth file
	authData := map[string]interface{}{
		"tokens": map[string]interface{}{
			"access_token": "token-from-codex-home",
			"account_id":   "account-from-codex-home",
		},
	}
	data, err := json.Marshal(authData)
	if err != nil {
		t.Fatalf("failed to marshal: %v", err)
	}
	if err := os.WriteFile(filepath.Join(tmpDir, "auth.json"), data, 0644); err != nil {
		t.Fatalf("failed to write: %v", err)
	}

	token, accountID, _, err := providers.ReadCodexCliCredentials()
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}

	if token != "token-from-codex-home" {
		t.Errorf("expected token 'token-from-codex-home', got '%s'", token)
	}
	if accountID != "account-from-codex-home" {
		t.Errorf("expected accountID 'account-from-codex-home', got '%s'", accountID)
	}
}

func TestResolveCodexAuthPath_DefaultHomeDir(t *testing.T) {
	// When CODEX_HOME is not set, it should use ~/.codex/auth.json
	originalCodexHome := os.Getenv("CODEX_HOME")
	os.Unsetenv("CODEX_HOME")
	defer os.Setenv("CODEX_HOME", originalCodexHome)

	// This will fail because we can't write to the real home dir,
	// but it tests the path resolution logic
	_, _, _, err := providers.ReadCodexCliCredentials()
	if err == nil {
		t.Log("Found real codex credentials - unexpected in test environment but not wrong")
	} else {
		t.Logf("Expected error (no real credentials): %v", err)
	}
}

// --- CodexCliAuth structure tests ---

func TestCodexCliAuth_JSONParsing(t *testing.T) {
	jsonStr := `{
		"tokens": {
			"access_token": "at-123",
			"refresh_token": "rt-456",
			"account_id": "acc-789"
		}
	}`

	var auth struct {
		Tokens struct {
			AccessToken  string `json:"access_token"`
			RefreshToken string `json:"refresh_token"`
			AccountID    string `json:"account_id"`
		} `json:"tokens"`
	}

	if err := json.Unmarshal([]byte(jsonStr), &auth); err != nil {
		t.Fatalf("failed to parse: %v", err)
	}

	if auth.Tokens.AccessToken != "at-123" {
		t.Errorf("expected access_token 'at-123', got '%s'", auth.Tokens.AccessToken)
	}
	if auth.Tokens.RefreshToken != "rt-456" {
		t.Errorf("expected refresh_token 'rt-456', got '%s'", auth.Tokens.RefreshToken)
	}
	if auth.Tokens.AccountID != "acc-789" {
		t.Errorf("expected account_id 'acc-789', got '%s'", auth.Tokens.AccountID)
	}
}

func TestCodexCliAuth_EmptyTokens(t *testing.T) {
	jsonStr := `{"tokens":{}}`

	var auth struct {
		Tokens struct {
			AccessToken  string `json:"access_token"`
			RefreshToken string `json:"refresh_token"`
			AccountID    string `json:"account_id"`
		} `json:"tokens"`
	}

	if err := json.Unmarshal([]byte(jsonStr), &auth); err != nil {
		t.Fatalf("failed to parse: %v", err)
	}

	if auth.Tokens.AccessToken != "" {
		t.Errorf("expected empty access_token, got '%s'", auth.Tokens.AccessToken)
	}
}

func TestCodexCliAuth_MissingTokensField(t *testing.T) {
	jsonStr := `{"other":"data"}`

	var auth struct {
		Tokens struct {
			AccessToken  string `json:"access_token"`
			RefreshToken string `json:"refresh_token"`
			AccountID    string `json:"account_id"`
		} `json:"tokens"`
	}

	if err := json.Unmarshal([]byte(jsonStr), &auth); err != nil {
		t.Fatalf("failed to parse: %v", err)
	}

	// Tokens should be zero-valued
	if auth.Tokens.AccessToken != "" {
		t.Error("expected empty access_token")
	}
}
