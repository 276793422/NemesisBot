//go:build windows

package desktop

import (
	"fmt"

	webview "github.com/shmspace/webview2"
)

// WebView interface defines the unified WebView API
type WebView interface {
	SetHtml(html string)
	Bind(name string, fn interface{})
	Run()
	Destroy()
}

// WindowsWebView wraps webview2.WebView
type WindowsWebView struct {
	w webview.WebView
}

// SetHtml sets HTML content
func (w *WindowsWebView) SetHtml(html string) {
	w.w.SetHtml(html)
}

// Bind binds a Go function to JavaScript
func (w *WindowsWebView) Bind(name string, fn interface{}) {
	w.w.Bind(name, fn)
}

// Run starts the WebView main loop
func (w *WindowsWebView) Run() {
	w.w.Run()
}

// Destroy destroys the WebView
func (w *WindowsWebView) Destroy() {
	w.w.Destroy()
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

// NewWebViewAdapter creates a new WebView adapter for Windows
func NewWebViewAdapter(config WebViewConfig) (*WebViewAdapter, error) {
	w := webview.NewWithOptions(webview.WebViewOptions{
		Debug:     config.Debug,
		AutoFocus: true,
		WindowOptions: webview.WindowOptions{
			Title:  config.Title,
			Width:  uint(config.Width),
			Height: uint(config.Height),
			Center: true,
		},
	})

	if w == nil {
		return nil, fmt.Errorf("failed to create WebView2 instance")
	}

	return &WebViewAdapter{
		impl: &WindowsWebView{w: w},
	}, nil
}

// SetHtml forwards to implementation
func (a *WebViewAdapter) SetHtml(html string) {
	a.impl.SetHtml(html)
}

// Bind forwards to implementation
func (a *WebViewAdapter) Bind(name string, fn interface{}) {
	a.impl.Bind(name, fn)
}

// Run forwards to implementation
func (a *WebViewAdapter) Run() {
	a.impl.Run()
}

// Destroy forwards to implementation
func (a *WebViewAdapter) Destroy() {
	a.impl.Destroy()
}

// BindFunction binds a function to JavaScript
func BindFunction(w interface{}, name string, fn interface{}) {
	if wb, ok := w.(webview.WebView); ok {
		wb.Bind(name, fn)
	}
}

// Dispatch executes on WebView thread (Windows webview2 handles this automatically)
func Dispatch(w interface{}, f func()) {
	// webview2 handles thread safety automatically
	if f != nil {
		f()
	}
}
