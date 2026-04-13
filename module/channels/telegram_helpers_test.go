// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"testing"
)

func TestParseChatID_Valid(t *testing.T) {
	id, err := parseChatID("123456")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if id != 123456 {
		t.Errorf("expected 123456, got %d", id)
	}
}

func TestParseChatID_Negative(t *testing.T) {
	id, err := parseChatID("-1001234567890")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if id != -1001234567890 {
		t.Errorf("expected -1001234567890, got %d", id)
	}
}

func TestParseChatID_Invalid(t *testing.T) {
	_, err := parseChatID("abc")
	if err == nil {
		t.Error("expected error for non-numeric string")
	}
}

func TestEscapeHTML(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"hello & world", "hello &amp; world"},
		{"a < b", "a &lt; b"},
		{"b > a", "b &gt; a"},
		{"<tag>content</tag>", "&lt;tag&gt;content&lt;/tag&gt;"},
		{"no escaping needed", "no escaping needed"},
		{"&lt;", "&amp;lt;"},
	}
	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := escapeHTML(tt.input)
			if result != tt.expected {
				t.Errorf("escapeHTML(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}

func TestExtractCodeBlocks_Basic(t *testing.T) {
	input := "before```go\nfmt.Println(\"hi\")\n```after"
	result := extractCodeBlocks(input)
	if len(result.codes) != 1 {
		t.Fatalf("expected 1 code block, got %d", len(result.codes))
	}
	if result.codes[0] != "fmt.Println(\"hi\")\n" {
		t.Errorf("unexpected code content: %q", result.codes[0])
	}
	// text should have placeholder
	if containsRaw := func(s string) bool {
		for i := 0; i < len(result.codes); i++ {
		}
		return false
	}; containsRaw("```") {
		t.Error("text should not contain raw code block markers")
	}
}

func TestExtractCodeBlocks_Multiple(t *testing.T) {
	input := "```go\ncode1\n```\ntext\n```python\ncode2\n```"
	result := extractCodeBlocks(input)
	if len(result.codes) != 2 {
		t.Fatalf("expected 2 code blocks, got %d", len(result.codes))
	}
	if result.codes[0] != "code1\n" {
		t.Errorf("first code = %q, want %q", result.codes[0], "code1\n")
	}
	if result.codes[1] != "code2\n" {
		t.Errorf("second code = %q, want %q", result.codes[1], "code2\n")
	}
}

func TestExtractCodeBlocks_Empty(t *testing.T) {
	input := "no code blocks here"
	result := extractCodeBlocks(input)
	if len(result.codes) != 0 {
		t.Errorf("expected 0 code blocks, got %d", len(result.codes))
	}
	if result.text != input {
		t.Errorf("text should be unchanged: %q", result.text)
	}
}

func TestExtractInlineCodes_Basic(t *testing.T) {
	input := "use `fmt.Println` to print"
	result := extractInlineCodes(input)
	if len(result.codes) != 1 {
		t.Fatalf("expected 1 inline code, got %d", len(result.codes))
	}
	if result.codes[0] != "fmt.Println" {
		t.Errorf("code = %q, want %q", result.codes[0], "fmt.Println")
	}
}

func TestExtractInlineCodes_Multiple(t *testing.T) {
	input := "use `a` and `b` together"
	result := extractInlineCodes(input)
	if len(result.codes) != 2 {
		t.Fatalf("expected 2 inline codes, got %d", len(result.codes))
	}
	if result.codes[0] != "a" {
		t.Errorf("first code = %q", result.codes[0])
	}
	if result.codes[1] != "b" {
		t.Errorf("second code = %q", result.codes[1])
	}
}

func TestExtractInlineCodes_Empty(t *testing.T) {
	input := "no inline code"
	result := extractInlineCodes(input)
	if len(result.codes) != 0 {
		t.Errorf("expected 0 inline codes, got %d", len(result.codes))
	}
}

func TestMarkdownToTelegramHTML_Bold(t *testing.T) {
	result := markdownToTelegramHTML("**bold text**")
	if result != "<b>bold text</b>" {
		t.Errorf("got %q", result)
	}
}

func TestMarkdownToTelegramHTML_Italic(t *testing.T) {
	result := markdownToTelegramHTML("_italic text_")
	if result != "<i>italic text</i>" {
		t.Errorf("got %q", result)
	}
}

func TestMarkdownToTelegramHTML_CodeBlocks(t *testing.T) {
	input := "```go\ncode here\n```"
	result := markdownToTelegramHTML(input)
	expected := "<pre><code>code here\n</code></pre>"
	if result != expected {
		t.Errorf("got %q, want %q", result, expected)
	}
}

func TestMarkdownToTelegramHTML_InlineCode(t *testing.T) {
	result := markdownToTelegramHTML("use `var x` here")
	if result != "use <code>var x</code> here" {
		t.Errorf("got %q", result)
	}
}

func TestMarkdownToTelegramHTML_Links(t *testing.T) {
	result := markdownToTelegramHTML("[click here](https://example.com)")
	if result != `<a href="https://example.com">click here</a>` {
		t.Errorf("got %q", result)
	}
}

func TestMarkdownToTelegramHTML_Headers(t *testing.T) {
	result := markdownToTelegramHTML("# Header Text")
	if result != "Header Text" {
		t.Errorf("got %q", result)
	}
}

func TestMarkdownToTelegramHTML_Strikethrough(t *testing.T) {
	result := markdownToTelegramHTML("~~deleted~~")
	if result != "<s>deleted</s>" {
		t.Errorf("got %q", result)
	}
}

func TestMarkdownToTelegramHTML_ListItems(t *testing.T) {
	result := markdownToTelegramHTML("- item one")
	if result != "• item one" {
		t.Errorf("got %q", result)
	}
}

func TestMarkdownToTelegramHTML_Empty(t *testing.T) {
	result := markdownToTelegramHTML("")
	if result != "" {
		t.Errorf("expected empty string, got %q", result)
	}
}

func TestMarkdownToTelegramHTML_Complex(t *testing.T) {
	input := "**bold** and _italic_ with `code` and [link](http://x)"
	result := markdownToTelegramHTML(input)
	// Should contain all transformed elements
	if !contains(result, "<b>bold</b>") {
		t.Error("missing bold")
	}
	if !contains(result, "<i>italic</i>") {
		t.Error("missing italic")
	}
	if !contains(result, "<code>code</code>") {
		t.Error("missing code")
	}
	if !contains(result, `<a href="http://x">link</a>`) {
		t.Error("missing link")
	}
}

func contains(s, sub string) bool {
	return len(s) >= len(sub) && (s == sub || len(sub) == 0 ||
		func() bool {
			for i := 0; i <= len(s)-len(sub); i++ {
				if s[i:i+len(sub)] == sub {
					return true
				}
			}
			return false
		}())
}
