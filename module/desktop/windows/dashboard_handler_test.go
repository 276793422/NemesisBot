//go:build !cross_compile

package windows

import (
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// TestDashboardHandlerInjectsToken verifies the handler injects the init script into HTML.
func TestDashboardHandlerInjectsToken(t *testing.T) {
	// Create a mock backend server
	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.Write([]byte("<html><head><title>Test</title></head><body>Hello</body></html>"))
	}))
	defer backend.Close()

	tokenTag := fmt.Sprintf(`<script>window.__DASHBOARD_TOKEN__="%s";window.__DASHBOARD_BACKEND__="%s:%d";</script>`, "test-token-123", "127.0.0.1", 49000)

	handler := newTestHandler(backend.URL, tokenTag)

	req := httptest.NewRequest("GET", "/", nil)
	rec := httptest.NewRecorder()
	handler.ServeHTTP(rec, req)

	if rec.Code != 200 {
		t.Fatalf("Expected 200, got %d", rec.Code)
	}

	body := rec.Body.String()
	if !strings.Contains(body, "__DASHBOARD_TOKEN__") {
		t.Error("Token script not injected into HTML")
	}
	if !strings.Contains(body, "__DASHBOARD_BACKEND__") {
		t.Error("Backend URL not injected into HTML")
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
		t.Error("Init script should be injected before </head>")
	}
}

// TestDashboardHandlerPreservesNonHTML verifies non-HTML responses pass through unchanged.
func TestDashboardHandlerPreservesNonHTML(t *testing.T) {
	cssContent := "body { background: red; }"

	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/css")
		w.Write([]byte(cssContent))
	}))
	defer backend.Close()

	tokenTag := fmt.Sprintf(`<script>window.__DASHBOARD_TOKEN__="%s";window.__DASHBOARD_BACKEND__="%s:%d";</script>`, "token", "127.0.0.1", 49000)

	handler := newTestHandler(backend.URL, tokenTag)

	req := httptest.NewRequest("GET", "/style.css", nil)
	rec := httptest.NewRecorder()
	handler.ServeHTTP(rec, req)

	body := rec.Body.String()
	if body != cssContent {
		t.Errorf("CSS content modified: got %q, want %q", body, cssContent)
	}
	if strings.Contains(body, "__DASHBOARD_TOKEN__") {
		t.Error("Token should NOT be injected into non-HTML responses")
	}
}

// TestDashboardHandlerPassesThroughJS verifies JS files pass through unchanged.
func TestDashboardHandlerPassesThroughJS(t *testing.T) {
	jsContent := "console.log('hello');"

	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/javascript")
		w.Write([]byte(jsContent))
	}))
	defer backend.Close()

	tokenTag := fmt.Sprintf(`<script>window.__DASHBOARD_TOKEN__="%s";window.__DASHBOARD_BACKEND__="%s:%d";</script>`, "token", "127.0.0.1", 49000)

	handler := newTestHandler(backend.URL, tokenTag)

	req := httptest.NewRequest("GET", "/app.js", nil)
	rec := httptest.NewRecorder()
	handler.ServeHTTP(rec, req)

	body := rec.Body.String()
	if body != jsContent {
		t.Errorf("JS content modified: got %q, want %q", body, jsContent)
	}
}

// newTestHandler creates an HTTP handler that proxies to backendURL and injects tokenTag into HTML.
func newTestHandler(backendURL, tokenTag string) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		resp, err := http.Get(backendURL + r.URL.RequestURI())
		if err != nil {
			http.Error(w, err.Error(), http.StatusBadGateway)
			return
		}
		defer resp.Body.Close()

		body, err := io.ReadAll(resp.Body)
		if err != nil {
			http.Error(w, err.Error(), http.StatusBadGateway)
			return
		}

		ct := resp.Header.Get("Content-Type")

		if strings.Contains(ct, "text/html") && len(body) > 0 {
			bodyStr := string(body)
			if idx := strings.LastIndex(bodyStr, "</head>"); idx != -1 {
				bodyStr = bodyStr[:idx] + tokenTag + bodyStr[idx:]
			}
			body = []byte(bodyStr)
		}

		for k, vals := range resp.Header {
			if strings.EqualFold(k, "Content-Length") {
				continue
			}
			for _, v := range vals {
				w.Header().Add(k, v)
			}
		}

		w.WriteHeader(resp.StatusCode)
		w.Write(body)
	})
}
