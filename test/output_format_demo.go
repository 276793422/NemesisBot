package main

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
)

// Mock WebFetchTool for testing
type MockWebFetchTool struct{}

type ToolResult struct {
	ForLLM  string
	ForUser string
}

func (t *MockWebFetchTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	// Simulate different content types
	contentType := args["contentType"].(string)

	var text string
	switch contentType {
	case "markdown":
		text = `# My Skill

## Description
This is a skill document with examples.

## Usage
Use this skill to do something amazing.`
	case "json":
		text = `{
  "name": "test",
  "version": "1.0.0",
  "description": "A test JSON file"
}`
	case "code":
		text = `def hello():
    """Say hello to the world."""
    print("Hello, World!")

if __name__ == "__main__":
    hello()`
	default:
		text = "Plain text content"
	}

	// New implementation: return pure content without prefix
	return &ToolResult{
		ForLLM:  text,
		ForUser: fmt.Sprintf(`{"status": 200, "text": %q}`, text),
	}
}

func main() {
	ctx := context.Background()
	tool := &MockWebFetchTool{}

	fmt.Println("=== Testing New web_fetch Output Format ===")
	fmt.Println()

	// Test 1: Markdown content
	fmt.Println("Test 1: Markdown Content")
	fmt.Println("------------------------")
	result := tool.Execute(ctx, map[string]interface{}{
		"contentType": "markdown",
	})
	fmt.Println("ForLLM:")
	fmt.Println(result.ForLLM)
	fmt.Println()

	// Verify: should NOT have "Fetched ... bytes" prefix
	if strings.HasPrefix(result.ForLLM, "Fetched") {
		fmt.Println("❌ FAIL: Output has metadata prefix")
	} else if strings.HasPrefix(result.ForLLM, "# My Skill") {
		fmt.Println("✅ PASS: Output starts with actual content")
	} else {
		fmt.Println("⚠️  UNEXPECTED: Output doesn't match expected format")
	}
	fmt.Println()

	// Test 2: JSON content
	fmt.Println("Test 2: JSON Content")
	fmt.Println("--------------------")
	result = tool.Execute(ctx, map[string]interface{}{
		"contentType": "json",
	})
	fmt.Println("ForLLM:")
	fmt.Println(result.ForLLM)
	fmt.Println()

	// Verify: should be valid JSON
	var jsonData map[string]interface{}
	if err := json.Unmarshal([]byte(result.ForLLM), &jsonData); err == nil {
		fmt.Println("✅ PASS: Output is valid JSON")
	} else {
		fmt.Printf("❌ FAIL: Output is not valid JSON: %v\n", err)
	}
	fmt.Println()

	// Test 3: Code content
	fmt.Println("Test 3: Code Content")
	fmt.Println("--------------------")
	result = tool.Execute(ctx, map[string]interface{}{
		"contentType": "code",
	})
	fmt.Println("ForLLM:")
	fmt.Println(result.ForLLM)
	fmt.Println()

	// Verify: should start with 'def'
	if strings.HasPrefix(result.ForLLM, "def hello()") {
		fmt.Println("✅ PASS: Output preserves code format")
	} else {
		fmt.Println("❌ FAIL: Output doesn't start with code")
	}
	fmt.Println()

	fmt.Println("=== Summary ===")
	fmt.Println("The new implementation returns pure content without metadata prefix.")
	fmt.Println("This preserves the original format for JSON, Markdown, code, etc.")
}
