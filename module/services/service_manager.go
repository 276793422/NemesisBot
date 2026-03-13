package services

import (
	"context"
	"fmt"
	"os"
	"os/signal"
	"sync"
	"syscall"

	"github.com/276793422/NemesisBot/module/logger"
)

// ServiceManager manages all services in the application
// It handles both basic services (always running) and the bot service (on-demand)
type ServiceManager struct {
	mu     sync.RWMutex
	ctx    context.Context
	cancel context.CancelFunc

	// Bot service (on-demand)
	botService *BotService

	// Running state
	basicServicesStarted bool
}

// NewServiceManager creates a new ServiceManager instance
func NewServiceManager() *ServiceManager {
	ctx, cancel := context.WithCancel(context.Background())

	return &ServiceManager{
		ctx:        ctx,
		cancel:     cancel,
		botService: NewBotService(),
	}
}

// StartBasicServices starts services that should always run
// This includes HTTP server for Web UI and Desktop UI
// Returns an error if critical services fail to start
func (m *ServiceManager) StartBasicServices() error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.basicServicesStarted {
		return fmt.Errorf("basic services are already started")
	}

	logger.InfoC("service_manager", "Starting basic services...")

	// TODO: Start HTTP server for Web UI
	// For now, Desktop UI is started separately in CmdDesktop

	m.basicServicesStarted = true
	logger.InfoC("service_manager", "Basic services started")

	return nil
}

// StartBot starts the bot service
// This is equivalent to starting the gateway
func (m *ServiceManager) StartBot() error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if !m.basicServicesStarted {
		return fmt.Errorf("basic services must be started first")
	}

	logger.InfoC("service_manager", "Starting bot service...")

	if err := m.botService.Start(); err != nil {
		logger.ErrorCF("service_manager", "Failed to start bot service", map[string]interface{}{
			"error": err.Error(),
		})
		return err
	}

	logger.InfoC("service_manager", "Bot service started")

	return nil
}

// StopBot stops the bot service
func (m *ServiceManager) StopBot() error {
	m.mu.Lock()
	defer m.mu.Unlock()

	logger.InfoC("service_manager", "Stopping bot service...")

	if err := m.botService.Stop(); err != nil {
		logger.ErrorCF("service_manager", "Failed to stop bot service", map[string]interface{}{
			"error": err.Error(),
		})
		return err
	}

	logger.InfoC("service_manager", "Bot service stopped")

	return nil
}

// RestartBot restarts the bot service
func (m *ServiceManager) RestartBot() error {
	m.mu.Lock()
	defer m.mu.Unlock()

	logger.InfoC("service_manager", "Restarting bot service...")

	if err := m.botService.Restart(); err != nil {
		logger.ErrorCF("service_manager", "Failed to restart bot service", map[string]interface{}{
			"error": err.Error(),
		})
		return err
	}

	logger.InfoC("service_manager", "Bot service restarted")

	return nil
}

// GetBotState returns the current state of the bot service
func (m *ServiceManager) GetBotState() BotState {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.botService.GetState()
}

// GetBotError returns the error from the bot service
func (m *ServiceManager) GetBotError() error {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.botService.GetError()
}

// GetBotConfig returns the current bot configuration
func (m *ServiceManager) GetBotConfig() (interface{}, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.botService.GetConfig()
}

// SaveBotConfig saves the bot configuration
func (m *ServiceManager) SaveBotConfig(cfg interface{}, restart bool) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	// Use BotService's SaveConfig method which handles type conversion
	// The BotService will validate and save the config
	return m.botService.SaveConfig(cfg, restart)
}

// GetBotComponents returns the bot components for external access
func (m *ServiceManager) GetBotComponents() map[string]interface{} {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.botService.GetComponents()
}

// Shutdown gracefully shuts down all services
func (m *ServiceManager) Shutdown() {
	m.mu.Lock()
	defer m.mu.Unlock()

	logger.InfoC("service_manager", "Shutting down service manager...")

	// Stop bot service if running
	if m.botService.GetState().IsRunning() {
		if err := m.botService.Stop(); err != nil {
			logger.ErrorCF("service_manager", "Error stopping bot service during shutdown", map[string]interface{}{
				"error": err.Error(),
			})
		}
	}

	// Cancel context
	m.cancel()

	logger.InfoC("service_manager", "Service manager shutdown complete")
}

// WaitForShutdown waits for a shutdown signal (Ctrl+C)
func (m *ServiceManager) WaitForShutdown() {
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	<-sigCh
	logger.InfoC("service_manager", "Shutdown signal received")
}

// WaitForShutdownWithDesktop waits for either shutdown signal or desktop UI close
func (m *ServiceManager) WaitForShutdownWithDesktop(desktopClosed <-chan struct{}) {
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	select {
	case <-sigCh:
		logger.InfoC("service_manager", "Shutdown signal received (Ctrl+C)")
	case <-desktopClosed:
		logger.InfoC("service_manager", "Desktop UI closed, initiating shutdown")
		// Give a small delay for UI to close gracefully
		// The caller should handle this
	}
}

// GetBotService returns the bot service instance
func (m *ServiceManager) GetBotService() *BotService {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.botService
}

// IsBasicServicesStarted returns true if basic services are started
func (m *ServiceManager) IsBasicServicesStarted() bool {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.basicServicesStarted
}

// IsBotRunning returns true if the bot service is running
func (m *ServiceManager) IsBotRunning() bool {
	return m.GetBotState().IsRunning()
}
