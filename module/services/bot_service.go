package services

import (
	"context"
	"fmt"
	"net/http"
	"path/filepath"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/cluster/handlers"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/cron"
	"github.com/276793422/NemesisBot/module/devices"
	"github.com/276793422/NemesisBot/module/forge"
	"github.com/276793422/NemesisBot/module/health"
	"github.com/276793422/NemesisBot/module/heartbeat"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/memory"
	"github.com/276793422/NemesisBot/module/observer"
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
	cronSvc      *cron.CronService
	heartbeatSvc *heartbeat.HeartbeatService
	deviceSvc    *devices.Service
	healthSrv    *health.Server
	stateMgr     *state.Manager
	forgeSvc     *forge.Forge
	memoryMgr    *memory.Manager

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
	if s.forgeSvc != nil {
		components["forge"] = s.forgeSvc
	}
	if s.memoryMgr != nil {
		components["memory"] = s.memoryMgr
	}

	return components
}

// GetForge returns the Forge instance if available.
func (s *BotService) GetForge() *forge.Forge {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.forgeSvc
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

	// --- Phase 1: Sequential core setup ---
	// These components have inter-dependencies and must be created in order.

	// Create provider
	provider, err := providers.CreateProvider(cfg)
	if err != nil {
		return fmt.Errorf("failed to create provider: %w", err)
	}
	s.provider = provider

	// Create message bus
	s.msgBus = bus.NewMessageBus()

	// Create agent loop and channel manager in parallel (both depend on msgBus+provider but not each other)
	if err := parallelInit(s.ctx,
		func() error {
			s.agentLoop = agent.NewAgentLoop(cfg, s.msgBus, provider)
			return nil
		},
		func() error {
			var mgrErr error
			s.channelMgr, mgrErr = channels.NewManager(cfg, s.msgBus)
			if mgrErr != nil {
				return fmt.Errorf("failed to create channel manager: %w", mgrErr)
			}
			return nil
		},
	); err != nil {
		return err
	}

	// Wire agent loop <-> channel manager
	s.agentLoop.SetChannelManager(s.channelMgr)

	// --- Phase 2: Parallel independent service creation ---
	// These services have no inter-dependencies and can be initialized concurrently.
	//
	// Group A: Services that only need cfg values and workspace string.
	// Group B: deviceSvc depends on stateMgr, so they are chained.
	//
	// We run Group A members and Group B chain in parallel.
	var cronSvc *cron.CronService
	var heartbeatSvc *heartbeat.HeartbeatService
	var stateMgr *state.Manager
	var healthSrv *health.Server

	if err := parallelInit(s.ctx,
		// Cron service
		func() error {
			cronStorePath := filepath.Join(s.workspace, "cron", "jobs.json")
			cronSvc = cron.NewCronService(cronStorePath, nil)
			return nil
		},
		// Heartbeat service
		func() error {
			heartbeatSvc = heartbeat.NewHeartbeatService(
				s.workspace,
				cfg.Heartbeat.Interval,
				cfg.Heartbeat.Enabled,
			)
			return nil
		},
		// State manager + device service (chained dependency)
		func() error {
			stateMgr = state.NewManager(s.workspace)
			return nil
		},
		// Health server
		func() error {
			healthSrv = health.NewServer(cfg.Gateway.Host, cfg.Gateway.Port)
			return nil
		},
	); err != nil {
		return err
	}

	// --- Phase 3: Wire up service dependencies (sequential) ---
	// These steps depend on the services created in Phase 2.

	// Setup cron tool
	s.cronSvc = cronSvc
	cronTool := tools.NewCronTool(cronSvc, s.agentLoop, s.msgBus, s.workspace,
		cfg.Agents.Defaults.RestrictToWorkspace,
		time.Duration(cfg.Tools.Cron.ExecTimeoutMinutes)*time.Minute, cfg)
	s.agentLoop.RegisterTool(cronTool)
	s.cronSvc.SetOnJob(func(job *cron.CronJob) (string, error) {
		result := cronTool.ExecuteJob(context.Background(), job)
		return result, nil
	})

	// Wire heartbeat service
	s.heartbeatSvc = heartbeatSvc
	s.heartbeatSvc.SetBus(s.msgBus)
	s.heartbeatSvc.SetHandler(s.createHeartbeatHandler(cfg))

	// Wire state manager and device service
	s.stateMgr = stateMgr
	s.deviceSvc = devices.NewService(devices.Config{
		Enabled:    cfg.Devices.Enabled,
		MonitorUSB: cfg.Devices.MonitorUSB,
	}, stateMgr)
	s.deviceSvc.SetBus(s.msgBus)

	// Wire health server
	s.healthSrv = healthSrv

	// --- Phase 4: Forge and Observer setup (depends on agentLoop + provider) ---
	// Initialize Forge self-learning module
	if cfg.Forge != nil && cfg.Forge.Enabled {
		forgeInstance, err := forge.NewForge(s.workspace, nil)
		if err != nil {
			logger.WarnCF("bot_service", "Forge init failed", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			forgeInstance.SetProvider(s.provider)
			s.forgeSvc = forgeInstance

			// Register Forge tools with agent loop
			forgeTools := forge.NewForgeTools(forgeInstance)
			for _, tool := range forgeTools {
				s.agentLoop.RegisterTool(tool)
			}

			// Phase 4: Connect Forge to Cluster for cross-node sharing
			if cl := s.agentLoop.GetCluster(); cl != nil {
				bridge := forge.NewClusterForgeBridge(cl)
				forgeInstance.SetBridge(bridge)

				// Register Forge RPC handlers with the cluster
				registrar := func(action string, handler func(payload map[string]interface{}) (map[string]interface{}, error)) {
					if err := cl.RegisterRPCHandler(action, handler); err != nil {
						logger.WarnCF("bot_service", "Failed to register forge RPC handler", map[string]interface{}{
							"action": action,
							"error":  err.Error(),
						})
					}
				}
				syncer := forgeInstance.GetSyncer()
				handlers.RegisterForgeHandlers(cl.GetLogger(), syncer, cl.GetNodeID, registrar)
			}

			logger.InfoC("bot_service", "Forge module initialized")
		}
	}

	// --- Phase 4.5: Memory Manager setup (depends on provider for API embedding tier) ---
	if cfg.Memory != nil && cfg.Memory.Enabled {
		memCfg := memory.DefaultConfig()
		memMgr, err := memory.NewManager(memCfg, s.workspace)
		if err != nil {
			logger.WarnCF("bot_service", "Memory Manager init failed", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			s.memoryMgr = memMgr

			// Register memory tools with agent loop
			memTools := memory.NewMemoryTools(memMgr)
			for _, tool := range memTools {
				s.agentLoop.RegisterTool(tool)
			}

			logger.InfoC("bot_service", "Memory Manager initialized")
		}
	}

	// Phase 5: Setup Observer Manager for conversation lifecycle events
	observerMgr := observer.NewManager()

	// Register RequestLogger as Observer (if logging enabled)
	if cfg.Logging != nil && cfg.Logging.LLM != nil && cfg.Logging.LLM.Enabled {
		rlObs := agent.NewRequestLoggerObserver(cfg.Logging, s.workspace)
		observerMgr.Register(rlObs)
		logger.InfoC("bot_service", "RequestLoggerObserver registered")
	}

	// Register Forge TraceCollector as Observer (if Forge and trace enabled)
	if s.forgeSvc != nil && s.forgeSvc.GetConfig().Trace.Enabled {
		if tc := s.forgeSvc.GetTraceCollector(); tc != nil {
			observerMgr.Register(tc)
			// Inject trace store into reflector for conversation-level analysis
			if ts := s.forgeSvc.GetTraceStore(); ts != nil {
				s.forgeSvc.GetReflector().SetTraceStore(ts)
			}
			logger.InfoC("bot_service", "TraceCollector observer registered")
		}
	}

	// Phase 6: Wire LearningEngine to Reflector (if learning enabled)
	if s.forgeSvc != nil && s.forgeSvc.GetConfig().Learning.Enabled {
		le := s.forgeSvc.GetLearningEngine()
		if le != nil {
			// Inject parent Forge for CreateSkill shared method
			le.SetForge(s.forgeSvc)
			// Inject LearningEngine into Reflector (post-injection pattern)
			s.forgeSvc.GetReflector().SetLearningEngine(le)
			logger.InfoC("bot_service", "Forge LearningEngine wired")
		}
	}

	// Inject observer manager into AgentLoop
	if observerMgr.HasObservers() {
		s.agentLoop.SetObserverManager(observerMgr)
	}

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

	// Inject dashboard dependencies (workspace, SSE hook, model name)
	s.injectDashboardDependencies(cfg)

	// Start cron service
	if s.cronSvc != nil {
		if err := s.cronSvc.Start(); err != nil {
			logger.WarnCF("bot_service", "Cron service start failed", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			logger.InfoC("bot_service", "Cron service started")
		}
	}

	// Start Forge self-learning module
	if s.forgeSvc != nil {
		s.forgeSvc.Start()
		logger.InfoC("bot_service", "Forge service started")
	}

	return nil
}

func (s *BotService) stopAll() {
	// Stop in reverse order
	if s.memoryMgr != nil {
		s.memoryMgr.Close()
	}
	if s.forgeSvc != nil {
		s.forgeSvc.Stop()
	}
	if s.healthSrv != nil {
		s.healthSrv.Stop(s.ctx)
	}
	if s.cronSvc != nil {
		s.cronSvc.Stop()
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

// injectDashboardDependencies injects workspace path, model name, and SSE logger hook
// into the web channel. Must be called AFTER channelMgr.StartAll() so that the web server exists.
func (s *BotService) injectDashboardDependencies(cfg *config.Config) {
	ch, ok := s.channelMgr.GetChannel("web")
	if !ok {
		return
	}
	webCh, ok := ch.(*channels.WebChannel)
	if !ok {
		return
	}

	// Inject workspace path for API handlers
	webCh.SetWorkspace(s.workspace)

	// Inject model name for status endpoint
	if s.provider != nil {
		modelName := s.provider.GetDefaultModel()
		webCh.SetModelName(modelName)
	}

	// Bridge logger → SSE EventHub
	server := webCh.GetServer()
	if server != nil {
		hub := server.GetEventHub()
		logger.SetLogHook(func(entry logger.LogEntry) {
			hub.Publish("log", map[string]interface{}{
				"source":    "general",
				"timestamp": entry.Timestamp,
				"level":     entry.Level,
				"component": entry.Component,
				"message":   entry.Message,
			})
		})
		logger.InfoC("bot_service", "SSE logger hook injected into web channel")
	}
}
