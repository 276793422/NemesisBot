// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package credential provides credential detection patterns for common secret types
package credential

import "strings"

// patternDef defines a credential detection pattern before compilation
type patternDef struct {
	Name        string
	RegexStr    string
	Severity    string
	Description string
	MaskFunc    func(match string) string
}

// defaultPatterns returns all built-in credential detection patterns
func defaultPatterns() []patternDef {
	return []patternDef{
		// ============================================================
		// AWS Credentials
		// ============================================================
		{
			Name:        "aws_access_key_id",
			RegexStr:    `(?:A3T[A-Z0-9]|AKIA|AGPA|AIDA|AROA|AIPA|ANPA|ANVA|ASIA)[A-Z0-9]{16}`,
			Severity:    "critical",
			Description: "AWS Access Key ID",
			MaskFunc:    maskKeepPrefix(4, 4),
		},
		{
			Name:        "aws_secret_access_key",
			RegexStr:    `(?i)aws[_\-]?secret[_\-]?access[_\-]?key\s*[=:]\s*[A-Za-z0-9/+=]{40}`,
			Severity:    "critical",
			Description: "AWS Secret Access Key",
			MaskFunc:    maskKeepPrefix(0, 0),
		},
		{
			Name:        "aws_session_token",
			RegexStr:    `(?i)aws[_\-]?session[_\-]?token\s*[=:]\s*[A-Za-z0-9/+=]{16,}`,
			Severity:    "critical",
			Description: "AWS Session Token",
			MaskFunc:    maskKeepPrefix(0, 0),
		},

		// ============================================================
		// Google Credentials
		// ============================================================
		{
			Name:        "google_api_key",
			RegexStr:    `AIza[0-9A-Za-z\-_]{35}`,
			Severity:    "high",
			Description: "Google API Key",
			MaskFunc:    maskKeepPrefix(4, 4),
		},
		{
			Name:        "google_oauth_token",
			RegexStr:    `ya29\.[0-9A-Za-z\-_]+`,
			Severity:    "high",
			Description: "Google OAuth Access Token",
			MaskFunc:    maskKeepPrefix(5, 4),
		},

		// ============================================================
		// Azure Credentials
		// ============================================================
		{
			Name:        "azure_tenant_id",
			RegexStr:    `(?i)(?:azure[_\-]?tenant[_\-]?id|tenant[_\-]?id)\s*[=:]\s*[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`,
			Severity:    "high",
			Description: "Azure Tenant ID",
			MaskFunc:    maskKeepPrefix(0, 0),
		},
		{
			Name:        "azure_subscription_id",
			RegexStr:    `(?i)(?:azure[_\-]?subscription[_\-]?id|subscription[_\-]?id)\s*[=:]\s*[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`,
			Severity:    "high",
			Description: "Azure Subscription ID",
			MaskFunc:    maskKeepPrefix(0, 0),
		},
		{
			Name:        "azure_storage_key",
			RegexStr:    `(?i)(?:azure[_\-]?storage[_\-]?key|storage[_\-]?account[_\-]?key)\s*[=:]\s*[A-Za-z0-9+/]{64,}={0,2}`,
			Severity:    "critical",
			Description: "Azure Storage Account Key",
			MaskFunc:    maskKeepPrefix(0, 0),
		},

		// ============================================================
		// GitHub Credentials
		// ============================================================
		{
			Name:        "github_token",
			RegexStr:    `gh[psoua]_[A-Za-z0-9_]{36,255}`,
			Severity:    "critical",
			Description: "GitHub Personal Access Token",
			MaskFunc:    maskKeepPrefix(4, 4),
		},

		// ============================================================
		// GitLab Credentials
		// ============================================================
		{
			Name:        "gitlab_token",
			RegexStr:    `glpat-[A-Za-z0-9\-_]{20}`,
			Severity:    "critical",
			Description: "GitLab Personal Access Token",
			MaskFunc:    maskKeepPrefix(6, 4),
		},

		// ============================================================
		// Slack Credentials
		// ============================================================
		{
			Name:        "slack_token",
			RegexStr:    `xox[bpsar]-[0-9]{10,13}-[0-9]{10,13}-[a-zA-Z0-9]{24,34}`,
			Severity:    "critical",
			Description: "Slack Token (Bot/User/App)",
			MaskFunc:    maskKeepPrefix(5, 4),
		},
		{
			Name:        "slack_webhook",
			RegexStr:    `https://hooks\.slack\.com/services/T[A-Z0-9]+/B[A-Z0-9]+/[a-zA-Z0-9]+`,
			Severity:    "high",
			Description: "Slack Webhook URL",
			MaskFunc:    maskKeepPrefix(0, 0),
		},

		// ============================================================
		// Stripe Credentials
		// ============================================================
		{
			Name:        "stripe_secret_key",
			RegexStr:    `sk_live_[0-9a-zA-Z]{24,}`,
			Severity:    "critical",
			Description: "Stripe Secret Key (Live)",
			MaskFunc:    maskKeepPrefix(8, 4),
		},
		{
			Name:        "stripe_test_key",
			RegexStr:    `sk_test_[0-9a-zA-Z]{24,}`,
			Severity:    "high",
			Description: "Stripe Secret Key (Test)",
			MaskFunc:    maskKeepPrefix(8, 4),
		},
		{
			Name:        "stripe_publishable_key",
			RegexStr:    `pk_(live|test)_[0-9a-zA-Z]{24,}`,
			Severity:    "medium",
			Description: "Stripe Publishable Key",
			MaskFunc:    maskKeepPrefix(8, 4),
		},

		// ============================================================
		// SendGrid Credentials
		// ============================================================
		{
			Name:        "sendgrid_api_key",
			RegexStr:    `SG\.[A-Za-z0-9\-_]{22}\.[A-Za-z0-9\-_]{43}`,
			Severity:    "critical",
			Description: "SendGrid API Key",
			MaskFunc:    maskKeepPrefix(3, 4),
		},

		// ============================================================
		// Twilio Credentials
		// ============================================================
		{
			Name:        "twilio_api_key",
			RegexStr:    `SK[0-9a-fA-F]{32}`,
			Severity:    "critical",
			Description: "Twilio API Key",
			MaskFunc:    maskKeepPrefix(2, 4),
		},
		{
			Name:        "twilio_account_sid",
			RegexStr:    `AC[a-z0-9]{32}`,
			Severity:    "high",
			Description: "Twilio Account SID",
			MaskFunc:    maskKeepPrefix(2, 4),
		},

		// ============================================================
		// Heroku Credentials
		// ============================================================
		{
			Name:        "heroku_api_key",
			RegexStr:    `(?i)heroku[_\-]?api[_\-]?key\s*[=:]\s*[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`,
			Severity:    "high",
			Description: "Heroku API Key",
			MaskFunc:    maskKeepPrefix(0, 0),
		},

		// ============================================================
		// JWT Tokens
		// ============================================================
		{
			Name:        "jwt_token",
			RegexStr:    `eyJ[A-Za-z0-9\-_]+\.eyJ[A-Za-z0-9\-_]+\.[A-Za-z0-9\-_]+`,
			Severity:    "high",
			Description: "JSON Web Token (JWT)",
			MaskFunc:    maskKeepPrefix(3, 3),
		},

		// ============================================================
		// Private Keys
		// ============================================================
		{
			Name:        "rsa_private_key",
			RegexStr:    `-----BEGIN RSA PRIVATE KEY-----`,
			Severity:    "critical",
			Description: "RSA Private Key",
			MaskFunc:    maskFixed("[REDACTED RSA PRIVATE KEY]"),
		},
		{
			Name:        "ec_private_key",
			RegexStr:    `-----BEGIN EC PRIVATE KEY-----`,
			Severity:    "critical",
			Description: "EC Private Key",
			MaskFunc:    maskFixed("[REDACTED EC PRIVATE KEY]"),
		},
		{
			Name:        "dsa_private_key",
			RegexStr:    `-----BEGIN DSA PRIVATE KEY-----`,
			Severity:    "critical",
			Description: "DSA Private Key",
			MaskFunc:    maskFixed("[REDACTED DSA PRIVATE KEY]"),
		},
		{
			Name:        "openssh_private_key",
			RegexStr:    `-----BEGIN OPENSSH PRIVATE KEY-----`,
			Severity:    "critical",
			Description: "OpenSSH Private Key",
			MaskFunc:    maskFixed("[REDACTED OPENSSH PRIVATE KEY]"),
		},
		{
			Name:        "generic_private_key",
			RegexStr:    `-----BEGIN PRIVATE KEY-----`,
			Severity:    "critical",
			Description: "Private Key (PKCS#8)",
			MaskFunc:    maskFixed("[REDACTED PRIVATE KEY]"),
		},

		// ============================================================
		// Generic Credential Patterns (lower severity, common in URLs and configs)
		// ============================================================
		{
			Name:        "password_in_url",
			RegexStr:    `(?i)[a-z][a-z0-9+.-]*://[^/\s:]+:([^@\s]{3,})@`,
			Severity:    "high",
			Description: "Password embedded in URL",
			MaskFunc:    maskFixed("[REDACTED URL PASSWORD]"),
		},
		{
			Name:        "password_assignment",
			RegexStr:    `(?i)(?:password|passwd|pwd)\s*[=:]\s*['"]?[^\s'"<>]{8,}['"]?`,
			Severity:    "high",
			Description: "Password assignment in configuration",
			MaskFunc:    maskKeyValue,
		},
		{
			Name:        "secret_assignment",
			RegexStr:    `(?i)(?:secret|secret_key|secret_token)\s*[=:]\s*['"]?[^\s'"<>]{8,}['"]?`,
			Severity:    "high",
			Description: "Secret key assignment in configuration",
			MaskFunc:    maskKeyValue,
		},
		{
			Name:        "token_assignment",
			RegexStr:    `(?i)(?:api[_\-]?token|auth[_\-]?token|access[_\-]?token|bearer[_\-]?token)\s*[=:]\s*['"]?[^\s'"<>]{8,}['"]?`,
			Severity:    "high",
			Description: "API/Auth token assignment in configuration",
			MaskFunc:    maskKeyValue,
		},
		{
			Name:        "api_key_assignment",
			RegexStr:    `(?i)(?:api[_\-]?key|apikey)\s*[=:]\s*['"]?[^\s'"<>]{8,}['"]?`,
			Severity:    "medium",
			Description: "API key assignment in configuration",
			MaskFunc:    maskKeyValue,
		},
	}
}

// maskKeepPrefix returns a mask function that keeps the first `prefix` and last `suffix` characters,
// replacing the middle with asterisks.
func maskKeepPrefix(prefix, suffix int) func(string) string {
	return func(value string) string {
		runes := []rune(value)
		length := len(runes)
		if length <= prefix+suffix {
			return strings.Repeat("*", length)
		}
		return string(runes[:prefix]) + strings.Repeat("*", length-prefix-suffix) + string(runes[length-suffix:])
	}
}

// maskFixed returns a mask function that replaces the entire value with a fixed string.
func maskFixed(replacement string) func(string) string {
	return func(_ string) string {
		return replacement
	}
}

// maskKeyValue masks the value portion of a key=value pattern
func maskKeyValue(match string) string {
	// Find the separator (= or :)
	separators := []string{"=", ":"}
	for _, sep := range separators {
		idx := strings.Index(match, sep)
		if idx >= 0 {
			return match[:idx+1] + " [REDACTED]"
		}
	}
	return "[REDACTED]"
}
