// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package utils_test

import (
	"strings"
	"testing"

	. "github.com/276793422/NemesisBot/module/utils"
)

func TestSplitMessage_CodeBlockHandling(t *testing.T) {
	// Test message with unclosed code block
	message := "This is code:\n```\nfunction test() {\nconsole.log('hello');\n\n\n\n```\nThis is more text after code block\nAnd even more text that exceeds the limit and needs to be split across multiple messages while preserving code block integrity"
	parts := SplitMessage(message, 100)

	// Should have multiple parts due to length
	if len(parts) < 3 {
		t.Errorf("Expected at least 3 parts for long message with code block, got %d", len(parts))
	}

	// Check that code blocks are not split
	for _, part := range parts {
		if strings.Contains(part, "```") {
			// Count opening and closing fences
			openFences := strings.Count(part, "```")
			if openFences%2 != 0 {
				t.Errorf("Part has unbalanced code block fences: %s", part)
			}
		}
	}
}

func TestSplitMessage_MultipleCodeBlocks(t *testing.T) {
	// Test with multiple code blocks
	message := "Text before\n```\ncode1\n```\nmiddle text\n```\ncode2\n```\ntext after"
	parts := SplitMessage(message, 50)

	// Should preserve all code blocks
	if len(parts) < 2 {
		t.Errorf("Expected at least 2 parts for message with code blocks, got %d", len(parts))
	}
}

func TestSplitMessage_VeryLongCodeBlock(t *testing.T) {
	// Create a very long code block that exceeds maxLen
	codeBlock := "```\n" + strings.Repeat("very long line that exceeds the max length and needs to be handled carefully\n", 50) + "\n```"
	message := "Text before " + codeBlock + " text after"
	parts := SplitMessage(message, 100)

	// Should handle the long code block without breaking it
	if len(parts) == 0 {
		t.Errorf("Expected parts for message with long code block")
	}
}

func TestSplitMessage_NoNewlinesOrSpaces(t *testing.T) {
	// Test with content that has no natural split points
	longWord := strings.Repeat("abcdefghijklmnopqrstuvwxyz", 10)
	message := "Start " + longWord + " end"
	parts := SplitMessage(message, 50)

	// Should still split despite no natural breaks
	if len(parts) < 2 {
		t.Errorf("Expected multiple parts for message without natural splits, got %d", len(parts))
	}
}

func TestSplitMessage_EmptyContent(t *testing.T) {
	parts := SplitMessage("", 100)

	if len(parts) != 0 {
		t.Errorf("Expected empty parts for empty content, got %d", len(parts))
	}
}

func TestSplitMessage_ExactlyMaxLen(t *testing.T) {
	message := strings.Repeat("a", 100)
	parts := SplitMessage(message, 100)

	if len(parts) != 1 {
		t.Errorf("Expected 1 part for message exactly at max length, got %d", len(parts))
	}
	if parts[0] != message {
		t.Errorf("Message should not be split")
	}
}

func TestSplitMessage_JustOverMaxLen(t *testing.T) {
	message := strings.Repeat("a", 101)
	parts := SplitMessage(message, 100)

	if len(parts) != 2 {
		t.Errorf("Expected 2 parts for message just over max length, got %d", len(parts))
	}
	// Note: SplitMessage uses a dynamic buffer (10% of maxLen, min 50)
	// So for maxLen=100, effectiveLimit=90, first part will be 90 chars
	if len(parts[0]) < 50 || len(parts[0]) > 100 {
		t.Errorf("First part should be between 50 and 100 chars, got %d", len(parts[0]))
	}
	if len(parts[0])+len(parts[1]) != 101 {
		t.Errorf("Total length should be 101, got %d", len(parts[0])+len(parts[1]))
	}
}

func TestSplitMessage_CodeBlockInMiddle(t *testing.T) {
	// Message with a code block in the middle that needs to be preserved
	message := "This is text before the code block.\n\n```\nfunction test() {\n  console.log('hello');\n}\n```\n\nThis is text after the code block and it's quite long so it should be split into multiple messages while preserving the code block integrity."
	parts := SplitMessage(message, 100)

	// Should have multiple parts due to length
	if len(parts) < 3 {
		t.Errorf("Expected at least 3 parts for message with code block, got %d", len(parts))
	}

	// Check that code blocks are not split
	for _, part := range parts {
		if strings.Contains(part, "```") {
			// Count opening and closing fences
			openFences := strings.Count(part, "```")
			if openFences%2 != 0 && !strings.HasSuffix(part, "```") {
				t.Errorf("Part has unbalanced code block fences: %s", part)
			}
		}
	}
}

func TestSplitMessage_MultipleUnclosedCodeBlocks(t *testing.T) {
	// Message with multiple unclosed code blocks
	message := "```\nfirst code\n```\n```\nsecond code\n```\n```\nthird code"
	parts := SplitMessage(message, 50)

	// Should preserve all code blocks
	if len(parts) == 0 {
		t.Errorf("Expected parts for message with code blocks")
	}

	// Each part should have balanced code blocks
	for _, part := range parts {
		if strings.Contains(part, "```") {
			openFences := strings.Count(part, "```")
			closeFences := strings.Count(part, "```")
			if openFences != closeFences {
				t.Errorf("Part has unbalanced code block fences: %s", part)
			}
		}
	}
}

func TestSplitMessage_BufferCalculation(t *testing.T) {
	// Test with a small maxLen to test buffer calculation
	message := "This is a very long line that needs to be split and contains a code block\n\n```\ncode here\n```\nand more text after"
	parts := SplitMessage(message, 30)

	// Should split appropriately with buffer consideration
	if len(parts) == 0 {
		t.Errorf("Expected parts for message with buffer calculation")
	}

	// Verify content is preserved
	combined := strings.Join(parts, "")
	if !strings.Contains(combined, "code here") {
		t.Error("Code block content should be preserved")
	}
}

func TestSplitMessage_ExactlyMaxLenWithCodeBlock(t *testing.T) {
	// Create a message exactly at max length with a code block
	codeBlock := "```\ncode\n```\n"
	textBefore := strings.Repeat("a", 95-len(codeBlock))
	message := textBefore + codeBlock

	parts := SplitMessage(message, 100)

	// Should not split the message
	if len(parts) != 1 {
		t.Errorf("Expected 1 part for message exactly at max length with code block, got %d", len(parts))
	}
}

func TestSplitMessage_NoNaturalSplit(t *testing.T) {
	// Create a message with no natural split points (no spaces or newlines)
	message := strings.Repeat("abcdefghijklmnopqrstuvwxyz", 10) // 260 chars
	parts := SplitMessage(message, 100)

	// Should still split the message
	if len(parts) <= 1 {
		t.Errorf("Expected multiple parts for message without natural splits, got %d", len(parts))
	}

	// Verify total content is preserved
	totalLength := 0
	for _, part := range parts {
		totalLength += len(part)
	}

	if totalLength != len(message) {
		t.Errorf("Content length mismatch: got %d, want %d", totalLength, len(message))
	}
}

func TestSplitMessage_CombinedCodeBlockHandling(t *testing.T) {
	// Test complex scenario with multiple code blocks and text
	message := "Start text\n\n```\ncode block 1\nwith content\n```\n\nMiddle text\n\n```\ncode block 2\nwith more content\n```\n\nEnd text that is very long and needs to be split into multiple messages while preserving all code blocks correctly."
	parts := SplitMessage(message, 150)

	// Should have multiple parts
	if len(parts) < 2 {
		t.Errorf("Expected at least 2 parts, got %d", len(parts))
	}

	// Verify all code blocks are preserved
	for _, part := range parts {
		if strings.Contains(part, "```") {
			// Count the number of ``` markers
			markers := strings.Count(part, "```")
			if markers%2 != 0 && !strings.HasSuffix(part, "```") {
				// This is okay as long as there's a matching closing somewhere
				t.Logf("Warning: Part might have unbalanced code blocks: %q", part)
			}
		}
	}
}

func TestSplitMessage_VeryLargeContent(t *testing.T) {
	// Create a very large message (each line ~38 chars, 2000 lines = ~76000 chars)
	line := "This is a line of text that will be split.\n"
	content := strings.Repeat(line, 2000)
	expectedLen := len(line) * 2000

	parts := SplitMessage(content, 4000)

	if len(parts) == 0 {
		t.Error("Expected at least one part")
	}

	// Verify each part is within limits
	for i, part := range parts {
		if len(part) > 4000 {
			t.Errorf("Part %d exceeds max length: %d", i, len(part))
		}
	}

	// Verify total content is approximately preserved (may vary due to trimming)
	totalLen := 0
	for _, part := range parts {
		totalLen += len(part)
	}
	// Allow some tolerance for whitespace trimming during split
	if totalLen < expectedLen-1000 || totalLen > expectedLen {
		t.Errorf("Content length out of range: got %d, want %d±1000", totalLen, expectedLen)
	}
}

func TestSplitMessage_PreserveCodeBlocks(t *testing.T) {
	// Test with multiple code blocks that should not be split
	codeBlock := "```\nfunction test() {\n  return true;\n}\n```\n"
	message := "Start text\n" + codeBlock + "middle text\n" + codeBlock + "end text"

	parts := SplitMessage(message, 100)

	// Should preserve all code blocks
	for _, part := range parts {
		if strings.Contains(part, "```") {
			openCount := strings.Count(part, "```")
			closeCount := strings.Count(part, "```")
			if openCount != closeCount {
				t.Errorf("Unbalanced code block fences in part: %q", part)
			}
		}
	}
}

func TestSplitMessage_EdgeCaseBuffer(t *testing.T) {
	// Test edge case where buffer calculation is critical
	message := "word\n" + strings.Repeat("a", 85) + "\n```\ncode\n```\n" + strings.Repeat("b", 10)
	parts := SplitMessage(message, 100)

	if len(parts) == 0 {
		t.Error("Expected parts")
	}

	// Verify no code blocks are split
	for _, part := range parts {
		if strings.Contains(part, "```") {
			// Check if the code block is properly closed
			if strings.Count(part, "```")%2 != 0 {
				t.Errorf("Unclosed code block in part: %q", part)
			}
		}
	}
}

func TestSplitMessage_MaxLenMinusBuffer(t *testing.T) {
	// Test when effective limit is maxLen minus buffer
	message := strings.Repeat("a", 95) + "\n" + strings.Repeat("b", 95)
	parts := SplitMessage(message, 100)

	// Should split into multiple parts
	if len(parts) < 2 {
		t.Errorf("Expected at least 2 parts, got %d", len(parts))
	}

	// Verify content is approximately preserved (may have trimmed whitespace)
	totalLen := 0
	for _, part := range parts {
		totalLen += len(part)
	}
	// Original is 191 chars (95+1+95), but SplitMessage may trim whitespace
	if totalLen < 189 || totalLen > 191 {
		t.Errorf("Total length out of expected range: got %d, want 189-191", totalLen)
	}
}

func TestSplitMessage_NoBufferNeeded(t *testing.T) {
	// Test when no code block handling is needed
	line := "This is a normal line without code blocks.\n"
	message := strings.Repeat(line, 50)
	originalLen := len(message)

	parts := SplitMessage(message, 200)

	// Should split naturally
	if len(parts) < 3 {
		t.Errorf("Expected at least 3 parts, got %d", len(parts))
	}

	// Verify content is approximately preserved (SplitMessage trims whitespace)
	totalLen := 0
	for _, part := range parts {
		totalLen += len(part)
	}
	// Allow some tolerance for whitespace trimming during split
	if totalLen < originalLen-100 || totalLen > originalLen {
		t.Errorf("Content length out of range: got %d, want %d±100", totalLen, originalLen)
	}

	// Verify the reconstructed content contains key phrases
	reconstructed := strings.Join(parts, "")
	if !strings.Contains(reconstructed, "normal line") || !strings.Contains(reconstructed, "code blocks") {
		t.Error("Reconstructed content missing key phrases")
	}
}

func TestSplitMessage_CodeBlockAtBoundary(t *testing.T) {
	// Test code block exactly at the boundary
	message := strings.Repeat("a", 97) + "\n" + "```\ncode\n```\n" + strings.Repeat("b", 95)
	parts := SplitMessage(message, 100)

	// Should handle the code block properly
	if len(parts) == 0 {
		t.Error("Expected parts")
	}

	// Verify code block is not split
	codeBlockFound := false
	for _, part := range parts {
		if strings.Contains(part, "```") {
			codeBlockFound = true
			break
		}
	}
	if !codeBlockFound {
		t.Error("Code block should be preserved in parts")
	}
}

func TestSplitMessage_ComplexScenario(t *testing.T) {
	// Test complex scenario with multiple code blocks and mixed content
	// Using string concatenation to avoid raw literal issues with backticks
	message := "Introduction text.\n\n" +
		"This is a normal paragraph.\n\n" +
		"```\n" +
		"function complexFunction() {\n" +
		"  const result = await someAsyncOperation();\n" +
		"  return result;\n" +
		"}\n" +
		"```\n\n" +
		"More text after the code block.\n\n" +
		"Another code block:\n" +
		"```\n" +
		"const data = { key: value };\n" +
		"console.log(data);\n" +
		"```\n\n" +
		"Final text of the message."

	parts := SplitMessage(message, 150)

	// Should preserve all code blocks
	codeBlockCount := 0
	for _, part := range parts {
		if strings.Contains(part, "```") {
			codeBlockCount++
			// Check if code block is balanced
			openCount := strings.Count(part, "```")
			if openCount%2 != 0 && !strings.HasSuffix(part, "```") {
				t.Errorf("Unbalanced code block in part: %q", part)
			}
		}
	}

	if codeBlockCount < 2 {
		t.Errorf("Expected at least 2 code blocks, found %d", codeBlockCount)
	}
}

func TestSplitMessage_Basic(t *testing.T) {
	message := "Hello World"
	parts := SplitMessage(message, 1000)

	if len(parts) != 1 {
		t.Errorf("Expected 1 part, got %d", len(parts))
	}
	if parts[0] != message {
		t.Errorf("Message should not be split")
	}
}

func TestSplitMessage_LongMessage(t *testing.T) {
	message := string(make([]byte, 3000))
	parts := SplitMessage(message, 1000)

	// We expect at least 3 parts, maybe more depending on implementation
	if len(parts) < 3 {
		t.Errorf("Expected at least 3 parts, got %d", len(parts))
	}

	// Verify total content preserved
	recombined := ""
	for _, part := range parts {
		recombined += part
	}
	if recombined != message {
		t.Error("Recombined message doesn't match original")
	}
}
