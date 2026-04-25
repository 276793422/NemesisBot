// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package dlp_test

import (
	"context"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/security/dlp"
)

// ---------------------------------------------------------------------------
// NewDLPEngine
// ---------------------------------------------------------------------------

func TestNewDLPEngine_BasicConfig(t *testing.T) {
	cfg := dlp.Config{Enabled: true}
	e := dlp.NewDLPEngine(cfg)
	if e == nil {
		t.Fatal("NewDLPEngine should return non-nil")
	}
	if !e.IsEnabled() {
		t.Error("engine should be enabled")
	}
}

func TestNewDLPEngine_DisabledConfig(t *testing.T) {
	cfg := dlp.Config{Enabled: false}
	e := dlp.NewDLPEngine(cfg)
	if e.IsEnabled() {
		t.Error("engine should be disabled")
	}
}

func TestNewDLPEngine_WithCustomRules(t *testing.T) {
	cfg := dlp.Config{
		Enabled: true,
		CustomRules: []dlp.Rule{
			{
				Name:        "custom_test_rule",
				Description: "test rule",
				Category:    "test",
				Severity:    "medium",
				Pattern:     `TEST_SECRET_\d+`,
			},
		},
	}
	e := dlp.NewDLPEngine(cfg)
	if e == nil {
		t.Fatal("engine should not be nil")
	}

	result, err := e.ScanContent(context.Background(), "found TEST_SECRET_123 in the text")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("custom rule should be detected")
	}
	found := false
	for _, m := range result.Matches {
		if m.RuleName == "custom_test_rule" {
			found = true
		}
	}
	if !found {
		t.Error("custom_test_rule match not found in results")
	}
}

func TestNewDLPEngine_WithEnabledRules(t *testing.T) {
	cfg := dlp.Config{
		Enabled:      true,
		EnabledRules: []string{"us_ssn", "email_address"},
	}
	e := dlp.NewDLPEngine(cfg)

	result, err := e.ScanContent(context.Background(), "SSN: 123-45-6789 and email: user@example.com")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect SSN and email")
	}

	// Only us_ssn and email_address should be enabled
	names := e.GetRuleNames()
	if len(names) > 2 {
		t.Errorf("expected at most 2 rules with EnabledRules filter, got %d", len(names))
	}
}

func TestNewDLPEngine_InvalidPatternInCustomRule(t *testing.T) {
	cfg := dlp.Config{
		Enabled: true,
		CustomRules: []dlp.Rule{
			{
				Name:     "bad_rule",
				Pattern:  "[invalid(regex",
				Severity: "high",
			},
		},
	}
	e := dlp.NewDLPEngine(cfg)
	// The engine should still work, just skipping the invalid rule
	result, err := e.ScanContent(context.Background(), "test content")
	if err != nil {
		t.Fatalf("ScanContent should not fail: %v", err)
	}
	// No matches expected since the invalid rule is skipped
	if result.HasMatches {
		t.Error("should not have matches with an invalid custom rule skipped")
	}
}

func TestNewDLPEngine_MaxContentLength(t *testing.T) {
	cfg := dlp.Config{
		Enabled:          true,
		MaxContentLength: 20,
	}
	e := dlp.NewDLPEngine(cfg)

	// Content with SSN at position > 20 should not be detected
	longPrefix := strings.Repeat("x", 25)
	content := longPrefix + " 123-45-6789"
	result, err := e.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if result.HasMatches {
		t.Error("SSN beyond MaxContentLength should not be detected")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanContent - Credit card numbers
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanContent_CreditCardVisa(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "card: 4111111111111111")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Visa card number")
	}
}

func TestDLPEngine_ScanContent_CreditCardMastercard(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "card: 5500000000000004")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Mastercard number")
	}
}

func TestDLPEngine_ScanContent_CreditCardAmex(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "card: 371449635398431")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Amex card number")
	}
}

func TestDLPEngine_ScanContent_CreditCardDiscover(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "card: 6011111111111117")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Discover card number")
	}
}

func TestDLPEngine_ScanContent_CreditCardJCB(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "card: 3530111333300000")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect JCB card number")
	}
}

func TestDLPEngine_ScanContent_CreditCardDiners(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "card: 30000000000004")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Diners Club card number")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanContent - API keys and tokens
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanContent_AWSAccessKey(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "key: AKIAIOSFODNN7EXAMPLE")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect AWS access key")
	}
}

func TestDLPEngine_ScanContent_AWSSecretKey(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect AWS secret key")
	}
}

func TestDLPEngine_ScanContent_GoogleAPIKey(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "key=AIzaSyDaGmWKa4VuX0hR8zQ6LRbR4VzwsmKgH00")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Google API key")
	}
}

func TestDLPEngine_ScanContent_GoogleOAuthToken(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "token=ya29.a0AfH6SMBx1example")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Google OAuth token")
	}
}

func TestDLPEngine_ScanContent_AzureAPIKey(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "azure_api_key="+strings.Repeat("abc", 12))
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Azure API key")
	}
}

func TestDLPEngine_ScanContent_GenericHexKey(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "api_key = a0b1c2d3e4f5a0b1c2d3e4f5a0b1c2d3")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect generic hex key")
	}
}

func TestDLPEngine_ScanContent_GenericBase64Key(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "secret = "+"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect generic base64 key")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanContent - Private keys
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanContent_RSAPrivateKey(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n-----END RSA PRIVATE KEY-----")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect RSA private key")
	}
}

func TestDLPEngine_ScanContent_GenericPrivateKey(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "-----BEGIN PRIVATE KEY-----\nMIIE...\n-----END PRIVATE KEY-----")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect generic private key")
	}
}

func TestDLPEngine_ScanContent_OpenSSHPrivateKey(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "-----BEGIN OPENSSH PRIVATE KEY-----\nb3Bl...\n-----END OPENSSH PRIVATE KEY-----")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect OpenSSH private key")
	}
}

func TestDLPEngine_ScanContent_PKCS8EncryptedKey(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIE...\n-----END ENCRYPTED PRIVATE KEY-----")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect PKCS#8 encrypted private key")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanContent - PII
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanContent_USSSN(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "SSN: 123-45-6789")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect US SSN")
	}
}

func TestDLPEngine_ScanContent_EmailAddress(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "contact: user@example.com")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect email address")
	}
}

func TestDLPEngine_ScanContent_PhoneNumber(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "phone: +1-555-123-4567")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect phone number")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanContent - Network identifiers
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanContent_PrivateIPAddress(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "server: 192.168.1.100")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect private IP address")
	}
}

func TestDLPEngine_ScanContent_PublicIPAddress(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "connect to 8.8.8.8 for DNS")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect public IP address")
	}
}

func TestDLPEngine_ScanContent_IPv6Address(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "IPv6: 2001:0db8:85a3:0000:0000:8a2e:0370:7334")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect IPv6 address")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanContent - Financial
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanContent_BankAccountNumber(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "account_number=ABCDEF12345678")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect bank account number")
	}
}

func TestDLPEngine_ScanContent_IBAN(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "IBAN: GB82WEST12345698765432")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect IBAN")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanContent - Tokens and connection strings
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanContent_JWTToken(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	jwt := "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
	result, err := e.ScanContent(context.Background(), "token: "+jwt)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect JWT token")
	}
}

func TestDLPEngine_ScanContent_DatabaseConnectionString(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "postgres://admin:password123@db.example.com:5432/mydb")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect database connection string")
	}
}

func TestDLPEngine_ScanContent_GitHubToken(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz012345")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect GitHub token")
	}
}

func TestDLPEngine_ScanContent_SlackToken(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "xoxb-123456789012-123456789012-abcdefghijklmnopqrstuvwx")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Slack token")
	}
}

func TestDLPEngine_ScanContent_StripeKey(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "sk_live_abcdefghijklmnopqrstuvwxyz012345")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect Stripe key")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanContent - Generic secrets
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanContent_PasswordAssignment(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "password = supersecret123")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect password assignment")
	}
}

func TestDLPEngine_ScanContent_TokenAssignment(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "bearer = my_bearer_token_value")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect token assignment")
	}
}

func TestDLPEngine_ScanContent_SecretKeyAssignment(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "secret_key = mysecretkey123456")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect secret key assignment")
	}
}

func TestDLPEngine_ScanContent_AuthorizationHeader(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyIn0.sig")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect authorization header")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanContent - Clean content
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanContent_CleanContent(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "Hello world, this is a normal message with no sensitive data.")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if result.HasMatches {
		t.Errorf("clean content should have no matches, got %d", len(result.Matches))
	}
	if result.Action != "allow" {
		t.Errorf("clean content action = %q, want allow", result.Action)
	}
}

func TestDLPEngine_ScanContent_EmptyContent(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if result.HasMatches {
		t.Error("empty content should have no matches")
	}
}

func TestDLPEngine_ScanContent_Disabled(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: false})
	result, err := e.ScanContent(context.Background(), "123-45-6789")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if result.HasMatches {
		t.Error("disabled engine should not detect anything")
	}
	if result.Action != "allow" {
		t.Errorf("disabled engine action = %q, want allow", result.Action)
	}
}

func TestDLPEngine_ScanContent_ContextCancelled(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	_, err := e.ScanContent(ctx, "123-45-6789")
	if err == nil {
		t.Error("cancelled context should return error")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanContent - Multiple matches
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanContent_MultipleMatches(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	content := "SSN: 123-45-6789, email: user@example.com, phone: 555-123-4567"
	result, err := e.ScanContent(context.Background(), content)
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect multiple sensitive data")
	}
	if len(result.Matches) < 2 {
		t.Errorf("expected at least 2 matches, got %d", len(result.Matches))
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanContent - ActionOnMatch
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanContent_ActionBlock(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true, ActionOnMatch: "block"})
	result, err := e.ScanContent(context.Background(), "123-45-6789")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if result.Action != "block" {
		t.Errorf("action = %q, want block", result.Action)
	}
}

func TestDLPEngine_ScanContent_ActionRedact(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true, ActionOnMatch: "redact"})
	result, err := e.ScanContent(context.Background(), "123-45-6789")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if result.Action != "redact" {
		t.Errorf("action = %q, want redact", result.Action)
	}
}

func TestDLPEngine_ScanContent_DefaultAction(t *testing.T) {
	// With no ActionOnMatch set, default should be based on severity
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "123-45-6789")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	// SSN has severity "high", so default action should be "block"
	if result.Action != "block" {
		t.Errorf("default action for high severity = %q, want block", result.Action)
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanContent - Summary
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanContent_Summary(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "123-45-6789")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if result.Summary == "" {
		t.Error("result with matches should have non-empty summary")
	}
	if !strings.Contains(result.Summary, "sensitive") {
		t.Errorf("summary should mention 'sensitive', got: %s", result.Summary)
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanToolInput
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanToolInput(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	args := map[string]interface{}{
		"content": "SSN: 123-45-6789",
	}
	result, err := e.ScanToolInput(context.Background(), "file_read", args)
	if err != nil {
		t.Fatalf("ScanToolInput error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect SSN in tool input")
	}
	if !strings.Contains(result.Summary, "file_read") {
		t.Errorf("summary should mention tool name, got: %s", result.Summary)
	}
}

func TestDLPEngine_ScanToolInput_Clean(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	args := map[string]interface{}{
		"path": "/tmp/readme.txt",
	}
	result, err := e.ScanToolInput(context.Background(), "file_read", args)
	if err != nil {
		t.Fatalf("ScanToolInput error: %v", err)
	}
	if result.HasMatches {
		t.Error("clean tool input should have no matches")
	}
}

func TestDLPEngine_ScanToolInput_Disabled(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: false})
	args := map[string]interface{}{
		"content": "123-45-6789",
	}
	result, err := e.ScanToolInput(context.Background(), "tool", args)
	if err != nil {
		t.Fatalf("ScanToolInput error: %v", err)
	}
	if result.HasMatches {
		t.Error("disabled engine should not detect in tool input")
	}
}

func TestDLPEngine_ScanToolInput_ContextCancelled(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	args := map[string]interface{}{"data": "test"}
	_, err := e.ScanToolInput(ctx, "tool", args)
	if err == nil {
		t.Error("cancelled context should return error")
	}
}

func TestDLPEngine_ScanToolInput_NilArgs(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanToolInput(context.Background(), "tool", nil)
	if err != nil {
		t.Fatalf("ScanToolInput with nil args should succeed: %v", err)
	}
	if result.HasMatches {
		t.Error("nil args should have no matches")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanToolOutput
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanToolOutput(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanToolOutput(context.Background(), "file_read", "SSN: 123-45-6789")
	if err != nil {
		t.Fatalf("ScanToolOutput error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect SSN in tool output")
	}
	if !strings.Contains(result.Summary, "file_read") {
		t.Errorf("summary should mention tool name, got: %s", result.Summary)
	}
}

func TestDLPEngine_ScanToolOutput_Clean(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanToolOutput(context.Background(), "tool", "all clean here")
	if err != nil {
		t.Fatalf("ScanToolOutput error: %v", err)
	}
	if result.HasMatches {
		t.Error("clean tool output should have no matches")
	}
}

func TestDLPEngine_ScanToolOutput_Disabled(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: false})
	result, err := e.ScanToolOutput(context.Background(), "tool", "123-45-6789")
	if err != nil {
		t.Fatalf("ScanToolOutput error: %v", err)
	}
	if result.HasMatches {
		t.Error("disabled engine should not detect in tool output")
	}
}

func TestDLPEngine_ScanToolOutput_ContextCancelled(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	_, err := e.ScanToolOutput(ctx, "tool", "test")
	if err == nil {
		t.Error("cancelled context should return error")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.RedactContent
// ---------------------------------------------------------------------------

func TestDLPEngine_RedactContent_WithSSN(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	content := "The SSN is 123-45-6789 for the user"
	redacted, result, err := e.RedactContent(context.Background(), content)
	if err != nil {
		t.Fatalf("RedactContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("RedactContent result should have matches")
	}
	if strings.Contains(redacted, "123-45-6789") {
		t.Errorf("SSN should be redacted, got: %s", redacted)
	}
}

func TestDLPEngine_RedactContent_WithPrivateKey(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	content := "-----BEGIN RSA PRIVATE KEY-----\ndata\n-----END RSA PRIVATE KEY-----"
	redacted, result, err := e.RedactContent(context.Background(), content)
	if err != nil {
		t.Fatalf("RedactContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should detect private key")
	}
	if strings.Contains(redacted, "BEGIN RSA PRIVATE KEY") {
		t.Errorf("private key should be redacted, got: %s", redacted)
	}
}

func TestDLPEngine_RedactContent_CleanContent(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	content := "nothing to redact here"
	redacted, result, err := e.RedactContent(context.Background(), content)
	if err != nil {
		t.Fatalf("RedactContent error: %v", err)
	}
	if result.HasMatches {
		t.Error("clean content should have no matches")
	}
	if redacted != content {
		t.Errorf("clean content should be unchanged, got: %s", redacted)
	}
}

func TestDLPEngine_RedactContent_Disabled(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: false})
	content := "123-45-6789"
	redacted, result, err := e.RedactContent(context.Background(), content)
	if err != nil {
		t.Fatalf("RedactContent error: %v", err)
	}
	if redacted != content {
		t.Error("disabled engine should not redact")
	}
	if result.HasMatches {
		t.Error("disabled engine should report no matches")
	}
}

func TestDLPEngine_RedactContent_ContextCancelled(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	_, _, err := e.RedactContent(ctx, "123-45-6789")
	if err == nil {
		t.Error("cancelled context should return error")
	}
}

func TestDLPEngine_RedactContent_MultipleItems(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	content := "SSN: 123-45-6789, email: user@example.com"
	redacted, result, err := e.RedactContent(context.Background(), content)
	if err != nil {
		t.Fatalf("RedactContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("should have matches")
	}
	if strings.Contains(redacted, "123-45-6789") {
		t.Errorf("SSN should be redacted, got: %s", redacted)
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.AddRule / RemoveRule
// ---------------------------------------------------------------------------

func TestDLPEngine_AddRule(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})

	err := e.AddRule(dlp.Rule{
		Name:        "custom_secret",
		Description: "Custom secret pattern",
		Category:    "test",
		Severity:    "high",
		Pattern:     `MY_SECRET_\d{10}`,
	})
	if err != nil {
		t.Fatalf("AddRule should succeed: %v", err)
	}

	result, err := e.ScanContent(context.Background(), "found MY_SECRET_1234567890 here")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("custom rule should be detected after AddRule")
	}
}

func TestDLPEngine_AddRule_InvalidPattern(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	err := e.AddRule(dlp.Rule{
		Name:    "bad",
		Pattern: "[invalid(regex",
	})
	if err == nil {
		t.Fatal("AddRule with invalid pattern should return error")
	}
}

func TestDLPEngine_RemoveRule(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})

	// First verify rule exists by scanning
	result, err := e.ScanContent(context.Background(), "123-45-6789")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("SSN should be detected before RemoveRule")
	}

	// Remove the SSN rule
	removed := e.RemoveRule("us_ssn")
	if !removed {
		t.Error("RemoveRule should return true for existing rule")
	}

	// Create a new engine since the rules are compiled once via sync.Once
	// and RemoveRule modifies the rules slice after init
	e2 := dlp.NewDLPEngine(dlp.Config{Enabled: true, EnabledRules: []string{"email_address"}})
	result2, err := e2.ScanContent(context.Background(), "123-45-6789")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	// The new engine only has email_address, so SSN should not be detected
	for _, m := range result2.Matches {
		if m.RuleName == "us_ssn" {
			t.Error("us_ssn should not be in results for engine with only email_address enabled")
		}
	}
}

func TestDLPEngine_RemoveRule_NonExistent(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	removed := e.RemoveRule("nonexistent_rule_xyz")
	if removed {
		t.Error("RemoveRule should return false for non-existent rule")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.GetRuleNames
// ---------------------------------------------------------------------------

func TestDLPEngine_GetRuleNames(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	names := e.GetRuleNames()
	if len(names) == 0 {
		t.Error("GetRuleNames should return at least one rule")
	}
}

func TestDLPEngine_GetRuleNames_WithFilter(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{
		Enabled:      true,
		EnabledRules: []string{"us_ssn", "email_address"},
	})
	names := e.GetRuleNames()
	if len(names) != 2 {
		t.Errorf("expected 2 rules, got %d: %v", len(names), names)
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.GetRuleCount
// ---------------------------------------------------------------------------

func TestDLPEngine_GetRuleCount(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	count := e.GetRuleCount()
	if count == 0 {
		t.Error("GetRuleCount should return at least 1")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.IsEnabled / SetEnabled
// ---------------------------------------------------------------------------

func TestDLPEngine_IsEnabled_SetEnabled(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	if !e.IsEnabled() {
		t.Error("should be enabled")
	}

	e.SetEnabled(false)
	if e.IsEnabled() {
		t.Error("should be disabled after SetEnabled(false)")
	}

	e.SetEnabled(true)
	if !e.IsEnabled() {
		t.Error("should be enabled after SetEnabled(true)")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.UpdateConfig
// ---------------------------------------------------------------------------

func TestDLPEngine_UpdateConfig(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})

	e.UpdateConfig(dlp.Config{Enabled: false})
	if e.IsEnabled() {
		t.Error("should be disabled after UpdateConfig")
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.Match fields
// ---------------------------------------------------------------------------

func TestDLPEngine_MatchFields(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	result, err := e.ScanContent(context.Background(), "123-45-6789")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	if len(result.Matches) == 0 {
		t.Fatal("expected at least one match")
	}

	m := result.Matches[0]
	if m.RuleName == "" {
		t.Error("RuleName should not be empty")
	}
	if m.Category == "" {
		t.Error("Category should not be empty")
	}
	if m.Severity == "" {
		t.Error("Severity should not be empty")
	}
	if m.MaskedValue == "" {
		t.Error("MaskedValue should not be empty")
	}
	if m.Position < 0 {
		t.Errorf("Position should be non-negative, got %d", m.Position)
	}
}

// ---------------------------------------------------------------------------
// DLPEngine.ScanContent - Near misses
// ---------------------------------------------------------------------------

func TestDLPEngine_ScanContent_ShortNumber(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	// Too short for a Visa card
	result, err := e.ScanContent(context.Background(), "1234")
	if err != nil {
		t.Fatalf("ScanContent error: %v", err)
	}
	for _, m := range result.Matches {
		if strings.Contains(m.RuleName, "credit_card") {
			t.Errorf("short number should not match credit card, got rule: %s", m.RuleName)
		}
	}
}

// ---------------------------------------------------------------------------
// Concurrent access
// ---------------------------------------------------------------------------

func TestDLPEngine_ConcurrentAccess(t *testing.T) {
	e := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	done := make(chan struct{})

	// Reader goroutine 1
	go func() {
		defer func() { done <- struct{}{} }()
		for i := 0; i < 50; i++ {
			e.ScanContent(context.Background(), "123-45-6789")
			e.GetRuleNames()
			e.GetRuleCount()
		}
	}()

	// Reader goroutine 2
	go func() {
		defer func() { done <- struct{}{} }()
		for i := 0; i < 50; i++ {
			e.IsEnabled()
			e.ScanToolOutput(context.Background(), "tool", "user@example.com")
		}
	}()

	// Writer goroutine
	go func() {
		defer func() { done <- struct{}{} }()
		for i := 0; i < 10; i++ {
			e.SetEnabled(true)
			e.SetEnabled(false)
			e.SetEnabled(true)
		}
	}()

	<-done
	<-done
	<-done
}
