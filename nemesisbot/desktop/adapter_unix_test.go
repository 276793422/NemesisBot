//go:build darwin || linux

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

	if err == nil {
		t.Log("Note: WebView adapter created (stub implementation)")
	}

	if adapter == nil {
		t.Fatal("Adapter should not be nil even on Unix platforms")
	}

	if adapter.impl == nil {
		t.Error("Adapter implementation should not be nil")
	}

	// Clean up
	adapter.Destroy()

	t.Log("Stub WebView adapter tested successfully")
}

func TestStubWebViewMethods(t *testing.T) {
	stub := &StubWebView{}

	// Test all stub methods
	stub.SetHtml("<html></html>")
	t.Log("SetHtml stub called")

	stub.Bind("test", func() {})
	t.Log("Bind stub called")

	stub.Run()
	t.Log("Run stub called")

	stub.Destroy()
	t.Log("Destroy stub called")

	t.Log("All stub methods execute without panic")
}

func TestBindFunction(t *testing.T) {
	// Test that BindFunction doesn't panic
	BindFunction(nil, "test", func() {})
	t.Log("BindFunction stub executed")
}

func TestDispatch(t *testing.T) {
	// Test that Dispatch executes the function
	called := false
	Dispatch(nil, func() {
		called = true
	})

	if !called {
		t.Error("Dispatch should execute the function")
	}

	t.Log("Dispatch stub executed successfully")
}
