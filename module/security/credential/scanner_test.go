// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package credential_test

import (
	"context"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/security/credential"
)

// ---------------------------------------------------------------------------
// NewScanner
// ---------------------------------------------------------------------------

func TestNewScanner_NilConfig(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner with nil config should succeed: %v", err)
	}
	if s == nil {
		t.Fatal("scanner should not be nil")
	}
}

func TestNewScanner_DefaultConfig(t *testing.T) {
	cfg := credential.DefaultConfig()
	s, err := credential.NewScanner(cfg)
	if err != nil {
		t.Fatalf("NewScanner with default config should succeed: %v", err)
	}
	if s == nil {
		t.Fatal("scanner should not be nil")
	}
}

func TestNewScanner_WithEnabledTypes(t *testing.T) {
	cfg := &credential.Config{
		Enabled:      true,
		EnabledTypes: []string{"aws_access_key_id", "github_token"},
		Action:       "block",
	}
	s, err := credential.NewScanner(cfg)
	if err != nil {
		t.Fatalf("NewScanner with enabled types should succeed: %v", err)
	}

	// AWS key should be detected
	result, err := s.ScanContent(context.Background(), "AKIAIOSFODNN7EXAMPLE")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Error("AWS access key should be detected")
	}

	// Private key should NOT be detected (not in enabled types)
	result, err = s.ScanContent(context.Background(), "-----BEGIN RSA PRIVATE KEY-----")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if result.HasMatches {
		t.Error("RSA private key should not be detected when type is not enabled")
	}
}

func TestNewScanner_Disabled(t *testing.T) {
	cfg := &credential.Config{
		Enabled: false,
		Action:  "warn",
	}
	s, err := credential.NewScanner(cfg)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	result, err := s.ScanContent(context.Background(), "AKIAIOSFODNN7EXAMPLE")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if result.HasMatches {
		t.Error("disabled scanner should not detect anything")
	}
}

// ---------------------------------------------------------------------------
// DefaultConfig
// ---------------------------------------------------------------------------

func TestDefaultConfig(t *testing.T) {
	cfg := credential.DefaultConfig()
	if !cfg.Enabled {
		t.Error("DefaultConfig should be enabled")
	}
	if cfg.Action != "redact" {
		t.Errorf("DefaultConfig Action = %q, want redact", cfg.Action)
	}
	if len(cfg.EnabledTypes) != 0 {
		t.Error("DefaultConfig EnabledTypes should be empty (all types)")
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - AWS credentials
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_AWSAccessKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	result, err := s.ScanContent(context.Background(), "export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect AWS access key")
	}
	found := false
	for _, m := range result.Matches {
		if m.Type == "aws_access_key_id" {
			found = true
			if m.Severity != "critical" {
				t.Errorf("aws_access_key_id severity = %q, want critical", m.Severity)
			}
		}
	}
	if !found {
		t.Error("aws_access_key_id match not found in results")
	}
}

func TestScanner_ScanContent_AWSSecretKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect AWS secret key")
	}
}

func TestScanner_ScanContent_AWSSessionToken(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "aws_session_token = FwoGZXIvYXdzEBYaDHJMRVxW"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect AWS session token")
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - GitHub tokens
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_GitHubToken(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz012345678"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect GitHub token")
	}
	found := false
	for _, m := range result.Matches {
		if m.Type == "github_token" {
			found = true
			if m.Severity != "critical" {
				t.Errorf("github_token severity = %q, want critical", m.Severity)
			}
		}
	}
	if !found {
		t.Error("github_token match not found")
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - Google credentials
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_GoogleAPIKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "AIzaSyDaGmWKa4VuX0hR8zQ6LRbR4VzwsmKgH00"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Google API key")
	}
}

func TestScanner_ScanContent_GoogleOAuthToken(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "ya29.a0AfH6SMBx1 example_token_data"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Google OAuth token")
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - Azure credentials
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_AzureTenantID(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "azure_tenant_id=12345678-1234-1234-1234-123456789012"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Azure tenant ID")
	}
}

func TestScanner_ScanContent_AzureStorageKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	// Need a base64 string that's at least 64 chars
	content := "azure_storage_key = " + strings.Repeat("ABCDEFGH", 10) + "=="
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Azure storage key")
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - Private keys
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_RSAPrivateKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "-----BEGIN RSA PRIVATE KEY-----\nMIIEowI...\n-----END RSA PRIVATE KEY-----"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect RSA private key")
	}
	found := false
	for _, m := range result.Matches {
		if m.Type == "rsa_private_key" {
			found = true
		}
	}
	if !found {
		t.Error("rsa_private_key match not found")
	}
}

func TestScanner_ScanContent_ECPrivateKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "-----BEGIN EC PRIVATE KEY-----\nMHQCAQ...\n-----END EC PRIVATE KEY-----"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect EC private key")
	}
}

func TestScanner_ScanContent_DSAPrivateKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "-----BEGIN DSA PRIVATE KEY-----\nMIIBuw...\n-----END DSA PRIVATE KEY-----"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect DSA private key")
	}
}

func TestScanner_ScanContent_OpenSSHPrivateKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC...\n-----END OPENSSH PRIVATE KEY-----"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect OpenSSH private key")
	}
}

func TestScanner_ScanContent_GenericPrivateKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "-----BEGIN PRIVATE KEY-----\nMIIEvg...\n-----END PRIVATE KEY-----"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect generic private key")
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - JWT tokens
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_JWTToken(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect JWT token")
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - Slack tokens
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_SlackToken(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "xoxb-123456789012-123456789012-abcdefghijklmnopqrstuvwx"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Slack token")
	}
}

func TestScanner_ScanContent_SlackWebhook(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "https://hooks.slack.com/services/T12345678/B12345678/abcdefghijklmnopqrstuvwx12345678"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Slack webhook")
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - Stripe keys
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_StripeSecretKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "sk_live_abcdefghijklmnopqrstuvwxyz012345"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Stripe secret key")
	}
}

func TestScanner_ScanContent_StripeTestKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "sk_test_abcdefghijklmnopqrstuvwxyz012345"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Stripe test key")
	}
}

func TestScanner_ScanContent_StripePublishableKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "pk_live_abcdefghijklmnopqrstuvwxyz012345"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Stripe publishable key")
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - Other credentials
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_SendGridAPIKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	// Pattern: SG\.[A-Za-z0-9\-_]{22}\.[A-Za-z0-9\-_]{43}
	part1 := strings.Repeat("A", 22)
	part2 := strings.Repeat("B", 43)
	content := "SG." + part1 + "." + part2
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect SendGrid API key")
	}
}

func TestScanner_ScanContent_TwilioAPIKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "SK" + strings.Repeat("a0", 16)
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Twilio API key")
	}
}

func TestScanner_ScanContent_TwilioAccountSID(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "AC" + strings.Repeat("a0", 16)
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Twilio Account SID")
	}
}

func TestScanner_ScanContent_GitLabToken(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "glpat-ABCDEFGHIJKLMNOPQRST"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect GitLab token")
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - Generic patterns
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_PasswordInURL(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "mysql://admin:secretpass123@db.example.com:3306/mydb"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect password in URL")
	}
}

func TestScanner_ScanContent_PasswordAssignment(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "password = supersecretvalue123"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect password assignment")
	}
}

func TestScanner_ScanContent_SecretAssignment(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "secret_key = mylongsecretkeyvalue12345"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect secret assignment")
	}
}

func TestScanner_ScanContent_TokenAssignment(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "api_token = mylongtokenvalue123456"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect token assignment")
	}
}

func TestScanner_ScanContent_APIKeyAssignment(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "api_key = mylongapikeyvalue123456"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect API key assignment")
	}
}

func TestScanner_ScanContent_HerokuAPIKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "heroku_api_key=12345678-1234-1234-1234-123456789012"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Heroku API key")
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - Clean content
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_CleanContent(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "This is a normal log line with no secrets: user connected from 192.168.1.1"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if result.HasMatches {
		t.Errorf("clean content should have no matches, got %d: %+v", len(result.Matches), result.Matches)
	}
	if result.Summary != "" {
		t.Errorf("clean content should have empty summary, got %q", result.Summary)
	}
}

func TestScanner_ScanContent_EmptyString(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	result, err := s.ScanContent(context.Background(), "")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if result.HasMatches {
		t.Error("empty string should have no matches")
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - Multiple credentials in one string
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_MultipleCredentials(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "AWS_KEY=AKIAIOSFODNN7EXAMPLE and GITHUB=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz012345678"
	result, err := s.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect multiple credentials")
	}
	if len(result.Matches) < 2 {
		t.Errorf("expected at least 2 matches, got %d", len(result.Matches))
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - Near misses
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_NearMisses(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	// Too short for AWS key (AKIA + 16 chars = 20 total, this is shorter)
	result, err := s.ScanContent(context.Background(), "AKIAshort")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	// The "AKIAshort" is only 9 chars, need AKIA + 16 = 20 chars for AWS key
	for _, m := range result.Matches {
		if m.Type == "aws_access_key_id" {
			t.Error("short string should not match AWS access key pattern")
		}
	}

	// Regular text should not match private keys
	result, err = s.ScanContent(context.Background(), "this is not a BEGIN RSA PRIVATE KEY")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	for _, m := range result.Matches {
		if m.Type == "rsa_private_key" {
			t.Error("casual mention should not match RSA private key")
		}
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanContent - Context cancellation
// ---------------------------------------------------------------------------

func TestScanner_ScanContent_ContextCancelled(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	cancel() // cancel immediately

	_, err = s.ScanContent(ctx, "AKIAIOSFODNN7EXAMPLE")
	if err == nil {
		t.Error("cancelled context should return error")
	}
}

// ---------------------------------------------------------------------------
// Scanner.ScanToolOutput
// ---------------------------------------------------------------------------

func TestScanner_ScanToolOutput(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	result, err := s.ScanToolOutput(context.Background(), "test_tool", "AKIAIOSFODNN7EXAMPLE")
	if err != nil {
		t.Fatalf("ScanToolOutput error: %v", err)
	}
	if !result.HasMatches {
		t.Error("should detect credential in tool output")
	}
}

func TestScanner_ScanToolOutput_Clean(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	result, err := s.ScanToolOutput(context.Background(), "test_tool", "all clear, no secrets here")
	if err != nil {
		t.Fatalf("ScanToolOutput error: %v", err)
	}
	if result.HasMatches {
		t.Error("clean tool output should have no matches")
	}
}

func TestScanner_ScanToolOutput_Disabled(t *testing.T) {
	cfg := &credential.Config{Enabled: false, Action: "warn"}
	s, err := credential.NewScanner(cfg)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	result, err := s.ScanToolOutput(context.Background(), "test_tool", "AKIAIOSFODNN7EXAMPLE")
	if err != nil {
		t.Fatalf("ScanToolOutput error: %v", err)
	}
	if result.HasMatches {
		t.Error("disabled scanner should not detect anything in tool output")
	}
}

func TestScanner_ScanToolOutput_ContextCancelled(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	_, err = s.ScanToolOutput(ctx, "test_tool", "AKIAIOSFODNN7EXAMPLE")
	if err == nil {
		t.Error("cancelled context should return error")
	}
}

// ---------------------------------------------------------------------------
// Scanner.RedactContent
// ---------------------------------------------------------------------------

func TestScanner_RedactContent_WithCredential(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "my key is AKIAIOSFODNN7EXAMPLE and that is all"
	redacted, err := s.RedactContent(context.Background(), content)
	if err != nil {
		t.Fatalf("RedactContent error: %v", err)
	}
	if redacted == content {
		t.Error("redacted content should differ from original")
	}
	if strings.Contains(redacted, "AKIAIOSFODNN7EXAMPLE") {
		t.Errorf("redacted content should not contain original key: %s", redacted)
	}
}

func TestScanner_RedactContent_CleanContent(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "no secrets here, just normal text"
	redacted, err := s.RedactContent(context.Background(), content)
	if err != nil {
		t.Fatalf("RedactContent error: %v", err)
	}
	if redacted != content {
		t.Errorf("clean content should be unchanged, got: %s", redacted)
	}
}

func TestScanner_RedactContent_EmptyString(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	redacted, err := s.RedactContent(context.Background(), "")
	if err != nil {
		t.Fatalf("RedactContent error: %v", err)
	}
	if redacted != "" {
		t.Errorf("empty string should remain empty, got: %q", redacted)
	}
}

func TestScanner_RedactContent_Disabled(t *testing.T) {
	cfg := &credential.Config{Enabled: false, Action: "warn"}
	s, err := credential.NewScanner(cfg)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "AKIAIOSFODNN7EXAMPLE"
	redacted, err := s.RedactContent(context.Background(), content)
	if err != nil {
		t.Fatalf("RedactContent error: %v", err)
	}
	if redacted != content {
		t.Error("disabled scanner should not redact content")
	}
}

func TestScanner_RedactContent_ContextCancelled(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	_, err = s.RedactContent(ctx, "AKIAIOSFODNN7EXAMPLE")
	if err == nil {
		t.Error("cancelled context should return error")
	}
}

func TestScanner_RedactContent_RSAPrivateKey(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "key = -----BEGIN RSA PRIVATE KEY-----\ndata\n-----END RSA PRIVATE KEY-----"
	redacted, err := s.RedactContent(context.Background(), content)
	if err != nil {
		t.Fatalf("RedactContent error: %v", err)
	}
	if strings.Contains(redacted, "BEGIN RSA PRIVATE KEY") {
		t.Errorf("RSA private key should be redacted, got: %s", redacted)
	}
}

func TestScanner_RedactContent_MultipleCredentials(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	content := "AWS AKIAIOSFODNN7EXAMPLE and github ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz012345678"
	redacted, err := s.RedactContent(context.Background(), content)
	if err != nil {
		t.Fatalf("RedactContent error: %v", err)
	}
	if strings.Contains(redacted, "AKIAIOSFODNN7EXAMPLE") {
		t.Error("AWS key should be redacted")
	}
	if strings.Contains(redacted, "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ") {
		t.Error("GitHub token should be redacted")
	}
}

// ---------------------------------------------------------------------------
// Scanner.IsEnabled
// ---------------------------------------------------------------------------

func TestScanner_IsEnabled(t *testing.T) {
	s1, _ := credential.NewScanner(nil)
	if !s1.IsEnabled() {
		t.Error("default scanner should be enabled")
	}

	s2, _ := credential.NewScanner(&credential.Config{Enabled: false})
	if s2.IsEnabled() {
		t.Error("disabled scanner should report IsEnabled() == false")
	}
}

// ---------------------------------------------------------------------------
// Scanner.GetAction / SetAction
// ---------------------------------------------------------------------------

func TestScanner_GetAction(t *testing.T) {
	s, _ := credential.NewScanner(nil)
	if s.GetAction() != "redact" {
		t.Errorf("default action = %q, want redact", s.GetAction())
	}
}

func TestScanner_SetAction(t *testing.T) {
	s, _ := credential.NewScanner(nil)

	tests := []struct {
		action string
		valid  bool
	}{
		{"block", true},
		{"redact", true},
		{"warn", true},
		{"invalid", false},
		{"", false},
	}

	for _, tc := range tests {
		err := s.SetAction(tc.action)
		if tc.valid {
			if err != nil {
				t.Errorf("SetAction(%q) should succeed: %v", tc.action, err)
			}
			if s.GetAction() != tc.action {
				t.Errorf("GetAction() = %q after SetAction(%q)", s.GetAction(), tc.action)
			}
		} else {
			if err == nil {
				t.Errorf("SetAction(%q) should fail for invalid action", tc.action)
			}
		}
	}
}

// ---------------------------------------------------------------------------
// ScanResult summary
// ---------------------------------------------------------------------------

func TestScanResult_Summary(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	result, err := s.ScanContent(context.Background(), "AKIAIOSFODNN7EXAMPLE")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if result.Summary == "" {
		t.Error("result with matches should have non-empty summary")
	}
	if !strings.Contains(result.Summary, "credential") {
		t.Errorf("summary should mention 'credential', got: %s", result.Summary)
	}
}

// ---------------------------------------------------------------------------
// Match position and masking
// ---------------------------------------------------------------------------

func TestScanResult_MatchPosition(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	result, err := s.ScanContent(context.Background(), "prefix AKIAIOSFODNN7EXAMPLE suffix")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	found := false
	for _, m := range result.Matches {
		if m.Type == "aws_access_key_id" {
			found = true
			if m.Position < 0 {
				t.Errorf("position should be non-negative, got %d", m.Position)
			}
			if m.MaskedValue == "" {
				t.Error("masked value should not be empty")
			}
		}
	}
	if !found {
		t.Error("AWS access key match not found")
	}
}

func TestScanResult_MaskedValue(t *testing.T) {
	s, err := credential.NewScanner(nil)
	if err != nil {
		t.Fatalf("NewScanner: %v", err)
	}

	result, err := s.ScanContent(context.Background(), "AKIAIOSFODNN7EXAMPLE")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	for _, m := range result.Matches {
		if m.Type == "aws_access_key_id" {
			// maskKeepPrefix(4, 4) should keep first 4 and last 4 chars
			if !strings.HasPrefix(m.MaskedValue, "AKIA") {
				t.Errorf("masked value should start with AKIA, got: %s", m.MaskedValue)
			}
		}
	}
}
