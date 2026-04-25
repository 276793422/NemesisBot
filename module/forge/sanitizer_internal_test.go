package forge

import (
	"strings"
	"testing"
)

// --- ReportSanitizer internal tests ---

func TestReportSanitizer_RedactSensitiveValues(t *testing.T) {
	cfg := DefaultForgeConfig()
	s := NewReportSanitizer(cfg)

	tests := []struct {
		name     string
		input    string
		contains string // should NOT contain the value
		missing  string // should contain [REDACTED]
	}{
		{
			name:     "api_key with colon",
			input:    "api_key: sk-abc123def456",
			contains: "sk-abc123def456",
			missing:  "[REDACTED]",
		},
		{
			name:     "token with equals",
			input:    "token=ghp_x123456789",
			contains: "ghp_x123456789",
			missing:  "[REDACTED]",
		},
		{
			name:     "password with colon",
			input:    "password: mySecretPass123!",
			contains: "mySecretPass123",
			missing:  "[REDACTED]",
		},
		{
			name:     "secret_key with colon",
			input:    "secret_key: abc123xyz",
			contains: "abc123xyz",
			missing:  "[REDACTED]",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := s.redactSensitiveValues(tt.input)
			if strings.Contains(result, tt.contains) {
				t.Errorf("Sensitive value should be redacted in '%s', got '%s'", tt.input, result)
			}
			if !strings.Contains(result, tt.missing) {
				t.Errorf("Expected '%s' in result, got '%s'", tt.missing, result)
			}
		})
	}
}

func TestReportSanitizer_RedactMultipleInOneLine(t *testing.T) {
	cfg := DefaultForgeConfig()
	s := NewReportSanitizer(cfg)

	input := "api_key: sk-abc and token: ghp-xyz"
	result := s.redactSensitiveValues(input)

	count := strings.Count(result, "[REDACTED]")
	if count < 2 {
		t.Errorf("Expected at least 2 [REDACTED] occurrences, got %d: %s", count, result)
	}
}

func TestReportSanitizer_NoRedactionNeeded(t *testing.T) {
	cfg := DefaultForgeConfig()
	s := NewReportSanitizer(cfg)

	input := "The tool executed successfully with normal parameters."
	result := s.redactSensitiveValues(input)

	if strings.Contains(result, "[REDACTED]") {
		t.Errorf("Should not redact clean content, got: %s", result)
	}
}

func TestReportSanitizer_CleanPaths_Windows(t *testing.T) {
	s := &ReportSanitizer{}

	input := "File at C:\\Users\\admin\\project\\config.json"
	result := s.cleanPaths(input)

	if strings.Contains(result, "admin") {
		t.Errorf("Username should be removed, got: %s", result)
	}
	if !strings.Contains(result, "~/") {
		t.Errorf("Should contain ~/, got: %s", result)
	}
}

func TestReportSanitizer_CleanPaths_Unix(t *testing.T) {
	s := &ReportSanitizer{}

	input := "File at /home/alice/project/config.json"
	result := s.cleanPaths(input)

	if strings.Contains(result, "alice") {
		t.Errorf("Username should be removed, got: %s", result)
	}
	if !strings.Contains(result, "~/") {
		t.Errorf("Should contain ~/, got: %s", result)
	}
}

func TestReportSanitizer_CleanPaths_GeneralWindows(t *testing.T) {
	s := &ReportSanitizer{}

	input := "Path: D:\\Projects\\data\\file.txt"
	result := s.cleanPaths(input)

	if strings.Contains(result, "D:\\") {
		t.Errorf("General Windows path should be cleaned, got: %s", result)
	}
}

func TestReportSanitizer_CleanPublicIPs_Public(t *testing.T) {
	s := &ReportSanitizer{}

	input := "Server at 8.8.8.8 responded, also 203.0.113.50"
	result := s.cleanPublicIPs(input)

	if strings.Contains(result, "8.8.8.8") {
		t.Errorf("Public IP should be replaced, got: %s", result)
	}
	if !strings.Contains(result, "[IP]") {
		t.Errorf("Should contain [IP], got: %s", result)
	}
}

func TestReportSanitizer_CleanPublicIPs_Private(t *testing.T) {
	s := &ReportSanitizer{}

	privateIPs := []string{"127.0.0.1", "10.0.0.1", "192.168.1.1", "172.16.0.1"}
	for _, ip := range privateIPs {
		result := s.cleanPublicIPs(ip)
		if result != ip {
			t.Errorf("Private IP %s should be preserved, got: %s", ip, result)
		}
	}
}

func TestReportSanitizer_CleanPublicIPs_PublicReplaced(t *testing.T) {
	s := &ReportSanitizer{}

	publicIPs := []string{"1.2.3.4", "8.8.8.8", "172.15.0.1", "172.32.0.1"}
	for _, ip := range publicIPs {
		result := s.cleanPublicIPs(ip)
		if result == ip {
			t.Errorf("Public IP %s should be replaced, got: %s", ip, result)
		}
		if result != "[IP]" {
			t.Errorf("Public IP %s should become [IP], got: %s", ip, result)
		}
	}
}

func TestReportSanitizer_SanitizeReport_FullPipeline(t *testing.T) {
	cfg := DefaultForgeConfig()
	s := NewReportSanitizer(cfg)

	input := `# Report
User api_key: sk-secret123 at C:\Users\admin\project
Server at 203.0.113.50 responded, internal 10.0.0.1 is fine.
Password: super_secret_pass`

	result := s.SanitizeReport(input)

	if strings.Contains(result, "sk-secret123") {
		t.Error("API key should be redacted")
	}
	if strings.Contains(result, "admin") {
		t.Error("Username should be cleaned from path")
	}
	if strings.Contains(result, "203.0.113.50") {
		t.Error("Public IP should be replaced")
	}
	if strings.Contains(result, "super_secret_pass") {
		t.Error("Password should be redacted")
	}
	if !strings.Contains(result, "10.0.0.1") {
		t.Error("Private IP should be preserved")
	}
}

func TestIsPrivateIP_EdgeCases(t *testing.T) {
	tests := []struct {
		ip       string
		expected bool
	}{
		{"not_an_ip", false},
		{"1.2.3", false},
		{"", false},
		{"0.0.0.0", false},
		{"255.255.255.255", false},
		{"172.16.0.0", true},
		{"172.31.255.255", true},
		{"172.15.255.255", false},
		{"172.32.0.0", false},
	}

	for _, tt := range tests {
		result := isPrivateIP(tt.ip)
		if result != tt.expected {
			t.Errorf("isPrivateIP(%q) = %v, expected %v", tt.ip, result, tt.expected)
		}
	}
}

func TestReportSanitizer_CustomFields(t *testing.T) {
	cfg := DefaultForgeConfig()
	cfg.Collection.SanitizeFields = []string{"custom_secret"}
	s := NewReportSanitizer(cfg)

	input := "custom_secret: myvalue123"
	result := s.redactSensitiveValues(input)

	if strings.Contains(result, "myvalue123") {
		t.Errorf("Custom field should be redacted, got: %s", result)
	}
	if !strings.Contains(result, "[REDACTED]") {
		t.Errorf("Expected [REDACTED], got: %s", result)
	}
}

func TestReportSanitizer_EmptyFields(t *testing.T) {
	cfg := DefaultForgeConfig()
	cfg.Collection.SanitizeFields = []string{}
	s := NewReportSanitizer(cfg)

	// With empty fields, NewReportSanitizer sets defaults
	input := "api_key: secret123"
	result := s.redactSensitiveValues(input)

	// Default fields include "api_key"
	if strings.Contains(result, "secret123") {
		t.Errorf("Default fields should include api_key, got: %s", result)
	}
}
