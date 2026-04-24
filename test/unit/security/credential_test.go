// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package security_test

import (
	"context"
	"strings"
	"testing"

	credential "github.com/276793422/NemesisBot/module/security/credential"
)

func TestCredentialScanner_AWSKey(t *testing.T) {
	scanner, err := credential.NewScanner(credential.DefaultConfig())
	if err != nil {
		t.Fatalf("NewScanner returned error: %v", err)
	}
	ctx := context.Background()

	tests := []struct {
		name    string
		content string
		wantOK  bool
	}{
		{"AWS Access Key ID", "AWS_ACCESS_KEY=AKIAIOSFODNN7EXAMPLE", true},
		{"AWS Secret Key", "aws_secret_access_key=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := scanner.ScanContent(ctx, tt.content)
			if err != nil {
				t.Fatalf("ScanContent returned error: %v", err)
			}
			if tt.wantOK && !result.HasMatches {
				t.Errorf("expected matches for %q, got none", tt.name)
			}
			if tt.wantOK {
				found := false
				for _, m := range result.Matches {
					if strings.Contains(m.Type, "aws") {
						found = true
						if m.MaskedValue == "" {
							t.Error("expected non-empty masked value")
						}
					}
				}
				if !found {
					t.Errorf("expected AWS credential match, got: %+v", result.Matches)
				}
			}
		})
	}
}

func TestCredentialScanner_GitHubToken(t *testing.T) {
	scanner, err := credential.NewScanner(credential.DefaultConfig())
	if err != nil {
		t.Fatalf("NewScanner returned error: %v", err)
	}
	ctx := context.Background()

	tests := []struct {
		name    string
		content string
	}{
		{"ghp_ token", "token=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij"},
		{"gho_ token", "token=gho_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := scanner.ScanContent(ctx, tt.content)
			if err != nil {
				t.Fatalf("ScanContent returned error: %v", err)
			}
			if !result.HasMatches {
				t.Fatalf("expected matches for GitHub token, got none")
			}
			found := false
			for _, m := range result.Matches {
				if m.Type == "github_token" {
					found = true
				}
			}
			if !found {
				t.Errorf("expected github_token match, got: %+v", result.Matches)
			}
		})
	}
}

func TestCredentialScanner_JWTToken(t *testing.T) {
	scanner, err := credential.NewScanner(credential.DefaultConfig())
	if err != nil {
		t.Fatalf("NewScanner returned error: %v", err)
	}
	ctx := context.Background()

	// Construct a JWT-like token
	content := "Authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
	result, err := scanner.ScanContent(ctx, content)
	if err != nil {
		t.Fatalf("ScanContent returned error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("expected JWT token match")
	}

	found := false
	for _, m := range result.Matches {
		if m.Type == "jwt_token" {
			found = true
			if m.Severity != "high" {
				t.Errorf("expected severity 'high' for JWT, got %q", m.Severity)
			}
		}
	}
	if !found {
		t.Errorf("expected jwt_token match, got: %+v", result.Matches)
	}
}

func TestCredentialScanner_PrivateKey(t *testing.T) {
	scanner, err := credential.NewScanner(credential.DefaultConfig())
	if err != nil {
		t.Fatalf("NewScanner returned error: %v", err)
	}
	ctx := context.Background()

	tests := []struct {
		name    string
		content string
		wantOK  bool
	}{
		{"RSA Private Key", "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA...", true},
		{"EC Private Key", "-----BEGIN EC PRIVATE KEY-----\nMHQCAQEEIAGb...", true},
		{"DSA Private Key", "-----BEGIN DSA PRIVATE KEY-----\nMIIBuwIBAAJBALJ...", true},
		{"OpenSSH Private Key", "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk...", true},
		{"Generic Private Key", "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkq...", true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := scanner.ScanContent(ctx, tt.content)
			if err != nil {
				t.Fatalf("ScanContent returned error: %v", err)
			}
			if tt.wantOK && !result.HasMatches {
				t.Errorf("expected private key match for %q", tt.name)
			}
		})
	}
}

func TestCredentialScanner_StripeKey(t *testing.T) {
	scanner, err := credential.NewScanner(credential.DefaultConfig())
	if err != nil {
		t.Fatalf("NewScanner returned error: %v", err)
	}
	ctx := context.Background()

	tests := []struct {
		name    string
		content string
	}{
		{"sk_live key", "key=sk_live_abcdefghijklmnopqrstuvwxyz1234"},
		{"sk_test key", "key=sk_test_abcdefghijklmnopqrstuvwxyz1234"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := scanner.ScanContent(ctx, tt.content)
			if err != nil {
				t.Fatalf("ScanContent returned error: %v", err)
			}
			if !result.HasMatches {
				t.Fatalf("expected Stripe key match for %q", tt.name)
			}
		})
	}
}

func TestCredentialScanner_MultipleCredentials(t *testing.T) {
	scanner, err := credential.NewScanner(credential.DefaultConfig())
	if err != nil {
		t.Fatalf("NewScanner returned error: %v", err)
	}
	ctx := context.Background()

	content := `AWS key: AKIAIOSFODNN7EXAMPLE
GitHub token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij
Stripe key: sk_live_abcdefghijklmnopqrstuvwxyz1234`
	result, err := scanner.ScanContent(ctx, content)
	if err != nil {
		t.Fatalf("ScanContent returned error: %v", err)
	}

	if !result.HasMatches {
		t.Fatal("expected matches for content with multiple credentials")
	}

	if len(result.Matches) < 2 {
		t.Errorf("expected at least 2 matches, got %d", len(result.Matches))
	}

	// Should have different types
	types := make(map[string]bool)
	for _, m := range result.Matches {
		types[m.Type] = true
	}
	if len(types) < 2 {
		t.Errorf("expected at least 2 different credential types, got %d", len(types))
	}
}

func TestCredentialScanner_CleanContent(t *testing.T) {
	scanner, err := credential.NewScanner(credential.DefaultConfig())
	if err != nil {
		t.Fatalf("NewScanner returned error: %v", err)
	}
	ctx := context.Background()

	content := "This is a normal log message without any credentials or secrets."
	result, err := scanner.ScanContent(ctx, content)
	if err != nil {
		t.Fatalf("ScanContent returned error: %v", err)
	}

	if result.HasMatches {
		t.Errorf("expected no matches for clean content, got %d", len(result.Matches))
	}
}

func TestCredentialScanner_RedactContent(t *testing.T) {
	scanner, err := credential.NewScanner(credential.DefaultConfig())
	if err != nil {
		t.Fatalf("NewScanner returned error: %v", err)
	}
	ctx := context.Background()

	content := "AWS key: AKIAIOSFODNN7EXAMPLE and GitHub token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij"
	redacted, err := scanner.RedactContent(ctx, content)
	if err != nil {
		t.Fatalf("RedactContent returned error: %v", err)
	}

	// Verify the original sensitive values are not present in the redacted output
	if strings.Contains(redacted, "AKIAIOSFODNN7EXAMPLE") {
		t.Error("expected AWS key to be redacted")
	}
	if strings.Contains(redacted, "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij") {
		t.Error("expected GitHub token to be redacted")
	}
}

func TestCredentialScanner_Disabled(t *testing.T) {
	cfg := &credential.Config{Enabled: false}
	scanner, err := credential.NewScanner(cfg)
	if err != nil {
		t.Fatalf("NewScanner returned error: %v", err)
	}
	ctx := context.Background()

	result, err := scanner.ScanContent(ctx, "AKIAIOSFODNN7EXAMPLE")
	if err != nil {
		t.Fatalf("ScanContent returned error: %v", err)
	}
	if result.HasMatches {
		t.Error("expected no matches when scanner is disabled")
	}
}

func TestCredentialScanner_EnabledTypes(t *testing.T) {
	// Only enable AWS-related types
	cfg := &credential.Config{
		Enabled:      true,
		EnabledTypes: []string{"aws_access_key_id"},
		Action:       "block",
	}
	scanner, err := credential.NewScanner(cfg)
	if err != nil {
		t.Fatalf("NewScanner returned error: %v", err)
	}
	ctx := context.Background()

	content := "AWS key: AKIAIOSFODNN7EXAMPLE and GitHub: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij"
	result, err := scanner.ScanContent(ctx, content)
	if err != nil {
		t.Fatalf("ScanContent returned error: %v", err)
	}

	// Should only match AWS, not GitHub
	for _, m := range result.Matches {
		if m.Type != "aws_access_key_id" {
			t.Errorf("expected only aws_access_key_id matches, got %q", m.Type)
		}
	}
}

func TestCredentialScanner_ToolOutputScanning(t *testing.T) {
	scanner, err := credential.NewScanner(credential.DefaultConfig())
	if err != nil {
		t.Fatalf("NewScanner returned error: %v", err)
	}
	ctx := context.Background()

	result, err := scanner.ScanToolOutput(ctx, "file_read", "config contains password=mysecretpassword123")
	if err != nil {
		t.Fatalf("ScanToolOutput returned error: %v", err)
	}
	// Just verify it doesn't crash and returns a result
	_ = result
}

func TestCredentialScanner_SetAction(t *testing.T) {
	scanner, err := credential.NewScanner(credential.DefaultConfig())
	if err != nil {
		t.Fatalf("NewScanner returned error: %v", err)
	}

	// Valid actions
	for _, action := range []string{"block", "redact", "warn"} {
		err := scanner.SetAction(action)
		if err != nil {
			t.Errorf("SetAction(%q) returned error: %v", action, err)
		}
	}

	// Invalid action
	err = scanner.SetAction("invalid")
	if err == nil {
		t.Error("expected error for invalid action")
	}
}

func TestCredentialScanner_NilConfig(t *testing.T) {
	scanner, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner(nil) returned error: %v", err)
	}
	if !scanner.IsEnabled() {
		t.Error("expected scanner to be enabled with default config")
	}
}
