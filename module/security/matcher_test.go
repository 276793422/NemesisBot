// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package security

import (
	"testing"
)

func TestMatchPattern(t *testing.T) {
	tests := []struct {
		name     string
		pattern  string
		target   string
		expected bool
	}{
		// Exact match
		{
			name:     "exact match - simple path",
			pattern:  "/etc/passwd",
			target:   "/etc/passwd",
			expected: true,
		},
		{
			name:     "exact match - Windows path",
			pattern:  "C:/Windows/System32/hosts",
			target:   "C:/Windows/System32/hosts",
			expected: true,
		},
		{
			name:     "exact match - different path",
			pattern:  "/etc/passwd",
			target:   "/etc/shadow",
			expected: false,
		},

		// Single wildcard (*)
		{
			name:     "single wildcard - extension match",
			pattern:  "*.key",
			target:   "test.key",
			expected: true,
		},
		{
			name:     "single wildcard - global pattern matches in directory",
			pattern:  "*.key",
			target:   "/home/user/test.key",
			expected: true,
		},
		{
			name:     "single wildcard - directory level",
			pattern:  "/home/*.txt",
			target:   "/home/user.txt",
			expected: true,
		},
		{
			name:     "single wildcard - no cross directory",
			pattern:  "/home/*.txt",
			target:   "/home/user/test.txt",
			expected: false,
		},
		{
			name:     "single wildcard - Windows path",
			pattern:  "D:/123/*.key",
			target:   "D:/123/test.key",
			expected: true,
		},

		// Double wildcard (**)
		{
			name:     "double wildcard - recursive match",
			pattern:  "D:/123/**.key",
			target:   "D:/123/test.key",
			expected: true,
		},
		{
			name:     "double wildcard - nested directory",
			pattern:  "D:/123/**.key",
			target:   "D:/123/subdir/test.key",
			expected: true,
		},
		{
			name:     "double wildcard - deeply nested",
			pattern:  "D:/123/**.key",
			target:   "D:/123/a/b/c/test.key",
			expected: true,
		},

		// Path normalization
		{
			name:     "Windows backslashes normalized",
			pattern:  "C:/Windows/*",
			target:   "C:\\Windows\\test.txt",
			expected: true,
		},
		{
			name:     "mixed slashes",
			pattern:  "C:/Windows/*/test",
			target:   "C:\\Windows\\sub\\test",
			expected: true,
		},

		// Special cases
		{
			name:     "pattern without wildcards and no separator",
			pattern:  "passwd",
			target:   "passwd",
			expected: true,
		},
		{
			name:     "empty pattern",
			pattern:  "",
			target:   "/etc/passwd",
			expected: false,
		},
		{
			name:     "empty target",
			pattern:  "/etc/passwd",
			target:   "",
			expected: false,
		},
		{
			name:     "wildcard at start",
			pattern:  "*.txt",
			target:   "test.txt",
			expected: true,
		},
		{
			name:     "wildcard at end",
			pattern:  "/etc/*",
			target:   "/etc/passwd",
			expected: true,
		},
		{
			name:     "multiple wildcards",
			pattern:  "*/*.txt",
			target:   "home/test.txt",
			expected: true,
		},
		{
			name:     "mixed single and double wildcards",
			pattern:  "/home/*/**/*.txt",
			target:   "/home/user/sub/test.txt",
			expected: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := MatchPattern(tt.pattern, tt.target)
			if result != tt.expected {
				t.Errorf("MatchPattern(%q, %q) = %v, want %v", tt.pattern, tt.target, result, tt.expected)
			}
		})
	}
}

func TestMatchCommandPattern(t *testing.T) {
	tests := []struct {
		name     string
		pattern  string
		command  string
		expected bool
	}{
		{
			name:     "exact command match",
			pattern:  "git status",
			command:  "git status",
			expected: true,
		},
		{
			name:     "wildcard matches arguments",
			pattern:  "git *",
			command:  "git status",
			expected: true,
		},
		{
			name:     "wildcard matches multiple arguments",
			pattern:  "git *",
			command:  "git commit -m 'message'",
			expected: true,
		},
		{
			name:     "wildcard at start",
			pattern:  "*sudo*",
			command:  "sudo apt-get install",
			expected: true,
		},
		{
			name:     "wildcard in middle",
			pattern:  "rm *",
			command:  "rm -rf /tmp/test",
			expected: true,
		},
		{
			name:     "no wildcard - exact match required",
			pattern:  "ls",
			command:  "ls -la",
			expected: false,
		},
		{
			name:     "different command",
			pattern:  "git *",
			command:  "svn status",
			expected: false,
		},
		{
			name:     "case sensitive",
			pattern:  "GIT *",
			command:  "git status",
			expected: false,
		},
		{
			name:     "special regex characters escaped",
			pattern:  "test[1].sh",
			command:  "test[1].sh",
			expected: true,
		},
		{
			name:     "empty pattern",
			pattern:  "",
			command:  "test",
			expected: false,
		},
		{
			name:     "empty command",
			pattern:  "test *",
			command:  "",
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := MatchCommandPattern(tt.pattern, tt.command)
			if result != tt.expected {
				t.Errorf("MatchCommandPattern(%q, %q) = %v, want %v", tt.pattern, tt.command, result, tt.expected)
			}
		})
	}
}

func TestMatchDomainPattern(t *testing.T) {
	tests := []struct {
		name     string
		pattern  string
		domain   string
		expected bool
	}{
		{
			name:     "exact match",
			pattern:  "github.com",
			domain:   "github.com",
			expected: true,
		},
		{
			name:     "wildcard subdomain",
			pattern:  "*.github.com",
			domain:   "api.github.com",
			expected: true,
		},
		{
			name:     "wildcard matches single subdomain level only",
			pattern:  "*.github.com",
			domain:   "raw.githubusercontent.com",
			expected: false, // * matches only single level (anything except dot)
		},
		{
			name:     "exact match no wildcard",
			pattern:  "github.com",
			domain:   "api.github.com",
			expected: false,
		},
		{
			name:     "case insensitive",
			pattern:  "*.GitHub.com",
			domain:   "api.github.com",
			expected: true,
		},
		{
			name:     "different domain",
			pattern:  "*.github.com",
			domain:   "gitlab.com",
			expected: false,
		},
		{
			name:     "single wildcard",
			pattern:  "*.openai.com",
			domain:   "api.openai.com",
			expected: true,
		},
		{
			name:     "empty pattern",
			pattern:  "",
			domain:   "github.com",
			expected: false,
		},
		{
			name:     "empty domain",
			pattern:  "*.github.com",
			domain:   "",
			expected: false,
		},
		{
			name:     "wildcard only",
			pattern:  "*",
			domain:   "github",
			expected: true,
		},
		{
			name:     "literal dots in pattern",
			pattern:  "api.github.com",
			domain:   "apighithubcom",
			expected: false,
		},
		{
			name:     "multiple wildcards",
			pattern:  "*.*.github.com",
			domain:   "api.raw.github.com",
			expected: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := MatchDomainPattern(tt.pattern, tt.domain)
			if result != tt.expected {
				t.Errorf("MatchDomainPattern(%q, %q) = %v, want %v", tt.pattern, tt.domain, result, tt.expected)
			}
		})
	}
}

func TestWildcardToRegex(t *testing.T) {
	tests := []struct {
		name     string
		pattern  string
		expected string
	}{
		{
			name:     "single wildcard",
			pattern:  "*.txt",
			expected: "^[^/]*\\.txt$",
		},
		{
			name:     "double wildcard",
			pattern:  "**.txt",
			expected: "^.*\\.txt$",
		},
		{
			name:     "mixed wildcards",
			pattern:  "/home/*/*.txt",
			expected: "^/home/[^/]*/[^/]*\\.txt$",
		},
		{
			name:     "no wildcards",
			pattern:  "/etc/passwd",
			expected: "^/etc/passwd$",
		},
		{
			name:     "special regex characters escaped",
			pattern:  "test[1].txt",
			expected: "^test\\[1\\]\\.txt$",
		},
		{
			name:     "plus sign escaped",
			pattern:  "test+",
			expected: "^test\\+$",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := wildcardToRegex(tt.pattern)
			if result != tt.expected {
				t.Errorf("wildcardToRegex(%q) = %q, want %q", tt.pattern, result, tt.expected)
			}
		})
	}
}

func TestNormalizePath(t *testing.T) {
	tests := []struct {
		name     string
		path     string
		expected string
	}{
		{
			name:     "forward slashes unchanged",
			path:     "/home/user/test.txt",
			expected: "/home/user/test.txt",
		},
		{
			name:     "backslashes converted",
			path:     "C:\\Windows\\test.txt",
			expected: "C:/Windows/test.txt",
		},
		{
			name:     "mixed slashes normalized",
			path:     "C:/Windows\\test.txt",
			expected: "C:/Windows/test.txt",
		},
		{
			name:     "empty string",
			path:     "",
			expected: "",
		},
		{
			name:     "UNC path",
			path:     "\\\\server\\share\\file.txt",
			expected: "//server/share/file.txt",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := normalizePath(tt.path)
			if result != tt.expected {
				t.Errorf("normalizePath(%q) = %q, want %q", tt.path, result, tt.expected)
			}
		})
	}
}

func BenchmarkMatchPattern(b *testing.B) {
	for i := 0; i < b.N; i++ {
		MatchPattern("*.key", "/home/user/test.key")
	}
}

func BenchmarkMatchCommandPattern(b *testing.B) {
	for i := 0; i < b.N; i++ {
		MatchCommandPattern("git *", "git status")
	}
}

func BenchmarkMatchDomainPattern(b *testing.B) {
	for i := 0; i < b.N; i++ {
		MatchDomainPattern("*.github.com", "api.github.com")
	}
}
