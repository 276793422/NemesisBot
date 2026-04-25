// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Tests for email.go pure helper functions (internal package for unexported access)

package channels

import (
	"testing"
)

// ---------------------------------------------------------------------------
// Email Channel Pure Helper Tests
// ---------------------------------------------------------------------------

func TestEmailChannel_ParseSearchResults(t *testing.T) {
	ch := &EmailChannel{}

	tests := []struct {
		name      string
		responses []string
		expected  []string
	}{
		{
			name:      "Empty responses",
			responses: nil,
			expected:  nil,
		},
		{
			name:      "No SEARCH responses",
			responses: []string{"* OK test", "NB00 OK done"},
			expected:  nil,
		},
		{
			name:      "Single SEARCH result",
			responses: []string{"* SEARCH 1 2 3"},
			expected:  []string{"1", "2", "3"},
		},
		{
			name:      "Multiple SEARCH responses",
			responses: []string{"* SEARCH 5 10", "* SEARCH 15"},
			expected:  []string{"5", "10", "15"},
		},
		{
			name:      "SEARCH with empty result",
			responses: []string{"* SEARCH"},
			expected:  nil,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ch.parseSearchResults(tt.responses)
			if len(result) != len(tt.expected) {
				t.Errorf("Expected %v, got %v", tt.expected, result)
				return
			}
			for i, v := range result {
				if v != tt.expected[i] {
					t.Errorf("At index %d: expected '%s', got '%s'", i, tt.expected[i], v)
				}
			}
		})
	}
}

func TestEmailChannel_ParseEmailHeaders(t *testing.T) {
	ch := &EmailChannel{}

	tests := []struct {
		name             string
		responses        []string
		expectedFrom     string
		expectedSubject  string
		expectedMsgID    string
	}{
		{
			name:      "Empty responses",
			responses: nil,
		},
		{
			name: "Standard headers",
			responses: []string{
				"From: John Doe <john@example.com>",
				"Subject: Test Subject",
				"Message-ID: <msg123@mail.example.com>",
			},
			expectedFrom:    "John Doe <john@example.com>",
			expectedSubject: "Test Subject",
			expectedMsgID:   "msg123@mail.example.com",
		},
		{
			name: "Headers in literal block",
			responses: []string{
				"* 1 FETCH (BODY[HEADER.FIELDS (SUBJECT FROM MESSAGE-ID)] {80}",
				"From: alice@example.com",
				"Subject: Hello World",
				"Message-ID: <abc@x.com>",
				")",
			},
			expectedFrom:    "alice@example.com",
			expectedSubject: "Hello World",
			expectedMsgID:   "abc@x.com",
		},
		{
			name: "Only From header",
			responses: []string{
				"From: bob@test.com",
			},
			expectedFrom: "bob@test.com",
		},
		{
			name: "Message-ID with angle brackets trimmed",
			responses: []string{
				"Message-ID: <id@host.com>",
			},
			expectedMsgID: "id@host.com",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			from, subject, msgID := ch.parseEmailHeaders(tt.responses)
			if from != tt.expectedFrom {
				t.Errorf("From: expected '%s', got '%s'", tt.expectedFrom, from)
			}
			if subject != tt.expectedSubject {
				t.Errorf("Subject: expected '%s', got '%s'", tt.expectedSubject, subject)
			}
			if msgID != tt.expectedMsgID {
				t.Errorf("Message-ID: expected '%s', got '%s'", tt.expectedMsgID, msgID)
			}
		})
	}
}

func TestEmailChannel_ParseEmailBody(t *testing.T) {
	ch := &EmailChannel{}

	tests := []struct {
		name      string
		responses []string
		expected  string
	}{
		{
			name:      "Nil responses",
			responses: nil,
			expected:  "",
		},
		{
			name:      "Empty responses",
			responses: []string{},
			expected:  "",
		},
		{
			name: "Simple body",
			responses: []string{
				"* 1 FETCH (BODY[TEXT] {11}",
				"Hello World",
				")",
			},
			expected: "Hello World",
		},
		{
			name: "FETCH command line skipped",
			responses: []string{
				"* 1 FETCH (BODY[TEXT] {5}",
				"Hello",
				")",
			},
			expected: "Hello",
		},
		{
			name: "Multiline body",
			responses: []string{
				"* 2 FETCH (BODY[TEXT] {20}",
				"Line 1",
				"Line 2",
				"Line 3",
				")",
			},
			expected: "Line 1\nLine 2\nLine 3",
		},
		{
			name: "Closing paren skipped",
			responses: []string{
				"body text",
				")",
			},
			expected: "body text",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ch.parseEmailBody(tt.responses)
			if result != tt.expected {
				t.Errorf("Expected '%s', got '%s'", tt.expected, result)
			}
		})
	}
}

func TestEmailChannel_ExtractEmailAddress(t *testing.T) {
	ch := &EmailChannel{}

	tests := []struct {
		name     string
		from     string
		expected string
	}{
		{
			name:     "Angle bracket format",
			from:     "John Doe <john@example.com>",
			expected: "john@example.com",
		},
		{
			name:     "Plain email",
			from:     "alice@example.com",
			expected: "alice@example.com",
		},
		{
			name:     "Just angle brackets",
			from:     "<bob@test.org>",
			expected: "bob@test.org",
		},
		{
			name:     "No email",
			from:     "Just a name",
			expected: "",
		},
		{
			name:     "Empty string",
			from:     "",
			expected: "",
		},
		{
			name:     "Whitespace padded",
			from:     "  user@domain.com  ",
			expected: "user@domain.com",
		},
		{
			name:     "Multiple angle brackets",
			from:     "Name <first> <real@email.com>",
			expected: "real@email.com",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ch.extractEmailAddress(tt.from)
			if result != tt.expected {
				t.Errorf("extractEmailAddress(%q) = '%s', expected '%s'", tt.from, result, tt.expected)
			}
		})
	}
}

func TestEmailChannel_IMAPQuote(t *testing.T) {
	ch := &EmailChannel{}

	tests := []struct {
		input    string
		expected string
	}{
		{"simple", "\"simple\""},
		{"has\"quote", "\"has\\\"quote\""},
		{"", "\"\""},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := ch.imapQuote(tt.input)
			if result != tt.expected {
				t.Errorf("imapQuote(%q) = '%s', expected '%s'", tt.input, result, tt.expected)
			}
		})
	}
}
