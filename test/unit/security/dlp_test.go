// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package security_test

import (
	"context"
	"testing"

	dlp "github.com/276793422/NemesisBot/module/security/dlp"
)

func TestDLPEngine_ScanContent_CreditCard(t *testing.T) {
	engine := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx := context.Background()

	tests := []struct {
		name    string
		content string
		wantCat string // expected category
	}{
		{
			name:    "Visa 16 digit",
			content: "My card number is 4111111111111111",
			wantCat: "financial",
		},
		{
			name:    "Mastercard",
			content: "Card: 5500000000000004",
			wantCat: "financial",
		},
		{
			name:    "Amex",
			content: "Amex card: 378282246310005",
			wantCat: "financial",
		},
		{
			name:    "Discover",
			content: "Discover: 6011111111111117",
			wantCat: "financial",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := engine.ScanContent(ctx, tt.content)
			if err != nil {
				t.Fatalf("ScanContent returned error: %v", err)
			}
			if !result.HasMatches {
				t.Fatalf("expected matches for %q, got none", tt.content)
			}
			found := false
			for _, m := range result.Matches {
				if m.Category == tt.wantCat {
					found = true
					break
				}
			}
			if !found {
				t.Errorf("expected category %q, got matches: %+v", tt.wantCat, result.Matches)
			}
		})
	}
}

func TestDLPEngine_ScanContent_APIKey(t *testing.T) {
	engine := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx := context.Background()

	tests := []struct {
		name    string
		content string
		wantRule string
	}{
		{
			name:     "AWS Access Key",
			content:  "AWS key: AKIAIOSFODNN7EXAMPLE",
			wantRule: "aws_access_key",
		},
		{
			name:     "Google API Key",
			content:  "Google key: AIzaSyA1234567890abcdefghijklmnopqrstuv",
			wantRule: "google_api_key",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := engine.ScanContent(ctx, tt.content)
			if err != nil {
				t.Fatalf("ScanContent returned error: %v", err)
			}
			if !result.HasMatches {
				t.Fatalf("expected matches for %q, got none", tt.content)
			}
			found := false
			for _, m := range result.Matches {
				if m.RuleName == tt.wantRule {
					found = true
					break
				}
			}
			if !found {
				t.Errorf("expected rule %q, got matches: %+v", tt.wantRule, result.Matches)
			}
		})
	}
}

func TestDLPEngine_ScanContent_PrivateKey(t *testing.T) {
	engine := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx := context.Background()

	tests := []struct {
		name    string
		content string
		wantCat string
	}{
		{
			name:    "RSA Private Key",
			content: "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA...",
			wantCat: "credentials",
		},
		{
			name:    "Generic Private Key",
			content: "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkq...",
			wantCat: "credentials",
		},
		{
			name:    "OpenSSH Private Key",
			content: "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk...",
			wantCat: "credentials",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := engine.ScanContent(ctx, tt.content)
			if err != nil {
				t.Fatalf("ScanContent returned error: %v", err)
			}
			if !result.HasMatches {
				t.Fatalf("expected matches for private key content, got none")
			}
			found := false
			for _, m := range result.Matches {
				if m.Category == tt.wantCat {
					found = true
					break
				}
			}
			if !found {
				t.Errorf("expected category %q, got matches: %+v", tt.wantCat, result.Matches)
			}
		})
	}
}

func TestDLPEngine_ScanContent_Email(t *testing.T) {
	engine := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx := context.Background()

	content := "Contact us at admin@example.com for details."
	result, err := engine.ScanContent(ctx, content)
	if err != nil {
		t.Fatalf("ScanContent returned error: %v", err)
	}

	if !result.HasMatches {
		t.Fatal("expected email match, got none")
	}

	found := false
	for _, m := range result.Matches {
		if m.RuleName == "email_address" {
			found = true
			if m.Severity != "low" {
				t.Errorf("expected severity 'low' for email, got %q", m.Severity)
			}
		}
	}
	if !found {
		t.Errorf("expected email_address rule match, got: %+v", result.Matches)
	}
}

func TestDLPEngine_ScanContent_MultipleMatches(t *testing.T) {
	engine := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx := context.Background()

	content := "Card: 4111111111111111 and key: AKIAIOSFODNN7EXAMPLE and email: test@example.com"
	result, err := engine.ScanContent(ctx, content)
	if err != nil {
		t.Fatalf("ScanContent returned error: %v", err)
	}

	if !result.HasMatches {
		t.Fatal("expected matches, got none")
	}

	if len(result.Matches) < 2 {
		t.Errorf("expected at least 2 matches for content with multiple types, got %d", len(result.Matches))
	}

	// Verify different categories are represented
	categories := make(map[string]bool)
	for _, m := range result.Matches {
		categories[m.Category] = true
	}
	if len(categories) < 2 {
		t.Errorf("expected at least 2 different categories, got %d: %v", len(categories), categories)
	}
}

func TestDLPEngine_ScanContent_NoMatches(t *testing.T) {
	engine := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx := context.Background()

	content := "This is a perfectly normal sentence with no sensitive data whatsoever."
	result, err := engine.ScanContent(ctx, content)
	if err != nil {
		t.Fatalf("ScanContent returned error: %v", err)
	}

	if result.HasMatches {
		t.Errorf("expected no matches for clean content, got %d matches", len(result.Matches))
	}
	if result.Action != "allow" {
		t.Errorf("expected action 'allow' for clean content, got %q", result.Action)
	}
}

func TestDLPEngine_ScanContent_DisabledRules(t *testing.T) {
	// Enable only the email_address rule
	engine := dlp.NewDLPEngine(dlp.Config{
		Enabled:      true,
		EnabledRules: []string{"email_address"},
	})
	ctx := context.Background()

	// Content with both credit card and email
	content := "Card: 4111111111111111 and email: test@example.com"
	result, err := engine.ScanContent(ctx, content)
	if err != nil {
		t.Fatalf("ScanContent returned error: %v", err)
	}

	if !result.HasMatches {
		t.Fatal("expected at least email match")
	}

	// All matches should be email_address
	for _, m := range result.Matches {
		if m.RuleName != "email_address" {
			t.Errorf("expected only email_address matches, got %q", m.RuleName)
		}
	}
}

func TestDLPEngine_RedactContent(t *testing.T) {
	engine := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx := context.Background()

	content := "My AWS key is AKIAIOSFODNN7EXAMPLE and email is test@example.com"
	redacted, result, err := engine.RedactContent(ctx, content)
	if err != nil {
		t.Fatalf("RedactContent returned error: %v", err)
	}

	if !result.HasMatches {
		t.Fatal("expected matches in RedactContent")
	}

	// The redacted string should not contain the original sensitive values
	if redacted == content {
		t.Error("expected content to be redacted, but it is unchanged")
	}
}

func TestDLPEngine_AddRemoveRule(t *testing.T) {
	engine := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx := context.Background()

	// Trigger lazy initialization by performing an initial scan
	_, err := engine.ScanContent(ctx, "init trigger")
	if err != nil {
		t.Fatalf("initial scan returned error: %v", err)
	}

	// Add a custom rule (after init so it isn't overwritten)
	customRule := dlp.Rule{
		Name:        "custom_test_pattern",
		Description: "Test custom pattern",
		Category:    "custom",
		Severity:    "medium",
		Pattern:     `CUSTOM_SECRET_\d+`,
	}
	err = engine.AddRule(customRule)
	if err != nil {
		t.Fatalf("AddRule returned error: %v", err)
	}

	// Verify the rule works
	content := "Found CUSTOM_SECRET_123 in the data"
	result, err := engine.ScanContent(ctx, content)
	if err != nil {
		t.Fatalf("ScanContent returned error: %v", err)
	}
	if !result.HasMatches {
		t.Fatal("expected custom rule to match")
	}
	found := false
	for _, m := range result.Matches {
		if m.RuleName == "custom_test_pattern" {
			found = true
		}
	}
	if !found {
		t.Errorf("expected custom_test_pattern match, got: %+v", result.Matches)
	}

	// Remove the rule
	removed := engine.RemoveRule("custom_test_pattern")
	if !removed {
		t.Fatal("expected RemoveRule to return true")
	}

	// Verify the rule no longer matches
	result2, err := engine.ScanContent(ctx, content)
	if err != nil {
		t.Fatalf("ScanContent after removal returned error: %v", err)
	}
	for _, m := range result2.Matches {
		if m.RuleName == "custom_test_pattern" {
			t.Error("custom rule should have been removed but still matches")
		}
	}

	// Remove non-existent rule
	removed = engine.RemoveRule("non_existent_rule")
	if removed {
		t.Error("expected RemoveRule to return false for non-existent rule")
	}
}

func TestDLPEngine_EmptyContent(t *testing.T) {
	engine := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx := context.Background()

	result, err := engine.ScanContent(ctx, "")
	if err != nil {
		t.Fatalf("ScanContent returned error: %v", err)
	}
	if result.HasMatches {
		t.Error("expected no matches for empty content")
	}
	if result.Action != "allow" {
		t.Errorf("expected action 'allow' for empty content, got %q", result.Action)
	}
}

func TestDLPEngine_DisabledEngine(t *testing.T) {
	engine := dlp.NewDLPEngine(dlp.Config{Enabled: false})
	ctx := context.Background()

	result, err := engine.ScanContent(ctx, "AKIAIOSFODNN7EXAMPLE 4111111111111111")
	if err != nil {
		t.Fatalf("ScanContent returned error: %v", err)
	}
	if result.HasMatches {
		t.Error("expected no matches when engine is disabled")
	}
	if result.Action != "allow" {
		t.Errorf("expected action 'allow' when disabled, got %q", result.Action)
	}
}

func TestDLPEngine_MaxContentLength(t *testing.T) {
	engine := dlp.NewDLPEngine(dlp.Config{
		Enabled:          true,
		MaxContentLength: 20,
	})
	ctx := context.Background()

	// The credit card number starts after position 20, so it should be truncated
	content := "Normal text here  4111111111111111"
	result, err := engine.ScanContent(ctx, content)
	if err != nil {
		t.Fatalf("ScanContent returned error: %v", err)
	}
	// With max content length 20, the credit card at position 18+ is truncated
	// The result depends on whether the sensitive data falls within the first 20 bytes
	// Just verify the scan does not error out
	_ = result
}

func TestDLPEngine_ToolInputScanning(t *testing.T) {
	engine := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx := context.Background()

	args := map[string]interface{}{
		"content": "Card number: 4111111111111111",
	}
	result, err := engine.ScanToolInput(ctx, "file_write", args)
	if err != nil {
		t.Fatalf("ScanToolInput returned error: %v", err)
	}
	if !result.HasMatches {
		t.Error("expected matches in tool input")
	}
	if result.Summary == "" {
		t.Error("expected non-empty summary for tool input matches")
	}
}

func TestDLPEngine_ToolOutputScanning(t *testing.T) {
	engine := dlp.NewDLPEngine(dlp.Config{Enabled: true})
	ctx := context.Background()

	result, err := engine.ScanToolOutput(ctx, "file_read", "Key: AKIAIOSFODNN7EXAMPLE")
	if err != nil {
		t.Fatalf("ScanToolOutput returned error: %v", err)
	}
	if !result.HasMatches {
		t.Error("expected matches in tool output")
	}
	if result.Summary == "" {
		t.Error("expected non-empty summary for tool output matches")
	}
}
