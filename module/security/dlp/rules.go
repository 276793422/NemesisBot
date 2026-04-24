// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package dlp

// builtinRules defines the built-in DLP detection rules.
// Each rule has a name, description, category, severity, and Go regex pattern.
// Rules cover financial data, credentials, PII, and network identifiers.
var builtinRules = []Rule{
	// -----------------------------------------------------------------------
	// Credit card numbers
	// -----------------------------------------------------------------------
	{
		Name:        "credit_card_visa",
		Description: "Visa credit card number (13 or 16 digits starting with 4)",
		Category:    "financial",
		Severity:    "high",
		Pattern:     `\b4\d{12}(\d{3})?\b`,
	},
	{
		Name:        "credit_card_mastercard",
		Description: "Mastercard number (16 digits, prefix 51-55 or 2221-2720)",
		Category:    "financial",
		Severity:    "high",
		Pattern:     `\b(?:5[1-5]\d{2}|222[1-9]|22[3-9]\d|2[3-6]\d{2}|27[01]\d|2720)\d{12}\b`,
	},
	{
		Name:        "credit_card_amex",
		Description: "American Express card number (15 digits, prefix 34 or 37)",
		Category:    "financial",
		Severity:    "high",
		Pattern:     `\b3[47]\d{13}\b`,
	},
	{
		Name:        "credit_card_discover",
		Description: "Discover card number (16 digits, prefix 6011, 622126-622925, 644-649, or 65)",
		Category:    "financial",
		Severity:    "high",
		Pattern:     `\b(?:6011|65\d{2}|64[4-9]\d|622(?:12[6-9]|1[3-9]\d|[2-8]\d{2}|9[01]\d|92[0-5]))\d{12}\b`,
	},
	{
		Name:        "credit_card_jcb",
		Description: "JCB card number (16 digits, prefix 3528-3589)",
		Category:    "financial",
		Severity:    "high",
		Pattern:     `\b(?:352[89]|35[3-8]\d)\d{12}\b`,
	},
	{
		Name:        "credit_card_diners",
		Description: "Diners Club card number (14-16 digits, prefix 300-305, 36, or 38)",
		Category:    "financial",
		Severity:    "high",
		Pattern:     `\b(?:3(?:0[0-5]|[68]\d))\d{11,13}\b`,
	},

	// -----------------------------------------------------------------------
	// API keys and tokens
	// -----------------------------------------------------------------------
	{
		Name:        "aws_access_key",
		Description: "AWS Access Key ID (starts with AKIA, 20 chars)",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `(?:A3T[A-Z0-9]|AKIA|AGPA|AIDA|AROA|AIPA|ANPA|ANVA|ASIA)[A-Z0-9]{16}`,
	},
	{
		Name:        "aws_secret_key",
		Description: "AWS Secret Access Key (40-char base64)",
		Category:    "credentials",
		Severity:    "high",
		Pattern: `(?i)aws[_\-]?secret[_\-]?access[_\-]?key\s*[=:]\s*[A-Za-z0-9/+=]{40}`,
	},
	{
		Name:        "google_api_key",
		Description: "Google API key (starts with AIza)",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `AIza[0-9A-Za-z\-_]{35}`,
	},
	{
		Name:        "google_oauth_token",
		Description: "Google OAuth access token (starts with ya29.)",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `ya29\.[0-9A-Za-z\-_]+`,
	},
	{
		Name:        "azure_api_key",
		Description: "Azure API key or subscription key pattern",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `(?i)azure[_\-]?(?:api|subscription)[_\-]?key\s*[=:]\s*[A-Za-z0-9\-_]{32,}`,
	},
	{
		Name:        "generic_hex_key",
		Description: "Generic high-entropy hex key (32+ hex chars in assignment context)",
		Category:    "credentials",
		Severity:    "medium",
		Pattern:     `(?i)(?:api[_\-]?key|apikey|secret|token|password|auth[_\-]?key)\s*[=:]\s*[0-9a-f]{32,}`,
	},
	{
		Name:        "generic_base64_key",
		Description: "Generic base64-encoded key (40+ chars in assignment context)",
		Category:    "credentials",
		Severity:    "medium",
		Pattern:     `(?i)(?:api[_\-]?key|apikey|secret|token|password|auth[_\-]?key)\s*[=:]\s*[A-Za-z0-9+/=]{40,}`,
	},

	// -----------------------------------------------------------------------
	// Private keys
	// -----------------------------------------------------------------------
	{
		Name:        "private_key_rsa",
		Description: "RSA private key in PEM format",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `-----BEGIN RSA PRIVATE KEY-----`,
	},
	{
		Name:        "private_key_generic",
		Description: "Generic private key in PEM format (EC, DSA, etc.)",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `-----BEGIN PRIVATE KEY-----`,
	},
	{
		Name:        "private_key_openssh",
		Description: "OpenSSH private key",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `-----BEGIN OPENSSH PRIVATE KEY-----`,
	},
	{
		Name:        "private_key_pkcs8",
		Description: "PKCS#8 encrypted private key",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `-----BEGIN ENCRYPTED PRIVATE KEY-----`,
	},

	// -----------------------------------------------------------------------
	// PII: Personal Identifiers
	// -----------------------------------------------------------------------
	{
		Name:        "us_ssn",
		Description: "US Social Security Number (XXX-XX-XXXX format)",
		Category:    "pii",
		Severity:    "high",
		Pattern:     `\b\d{3}-\d{2}-\d{4}\b`,
	},
	{
		Name:        "email_address",
		Description: "Email address",
		Category:    "pii",
		Severity:    "low",
		Pattern:     `\b[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}\b`,
	},
	{
		Name:        "phone_international",
		Description: "International phone number (E.164 or common formats)",
		Category:    "pii",
		Severity:    "low",
		Pattern:     `(?:\+?\d{1,3}[\s\-.]?)?\(?\d{2,4}\)?[\s\-.]?\d{3,4}[\s\-.]?\d{3,4}`,
	},

	// -----------------------------------------------------------------------
	// Network identifiers
	// -----------------------------------------------------------------------
	{
		Name:        "ip_address_private",
		Description: "Private/internal IP address (RFC 1918)",
		Category:    "network",
		Severity:    "low",
		Pattern:     `\b(?:10\.\d{1,3}\.\d{1,3}\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3}|192\.168\.\d{1,3}\.\d{1,3})\b`,
	},
	{
		Name:        "ip_address_public",
		Description: "Public IP address (non-private, non-reserved)",
		Category:    "network",
		Severity:    "medium",
		Pattern:     `\b(?:[1-9]\d?|1\d\d|2[01]\d|22[0-3])(?:\.\d{1,3}){3}\b`,
	},

	// -----------------------------------------------------------------------
	// Financial: Bank accounts
	// -----------------------------------------------------------------------
	{
		Name:        "bank_account_number",
		Description: "Bank account number (8-17 digits, often in IBAN context)",
		Category:    "financial",
		Severity:    "high",
		Pattern:     `\b(?:account[_\s\-]?number|acct|iban|swift|bic)\s*[=:]\s*[A-Z0-9]{8,17}\b`,
	},
	{
		Name:        "iban",
		Description: "International Bank Account Number (IBAN)",
		Category:    "financial",
		Severity:    "high",
		Pattern:     `\b[A-Z]{2}\d{2}[A-Z0-9]{4}\d{7}(?:[A-Z0-9]?){0,16}\b`,
	},

	// -----------------------------------------------------------------------
	// Tokens and connection strings
	// -----------------------------------------------------------------------
	{
		Name:        "jwt_token",
		Description: "JSON Web Token (three base64url segments separated by dots)",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `\beyJ[A-Za-z0-9\-_]+\.eyJ[A-Za-z0-9\-_]+\.[A-Za-z0-9\-_]+\b`,
	},
	{
		Name:        "database_connection_string",
		Description: "Database connection string with embedded credentials",
		Category:    "credentials",
		Severity:    "high",
		Pattern: `(?i)(?:mysql|postgres|postgresql|mongodb|redis|mssql|sqlserver|oracle)` +
			`://[^\s'"]+:[^\s'"]+@[^\s'"]+`,
	},
	{
		Name:        "github_token",
		Description: "GitHub personal access token (ghp_ or gho_ prefix)",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `gh[ps]_[A-Za-z0-9_]{36,}`,
	},
	{
		Name:        "slack_token",
		Description: "Slack token (xoxb-, xoxp-, xoxa- prefix)",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `xox[bopsa]-[0-9]{10,13}-[0-9]{10,13}-[a-zA-Z0-9]{24,34}`,
	},
	{
		Name:        "stripe_key",
		Description: "Stripe secret or publishable key",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `(?:sk|pk)_(?:test_|live_)[A-Za-z0-9]{24,}`,
	},

	// -----------------------------------------------------------------------
	// Generic secrets patterns
	// -----------------------------------------------------------------------
	{
		Name:        "secret_password_assignment",
		Description: "Password assignment in config/code (password=, pwd=, passwd=)",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `(?i)(?:password|passwd|pwd)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?`,
	},
	{
		Name:        "secret_token_assignment",
		Description: "Token assignment in config/code (token=, bearer=, access_token=)",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `(?i)(?:token|bearer|access[_\-]?token|auth[_\-]?token|refresh[_\-]?token)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?`,
	},
	{
		Name:        "secret_key_assignment",
		Description: "Secret assignment in config/code (secret=, secret_key=, client_secret=)",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `(?i)(?:secret[_\-]?key|client[_\-]?secret|shared[_\-]?secret|encryption[_\-]?key)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?`,
	},
	{
		Name:        "authorization_header",
		Description: "HTTP Authorization header with Bearer or Basic credentials",
		Category:    "credentials",
		Severity:    "high",
		Pattern:     `(?i)authorization\s*:\s*(?:bearer|basic)\s+[A-Za-z0-9\-_.~+/]+=*`,
	},

	// -----------------------------------------------------------------------
	// Additional identifiers
	// -----------------------------------------------------------------------
	{
		Name:        "ip_address_ipv6",
		Description: "IPv6 address",
		Category:    "network",
		Severity:    "low",
		Pattern:     `(?:[0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}`,
	},
}
