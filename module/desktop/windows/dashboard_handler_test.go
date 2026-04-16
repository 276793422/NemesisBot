//go:build !cross_compile

package windows

import (
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"net/http/httputil"
	"net/url"
	"strings"
	"testing"
)

// TestDashboardProxyInjectsToken verifies the proxy injects the token script into HTML.
func TestDashboardProxyInjectsToken(t *testing.T) {
	// Create a mock backend server
	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.Write([]byte("<html><head><title>Test</title></head><body>Hello</body></html>"))
	}))
	defer backend.Close()

	backendURL, _ := url.Parse(backend.URL)
	proxy := NewTestProxy(backendURL, "test-token-123")

	req := httptest.NewRequest("GET", "/", nil)
	rec := httptest.NewRecorder()
	proxy.ServeHTTP(rec, req)

	if rec.Code != 200 {
		t.Fatalf("Expected 200, got %d", rec.Code)
	}

	body := rec.Body.String()
	if !strings.Contains(body, "__DASHBOARD_TOKEN__") {
		t.Error("Token script not injected into HTML")
	}
	if !strings.Contains(body, "test-token-123") {
		t.Error("Token value not in response")
	}
	if !strings.Contains(body, "<title>Test</title>") {
		t.Error("Original HTML content lost")
	}
	// Verify injection is before </head>
	headIdx := strings.Index(body, "</head>")
	tokenIdx := strings.Index(body, "__DASHBOARD_TOKEN__")
	if headIdx == -1 || tokenIdx == -1 || tokenIdx > headIdx {
		t.Error("Token script should be injected before </head>")
	}
}

// TestDashboardProxyPreservesNonHTML verifies non-HTML responses pass through unchanged.
func TestDashboardProxyPreservesNonHTML(t *testing.T) {
	cssContent := "body { background: red; }"

	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/css")
		w.Write([]byte(cssContent))
	}))
	defer backend.Close()

	backendURL, _ := url.Parse(backend.URL)
	proxy := NewTestProxy(backendURL, "token")

	req := httptest.NewRequest("GET", "/style.css", nil)
	rec := httptest.NewRecorder()
	proxy.ServeHTTP(rec, req)

	body := rec.Body.String()
	if body != cssContent {
		t.Errorf("CSS content modified: got %q, want %q", body, cssContent)
	}
	if strings.Contains(body, "__DASHBOARD_TOKEN__") {
		t.Error("Token should NOT be injected into non-HTML responses")
	}
}

// TestDashboardProxyPassesThroughJS verifies JS files pass through unchanged.
func TestDashboardProxyPassesThroughJS(t *testing.T) {
	jsContent := "console.log('hello');"

	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/javascript")
		w.Write([]byte(jsContent))
	}))
	defer backend.Close()

	backendURL, _ := url.Parse(backend.URL)
	proxy := NewTestProxy(backendURL, "token")

	req := httptest.NewRequest("GET", "/app.js", nil)
	rec := httptest.NewRecorder()
	proxy.ServeHTTP(rec, req)

	body := rec.Body.String()
	if body != jsContent {
		t.Errorf("JS content modified: got %q, want %q", body, jsContent)
	}
}

// Ensure io.Writer is satisfied by responseRecorder
var _ io.Writer = (*responseRecorder)(nil)
func NewTestProxy(target *url.URL, token string) *DashboardProxy {
	return &DashboardProxy{
		proxy: func() *httputil.ReverseProxy {
			p := httputil.NewSingleHostReverseProxy(target)
			p.Director = func(req *http.Request) {
				req.URL.Scheme = target.Scheme
				req.URL.Host = target.Host
				req.Host = target.Host
			}
			return p
		}(),
		tokenTag: fmt.Sprintf(`<script>window.__DASHBOARD_TOKEN__="%s";</script>`, token),
	}
}

// Ensure io.Writer is satisfied by responseRecorder
var _ io.Writer = (*responseRecorder)(nil)
