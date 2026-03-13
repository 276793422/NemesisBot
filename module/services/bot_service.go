package services

import (
	"context"
	"fmt"
	"net/http"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/devices"
	"github.com/276793422/NemesisBot/module/health"
	"github.com/276793422/NemesisBot/module/heartbeat"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/state"
	"github.com/276793422/NemesisBot/module/tools"
)

// BotService encapsulates all core bot services
// It manages the lifecycle of the entire bot functionality
type BotService struct {
	mu    sync.RWMutex
	state BotState
	err   error

	// Configuration
	configPath string
	workspace  string

	// Core components
	provider     providers.LLMProvider
	msgBus       *bus.MessageBus
	agentLoop    *agent.AgentLoop
	channelMgr   *channels.Manager
	cronSvc      interface{} // Using interface{} to avoid type issues
	heartbeatSvc *heartbeat.HeartbeatService
	deviceSvc    *devices.Service
	healthSrv    *health.Server
	stateMgr     *state.Manager

	// Context management
	ctx    context.Context
	cancel context.CancelFunc

	// Dependencies
	configLoaded bool
}

// NewBotService creates a new BotService instance
func NewBotService() *BotService {
	return &BotService{
		state:      BotStateNotStarted,
		configPath: GetConfigPath(),
	}
}

// Start initializes and starts all bot services
// This is equivalent to what the gateway command does
func (s *BotService) Start() error {
	s.mu.Lock()
	defer s.mu.Unlock()

	// Check if already running
	if s.state == BotStateRunning || s.state == BotStateStarting {
		return fmt.Errorf("bot is already %s", s.state)
	}

	logger.InfoC("bot_service", "Starting bot service...")

	// Update state to starting
	s.state = BotStateStarting
	s.err = nil

	// Create context
	s.ctx, s.cancel = context.WithCancel(context.Background())

	// Step 1: Load configuration
	if err := s.loadConfig(); err != nil {
		s.setStateWithError(BotStateError, err)
		return fmt.Errorf("failed to load config: %w", err)
	}

	// Step 2: Validate configuration
	if err := s.validateConfig(); err != nil {
		s.setStateWithError(BotStateError, err)
		return fmt.Errorf("config validation failed: %w", err)
	}

	// Step 3: Initialize components
	if err := s.initComponents(); err != nil {
		s.setStateWithError(BotStateError, err)
		return fmt.Errorf("failed to initialize components: %w", err)
	}

	// Step 4: Start services
	if err := s.startServices(); err != nil {
		// Rollback: stop any started services
		s.stopAll()
		s.setStateWithError(BotStateError, err)
		return fmt.Errorf("failed to start services: %w", err)
	}

	s.state = BotStateRunning
	logger.InfoC("bot_service", "Bot service started successfully")

	return nil
}

// Stop gracefully stops all bot services
func (s *BotService) Stop() error {
	s.mu.Lock()
	defer s.mu.Unlock()

	if s.state != BotStateRunning && s.state != BotStateStarting {
		return fmt.Errorf("bot is not running (current state: %s)", s.state)
	}

	logger.InfoC("bot_service", "Stopping bot service...")

	// Cancel context to signal all services to stop
	if s.cancel != nil {
		s.cancel()
	}

	// Stop all services
	s.stopAll()

	s.state = BotStateNotStarted
	logger.InfoC("bot_service", "Bot service stopped")

	return nil
}

// Restart stops and then starts the bot service
func (s *BotService) Restart() error {
	logger.InfoC("bot_service", "Restarting bot service...")

	// Stop if running
	if s.state == BotStateRunning {
		if err := s.Stop(); err != nil {
			return fmt.Errorf("failed to stop bot: %w", err)
		}
	}

	// Wait a bit for cleanup
	time.Sleep(500 * time.Millisecond)

	// Start again
	if err := s.Start(); err != nil {
		return fmt.Errorf("failed to start bot: %w", err)
	}

	logger.InfoC("bot_service", "Bot service restarted successfully")

	return nil
}

// GetState returns the current state of the bot service
func (s *BotService) GetState() BotState {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.state
}

// GetError returns the error that caused the error state
func (s *BotService) GetError() error {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.err
}

// GetConfig returns the current configuration
func (s *BotService) GetConfig() (*config.Config, error) {
	cfg, err := config.LoadConfig(s.configPath)
	if err != nil {
		return nil, err
	}
	return cfg, nil
}

// SaveConfig saves the configuration and optionally restarts the bot
func (s *BotService) SaveConfig(cfg interface{}, restart bool) error {
	// Type assertion for config
	configCfg, ok := cfg.(*config.Config)
	if !ok {
		return fmt.Errorf("invalid config type")
	}

	if err := config.SaveConfig(s.configPath, configCfg); err != nil {
		return fmt.Errorf("failed to save config: %w", err)
	}

	logger.InfoC("bot_service", "Configuration saved successfully")

	// Trigger restart if requested and bot is running
	if restart && s.state == BotStateRunning {
		go func() {
			time.Sleep(100 * time.Millisecond)
			if err := s.Restart(); err != nil {
				logger.ErrorCF("bot_service", "Failed to restart after config save", map[string]interface{}{
					"error": err.Error(),
				})
			}
		}()
	}

	return nil
}

// GetComponents returns the core components for external access
func (s *BotService) GetComponents() map[string]interface{} {
	s.mu.RLock()
	defer s.mu.RUnlock()

	components := make(map[string]interface{})

	if s.agentLoop != nil {
		components["agentLoop"] = s.agentLoop
	}
	if s.msgBus != nil {
		components["msgBus"] = s.msgBus
	}
	if s.channelMgr != nil {
		components["channelMgr"] = s.channelMgr
	}

	return components
}

// Internal methods

func (s *BotService) loadConfig() error {
	logger.InfoCF("bot_service", "Loading config from", map[string]interface{}{
		"path": s.configPath,
	})

	cfg, err := config.LoadConfig(s.configPath)
	if err != nil {
		return err
	}

	s.workspace = cfg.WorkspacePath()
	s.configLoaded = true

	logger.InfoCF("bot_service", "Config loaded", map[string]interface{}{
		"workspace": s.workspace,
	})

	return nil
}

func (s *BotService) validateConfig() error {
	cfg, err := config.LoadConfig(s.configPath)
	if err != nil {
		return err
	}

	// Check if at least one model is configured
	if len(cfg.ModelList) == 0 {
		return fmt.Errorf("no models configured")
	}

	// Check if at least one model has a valid API key
	hasValidModel := false
	for _, m := range cfg.ModelList {
		if m.APIKey != "" {
			hasValidModel = true
			break
		}
	}

	if !hasValidModel {
		return fmt.Errorf("no model with valid API key found")
	}

	logger.InfoC("bot_service", "Configuration validated")

	return nil
}

func (s *BotService) initComponents() error {
	cfg, err := config.LoadConfig(s.configPath)
	if err != nil {
		return err
	}

	// Create provider
	provider, err := providers.CreateProvider(cfg)
	if err != nil {
		return fmt.Errorf("failed to create provider: %w", err)
	}
	s.provider = provider

	// Create message bus
	s.msgBus = bus.NewMessageBus()

	// Create agent loop
	s.agentLoop = agent.NewAgentLoop(cfg, s.msgBus, provider)

	// Create channel manager
	s.channelMgr, err = channels.NewManager(cfg, s.msgBus)
	if err != nil {
		return fmt.Errorf("failed to create channel manager: %w", err)
	}

	// Inject channel manager into agent loop
	s.agentLoop.SetChannelManager(s.channelMgr)

	// Create heartbeat service
	s.heartbeatSvc = heartbeat.NewHeartbeatService(
		s.workspace,
		cfg.Heartbeat.Interval,
		cfg.Heartbeat.Enabled,
	)
	s.heartbeatSvc.SetBus(s.msgBus)
	s.heartbeatSvc.SetHandler(s.createHeartbeatHandler(cfg))

	// Create state manager
	s.stateMgr = state.NewManager(s.workspace)

	// Create device service
	s.deviceSvc = devices.NewService(devices.Config{
		Enabled:    cfg.Devices.Enabled,
		MonitorUSB: cfg.Devices.MonitorUSB,
	}, s.stateMgr)
	s.deviceSvc.SetBus(s.msgBus)

	// Create health server
	s.healthSrv = health.NewServer(cfg.Gateway.Host, cfg.Gateway.Port)

	logger.InfoC("bot_service", "Components initialized")

	return nil
}

func (s *BotService) startServices() error {
	cfg, _ := config.LoadConfig(s.configPath)

	// Start heartbeat service
	if err := s.heartbeatSvc.Start(); err != nil {
		return fmt.Errorf("failed to start heartbeat service: %w", err)
	}
	logger.InfoC("bot_service", "Heartbeat service started")

	// Start device service
	if err := s.deviceSvc.Start(s.ctx); err != nil {
		return fmt.Errorf("failed to start device service: %w", err)
	}
	logger.InfoC("bot_service", "Device service started")

	// Start channel manager
	if err := s.channelMgr.StartAll(s.ctx); err != nil {
		return fmt.Errorf("failed to start channel manager: %w", err)
	}

	enabledChannels := s.channelMgr.GetEnabledChannels()
	if len(enabledChannels) > 0 {
		logger.InfoCF("bot_service", "Channels enabled", map[string]interface{}{
			"channels": enabledChannels,
		})
	}

	// Start health server
	go func() {
		if err := s.healthSrv.Start(); err != nil && err != http.ErrServerClosed {
			logger.ErrorCF("bot_service", "Health server error", map[string]interface{}{
				"error": err.Error(),
			})
		}
	}()
	logger.InfoCF("bot_service", "Health server started", map[string]interface{}{
		"host": cfg.Gateway.Host,
		"port": cfg.Gateway.Port,
	})

	// Start agent loop
	go s.agentLoop.Run(s.ctx)
	logger.InfoC("bot_service", "Agent loop started")

	return nil
}

func (s *BotService) stopAll() {
	// Stop in reverse order
	if s.healthSrv != nil {
		s.healthSrv.Stop(s.ctx)
	}
	if s.deviceSvc != nil {
		s.deviceSvc.Stop()
	}
	if s.heartbeatSvc != nil {
		s.heartbeatSvc.Stop()
	}
	if s.channelMgr != nil {
		s.channelMgr.StopAll(s.ctx)
	}
	if s.agentLoop != nil {
		s.agentLoop.Stop()
	}
}

func (s *BotService) createHeartbeatHandler(cfg *config.Config) func(prompt, channel, chatID string) *tools.ToolResult {
	return func(prompt, channel, chatID string) *tools.ToolResult {
		// Check if BOOTSTRAP.md exists - if so, skip heartbeat LLM call entirely
		if ShouldSkipHeartbeatForBootstrap(s.workspace) {
			// BOOTSTRAP.md exists, skip heartbeat processing and return OK directly
			logger.InfoC("heartbeat", "BOOTSTRAP.md exists, skipping heartbeat LLM call")
			return tools.SilentResult("HEARTBEAT_OK")
		}

		// Use cli:direct as fallback
		if channel == "" || chatID == "" {
			channel, chatID = "cli", "direct"
		}

		// Process heartbeat
		response, err := s.agentLoop.ProcessHeartbeat(context.Background(), prompt, channel, chatID)
		if err != nil {
			return tools.ErrorResult(fmt.Sprintf("Heartbeat error: %v", err))
		}
		if response == "HEARTBEAT_OK" {
			return tools.SilentResult("Heartbeat OK")
		}
		return tools.SilentResult(response)
	}
}

func (s *BotService) setStateWithError(state BotState, err error) {
	s.state = state
	s.err = err
	logger.ErrorCF("bot_service", "Bot service error", map[string]interface{}{
		"state": state.String(),
		"error": err.Error(),
	})
}
