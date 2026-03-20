//go:build darwin || linux

package desktop

import (
	"fmt"
	"runtime"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/services"
)

// Run starts the desktop UI (stub for Unix platforms)
func Run(cfg *Config) {
	logger.WarnC("desktop", fmt.Sprintf("Desktop UI is not yet implemented for %s", runtime.GOOS))
	logger.InfoC("desktop", "Please use the Web UI instead:")
	logger.InfoC("desktop", "  nemesisbot gateway")
	logger.InfoC("desktop", "")
	logger.InfoC("desktop", "Then open your browser to:")
	logger.InfoC("desktop", "  http://localhost:8080")
}

// RunWithCallbacks starts the desktop UI with callbacks (stub for Unix platforms)
func RunWithCallbacks(cfg *Config, agentLoop *agent.AgentLoop, msgBus *bus.MessageBus, channelManager *channels.Manager) {
	Run(cfg)
}

// RunWithServiceManager starts the desktop UI with ServiceManager (stub for Unix platforms)
func RunWithServiceManager(cfg *Config, svcMgr *services.ServiceManager) {
	Run(cfg)
}

// CheckSystemRequirements checks system requirements (stub for Unix platforms)
func CheckSystemRequirements() bool {
	logger.InfoC("desktop", fmt.Sprintf("Desktop UI is not yet implemented for %s", runtime.GOOS))
	logger.InfoC("desktop", "The Web UI is fully supported and recommended for this platform")
	return false
}

// GetWebView2Version returns WebView2 version (not applicable on Unix)
func GetWebView2Version() *WebView2VersionInfo {
	return &WebView2VersionInfo{
		Installed: false,
		Version:   "N/A",
		Path:      "N/A",
	}
}
