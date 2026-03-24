package command

import (
	"os"
	"os/signal"
	"syscall"
)

// globalShutdownChan is the global shutdown channel
// It allows any component (system tray, desktop UI, WebSocket, etc.) to trigger application shutdown
var globalShutdownChan chan struct{}

// init initializes the global shutdown channel
func init() {
	globalShutdownChan = make(chan struct{})
}

// GetGlobalShutdownChan returns the global shutdown channel
// Components can wait on this channel to be notified when shutdown is requested
func GetGlobalShutdownChan() chan struct{} {
	return globalShutdownChan
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
