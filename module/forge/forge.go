// Package forge provides the self-learning framework for NemesisBot.
// It follows a Read → Execute → Reflect → Write cycle to learn from
// daily tasks, generate Skills, scripts, and MCP modules automatically.
package forge

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/plugin"
	"github.com/276793422/NemesisBot/module/providers"
)

// Forge is the core self-learning system that runs alongside AgentLoop and Cluster.
// It consists of four subsystems: Collector, Reflector, Factory, and Registry.
// Phase 4 adds Syncer for cluster learning (cross-node reflection sharing).
// Phase 5 adds TraceCollector for conversation-level trace collection.
// Phase 6 adds LearningEngine for closed-loop self-learning.
type Forge struct {
	workspace      string
	config         *ForgeConfig
	collector      *Collector
	reflector      *Reflector
	registry       *Registry
	store          *ExperienceStore
	pipeline       *Pipeline
	mcpInstaller   *MCPInstaller
	exporter       *Exporter
	syncer         *Syncer
	traceCollector *TraceCollector   // Phase 5
	traceStore     *TraceStore       // Phase 5
	learningEngine *LearningEngine   // Phase 6
	deploymentMonitor *DeploymentMonitor // Phase 6
	cycleStore     *CycleStore       // Phase 6

	provider providers.LLMProvider
	stopCh   chan struct{}
	wg       sync.WaitGroup
}

// NewForge creates a new Forge instance with the given workspace and plugin manager.
// The plugin manager is used to register the ForgePlugin for experience collection.
func NewForge(workspace string, pluginMgr *plugin.Manager) (*Forge, error) {
	// Load forge-specific config
	forgeDir := filepath.Join(workspace, "forge")
	configPath := filepath.Join(forgeDir, "forge.json")

	cfg, err := LoadForgeConfig(configPath)
	if err != nil {
		// Use defaults if config file doesn't exist
		cfg = DefaultForgeConfig()
	}

	// Ensure directory structure exists
	dirs := []string{
		filepath.Join(forgeDir, "experiences"),
		filepath.Join(forgeDir, "reflections"),
		filepath.Join(forgeDir, "skills"),
		filepath.Join(forgeDir, "scripts"),
		filepath.Join(forgeDir, "mcp"),
		filepath.Join(forgeDir, "traces"),
		filepath.Join(forgeDir, "learning"),
		filepath.Join(workspace, "prompts"),
	}
	for _, dir := range dirs {
		if err := os.MkdirAll(dir, 0755); err != nil {
			return nil, err
		}
	}

	// Create experience store
	store := NewExperienceStore(forgeDir, cfg)

	// Create collector
	collector := NewCollector(store, cfg)

	// Create registry
	registryPath := filepath.Join(forgeDir, "registry.json")
	registry := NewRegistry(registryPath)

	// Create reflector
	reflector := NewReflector(forgeDir, store, registry, cfg)

	// Create pipeline
	pipeline := NewPipeline(registry, cfg)

	// Create MCP installer
	mcpInstaller := NewMCPInstaller(workspace)

	// Create exporter
	exporter := NewExporter(workspace, registry)

	// Create syncer (Phase 4: cluster learning)
	syncer := NewSyncer(forgeDir, registry, cfg)

	// Create trace store and collector (Phase 5: conversation-level traces)
	var traceStore *TraceStore
	var traceCollector *TraceCollector
	if cfg.Trace.Enabled {
		traceStore = NewTraceStore(forgeDir, cfg)
		traceCollector = NewTraceCollector(traceStore, cfg)
	}

	// Phase 6: Closed-loop learning components
	var learningEngine *LearningEngine
	var deploymentMonitor *DeploymentMonitor
	var cycleStore *CycleStore
	if cfg.Learning.Enabled {
		// Auto-enable trace if learning is enabled (cascade)
		if !cfg.Trace.Enabled {
			cfg.Trace.Enabled = true
			traceStore = NewTraceStore(forgeDir, cfg)
			traceCollector = NewTraceCollector(traceStore, cfg)
		}
		cycleStore = NewCycleStore(forgeDir, cfg)
		deploymentMonitor = NewDeploymentMonitor(traceStore, registry, cfg)
		learningEngine = NewLearningEngine(forgeDir, registry, traceStore, pipeline, cycleStore, deploymentMonitor, cfg)
	}

	f := &Forge{
		workspace:         workspace,
		config:            cfg,
		collector:         collector,
		reflector:         reflector,
		registry:          registry,
		store:             store,
		pipeline:          pipeline,
		mcpInstaller:      mcpInstaller,
		exporter:          exporter,
		syncer:            syncer,
		traceCollector:    traceCollector,
		traceStore:        traceStore,
		learningEngine:    learningEngine,
		deploymentMonitor: deploymentMonitor,
		cycleStore:        cycleStore,
		stopCh:            make(chan struct{}),
	}

	// Register the ForgePlugin with the plugin manager
	if pluginMgr != nil && cfg.Collection.Enabled {
		forgePlugin := NewForgePlugin(collector)
		if err := pluginMgr.Register(forgePlugin); err != nil {
			logger.WarnCF("forge", "Failed to register Forge plugin", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			logger.InfoC("forge", "Forge plugin registered with plugin manager")
		}
	}

	return f, nil
}

// SetProvider sets the LLM provider for semantic reflection and quality evaluation.
func (f *Forge) SetProvider(provider providers.LLMProvider) {
	f.provider = provider
	f.reflector.SetProvider(provider)
	f.pipeline.SetProvider(provider)
	if f.learningEngine != nil {
		f.learningEngine.SetProvider(provider)
	}
}

// SetBridge injects the cluster bridge for cross-node reflection sharing (Phase 4).
func (f *Forge) SetBridge(bridge ClusterForgeBridge) {
	if f.syncer != nil {
		f.syncer.SetBridge(bridge)
		logger.InfoC("forge", "Cluster bridge injected for Forge syncer")
	}
}

// Start launches the collector and reflector goroutines.
func (f *Forge) Start() {
	if !f.config.Collection.Enabled {
		logger.InfoC("forge", "Forge collection disabled, skipping start")
		return
	}

	logger.InfoC("forge", "Starting Forge self-learning system")

	// Start collector goroutine (flushes buffered experiences to disk)
	f.wg.Add(1)
	go f.runCollector()

	// Start reflector goroutine (periodic reflection)
	f.wg.Add(1)
	go f.runReflector()

	// Start cleanup goroutine
	f.wg.Add(1)
	go f.runCleanup()

	logger.InfoCF("forge", "Forge started", map[string]interface{}{
		"collection_interval": f.config.Collection.FlushInterval.Duration.String(),
		"reflection_interval": f.config.Reflection.Interval.Duration.String(),
	})
}

// Stop gracefully shuts down Forge, flushing remaining experiences.
func (f *Forge) Stop() {
	logger.InfoC("forge", "Stopping Forge self-learning system")
	close(f.stopCh)
	f.wg.Wait()

	// Final flush
	if f.collector != nil {
		f.collector.Flush()
	}

	logger.InfoC("forge", "Forge stopped")
}

// GetCollector returns the experience collector for plugin integration.
func (f *Forge) GetCollector() *Collector {
	return f.collector
}

// GetRegistry returns the artifact registry.
func (f *Forge) GetRegistry() *Registry {
	return f.registry
}

// GetReflector returns the reflection engine.
func (f *Forge) GetReflector() *Reflector {
	return f.reflector
}

// GetPipeline returns the validation pipeline.
func (f *Forge) GetPipeline() *Pipeline {
	return f.pipeline
}

// GetConfig returns the forge configuration.
func (f *Forge) GetConfig() *ForgeConfig {
	return f.config
}

// GetWorkspace returns the forge workspace directory.
func (f *Forge) GetWorkspace() string {
	return filepath.Join(f.workspace, "forge")
}

// ReflectNow triggers an immediate reflection, returning the report path.
func (f *Forge) ReflectNow(ctx context.Context, period string, focus string) (string, error) {
	return f.reflector.Reflect(ctx, period, focus)
}

// GetMCPInstaller returns the MCP installer for config registration.
func (f *Forge) GetMCPInstaller() *MCPInstaller {
	return f.mcpInstaller
}

// GetExporter returns the artifact exporter.
func (f *Forge) GetExporter() *Exporter {
	return f.exporter
}

// GetSyncer returns the cluster syncer for Phase 4 cross-node sharing.
func (f *Forge) GetSyncer() *Syncer {
	return f.syncer
}

// GetTraceCollector returns the trace collector for Phase 5 conversation-level analysis.
func (f *Forge) GetTraceCollector() *TraceCollector {
	return f.traceCollector
}

// GetTraceStore returns the trace store.
func (f *Forge) GetTraceStore() *TraceStore {
	return f.traceStore
}

// GetLearningEngine returns the Phase 6 learning engine.
func (f *Forge) GetLearningEngine() *LearningEngine {
	return f.learningEngine
}

// GetDeploymentMonitor returns the Phase 6 deployment monitor.
func (f *Forge) GetDeploymentMonitor() *DeploymentMonitor {
	return f.deploymentMonitor
}

// GetCycleStore returns the Phase 6 cycle store.
func (f *Forge) GetCycleStore() *CycleStore {
	return f.cycleStore
}

// CreateSkill creates and registers a Skill artifact. This is a shared method
// used by both the forge_create tool and the LearningEngine to avoid code duplication.
func (f *Forge) CreateSkill(ctx context.Context, name, content, description string, toolSignature []string) (Artifact, error) {
	forgeDir := f.GetWorkspace()
	artifactPath := filepath.Join(forgeDir, "skills", name, "SKILL.md")

	// Auto-generate frontmatter if missing
	if !strings.Contains(content, "---") {
		content = fmt.Sprintf("---\nname: %s\ndescription: %s\n---\n\n%s", name, description, content)
	}

	// Create directory
	if err := os.MkdirAll(filepath.Dir(artifactPath), 0755); err != nil {
		return Artifact{}, fmt.Errorf("create dir failed: %w", err)
	}

	// Write content
	if err := os.WriteFile(artifactPath, []byte(content), 0644); err != nil {
		return Artifact{}, fmt.Errorf("write file failed: %w", err)
	}

	// Register in registry
	artifactID := fmt.Sprintf("skill-%s", name)
	artifact := Artifact{
		ID:            artifactID,
		Type:          ArtifactSkill,
		Name:          name,
		Version:       "1.0",
		Status:        ArtifactStatus(f.config.Artifacts.DefaultStatus),
		Path:          artifactPath,
		ToolSignature: toolSignature,
		Evolution: []Evolution{
			{
				Version: "1.0",
				Date:    time.Now().UTC(),
				Change:  fmt.Sprintf("初始创建: %s", description),
			},
		},
	}

	if err := f.registry.Add(artifact); err != nil {
		return Artifact{}, fmt.Errorf("registry add failed: %w", err)
	}

	// Copy to workspace/skills/ with -forge suffix
	workspaceSkills := filepath.Join(f.workspace, "skills", name+"-forge")
	if err := os.MkdirAll(workspaceSkills, 0755); err == nil {
		os.WriteFile(filepath.Join(workspaceSkills, "SKILL.md"), []byte(content), 0644)
	}

	// Auto-validate if configured
	if f.config.Validation.AutoValidate && f.pipeline != nil {
		validation := f.pipeline.RunFromContent(ctx, &artifact, content)
		newStatus := f.pipeline.DetermineStatus(validation)
		f.registry.Update(artifactID, func(a *Artifact) {
			a.Validation = validation
			a.Status = newStatus
		})
		// Refresh artifact with updated status
		if updated, found := f.registry.Get(artifactID); found {
			artifact = updated
		}
	}

	return artifact, nil
}

// ReceiveReflection receives a remote reflection report (used by RPC handler).
func (f *Forge) ReceiveReflection(payload map[string]interface{}) error {
	if f.syncer == nil {
		return fmt.Errorf("syncer not initialized")
	}
	return f.syncer.ReceiveReflection(payload)
}

// runCollector periodically flushes buffered experiences to disk.
func (f *Forge) runCollector() {
	defer f.wg.Done()

	ticker := time.NewTicker(f.config.Collection.FlushInterval.Duration)
	defer ticker.Stop()

	for {
		select {
		case <-f.stopCh:
			return
		case <-ticker.C:
			f.collector.Flush()
		}
	}
}

// runReflector periodically runs the reflection engine.
func (f *Forge) runReflector() {
	defer f.wg.Done()

	ticker := time.NewTicker(f.config.Reflection.Interval.Duration)
	defer ticker.Stop()

	for {
		select {
		case <-f.stopCh:
			return
		case <-ticker.C:
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
			reportPath, err := f.reflector.Reflect(ctx, "today", "all")
			cancel()
			if err != nil {
				logger.ErrorCF("forge", "Reflection failed", map[string]interface{}{
					"error": err.Error(),
				})
			} else {
				logger.InfoCF("forge", "Reflection report generated", map[string]interface{}{
					"path": reportPath,
				})
				// Phase 4: Auto-share reflection with cluster peers
				if f.syncer != nil && f.syncer.IsEnabled() {
					shareCtx, shareCancel := context.WithTimeout(context.Background(), 2*time.Minute)
					if shareErr := f.syncer.ShareReflection(shareCtx, reportPath); shareErr != nil {
						logger.WarnCF("forge", "Auto-share reflection failed", map[string]interface{}{
							"error": shareErr.Error(),
						})
					}
					shareCancel()
				}
			}
		}
	}
}

// runCleanup periodically removes old experiences and reports.
func (f *Forge) runCleanup() {
	defer f.wg.Done()

	ticker := time.NewTicker(f.config.Storage.CleanupInterval.Duration)
	defer ticker.Stop()

	for {
		select {
		case <-f.stopCh:
			return
		case <-ticker.C:
			if err := f.store.Cleanup(f.config.Storage.MaxExperienceAgeDays); err != nil {
				logger.ErrorCF("forge", "Experience cleanup failed", map[string]interface{}{
					"error": err.Error(),
				})
			}
			if err := f.reflector.CleanupReports(f.config.Storage.MaxReportAgeDays); err != nil {
				logger.ErrorCF("forge", "Report cleanup failed", map[string]interface{}{
					"error": err.Error(),
				})
			}
			// Phase 5: trace cleanup
			if f.traceStore != nil && f.config.Trace.MaxTraceAgeDays > 0 {
				if err := f.traceStore.Cleanup(f.config.Trace.MaxTraceAgeDays); err != nil {
					logger.ErrorCF("forge", "Trace cleanup failed", map[string]interface{}{
						"error": err.Error(),
					})
				}
			}
			// Phase 6: learning cycle cleanup
			if f.cycleStore != nil && f.config.Storage.MaxReportAgeDays > 0 {
				if err := f.cycleStore.Cleanup(f.config.Storage.MaxReportAgeDays); err != nil {
					logger.ErrorCF("forge", "Cycle cleanup failed", map[string]interface{}{
						"error": err.Error(),
					})
				}
			}
			// Phase 6: cleanup old prompt suggestions (7 days)
			f.cleanupPromptSuggestions(7)
		}
	}
}

// cleanupPromptSuggestions removes prompt suggestion files older than maxAgeDays.
func (f *Forge) cleanupPromptSuggestions(maxAgeDays int) {
	promptsDir := filepath.Join(f.workspace, "prompts")
	entries, err := os.ReadDir(promptsDir)
	if err != nil {
		return
	}
	cutoff := time.Now().UTC().AddDate(0, 0, -maxAgeDays)
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), "_suggestion.md") {
			continue
		}
		info, err := entry.Info()
		if err != nil {
			continue
		}
		if info.ModTime().Before(cutoff) {
			os.Remove(filepath.Join(promptsDir, entry.Name()))
		}
	}
}
