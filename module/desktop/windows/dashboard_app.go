//go:build !cross_compile

package windows

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"strings"

	"github.com/276793422/NemesisBot/module/desktop/websocket"
	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
	wailsruntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

// dashboardTokenScript is injected into HTML responses to provide the
// authentication token to the frontend JavaScript.
const dashboardTokenScript = `<script>window.__DASHBOARD_TOKEN__="%s";</script>`

// RunDashboardWindow runs the Dashboard as a Wails window.
// Architecture:
//  1. Wails loads pages through a reverse proxy to the web server
//  2. The proxy injects __DASHBOARD_TOKEN__ into HTML responses
//  3. app.js detects the token and auto-authenticates
//  4. All content is served through wails.localhost, so WebView2 navigation
//     restrictions don't apply
func RunDashboardWindow(windowID string, data *DashboardWindowData, wsClient *websocket.WebSocketClient) error {
	fmt.Fprintf(os.Stderr, "[RunDashboardWindow] Starting: %s (web=%s:%d)\n", windowID, data.WebHost, data.WebPort)

	// Create window
	window := NewDashboardWindow(windowID, data, wsClient)

	// Create reverse proxy to the web server
	targetURL, _ := url.Parse(fmt.Sprintf("http://%s:%d", data.WebHost, data.WebPort))
	proxy := httputil.NewSingleHostReverseProxy(targetURL)
	proxy.Director = func(req *http.Request) {
		req.URL.Scheme = targetURL.Scheme
		req.URL.Host = targetURL.Host
		req.Host = targetURL.Host
		req.Header.Del("X-Forwarded-For")
		req.Header.Del("X-Forwarded-Host")
		req.Header.Del("X-Forwarded-Proto")
	}
	proxy.ErrorHandler = func(w http.ResponseWriter, r *http.Request, err error) {
		fmt.Fprintf(os.Stderr, "[DashboardProxy] Proxy error for %s %s: %v\n", r.Method, r.URL.Path, err)
		http.Error(w, fmt.Sprintf("Proxy error: %v", err), http.StatusBadGateway)
	}

	backendBase := fmt.Sprintf("http://%s:%d", data.WebHost, data.WebPort)
	httpClient := &http.Client{}
	tokenTag := fmt.Sprintf(dashboardTokenScript, data.Token)

	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// WebSocket: proxy directly
		if strings.EqualFold(r.Header.Get("Upgrade"), "websocket") {
			proxy.ServeHTTP(w, r)
			return
		}

		// Fetch from backend
		backendURL := backendBase + r.URL.RequestURI()
		resp, err := httpClient.Get(backendURL)
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

		// For HTML: inject token script before </head>
		if strings.Contains(ct, "text/html") && len(body) > 0 {
			bodyStr := string(body)
			if idx := strings.LastIndex(bodyStr, "</head>"); idx != -1 {
				bodyStr = bodyStr[:idx] + tokenTag + bodyStr[idx:]
			}
			body = []byte(bodyStr)
		}

		// Copy headers EXCEPT Content-Length (Wails injects scripts, changing the length)
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

	err := wails.Run(&options.App{
		Title:  "NemesisBot Dashboard",
		Width:  1280,
		Height: 800,
		AssetServer: &assetserver.Options{
			Handler: handler,
		},
		Bind: []interface{}{
			window,
			&DashboardBindings{window: window},
		},
		OnStartup: func(ctx context.Context) {
			// Deliver token to frontend via Wails event (fallback if HTML injection missed)
			wailsruntime.EventsEmit(ctx, "dashboard-token", data.Token)
			if err := window.Startup(ctx); err != nil {
				fmt.Fprintf(os.Stderr, "[RunDashboardWindow] Startup failed: %v\n", err)
			}
		},
		OnShutdown: func(ctx context.Context) {
			window.Shutdown(ctx)
		},
	})

	if err != nil {
		fmt.Fprintf(os.Stderr, "[RunDashboardWindow] Wails error: %v\n", err)
		return err
	}

	fmt.Fprintf(os.Stderr, "[RunDashboardWindow] Window completed: %s\n", windowID)
	return nil
}
