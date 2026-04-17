//go:build !cross_compile

package windows

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"

	"github.com/276793422/NemesisBot/module/desktop/websocket"
	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
	wailsruntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

const dashboardInitScript = `<script>window.__DASHBOARD_TOKEN__="%s";window.__DASHBOARD_BACKEND__="%s:%d";</script>`

// RunDashboardWindow runs the Dashboard as a Wails window.
func RunDashboardWindow(windowID string, data *DashboardWindowData, wsClient *websocket.WebSocketClient) error {
	fmt.Fprintf(os.Stderr, "[RunDashboardWindow] Starting: %s (web=%s:%d)\n", windowID, data.WebHost, data.WebPort)

	window := NewDashboardWindow(windowID, data, wsClient)

	backendBase := fmt.Sprintf("http://%s:%d", data.WebHost, data.WebPort)
	httpClient := &http.Client{}
	tokenTag := fmt.Sprintf(dashboardInitScript, data.Token, data.WebHost, data.WebPort)

	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
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

		if strings.Contains(ct, "text/html") && len(body) > 0 {
			bodyStr := string(body)
			// Inject token before </head>
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
