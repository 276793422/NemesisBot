package forge

import (
	"regexp"
	"strconv"
	"strings"
)

// ReportSanitizer cleans sensitive information from reflection reports
// before sharing them with remote cluster nodes.
type ReportSanitizer struct {
	sensitiveKeys []string
}

// NewReportSanitizer creates a sanitizer using the Forge config's SanitizeFields.
func NewReportSanitizer(config *ForgeConfig) *ReportSanitizer {
	keys := config.Collection.SanitizeFields
	if len(keys) == 0 {
		keys = []string{"api_key", "token", "password", "secret", "credential", "key"}
	}
	return &ReportSanitizer{
		sensitiveKeys: keys,
	}
}

// SanitizeReport applies all privacy filters to the report content.
func (s *ReportSanitizer) SanitizeReport(content string) string {
	result := content
	result = s.redactSensitiveValues(result)
	result = s.cleanPaths(result)
	result = s.cleanPublicIPs(result)
	return result
}

// redactSensitiveValues replaces values following sensitive key patterns with [REDACTED].
// Handles patterns like: key: value, key=value, "key": "value", key: "value"
func (s *ReportSanitizer) redactSensitiveValues(content string) string {
	for _, key := range s.sensitiveKeys {
		// Pattern: key followed by separator and a value
		// Matches: key: value, key=value, key: "value", key='value', "key": "value"
		pattern := `(?i)(` + regexp.QuoteMeta(key) + `)\s*[:=]\s*["']?([^\s"']+)["']?`
		re := regexp.MustCompile(pattern)
		content = re.ReplaceAllString(content, "${1}: [REDACTED]")
	}
	return content
}

// cleanPaths replaces absolute paths with relative paths.
// C:\Users\xxx\ → ~/ , /home/xxx/ → ~/
func (s *ReportSanitizer) cleanPaths(content string) string {
	// Windows paths: C:\Users\username\... → ~/...
	winPath := regexp.MustCompile(`[A-Za-z]:\\Users\\[^\\]+\\`)
	content = winPath.ReplaceAllString(content, "~/")

	// Unix paths: /home/username/... → ~/...
	unixPath := regexp.MustCompile(`/home/[^/]+/`)
	content = unixPath.ReplaceAllString(content, "~/")

	// General Windows absolute paths (non-Users): C:\path → /path
	winGeneral := regexp.MustCompile(`[A-Za-z]:\\`)
	content = winGeneral.ReplaceAllString(content, "/")

	return content
}

// cleanPublicIPs replaces public IP addresses with [IP].
// Private/internal IPs (10.x, 172.16-31.x, 192.168.x, 127.x) are preserved.
func (s *ReportSanitizer) cleanPublicIPs(content string) string {
	ipPattern := regexp.MustCompile(`\b(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})\b`)
	return ipPattern.ReplaceAllStringFunc(content, func(ip string) string {
		if isPrivateIP(ip) {
			return ip
		}
		return "[IP]"
	})
}

// isPrivateIP checks if an IP address is in a private/reserved range.
func isPrivateIP(ip string) bool {
	parts := strings.Split(ip, ".")
	if len(parts) != 4 {
		return false
	}

	// 127.x.x.x - loopback
	if parts[0] == "127" {
		return true
	}

	// 10.x.x.x - class A private
	if parts[0] == "10" {
		return true
	}

	// 192.168.x.x - class C private
	if parts[0] == "192" && parts[1] == "168" {
		return true
	}

	// 172.16.x.x - 172.31.x.x - class B private
	if parts[0] == "172" {
		second, err := strconv.Atoi(parts[1])
		if err != nil {
			return false
		}
		if second >= 16 && second <= 31 {
			return true
		}
	}

	return false
}

// NewReportSanitizerForTest creates a ReportSanitizer with the given config for testing.
func NewReportSanitizerForTest(config *ForgeConfig) *ReportSanitizer {
	return NewReportSanitizer(config)
}

// SanitizeReportForTest exposes SanitizeReport as a package-level function for testing.
func SanitizeReportForTest(config *ForgeConfig, content string) string {
	s := NewReportSanitizer(config)
	return s.SanitizeReport(content)
}

// RedactSensitiveValuesForTest exposes redactSensitiveValues for testing.
func RedactSensitiveValuesForTest(config *ForgeConfig, content string) string {
	s := NewReportSanitizer(config)
	return s.redactSensitiveValues(content)
}

// CleanPathsForTest exposes cleanPaths for testing.
func CleanPathsForTest(content string) string {
	s := &ReportSanitizer{}
	return s.cleanPaths(content)
}

// CleanPublicIPsForTest exposes cleanPublicIPs for testing.
func CleanPublicIPsForTest(content string) string {
	s := &ReportSanitizer{}
	return s.cleanPublicIPs(content)
}

// IsPrivateIPForTest exposes isPrivateIP for testing.
func IsPrivateIPForTest(ip string) bool {
	return isPrivateIP(ip)
}
