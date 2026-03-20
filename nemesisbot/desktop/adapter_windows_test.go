//go:build windows

package desktop

import (
	"testing"
)

func TestNewWebViewAdapter(t *testing.T) {
	config := WebViewConfig{
		Title:  "Test Window",
		Width:  800,
		Height: 600,
		Debug:  false,
	}

	adapter, err := NewWebViewAdapter(config)

	if err != nil {
		t.Fatalf("Failed to create WebView adapter: %v", err)
	}

	if adapter == nil {
		t.Fatal("Adapter is nil")
	}

	if adapter.impl == nil {
		t.Error("Adapter implementation is nil")
	}

	// Clean up
	adapter.Destroy()

	t.Log("WebView adapter created and tested successfully")
}

func TestWebViewAdapterMethods(t *testing.T) {
	config := WebViewConfig{
		Title:  "Test Window",
		Width:  800,
		Height: 600,
		Debug:  false,
	}

	adapter, err := NewWebViewAdapter(config)
	if err != nil {
		t.Fatalf("Failed to create WebView adapter: %v", err)
	}
	defer adapter.Destroy()

	// Test SetHtml
	htmlContent := "<html><body>Test</body></html>"
	adapter.SetHtml(htmlContent)
	t.Log("SetHtml called successfully")

	// Test Bind
	adapter.Bind("testFunc", func() string {
		return "test"
	})
	t.Log("Bind called successfully")

	// Note: We can't test Run() here as it's blocking
}
