//go:build !windows && !darwin && !linux

package desktop

import (
	"fmt"
)

// WebView interface defines the unified WebView API across all platforms
type WebView interface {
	// SetHtml sets the HTML content
	SetHtml(html string)

	// Bind binds a Go function to JavaScript
	Bind(name string, fn interface{})

	// Run starts the WebView main loop
	Run()

	// Destroy destroys the WebView
	Destroy()
}

// WebViewAdapter wraps platform-specific WebView implementations
type WebViewAdapter struct {
	impl WebView
}

// NewWebViewAdapter creates a new WebView adapter for the current platform
func NewWebViewAdapter(config WebViewConfig) (*WebViewAdapter, error) {
	// This is a placeholder - actual implementation is in platform-specific files
	return nil, fmt.Errorf("WebView not implemented for this platform")
}

// WebViewConfig holds configuration for creating a WebView
type WebViewConfig struct {
	Title  string
	Width  int
	Height int
	Debug  bool
}

// BindFunction is a helper to bind functions regardless of platform
func BindFunction(w interface{}, name string, fn interface{}) {
	// Platform-specific implementation handles this
}

// Dispatch executes a function on the WebView thread (for thread safety)
func Dispatch(w interface{}, f func()) {
	// Platform-specific implementation handles this
}
