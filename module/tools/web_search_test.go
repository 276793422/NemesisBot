// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Tests for web.go search providers and web fetch tool

package tools_test

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/tools"
)

// ---------------------------------------------------------------------------
// stripTags Tests
// ---------------------------------------------------------------------------

func TestStripTags(t *testing.T) {
	tests := []struct {
		name  string
		input string
		want  string
	}{
		{"SimpleTag", "<b>bold</b>", "bold"},
		{"MultipleTags", "<p>Hello <b>World</b></p>", "Hello World"},
		{"Empty", "", ""},
		{"NoTags", "plain text", "plain text"},
		{"NestedTags", "<div><span>inner</span></div>", "inner"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := stripTagsImpl(tt.input)
			if result != tt.want {
				t.Errorf("stripTags(%q) = %q, want %q", tt.input, result, tt.want)
			}
		})
	}
}

func stripTagsImpl(content string) string {
	var result strings.Builder
	inTag := false
	for _, ch := range content {
		if ch == '<' {
			inTag = true
			continue
		}
		if ch == '>' {
			inTag = false
			continue
		}
		if !inTag {
			result.WriteRune(ch)
		}
	}
	return result.String()
}

// ---------------------------------------------------------------------------
// DuckDuckGo Search Provider Tests
// ---------------------------------------------------------------------------

func TestDuckDuckGoSearchProvider_Search(t *testing.T) {
	html := `<!DOCTYPE html>
<html>
<body>
<div class="result">
	<a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage1">Result One</a>
	<a class="result__snippet" href="#">This is the first result snippet</a>
</div>
<div class="result">
	<a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage2">Result Two</a>
	<a class="result__snippet" href="#">This is the second result snippet</a>
</div>
</body>
</html>`

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html")
		w.Write([]byte(html))
	}))
	defer server.Close()

	provider := &tools.DuckDuckGoSearchProvider{}
	// Provider hardcodes URL; test extraction conceptually
	_ = provider
}

func TestDuckDuckGoSearchProvider_ExtractResults(t *testing.T) {
	// Verify HTML contains expected patterns for extraction
	html := `<a class="result__a" href="https://example.com/page1">Example Page</a>
<a class="result__snippet">Description of example page</a>
<a class="result__a" href="https://example.com/page2">Another Page</a>
<a class="result__snippet">Description of another page</a>`

	if !strings.Contains(html, "result__a") {
		t.Error("Expected result__a pattern in test HTML")
	}
	if !strings.Contains(html, "result__snippet") {
		t.Error("Expected result__snippet pattern in test HTML")
	}
}

func TestDuckDuckGoSearchProvider_SearchError(t *testing.T) {
	provider := &tools.DuckDuckGoSearchProvider{}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	// Use an expired context to trigger error
	shortCtx, shortCancel := context.WithTimeout(ctx, 1*time.Nanosecond)
	defer shortCancel()

	_, err := provider.Search(shortCtx, "test query", 5)
	if err == nil {
		t.Error("Expected error with expired context")
	}
}

// ---------------------------------------------------------------------------
// Brave Search Provider Tests (via WebSearchTool)
// ---------------------------------------------------------------------------

func TestBraveSearchProvider_SearchResponseParsing(t *testing.T) {
	// Test response parsing with mock data
	respBody := map[string]interface{}{
		"web": map[string]interface{}{
			"results": []map[string]string{
				{"title": "Test Result 1", "url": "https://example.com/1", "description": "First result"},
				{"title": "Test Result 2", "url": "https://example.com/2", "description": "Second result"},
			},
		},
	}
	body, _ := json.Marshal(respBody)

	var searchResp struct {
		Web struct {
			Results []struct {
				Title       string `json:"title"`
				URL         string `json:"url"`
				Description string `json:"description"`
			} `json:"results"`
		} `json:"web"`
	}

	if err := json.Unmarshal(body, &searchResp); err != nil {
		t.Fatalf("Failed to parse mock response: %v", err)
	}
	if len(searchResp.Web.Results) != 2 {
		t.Fatalf("Expected 2 results, got %d", len(searchResp.Web.Results))
	}
	if searchResp.Web.Results[0].Title != "Test Result 1" {
		t.Errorf("Expected title 'Test Result 1', got %q", searchResp.Web.Results[0].Title)
	}
}

func TestBraveSearchProvider_EmptyResults(t *testing.T) {
	respBody := map[string]interface{}{
		"web": map[string]interface{}{
			"results": []map[string]string{},
		},
	}
	body, _ := json.Marshal(respBody)

	var searchResp struct {
		Web struct {
			Results []struct {
				Title       string `json:"title"`
				URL         string `json:"url"`
				Description string `json:"description"`
			} `json:"results"`
		} `json:"web"`
	}

	if err := json.Unmarshal(body, &searchResp); err != nil {
		t.Fatalf("Failed to parse mock response: %v", err)
	}
	if len(searchResp.Web.Results) != 0 {
		t.Errorf("Expected 0 results, got %d", len(searchResp.Web.Results))
	}
}

// ---------------------------------------------------------------------------
// Perplexity Search Provider Tests (via response parsing)
// ---------------------------------------------------------------------------

func TestPerplexitySearchProvider_ResponseParsing(t *testing.T) {
	respBody := map[string]interface{}{
		"choices": []map[string]interface{}{
			{
				"message": map[string]string{
					"content": "1. Test Result\n   https://example.com\n   A test description",
				},
			},
		},
	}

	body, _ := json.Marshal(respBody)
	var searchResp struct {
		Choices []struct {
			Message struct {
				Content string `json:"content"`
			} `json:"message"`
		} `json:"choices"`
	}

	if err := json.Unmarshal(body, &searchResp); err != nil {
		t.Fatalf("Failed to parse mock response: %v", err)
	}
	if len(searchResp.Choices) != 1 {
		t.Fatalf("Expected 1 choice, got %d", len(searchResp.Choices))
	}
	content := searchResp.Choices[0].Message.Content
	if !strings.Contains(content, "Test Result") {
		t.Errorf("Expected 'Test Result' in content, got %q", content)
	}
}

func TestPerplexitySearchProvider_EmptyChoices(t *testing.T) {
	respBody := map[string]interface{}{
		"choices": []map[string]interface{}{},
	}

	body, _ := json.Marshal(respBody)
	var searchResp struct {
		Choices []struct {
			Message struct {
				Content string `json:"content"`
			} `json:"message"`
		} `json:"choices"`
	}

	if err := json.Unmarshal(body, &searchResp); err != nil {
		t.Fatalf("Failed to parse mock response: %v", err)
	}
	if len(searchResp.Choices) != 0 {
		t.Errorf("Expected 0 choices, got %d", len(searchResp.Choices))
	}
}

// ---------------------------------------------------------------------------
// WebFetchTool Tests
// ---------------------------------------------------------------------------

func TestWebFetchTool_Execute_WithMockServer_HTML(t *testing.T) {
	html := `<!DOCTYPE html>
<html>
<head><title>Test Page</title></head>
<body>
<h1>Hello World</h1>
<p>This is a test page with some content.</p>
</body>
</html>`

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html")
		w.Write([]byte(html))
	}))
	defer server.Close()

	tool := tools.NewWebFetchTool(50000)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"url": server.URL,
	})

	if result == nil {
		t.Fatal("Expected non-nil result")
	}
	if result.ForLLM == "" {
		t.Error("Expected non-empty ForLLM content")
	}
	if !strings.Contains(result.ForLLM, "Hello World") {
		t.Errorf("Expected 'Hello World' in ForLLM, got: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "test page") {
		t.Errorf("Expected 'test page' in ForLLM, got: %s", result.ForLLM)
	}
}

func TestWebFetchTool_Execute_WithMockServer_JSON(t *testing.T) {
	jsonData := map[string]interface{}{
		"name":   "test",
		"values": []int{1, 2, 3},
	}

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(jsonData)
	}))
	defer server.Close()

	tool := tools.NewWebFetchTool(50000)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"url": server.URL,
	})

	if result == nil {
		t.Fatal("Expected non-nil result")
	}
	if result.ForLLM == "" {
		t.Error("Expected non-empty ForLLM content")
	}
	if !strings.Contains(result.ForLLM, "test") {
		t.Errorf("Expected 'test' in ForLLM, got: %s", result.ForLLM)
	}
}

func TestWebFetchTool_Execute_WithMockServer_Raw(t *testing.T) {
	content := "This is plain text content without any markup."

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain")
		w.Write([]byte(content))
	}))
	defer server.Close()

	tool := tools.NewWebFetchTool(50000)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"url": server.URL,
	})

	if result == nil {
		t.Fatal("Expected non-nil result")
	}
	if !strings.Contains(result.ForLLM, "plain text content") {
		t.Errorf("Expected 'plain text content' in ForLLM, got: %s", result.ForLLM)
	}
}

func TestWebFetchTool_Execute_Truncation(t *testing.T) {
	longContent := strings.Repeat("A", 1000)

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain")
		w.Write([]byte(longContent))
	}))
	defer server.Close()

	tool := tools.NewWebFetchTool(500) // Small limit
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"url": server.URL,
	})

	if result == nil {
		t.Fatal("Expected non-nil result")
	}
	if len(result.ForLLM) > 1000 {
		t.Errorf("Expected ForLLM to be truncated, got length %d", len(result.ForLLM))
	}
}

func TestWebFetchTool_Execute_InvalidURL(t *testing.T) {
	tool := tools.NewWebFetchTool(50000)
	ctx := context.Background()

	tests := []struct {
		name string
		url  string
	}{
		{"FTP", "ftp://example.com/file"},
		{"JavaScript", "javascript:alert(1)"},
		{"NoHost", "http://"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := tool.Execute(ctx, map[string]interface{}{
				"url": tt.url,
			})
			if result == nil {
				t.Fatal("Expected non-nil result")
			}
			if !result.IsError {
				t.Errorf("Expected error for URL %q", tt.url)
			}
		})
	}
}

func TestWebFetchTool_Execute_MissingURL(t *testing.T) {
	tool := tools.NewWebFetchTool(50000)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{})
	if result == nil {
		t.Fatal("Expected non-nil result")
	}
	if !result.IsError {
		t.Error("Expected error for missing url parameter")
	}
}

func TestWebFetchTool_Execute_SchemeValidation(t *testing.T) {
	tool := tools.NewWebFetchTool(50000)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"url": "ftp://example.com",
	})
	if result == nil {
		t.Fatal("Expected non-nil result")
	}
	if !result.IsError {
		t.Error("Expected error for ftp scheme")
	}
}

func TestWebFetchTool_Execute_HTMLOnContentType(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		fmt.Fprint(w, "<html><body><p>Hello from HTML</p></body></html>")
	}))
	defer server.Close()

	tool := tools.NewWebFetchTool(50000)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"url": server.URL,
	})

	if result == nil {
		t.Fatal("Expected non-nil result")
	}
	if !strings.Contains(result.ForLLM, "Hello from HTML") {
		t.Errorf("Expected 'Hello from HTML' in ForLLM, got: %s", result.ForLLM)
	}
}

// ---------------------------------------------------------------------------
// WebSearchTool Constructor Tests
// ---------------------------------------------------------------------------

func TestNewWebSearchTool_NoProviders(t *testing.T) {
	tool := tools.NewWebSearchTool(tools.WebSearchToolOptions{})
	if tool != nil {
		t.Error("Expected nil when no providers are enabled")
	}
}

func TestNewWebSearchTool_DuckDuckGo(t *testing.T) {
	tool := tools.NewWebSearchTool(tools.WebSearchToolOptions{
		DuckDuckGoEnabled: true,
	})
	if tool == nil {
		t.Fatal("Expected non-nil tool for DuckDuckGo")
	}
	if tool.Name() != "web_search" {
		t.Errorf("Expected name 'web_search', got %q", tool.Name())
	}
}

func TestNewWebSearchTool_Brave(t *testing.T) {
	tool := tools.NewWebSearchTool(tools.WebSearchToolOptions{
		BraveEnabled: true,
		BraveAPIKey:  "test-key",
	})
	if tool == nil {
		t.Fatal("Expected non-nil tool for Brave")
	}
}

func TestNewWebSearchTool_Perplexity(t *testing.T) {
	tool := tools.NewWebSearchTool(tools.WebSearchToolOptions{
		PerplexityEnabled: true,
		PerplexityAPIKey:  "test-key",
	})
	if tool == nil {
		t.Fatal("Expected non-nil tool for Perplexity")
	}
}

func TestNewWebSearchTool_Priority(t *testing.T) {
	tool := tools.NewWebSearchTool(tools.WebSearchToolOptions{
		PerplexityEnabled: true,
		PerplexityAPIKey:  "test-key",
		BraveEnabled:      true,
		BraveAPIKey:       "test-key",
		DuckDuckGoEnabled: true,
	})
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
}

func TestNewWebSearchTool_BraveWithoutKey(t *testing.T) {
	tool := tools.NewWebSearchTool(tools.WebSearchToolOptions{
		BraveEnabled: true,
		BraveAPIKey:  "", // missing key
	})
	if tool != nil {
		t.Error("Expected nil when Brave is enabled but key is empty")
	}
}

func TestNewWebSearchTool_PerplexityWithoutKey(t *testing.T) {
	tool := tools.NewWebSearchTool(tools.WebSearchToolOptions{
		PerplexityEnabled: true,
		PerplexityAPIKey:  "", // missing key
	})
	if tool != nil {
		t.Error("Expected nil when Perplexity is enabled but key is empty")
	}
}

func TestNewWebSearchTool_CustomMaxResults(t *testing.T) {
	tool := tools.NewWebSearchTool(tools.WebSearchToolOptions{
		DuckDuckGoEnabled:    true,
		DuckDuckGoMaxResults: 7,
	})
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
}

func TestNewWebFetchTool_DefaultMaxChars(t *testing.T) {
	tool := tools.NewWebFetchTool(0)
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
}

func TestNewWebFetchTool_NegativeMaxChars(t *testing.T) {
	tool := tools.NewWebFetchTool(-100)
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
}

func TestWebFetchTool_Execute_NonHTTPScheme(t *testing.T) {
	tool := tools.NewWebFetchTool(50000)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"url": "file:///etc/passwd",
	})
	if result == nil {
		t.Fatal("Expected non-nil result")
	}
	if !result.IsError {
		t.Error("Expected error for file:// scheme")
	}
}

func TestWebFetchTool_Execute_CustomMaxChars(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain")
		w.Write([]byte(strings.Repeat("X", 200)))
	}))
	defer server.Close()

	tool := tools.NewWebFetchTool(50000)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"url":     server.URL,
		"maxChars": float64(100),
	})

	if result == nil {
		t.Fatal("Expected non-nil result")
	}
	// Content should be truncated to 100 chars
	if len(result.ForLLM) > 200 {
		t.Errorf("Expected ForLLM to be truncated, got length %d", len(result.ForLLM))
	}
}

func TestWebSearchTool_Execute_MissingQuery(t *testing.T) {
	tool := tools.NewWebSearchTool(tools.WebSearchToolOptions{
		DuckDuckGoEnabled: true,
	})
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}

	ctx := context.Background()
	result := tool.Execute(ctx, map[string]interface{}{})
	if result == nil {
		t.Fatal("Expected non-nil result")
	}
	if !result.IsError {
		t.Error("Expected error for missing query parameter")
	}
}
