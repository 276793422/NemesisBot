//go:build windows

package desktop

import (
	"embed"
	"fmt"
	"os/exec"
	"strconv"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/services"
	webview "github.com/shmspace/webview2"
)

//go:embed static/*
var staticFiles embed.FS

// Run starts the desktop UI with webview2 (Windows)
func Run(cfg *Config) {
	RunWithCallbacks(cfg, nil, nil, nil)
}

// RunWithCallbacks starts the desktop UI with access to main program components
func RunWithCallbacks(cfg *Config, agentLoop *agent.AgentLoop, msgBus *bus.MessageBus, channelManager *channels.Manager) {
	logger.InfoC("desktop", "Initializing NemesisBot Desktop (Windows WebView2)")
	logger.InfoC("desktop", "========================================")
	logger.InfoC("desktop", "")

	// Calculate optimal window size
	windowWidth, windowHeight := calculateOptimalWindowSize()
	if cfg.Width > 0 {
		windowWidth = cfg.Width
	}
	if cfg.Height > 0 {
		windowHeight = cfg.Height
	}

	logger.InfoCF("desktop", "Window configuration", map[string]interface{}{
		"width":  windowWidth,
		"height": windowHeight,
	})

	logger.InfoC("desktop", "Creating WebView2 window...")

	// Validate window size before creating WebView2
	if windowWidth < 400 || windowHeight < 300 {
		logger.ErrorCF("desktop", "Invalid window size detected, using safe defaults", map[string]interface{}{
			"requested_width":  windowWidth,
			"requested_height": windowHeight,
		})
		windowWidth = 1280
		windowHeight = 800
	}

	logger.InfoCF("desktop", "Creating WebView2 with window size", map[string]interface{}{
		"width":  windowWidth,
		"height": windowHeight,
	})

	// Create webview with precise window control
	debug := cfg.Debug
	w := webview.NewWithOptions(webview.WebViewOptions{
		Debug:     debug,
		AutoFocus: true,
		WindowOptions: webview.WindowOptions{
			Title:  "NemesisBot Desktop",
			Width:  uint(windowWidth),
			Height: uint(windowHeight),
			Center: true,
		},
	})

	if w == nil {
		logger.ErrorC("desktop", "Failed to create WebView2 window")
		logger.ErrorC("desktop", "Please ensure WebView2 Runtime is installed")
		logger.ErrorC("desktop", "Download from: https://developer.microsoft.com/en-us/microsoft-edge/webview2/")
		return
	}

	defer w.Destroy()

	// Set window size explicitly
	w.SetSize(windowWidth, windowHeight, webview.HintFixed)

	// Bind Go functions to JavaScript with access to main program
	bindFunctionsWithCallbacks(w, agentLoop, msgBus, channelManager)

	// Load HTML content
	htmlContent, err := loadStaticFiles()
	if err != nil {
		logger.ErrorCF("desktop", "Error loading static files", map[string]interface{}{
			"error": err.Error(),
		})
		return
	}

	// Validate HTML content before setting
	if len(htmlContent) == 0 {
		logger.ErrorC("desktop", "HTML content is empty, cannot initialize WebView2")
		return
	}

	// Set HTML content with error handling
	defer func() {
		if r := recover(); r != nil {
			logger.ErrorCF("desktop", "Panic when setting HTML content", map[string]interface{}{
				"error": r,
			})
		}
	}()

	w.SetHtml(htmlContent)

	logger.InfoCF("desktop", "WebView2 window created", map[string]interface{}{
		"width":  windowWidth,
		"height": windowHeight,
	})

	logger.InfoC("desktop", "")
	logger.InfoC("desktop", "========================================")
	logger.InfoC("desktop", " NemesisBot Desktop is running!")
	logger.InfoC("desktop", "========================================")
	logger.InfoCF("desktop", "Window", map[string]interface{}{
		"width":  windowWidth,
		"height": windowHeight,
	})
	logger.InfoC("desktop", "")

	// Run webview (blocking - this is intentional as it runs in a goroutine)
	// Add panic recovery for the main webview loop
	defer func() {
		if r := recover(); r != nil {
			logger.ErrorCF("desktop", "Panic in webview main loop", map[string]interface{}{
				"error": r,
			})
		}
	}()

	w.Run()

	logger.InfoC("desktop", "Desktop window closed")
}

// RunWithServiceManager starts the desktop UI with ServiceManager integration
func RunWithServiceManager(cfg *Config, svcMgr *services.ServiceManager) {
	logger.InfoC("desktop", "Initializing NemesisBot Desktop (Windows WebView2)")
	logger.InfoC("desktop", "========================================")
	logger.InfoC("desktop", "")

	// Calculate optimal window size
	windowWidth, windowHeight := calculateOptimalWindowSize()
	if cfg.Width > 0 {
		windowWidth = cfg.Width
	}
	if cfg.Height > 0 {
		windowHeight = cfg.Height
	}

	logger.InfoCF("desktop", "Window configuration", map[string]interface{}{
		"width":  windowWidth,
		"height": windowHeight,
	})

	logger.InfoC("desktop", "Creating WebView2 window...")

	// Validate window size before creating WebView2
	if windowWidth < 400 || windowHeight < 300 {
		logger.ErrorCF("desktop", "Invalid window size detected, using safe defaults", map[string]interface{}{
			"requested_width":  windowWidth,
			"requested_height": windowHeight,
		})
		windowWidth = 1280
		windowHeight = 800
	}

	logger.InfoCF("desktop", "Creating WebView2 with window size", map[string]interface{}{
		"width":  windowWidth,
		"height": windowHeight,
	})

	// Create webview with precise window control
	debug := cfg.Debug
	w := webview.NewWithOptions(webview.WebViewOptions{
		Debug:     debug,
		AutoFocus: true,
		WindowOptions: webview.WindowOptions{
			Title:  "NemesisBot Desktop",
			Width:  uint(windowWidth),
			Height: uint(windowHeight),
			Center: true,
		},
	})

	if w == nil {
		logger.ErrorC("desktop", "Failed to create WebView2 window")
		logger.ErrorC("desktop", "Please ensure WebView2 Runtime is installed")
		logger.ErrorC("desktop", "Download from: https://developer.microsoft.com/en-us/microsoft-edge/webview2/")
		return
	}

	defer w.Destroy()

	// Set window size explicitly
	w.SetSize(windowWidth, windowHeight, webview.HintFixed)

	// Bind Go functions to JavaScript with ServiceManager
	bindFunctionsWithServiceManager(w, svcMgr)

	// Load HTML content
	htmlContent, err := loadStaticFiles()
	if err != nil {
		logger.ErrorCF("desktop", "Error loading static files", map[string]interface{}{
			"error": err.Error(),
		})
		return
	}

	// Validate HTML content before setting
	if len(htmlContent) == 0 {
		logger.ErrorC("desktop", "HTML content is empty, cannot initialize WebView2")
		return
	}

	// Set HTML content with error handling
	defer func() {
		if r := recover(); r != nil {
			logger.ErrorCF("desktop", "Panic when setting HTML content", map[string]interface{}{
				"error": r,
			})
		}
	}()

	w.SetHtml(htmlContent)

	logger.InfoCF("desktop", "WebView2 window created", map[string]interface{}{
		"width":  windowWidth,
		"height": windowHeight,
	})

	logger.InfoC("desktop", "")
	logger.InfoC("desktop", "========================================")
	logger.InfoC("desktop", " NemesisBot Desktop is running!")
	logger.InfoC("desktop", "========================================")
	logger.InfoCF("desktop", "Window", map[string]interface{}{
		"width":  windowWidth,
		"height": windowHeight,
	})
	logger.InfoC("desktop", "")

	// Run webview (blocking - this is intentional as it runs in a goroutine)
	// Add panic recovery for the main webview loop
	defer func() {
		if r := recover(); r != nil {
			logger.ErrorCF("desktop", "Panic in webview main loop", map[string]interface{}{
				"error": r,
			})
		}
	}()

	w.Run()

	logger.InfoC("desktop", "Desktop window closed")
}

// bindFunctionsWithCallbacks binds Go functions to JavaScript with access to main program
func bindFunctionsWithCallbacks(w webview.WebView, agentLoop *agent.AgentLoop, msgBus *bus.MessageBus, channelManager *channels.Manager) {
	// Get system configuration
	w.Bind("getConfig", func() SystemInfo {
		return SystemInfo{
			Name:    "NemesisBot",
			Version: "0.3.34",
			Status:  "running",
			OS:      "windows",
			Arch:    "amd64",
		}
	})

	// Get version
	w.Bind("getVersion", func() string {
		return "0.3.34"
	})

	// Send message to agent
	w.Bind("sendMessage", func(msg string) string {
		logger.InfoCF("desktop", "Message received", map[string]interface{}{
			"message": msg,
		})

		// If agentLoop is available, send to agent
		if agentLoop != nil && msgBus != nil {
			// Create inbound message
			inboundMsg := bus.InboundMessage{
				Channel:       "desktop",
				SenderID:      "desktop-ui",
				ChatID:        "desktop-chat",
				Content:       msg,
				SessionKey:    "desktop-session",
				CorrelationID: fmt.Sprintf("desktop-%d", time.Now().UnixNano()),
			}

			// Publish to message bus
			msgBus.PublishInbound(inboundMsg)

			// Wait for response (simplified - in real implementation, use correlation ID)
			return "Message sent to agent"
		}

		// Fallback: echo message
		return fmt.Sprintf("Echo: %s", msg)
	})

	// Execute command
	w.Bind("executeCommand", func(cmd string) (string, error) {
		logger.InfoCF("desktop", "Executing command", map[string]interface{}{
			"command": cmd,
		})

		// Security check: only allow safe commands
		// TODO: Implement proper security checks

		return "", fmt.Errorf("command execution not implemented")
	})

	// Log message
	w.Bind("log", func(level, message string) {
		switch level {
		case "info":
			logger.InfoC("desktop-ui", message)
		case "warn":
			logger.WarnC("desktop-ui", message)
		case "error":
			logger.ErrorC("desktop-ui", message)
		default:
			logger.InfoCF("desktop-ui", message, map[string]interface{}{
				"level": level,
			})
		}
	})

	// Get health status
	w.Bind("getHealth", func() map[string]interface{} {
		status := map[string]interface{}{
			"status":  "ok",
			"service": "nemesisbot-desktop",
			"version": "0.3.34",
			"uptime":  time.Since(time.Now()).String(),
		}

		// Add agent status if available
		if agentLoop != nil {
			status["agent"] = "running"
		}

		// Add channel manager status if available
		if channelManager != nil {
			status["channels"] = "connected"
		}

		return status
	})

	// Navigate to page
	w.Bind("navigate", func(page string) {
		logger.InfoCF("desktop", "Navigate to page", map[string]interface{}{
			"page": page,
		})
		// Page navigation is handled by JS
	})

	// Set theme
	w.Bind("setTheme", func(theme string) {
		logger.InfoCF("desktop", "Theme changed", map[string]interface{}{
			"theme": theme,
		})
		// Theme change is handled by JS
	})

	logger.InfoC("desktop", "Go functions bound to JavaScript (Windows WebView2)")
}

// bindFunctionsWithServiceManager binds Go functions to JavaScript with ServiceManager
func bindFunctionsWithServiceManager(w webview.WebView, svcMgr *services.ServiceManager) {
	// Wrap all bindings in panic recovery
	defer func() {
		if r := recover(); r != nil {
			logger.ErrorCF("desktop", "Panic in bindFunctionsWithServiceManager", map[string]interface{}{
				"error": r,
			})
		}
	}()

	// Get system configuration
	w.Bind("getConfig", func() (*config.Config, error) {
		defer func() {
			if r := recover(); r != nil {
				logger.ErrorCF("desktop", "Panic in getConfig binding", map[string]interface{}{
					"error": r,
				})
			}
		}()
		return config.LoadConfig(services.GetConfigPath())
	})

	// Get version
	w.Bind("getVersion", func() string {
		return "0.3.34"
	})

	// Get bot state
	w.Bind("getBotState", func() map[string]interface{} {
		state := svcMgr.GetBotState()
		result := map[string]interface{}{
			"state": state.String(),
		}

		if err := svcMgr.GetBotError(); err != nil {
			result["error"] = err.Error()
		}

		return result
	})

	// Start bot service
	w.Bind("startBot", func() error {
		logger.InfoC("desktop", "User requested to start bot service")
		return svcMgr.StartBot()
	})

	// Stop bot service
	w.Bind("stopBot", func() error {
		logger.InfoC("desktop", "User requested to stop bot service")
		return svcMgr.StopBot()
	})

	// Restart bot service
	w.Bind("restartBot", func() error {
		logger.InfoC("desktop", "User requested to restart bot service")
		return svcMgr.RestartBot()
	})

	// Send message to agent
	w.Bind("sendMessage", func(msg string) string {
		logger.InfoCF("desktop", "Message received", map[string]interface{}{
			"message": msg,
		})

		// Get bot components
		components := svcMgr.GetBotComponents()
		if components == nil {
			return "Error: Bot service is not running"
		}

		_, ok := components["agentLoop"].(*agent.AgentLoop)
		if !ok {
			return "Error: Agent loop not available"
		}

		msgBus, ok := components["msgBus"].(*bus.MessageBus)
		if !ok {
			return "Error: Message bus not available"
		}

		// Create inbound message
		inboundMsg := bus.InboundMessage{
			Channel:       "desktop",
			SenderID:      "desktop-ui",
			ChatID:        "desktop-chat",
			Content:       msg,
			SessionKey:    "desktop-session",
			CorrelationID: fmt.Sprintf("desktop-%d", time.Now().UnixNano()),
		}

		// Publish to message bus
		msgBus.PublishInbound(inboundMsg)

		return "Message sent to agent"
	})

	// Execute command
	w.Bind("executeCommand", func(cmd string) (string, error) {
		logger.InfoCF("desktop", "Executing command", map[string]interface{}{
			"command": cmd,
		})

		// Security check: only allow safe commands
		// TODO: Implement proper security checks

		return "", fmt.Errorf("command execution not implemented")
	})

	// Log message
	w.Bind("log", func(level, message string) {
		switch level {
		case "info":
			logger.InfoC("desktop-ui", message)
		case "warn":
			logger.WarnC("desktop-ui", message)
		case "error":
			logger.ErrorC("desktop-ui", message)
		default:
			logger.InfoCF("desktop-ui", message, map[string]interface{}{
				"level": level,
			})
		}
	})

	// Get health status
	w.Bind("getHealth", func() map[string]interface{} {
		status := map[string]interface{}{
			"status":  "ok",
			"service": "nemesisbot-desktop",
			"version": "0.3.34",
			"uptime":  time.Since(time.Now()).String(),
		}

		// Add bot state
		botState := svcMgr.GetBotState()
		status["bot_state"] = botState.String()
		status["bot_running"] = botState.IsRunning()

		return status
	})

	// Navigate to page
	w.Bind("navigate", func(page string) {
		logger.InfoCF("desktop", "Navigate to page", map[string]interface{}{
			"page": page,
		})
		// Page navigation is handled by JS
	})

	// Set theme
	w.Bind("setTheme", func(theme string) {
		logger.InfoCF("desktop", "Theme changed", map[string]interface{}{
			"theme": theme,
		})
		// Theme change is handled by JS
	})

	logger.InfoC("desktop", "Go functions bound to JavaScript (Windows WebView2 with ServiceManager)")
}

// calculateOptimalWindowSize calculates the best window size (Windows)
func calculateOptimalWindowSize() (width, height int) {
	const (
		defaultWidth  = 1280
		defaultHeight = 800
		minWidth      = 800
		minHeight     = 600
	)

	// Get desktop screen size
	screenWidth, screenHeight := getScreenSizeWindows()

	logger.InfoCF("desktop", "Desktop screen size detected", map[string]interface{}{
		"width":  screenWidth,
		"height": screenHeight,
	})

	// Validate screen size - if invalid, use defaults
	if screenWidth <= 0 || screenHeight <= 0 {
		logger.WarnC("desktop", "Invalid screen size detected, using defaults")
		return defaultWidth, defaultHeight
	}

	// Check if default size fits
	if screenWidth >= defaultWidth && screenHeight >= defaultHeight {
		return defaultWidth, defaultHeight
	}

	// Desktop is smaller, calculate scaled size
	width, height = defaultWidth, defaultHeight

	// Scale down by 10% until it fits
	for i := 0; i < 20; i++ {
		if width <= screenWidth && height <= screenHeight {
			if width >= minWidth && height >= minHeight {
				logger.InfoCF("desktop", "Window scaled to fit screen", map[string]interface{}{
					"iterations":   i + 1,
					"scale":        fmt.Sprintf("%d%%", 100-(i+1)*10),
					"final_width":  width,
					"final_height": height,
				})
				return width, height
			}
			break
		}

		width = int(float64(width) * 0.90)
		height = int(float64(height) * 0.90)
	}

	logger.WarnC("desktop", "Screen too small, using minimum window size")
	return minWidth, minHeight
}

// getScreenSizeWindows returns the screen dimensions (Windows)
func getScreenSizeWindows() (width, height int) {
	// On Windows, use PowerShell to get screen resolution
	cmd := exec.Command("powershell", "-NonInteractive", "-Command",
		"Add-Type -AssemblyName System.Windows.Forms; "+
			"[System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width, "+
			"[System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Height")
	output, err := cmd.CombinedOutput()
	if err == nil && len(output) > 0 {
		// Parse output - trim whitespace and split
		outputStr := string(output)
		// Try to parse two integers
		var w, h int
		n, err := fmt.Sscanf(outputStr, "%d %d", &w, &h)
		if n == 2 && err == nil && w > 0 && h > 0 {
			return w, h
		}
		// If Sscanf failed, try splitting by whitespace
		parts := strings.Fields(outputStr)
		if len(parts) >= 2 {
			w, err1 := strconv.Atoi(strings.TrimSpace(parts[0]))
			h, err2 := strconv.Atoi(strings.TrimSpace(parts[1]))
			if err1 == nil && err2 == nil && w > 0 && h > 0 {
				return w, h
			}
		}
	}
	// Fallback: assume common resolution
	logger.InfoC("desktop", "Could not detect screen size, using default 1920x1080")
	return 1920, 1080
}

// loadStaticFiles loads and prepares the HTML content (Windows)
func loadStaticFiles() (string, error) {
	// Read index.html
	indexHTML, err := staticFiles.ReadFile("static/index.html")
	if err != nil {
		return "", fmt.Errorf("error reading index.html: %w", err)
	}

	// Read CSS files
	themeCSS, err := staticFiles.ReadFile("static/css/theme.css")
	if err != nil {
		return "", fmt.Errorf("error reading theme.css: %w", err)
	}

	layoutCSS, err := staticFiles.ReadFile("static/css/layout.css")
	if err != nil {
		return "", fmt.Errorf("error reading layout.css: %w", err)
	}

	// Read JS file
	appJS, err := staticFiles.ReadFile("static/js/app.js")
	if err != nil {
		return "", fmt.Errorf("error reading app.js: %w", err)
	}

	// Combine all content into a single HTML document
	htmlContent := fmt.Sprintf(`<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>NemesisBot Desktop</title>
    <style>
%s
%s
    </style>
</head>
<body>
%s
    <script>
%s
    </script>
</body>
</html>`, string(themeCSS), string(layoutCSS), string(indexHTML), string(appJS))

	return htmlContent, nil
}
