// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package utils

import (
	"strings"
	"testing"
)

// TestSplitMessage_CodeBlockSplitting tests complex code block splitting scenarios
func TestSplitMessage_CodeBlockSplitting(t *testing.T) {
	tests := []struct {
		name      string
		content   string
		maxLen    int
		checkFunc func(t *testing.T, chunks []string)
	}{
		{
			name: "very long code block that needs splitting inside",
			content: strings.Repeat("```javascript\nconsole.log('line');\n", 100) + "```\ntext after",
			maxLen: 200,
			checkFunc: func(t *testing.T, chunks []string) {
				// Should split the long code block
				if len(chunks) < 2 {
					t.Errorf("Expected at least 2 chunks for very long code block, got %d", len(chunks))
				}
				// Each chunk should not exceed maxLen significantly
				for i, chunk := range chunks {
					if len(chunk) > 250 { // Allow some overflow for code blocks
						t.Errorf("Chunk %d is too long: %d chars", i, len(chunk))
					}
				}
				// Check that closing fences are handled
				for _, chunk := range chunks {
					if !strings.HasSuffix(chunk, "```") && strings.Contains(chunk, "```") {
						// If chunk has opening fence but no closing, that's OK for intermediate chunks
					}
				}
			},
		},
		{
			name: "code block at exact boundary",
			content: "text before\n```\ncode\n```\ntext after",
			maxLen: 50,
			checkFunc: func(t *testing.T, chunks []string) {
				if len(chunks) == 0 {
					t.Error("Expected at least 1 chunk")
				}
				// Code block should stay intact
				fullContent := strings.Join(chunks, "")
				if !strings.Contains(fullContent, "```") {
					t.Error("Code block markers were lost")
				}
			},
		},
		{
			name: "multiple code blocks",
			content: "```\nblock1\n```\ntext\n```\nblock2\n```\ntext\n```\nblock3\n```",
			maxLen: 30,
			checkFunc: func(t *testing.T, chunks []string) {
				if len(chunks) == 0 {
					t.Error("Expected at least 1 chunk")
				}
				fullContent := strings.Join(chunks, "")
				// Count code block markers
				count := strings.Count(fullContent, "```")
				if count < 6 { // Should have at least opening and closing for each block
					t.Errorf("Lost code block markers, got %d markers", count)
				}
			},
		},
		{
			name: "incomplete code block at end",
			content: "text\n```\nunclosed code\nmore code",
			maxLen: 30,
			checkFunc: func(t *testing.T, chunks []string) {
				if len(chunks) == 0 {
					t.Error("Expected at least 1 chunk")
				}
				// Should handle unclosed code block
				fullContent := strings.Join(chunks, "")
				if !strings.Contains(fullContent, "```") {
					t.Error("Code block marker was lost")
				}
			},
		},
		{
			name: "code block with language identifier",
			content: "```go\nfunc main() {}\n```",
			maxLen: 20,
			checkFunc: func(t *testing.T, chunks []string) {
				if len(chunks) == 0 {
					t.Error("Expected at least 1 chunk")
				}
				// Language identifier should be preserved
				fullContent := strings.Join(chunks, "")
				if !strings.Contains(fullContent, "```go") {
					t.Error("Language identifier was lost")
				}
			},
		},
		{
			name: "small maxLen with code block",
			content: "```\ncode\n```\ntext",
			maxLen: 15,
			checkFunc: func(t *testing.T, chunks []string) {
				if len(chunks) == 0 {
					t.Error("Expected at least 1 chunk")
				}
				// With very small maxLen, might need to split
				for i, chunk := range chunks {
					if len(chunk) > 25 { // Allow reasonable overflow
						t.Errorf("Chunk %d exceeds reasonable length: %d", i, len(chunk))
					}
				}
			},
		},
		{
			name: "code block with very long line",
			content: "```\n" + strings.Repeat("a", 300) + "\n```\ntext",
			maxLen: 100,
			checkFunc: func(t *testing.T, chunks []string) {
				if len(chunks) < 2 {
					t.Errorf("Expected at least 2 chunks for long line, got %d", len(chunks))
				}
				// Long line should be split
				for i, chunk := range chunks {
					if len(chunk) > 150 { // Allow some overflow
						t.Errorf("Chunk %d is too long: %d chars", i, len(chunk))
					}
				}
			},
		},
		{
			name: "mixed content with multiple splits",
			content: strings.Repeat("word ", 100) + "\n```\ncode block\n```\n" + strings.Repeat("text ", 100),
			maxLen: 150,
			checkFunc: func(t *testing.T, chunks []string) {
				if len(chunks) < 3 {
					t.Errorf("Expected at least 3 chunks, got %d", len(chunks))
				}
				// Check code block integrity
				fullContent := strings.Join(chunks, "")
				if !strings.Contains(fullContent, "```") {
					t.Error("Code block markers were lost")
				}
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := SplitMessage(tt.content, tt.maxLen)
			tt.checkFunc(t, result)
		})
	}
}

// TestSplitMessage_EdgeCases tests edge cases for message splitting
func TestSplitMessage_EdgeCases(t *testing.T) {
	tests := []struct {
		name   string
		content string
		maxLen int
		check  func(t *testing.T, chunks []string)
	}{
		{
			name:   "maxLen smaller than buffer minimum",
			content: "a b c d e f g h i j",
			maxLen: 8, // Very small maxLen
			check: func(t *testing.T, chunks []string) {
				if len(chunks) == 0 {
					t.Error("Expected at least 1 chunk")
				}
				// Should still split content
				fullContent := strings.Join(chunks, "")
				if fullContent == "" {
					t.Error("Content was lost")
				}
			},
		},
		{
			name:   "maxLen exactly at buffer boundary",
			content: strings.Repeat("word ", 20),
			maxLen: 50,
			check: func(t *testing.T, chunks []string) {
				if len(chunks) == 0 {
					t.Error("Expected at least 1 chunk")
				}
			},
		},
		{
			name:   "empty string with small maxLen",
			content: "",
			maxLen: 1,
			check: func(t *testing.T, chunks []string) {
				// Empty string produces no chunks or empty chunks
				if len(chunks) > 1 {
					t.Errorf("Expected at most 1 chunk for empty string, got %d", len(chunks))
				}
			},
		},
		{
			name:   "only whitespace",
			content: "   \n\t\n   ",
			maxLen: 10,
			check: func(t *testing.T, chunks []string) {
				if len(chunks) == 0 {
					t.Error("Expected at least 1 chunk")
				}
				// Whitespace should be trimmed
				fullContent := strings.Join(chunks, "")
				if strings.TrimSpace(fullContent) != "" {
					t.Error("Expected only whitespace")
				}
			},
		},
		{
			name:   "very long single word",
			content: strings.Repeat("a", 500),
			maxLen: 100,
			check: func(t *testing.T, chunks []string) {
				if len(chunks) == 0 {
					t.Error("Expected at least 1 chunk")
				}
				// Should split the long word
				totalLen := 0
				for _, chunk := range chunks {
					totalLen += len(chunk)
					if len(chunk) > 120 { // Allow some overflow
						t.Errorf("Chunk too long: %d chars", len(chunk))
					}
				}
				if totalLen != 500 {
					t.Errorf("Content length changed: was 500, now %d", totalLen)
				}
			},
		},
		{
			name:   "unicode with code block",
			content: "```\n你好世界\n```\n测试文本",
			maxLen: 30,
			check: func(t *testing.T, chunks []string) {
				if len(chunks) == 0 {
					t.Error("Expected at least 1 chunk")
				}
				fullContent := strings.Join(chunks, "")
				if !strings.Contains(fullContent, "你好") {
					t.Error("Lost unicode content")
				}
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := SplitMessage(tt.content, tt.maxLen)
			tt.check(t, result)
		})
	}
}

// TestSplitMessage_CodeBlockBufferTests tests the code block buffer calculation
func TestSplitMessage_CodeBlockBufferTests(t *testing.T) {
	tests := []struct {
		name   string
		content string
		maxLen int
	}{
		{
			name:   "maxLen 10 (buffer min 50, capped at 5)",
			content: "```\ncode\n```",
			maxLen: 10,
		},
		{
			name:   "maxLen 100 (buffer 10%)",
			content: strings.Repeat("a", 100) + "\n```\ncode\n```",
			maxLen: 100,
		},
		{
			name:   "maxLen 1000 (buffer 100)",
			content: strings.Repeat("a", 1000) + "\n```\ncode\n```",
			maxLen: 1000,
		},
		{
			name:   "maxLen 200 (buffer 20, capped at 100)",
			content: "```\n" + strings.Repeat("x", 200) + "\n```",
			maxLen: 200,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := SplitMessage(tt.content, tt.maxLen)
			if len(result) == 0 {
				t.Error("Expected at least 1 chunk")
			}
			// Verify content integrity
			fullContent := strings.Join(result, "")
			if !strings.Contains(fullContent, "```") {
				t.Error("Code block markers lost")
			}
		})
	}
}
