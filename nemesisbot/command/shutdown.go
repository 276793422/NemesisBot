package command

import (
	"fmt"
	"os"
	"os/signal"
	"syscall"

	"github.com/276793422/NemesisBot/module/desktop/systray"
	"github.com/276793422/NemesisBot/module/logger"
)

// globalShutdownChan is the global shutdown channel
// It allows any component (system tray, desktop UI, WebSocket, etc.) to trigger application shutdown
var globalShutdownChan chan struct{}

// globalSystemTray holds the system tray instance for callback wiring
var globalSystemTray *systray.SystemTray

// init initializes the global shutdown channel
func init() {
	globalShutdownChan = make(chan struct{})
}

// GetGlobalShutdownChan returns the global shutdown channel
// Components can wait on this channel to be notified when shutdown is requested
func GetGlobalShutdownChan() chan struct{} {
	return globalShutdownChan
}

// SetSystemTray stores the system tray instance for later callback configuration
func SetSystemTray(tray *systray.SystemTray) {
	globalSystemTray = tray
}

// ConfigureSystemTray wires system tray callbacks to the service manager
// This must be called after the service manager is created in CmdGateway
func ConfigureSystemTray(webURL, chatURL string, onStart, onStop func() error) {
	if globalSystemTray == nil {
		return
	}

	if webURL != "" {
		globalSystemTray.SetWebUIURL(webURL)
	}
	if chatURL != "" {
		globalSystemTray.SetChatURL(chatURL)
	}

	if onStart != nil {
		globalSystemTray.SetOnStart(func() {
			logger.InfoC("systray", "Starting bot service via system tray")
			if err := onStart(); err != nil {
				logger.ErrorCF("systray", "Failed to start bot service", map[string]interface{}{
					"error": err.Error(),
				})
				globalSystemTray.Notify("NemesisBot", fmt.Sprintf("启动失败: %v", err))
			} else {
				globalSystemTray.Notify("NemesisBot", "服务已启动")
			}
		})
	}

	if onStop != nil {
		globalSystemTray.SetOnStop(func() {
			logger.InfoC("systray", "Stopping bot service via system tray")
			if err := onStop(); err != nil {
				logger.ErrorCF("systray", "Failed to stop bot service", map[string]interface{}{
					"error": err.Error(),
				})
				globalSystemTray.Notify("NemesisBot", fmt.Sprintf("停止失败: %v", err))
			} else {
				globalSystemTray.Notify("NemesisBot", "服务已停止")
			}
		})
	}
}

// TriggerShutdown triggers the global shutdown signal
// This can be called from:
// - System tray quit menu
// - Desktop UI close event
// - WebSocket close message
// - Any other component that needs to initiate shutdown
func TriggerShutdown() {
	select {
	case <-globalShutdownChan:
		// Already closed
	default:
		close(globalShutdownChan)
	}
}

// WaitForShutdownOrSignal waits for either global shutdown signal or OS signal (Ctrl+C)
// This is the preferred method for waiting for shutdown in long-running commands
func WaitForShutdownOrSignal() {
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	select {
	case <-sigCh:
		// OS signal (Ctrl+C)
	case <-globalShutdownChan:
		// Global shutdown signal (from system tray, desktop UI, etc.)
	}
}
