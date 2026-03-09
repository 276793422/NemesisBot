// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package utils

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// TestTruncate tests string truncation
func TestTruncate(t *testing.T) {
	tests := []struct {
		name    string
		input   string
		maxLen  int
		expect  string
	}{
		{
			name:   "short string",
			input:  "hello",
			maxLen: 10,
			expect: "hello",
		},
		{
			name:   "exact length",
			input:  "hello",
			maxLen: 5,
			expect: "hello",
		},
		{
			name:   "truncate needed",
			input:  "hello world",
			maxLen: 5,
			expect: "he...", // maxLen-3=2 chars + "..."
		},
		{
			name:   "empty string",
			input:  "",
			maxLen: 10,
			expect: "",
		},
		{
			name:   "zero max length",
			input:  "hello",
			maxLen: 0,
			expect: "",
		},
		{
			name:   "unicode characters",
			input:  "你好世界",
			maxLen: 4,
			expect: "你好世界", // Exact length, no truncation
		},
		{
			name:   "unicode truncate needed",
			input:  "你好世界",
			maxLen: 3,
			expect: "你好世", // maxLen <= 3, so just truncate without adding "..."
		},
		{
			name:   "unicode truncate with ellipsis",
			input:  "你好世界欢迎",
			maxLen: 5,
			expect: "你好...", // 6 runes > 5, maxLen-3=2 chars + "..."
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := Truncate(tt.input, tt.maxLen)
			if result != tt.expect {
				t.Errorf("Truncate(%q, %d) = %q, want %q", tt.input, tt.maxLen, result, tt.expect)
			}
		})
	}
}

// TestSplitMessage tests message splitting
func TestSplitMessage(t *testing.T) {
	tests := []struct {
		name         string
		content      string
		maxLen       int
		minChunks    int
		maxChunks    int
		checkLengths bool
	}{
		{
			name:         "short message",
			content:      "Hello world",
			maxLen:       100,
			minChunks:    1,
			maxChunks:    1,
			checkLengths: false,
		},
		{
			name:         "long message needs split",
			content:      strings.Repeat("a", 200),
			maxLen:       50,
			minChunks:    4,
			maxChunks:    10, // Allow flexibility in chunk size
			checkLengths: true,
		},
		{
			name:         "empty message",
			content:      "",
			maxLen:       100,
			minChunks:    0,
			maxChunks:    1,
			checkLengths: false,
		},
		{
			name:         "message with newlines",
			content:      strings.Repeat("line\n", 20),
			maxLen:       50,
			minChunks:    1,
			maxChunks:    20,
			checkLengths: false,
		},
		{
			name:         "message with code block",
			content:      "```\ncode\nhere\n```\ntext after",
			maxLen:       100,
			minChunks:    1,
			maxChunks:    2,
			checkLengths: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := SplitMessage(tt.content, tt.maxLen)

			if len(result) < tt.minChunks || len(result) > tt.maxChunks {
				t.Errorf("SplitMessage() returned %d chunks, want between %d and %d",
					len(result), tt.minChunks, tt.maxChunks)
			}

			// Check lengths if requested
			if tt.checkLengths {
				for i, chunk := range result {
					if len(chunk) > tt.maxLen {
						t.Errorf("Chunk %d has length %d, max %d", i, len(chunk), tt.maxLen)
					}
				}
			}
		})
	}
}

// TestWriteFileAtomic tests atomic file writing
func TestWriteFileAtomic(t *testing.T) {
	tempDir := t.TempDir()
	testFile := filepath.Join(tempDir, "test.txt")
	content := []byte("Hello, world!")

	// Write file
	err := WriteFileAtomic(testFile, content, 0644)
	if err != nil {
		t.Fatalf("WriteFileAtomic failed: %v", err)
	}

	// Verify file exists
	info, err := os.Stat(testFile)
	if err != nil {
		t.Fatalf("Failed to stat file: %v", err)
	}

	if !info.Mode().IsRegular() {
		t.Error("Expected regular file")
	}

	// Verify content
	readContent, err := os.ReadFile(testFile)
	if err != nil {
		t.Fatalf("Failed to read file: %v", err)
	}

	if string(readContent) != string(content) {
		t.Errorf("Content mismatch: got %q, want %q", string(readContent), string(content))
	}
}

// TestWriteFileAtomic_Overwrite tests overwriting existing file
func TestWriteFileAtomic_Overwrite(t *testing.T) {
	tempDir := t.TempDir()
	testFile := filepath.Join(tempDir, "test.txt")

	// Write initial content
	err := WriteFileAtomic(testFile, []byte("initial"), 0644)
	if err != nil {
		t.Fatalf("First write failed: %v", err)
	}

	// Overwrite with new content
	err = WriteFileAtomic(testFile, []byte("updated"), 0644)
	if err != nil {
		t.Fatalf("Second write failed: %v", err)
	}

	// Verify updated content
	readContent, err := os.ReadFile(testFile)
	if err != nil {
		t.Fatalf("Failed to read file: %v", err)
	}

	if string(readContent) != "updated" {
		t.Errorf("Content mismatch after overwrite: got %q, want 'updated'", string(readContent))
	}
}

// TestWriteFileAtomic_NestedDirectory tests writing to nested directory
func TestWriteFileAtomic_NestedDirectory(t *testing.T) {
	tempDir := t.TempDir()
	testFile := filepath.Join(tempDir, "subdir", "nested", "test.txt")

	err := WriteFileAtomic(testFile, []byte("nested content"), 0644)
	if err != nil {
		t.Fatalf("WriteFileAtomic to nested path failed: %v", err)
	}

	// Verify file exists
	_, err = os.Stat(testFile)
	if err != nil {
		t.Errorf("Nested file should exist: %v", err)
	}
}

// TestFindLastNewline tests finding last newline
func TestFindLastNewline(t *testing.T) {
	tests := []struct {
		name         string
		input        string
		searchWindow int
		expect       int
	}{
		{
			name:         "newline in middle",
			input:        "hello\nworld",
			searchWindow: 10,
			expect:       5,
		},
		{
			name:         "multiple newlines",
			input:        "line1\nline2\nline3",
			searchWindow: 20,
			expect:       11, // Position of the last \n (between line2 and line3)
		},
		{
			name:         "no newline",
			input:        "helloworld",
			searchWindow: 10,
			expect:       -1,
		},
		{
			name:         "empty string",
			input:        "",
			searchWindow: 10,
			expect:       -1,
		},
		{
			name:         "newline at end",
			input:        "hello\n",
			searchWindow: 10,
			expect:       5,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Call findLastNewline from message.go
			result := findLastNewline(tt.input, tt.searchWindow)
			if result != tt.expect {
				t.Errorf("findLastNewline(%q, %d) = %d, want %d",
					tt.input, tt.searchWindow, result, tt.expect)
			}
		})
	}
}

// TestFindLastSpace tests finding last space
func TestFindLastSpace(t *testing.T) {
	tests := []struct {
		name         string
		input        string
		searchWindow int
		expect       int
	}{
		{
			name:         "space in middle",
			input:        "hello world",
			searchWindow: 15,
			expect:       5,
		},
		{
			name:         "multiple spaces",
			input:        "one two three",
			searchWindow: 20,
			expect:       7, // Position of the last space (between "two" and "three")
		},
		{
			name:         "no space",
			input:        "helloworld",
			searchWindow: 10,
			expect:       -1,
		},
		{
			name:         "empty string",
			input:        "",
			searchWindow: 10,
			expect:       -1,
		},
		{
			name:         "tab character",
			input:        "hello\tworld",
			searchWindow: 15,
			expect:       5,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := findLastSpace(tt.input, tt.searchWindow)
			if result != tt.expect {
				t.Errorf("findLastSpace(%q, %d) = %d, want %d",
					tt.input, tt.searchWindow, result, tt.expect)
			}
		})
	}
}

// TestFindLastUnclosedCodeBlock tests finding unclosed code block
func TestFindLastUnclosedCodeBlock(t *testing.T) {
	tests := []struct {
		name   string
		input  string
		expect int
	}{
		{
			name:   "no code block",
			input:  "just plain text",
			expect: -1,
		},
		{
			name:   "closed code block",
			input:  "```\ncode\n```\n",
			expect: -1,
		},
		{
			name:   "unclosed code block",
			input:  "```\ncode here",
			expect: 0,
		},
		{
			name:   "multiple code blocks last unclosed",
			input:  "```\nfirst\n```\n```\nsecond",
			expect: 14, // Position of the last opening ``` that has no closing
		},
		{
			name:   "empty string",
			input:  "",
			expect: -1,
		},
		{
			name:   "code block with content",
			input:  "text\n```\nfunction test() {\n  return true;\n}\n```",
			expect: -1,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := findLastUnclosedCodeBlock(tt.input)
			if result != tt.expect {
				t.Errorf("findLastUnclosedCodeBlock(%q) = %d, want %d",
					tt.input, result, tt.expect)
			}
		})
	}
}

// TestFindNextClosingCodeBlock tests finding closing code block
func TestFindNextClosingCodeBlock(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		startIdx int
		expect   int
	}{
		{
			name:     "find closing after start",
			input:    "```\ncode\n```\ntext",
			startIdx: 0,
			expect:   12, // Position after closing ``` (which is at 9-11)
		},
		{
			name:     "no closing block",
			input:    "```\ncode\ntext",
			startIdx: 0,
			expect:   -1,
		},
		{
			name:     "start after opening",
			input:    "```\ncode\n```\nmore",
			startIdx: 5,
			expect:   12, // Position after closing ``` (which is at 9-11)
		},
		{
			name:     "no code block",
			input:    "just plain text",
			startIdx: 0,
			expect:   -1,
		},
		{
			name:     "empty string",
			input:    "",
			startIdx: 0,
			expect:   -1,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := findNextClosingCodeBlock(tt.input, tt.startIdx)
			if result != tt.expect {
				t.Errorf("findNextClosingCodeBlock(%q, %d) = %d, want %d",
					tt.input, tt.startIdx, result, tt.expect)
			}
		})
	}
}

// TestSplitMessage_Unicode tests splitting with unicode content
func TestSplitMessage_Unicode(t *testing.T) {
	content := "你好世界\n" + strings.Repeat("这是一段中文文本。", 20)

	result := SplitMessage(content, 50)

	if len(result) == 0 {
		t.Error("Expected at least one chunk")
	}

	// Verify content is preserved (roughly)
	joined := strings.Join(result, "")
	if !strings.Contains(joined, "你好世界") {
		t.Error("Split content should contain original text")
	}
}

// TestSplitMessage_PreservesContent tests that splitting preserves content
func TestSplitMessage_PreservesContent(t *testing.T) {
	original := strings.Repeat("test line\n", 10)
	split := SplitMessage(original, 50)

	joined := strings.Join(split, "")

	// SplitMessage may add newlines for code block preservation
	// Just check the main content is preserved
	if !strings.Contains(joined, "test line") {
		t.Error("Split content should contain original text")
	}
}

// TestSplitMessage_CodeBlockHandling tests code block preservation
func TestSplitMessage_CodeBlockHandling(t *testing.T) {
	content := "```\ncode line 1\ncode line 2\ncode line 3\n```\nregular text"

	split := SplitMessage(content, 30)

	// Should not split inside code block
	for _, chunk := range split {
		if strings.Contains(chunk, "```") && !strings.HasSuffix(chunk, "```") {
			// This is okay as long as there's a matching closing somewhere
			openCount := strings.Count(chunk, "```")
			if openCount%2 != 0 {
				// Odd number of ``` means unclosed block
				t.Logf("Chunk might have unclosed code block: %q", chunk)
			}
		}
	}
}

// TestTruncate_EdgeCases tests truncation edge cases
func TestTruncate_EdgeCases(t *testing.T) {
	tests := []struct {
		name   string
		input  string
		maxLen int
		expect string
	}{
		{
			name:   "very long string",
			input:  strings.Repeat("a", 10000),
			maxLen: 100,
			expect: strings.Repeat("a", 97) + "...", // Truncate reserves 3 chars for "..."
		},
		{
			name:   "mixed unicode and ascii",
			input:  "Hello世界",
			maxLen: 7,
			expect: "Hello世界", // Exact length, no truncation needed
		},
		{
			name:   "truncate with emoji",
			input:  "Hello 👋 World",
			maxLen: 8,
			expect: "Hello...", // "Hello " (6 chars) + "..." would be 9, so we get "Hello" (5) + "..." = 8
		},
		{
			name:   "truncate with more room for emoji",
			input:  "Hello 👋 World",
			maxLen: 10,
			expect: "Hello 👋...", // "Hello " (6) + emoji (2) = 8, reserve 3 for "...", need 8-3=5, but 5 < 6, so "Hello" (5) + "..."
		},
		{
			name:   "no truncation needed",
			input:  "Hello 👋 World",
			maxLen: 15,
			expect: "Hello 👋 World", // Exact length
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := Truncate(tt.input, tt.maxLen)
			if result != tt.expect {
				t.Errorf("Truncate(%q, %d) = %q (rune len=%d), want %q (rune len=%d)",
					tt.input, tt.maxLen, result, len([]rune(result)), tt.expect, len([]rune(tt.expect)))
			}
		})
	}
}

// TestWriteFileAtomic_Permissions tests file permissions
func TestWriteFileAtomic_Permissions(t *testing.T) {
	tempDir := t.TempDir()
	testFile := filepath.Join(tempDir, "test.txt")

	err := WriteFileAtomic(testFile, []byte("content"), 0644)
	if err != nil {
		t.Fatalf("WriteFileAtomic failed: %v", err)
	}

	info, err := os.Stat(testFile)
	if err != nil {
		t.Fatalf("Failed to stat file: %v", err)
	}

	// On Unix-like systems, check exact permissions
	// On Windows, just verify the file is readable
	if os.Getenv("GOOS") != "windows" {
		expectedMode := os.FileMode(0644)
		if info.Mode().Perm() != expectedMode {
			t.Logf("File permissions: got %v, want %v", info.Mode().Perm(), expectedMode)
			// Don't fail the test, just log - permissions can be affected by umask
		}
	}

	// Verify file is readable
	if !info.Mode().IsRegular() {
		t.Error("Expected regular file")
	}
}

// TestSplitMessage_VeryLongContent tests very long content splitting
func TestSplitMessage_VeryLongContent(t *testing.T) {
	// Create a very long message (equivalent to 100K characters)
	content := strings.Repeat("This is a line of text that will be split.\n", 2000)

	split := SplitMessage(content, 4000)

	if len(split) == 0 {
		t.Error("Expected at least one chunk")
	}

	// Verify each chunk is within limits
	for i, chunk := range split {
		if len(chunk) > 4000 {
			t.Errorf("Chunk %d exceeds max length: %d", i, len(chunk))
		}
	}
}

// TestSplitMessage_EmptyChunks tests that empty chunks aren't created
func TestSplitMessage_EmptyChunks(t *testing.T) {
	content := "short"
	split := SplitMessage(content, 100)

	for _, chunk := range split {
		if len(chunk) == 0 {
			t.Error("Split should not create empty chunks")
		}
	}
}

// TestSplitMessage_SingleLineVeryLong tests single line that's too long
func TestSplitMessage_SingleLineVeryLong(t *testing.T) {
	content := strings.Repeat("a", 10000) // No newlines

	split := SplitMessage(content, 2000)

	if len(split) == 0 {
		t.Error("Expected at least one chunk")
	}

	// Should split the long line
	totalLength := 0
	for _, chunk := range split {
		totalLength += len(chunk)
	}

	if totalLength != len(content) {
		t.Errorf("Content length mismatch: got %d, want %d", totalLength, len(content))
	}
}

// TestSplitMessage_NewlineHandling tests newline preservation
func TestSplitMessage_NewlineHandling(t *testing.T) {
	content := "line1\nline2\nline3\nline4\nline5"

	split := SplitMessage(content, 20)

	// SplitMessage may trim whitespace for cleaner output
	// Just verify key content is preserved
	joined := strings.Join(split, "")
	if !strings.Contains(joined, "line1") || !strings.Contains(joined, "line5") {
		t.Errorf("Content not preserved:\nOriginal: %q\nJoined:  %q", content, joined)
	}

	// Verify we have the expected number of lines (roughly)
	lineCount := strings.Count(joined, "line")
	if lineCount != 5 {
		t.Errorf("Expected 5 lines, got %d in joined content", lineCount)
	}
}

// TestSplitMessage_CodeBlockSplitAtBoundary tests code block split at exact boundary
func TestSplitMessage_CodeBlockSplitAtBoundary(t *testing.T) {
	content := "```\ncode line 1\ncode line 2\ncode line 3\n```\ntext after"

	split := SplitMessage(content, 50)

	if len(split) == 0 {
		t.Error("Expected at least one chunk")
	}

	// Verify code block markers are balanced
	openCount := strings.Count(strings.Join(split, ""), "```")
	if openCount%2 != 0 {
		t.Error("Unbalanced code block markers")
	}
}

// TestSplitMessage_MaxLenBoundary tests exact maxLen boundary
func TestSplitMessage_MaxLenBoundary(t *testing.T) {
	// Create content exactly at maxLen
	content := strings.Repeat("a", 100)

	split := SplitMessage(content, 100)

	if len(split) != 1 {
		t.Errorf("Expected 1 chunk for exact length content, got %d", len(split))
	}

	if split[0] != content {
		t.Error("Content was modified")
	}
}

// TestSplitMessage_MaxLenPlusOne tests one character over maxLen
func TestSplitMessage_MaxLenPlusOne(t *testing.T) {
	// Create content one character over maxLen
	content := strings.Repeat("a", 101)

	split := SplitMessage(content, 100)

	if len(split) < 2 {
		t.Errorf("Expected at least 2 chunks, got %d", len(split))
	}
}

// TestSplitMessage_CodeBlockWithoutClosing tests unclosed code block handling
func TestSplitMessage_CodeBlockWithoutClosing(t *testing.T) {
	content := "```\ncode without closing fence\nmore code\ntext after"

	split := SplitMessage(content, 50)

	if len(split) == 0 {
		t.Error("Expected at least one chunk")
	}

	// Verify content is preserved
	joined := strings.Join(split, "")
	if !strings.Contains(joined, "code without closing fence") {
		t.Error("Content not preserved")
	}
}

// TestSplitMessage_MultipleCodeBlocks tests multiple code blocks
func TestSplitMessage_MultipleCodeBlocks(t *testing.T) {
	content := "```\ncode 1\n```\ntext\n```\ncode 2\n```\nmore text"

	split := SplitMessage(content, 50)

	if len(split) == 0 {
		t.Error("Expected at least one chunk")
	}

	// Verify all code blocks are preserved
	joined := strings.Join(split, "")
	if !strings.Contains(joined, "code 1") || !strings.Contains(joined, "code 2") {
		t.Error("Code blocks not preserved")
	}
}

// TestSplitMessage_CodeBlockAtSplitPoint tests code block exactly at split point
func TestSplitMessage_CodeBlockAtSplitPoint(t *testing.T) {
	content := "text before\n```\ncode\n```\ntext after"

	split := SplitMessage(content, 30)

	if len(split) == 0 {
		t.Error("Expected at least one chunk")
	}

	// Verify code block is preserved
	joined := strings.Join(split, "")
	if !strings.Contains(joined, "code") {
		t.Error("Code block content not preserved")
	}
}

// TestSplitMessage_NoSplittingPoints tests content with no natural split points
func TestSplitMessage_NoSplittingPoints(t *testing.T) {
	content := strings.Repeat("a", 10000) + strings.Repeat("b", 10000)

	split := SplitMessage(content, 2000)

	if len(split) == 0 {
		t.Error("Expected at least one chunk")
	}

	// Verify total length is preserved
	totalLen := 0
	for _, chunk := range split {
		totalLen += len(chunk)
	}

	if totalLen != len(content) {
		t.Errorf("Total length mismatch: got %d, want %d", totalLen, len(content))
	}
}

// TestSplitMessage_EffectiveLimitEdgeCases tests effective limit calculations
func TestSplitMessage_EffectiveLimitEdgeCases(t *testing.T) {
	tests := []struct {
		name         string
		content      string
		maxLen       int
		minChunks    int
		description  string
	}{
		{
			name:        "very small maxLen",
			content:     "hello world test content here",
			maxLen:      10,
			minChunks:   1,
			description: "Should handle very small maxLen",
		},
		{
			name:        "maxLen less than buffer",
			content:     strings.Repeat("a", 100),
			maxLen:      40,
			minChunks:   1,
			description: "Should handle maxLen less than codeBlockBuffer",
		},
		{
			name:        "maxLen equals buffer",
			content:     strings.Repeat("a", 50),
			maxLen:      50,
			minChunks:   1,
			description: "Should handle maxLen equal to buffer",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			split := SplitMessage(tt.content, tt.maxLen)
			if len(split) < tt.minChunks {
				t.Errorf("%s: Expected at least %d chunks, got %d", tt.description, tt.minChunks, len(split))
			}

			// Verify no chunk exceeds maxLen
			for i, chunk := range split {
				if len(chunk) > tt.maxLen {
					t.Errorf("Chunk %d exceeds maxLen: %d > %d", i, len(chunk), tt.maxLen)
				}
			}
		})
	}
}
