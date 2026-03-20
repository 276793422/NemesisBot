//go:build darwin || linux

package desktop

import (
	"fmt"
)

// WebView interface defines the unified WebView API
type WebView interface {
	SetHtml(html string)
	Bind(name string, fn interface{})
	Run()
	Destroy()
}

// StubWebView is a placeholder for Unix platforms
type StubWebView struct{}

// SetHtml is a stub that does nothing
func (w *StubWebView) SetHtml(html string) {
	// Stub implementation
}

// Bind is a stub that does nothing
func (w *StubWebView) Bind(name string, fn interface{}) {
	// Stub implementation
}

// Run is a stub that does nothing
func (w *StubWebView) Run() {
	// Stub implementation
}

// Destroy is a stub that does nothing
func (w *StubWebView) Destroy() {
	// Stub implementation
}

// WebViewAdapter wraps platform-specific WebView
type WebViewAdapter struct {
	impl WebView
}

// WebViewConfig holds configuration
type WebViewConfig struct {
	Title  string
	Width  int
	Height int
	Debug  bool
}

// NewWebViewAdapter creates a new WebView adapter for Unix (returns stub)
func NewWebViewAdapter(config WebViewConfig) (*WebViewAdapter, error) {
	return &WebViewAdapter{
		impl: &StubWebView{},
	}, fmt.Errorf("desktop UI not yet implemented for this platform")
}

// SetHtml forwards to stub implementation
func (a *WebViewAdapter) SetHtml(html string) {
	a.impl.SetHtml(html)
}

// Bind forwards to stub implementation
func (a *WebViewAdapter) Bind(name string, fn interface{}) {
	a.impl.Bind(name, fn)
}

// Run forwards to stub implementation
func (a *WebViewAdapter) Run() {
	a.impl.Run()
}

// Destroy forwards to stub implementation
func (a *WebViewAdapter) Destroy() {
	a.impl.Destroy()
}

// BindFunction is a stub for Unix platforms
func BindFunction(w interface{}, name string, fn interface{}) {
	// Stub implementation
}

// Dispatch is a stub for Unix platforms
func Dispatch(w interface{}, f func()) {
	// Stub implementation - execute directly
	if f != nil {
		f()
	}
}
