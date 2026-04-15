// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package scanner

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/logger"
)

// ScanChain executes multiple VirusScanner engines in order.
// The first engine to report an infection short-circuits the chain.
// Engines that are not ready are silently skipped (degraded mode).
type ScanChain struct {
	engines []VirusScanner
	configs map[string]json.RawMessage // raw engine configs for on-demand access
	mu      sync.RWMutex
}

// NewScanChain creates an empty scan chain.
func NewScanChain() *ScanChain {
	return &ScanChain{
		configs: make(map[string]json.RawMessage),
	}
}

// LoadFromConfig builds the scan chain from a ScannerFullConfig.
// Only engines listed in cfg.Enabled (in order) are instantiated and started.
func (sc *ScanChain) LoadFromConfig(cfg *config.ScannerFullConfig) error {
	sc.mu.Lock()
	defer sc.mu.Unlock()

	// Store raw configs for all engines
	sc.configs = cfg.Engines

	// Instantiate only enabled engines
	var engines []VirusScanner
	for _, name := range cfg.Enabled {
		rawCfg, ok := cfg.Engines[name]
		if !ok {
			logger.WarnCF("scanner", "Engine listed in enabled but has no config", map[string]interface{}{
				"engine": name,
			})
			continue
		}

		engine, err := CreateEngine(name, rawCfg)
		if err != nil {
			logger.WarnCF("scanner", "Failed to create engine", map[string]interface{}{
				"engine": name,
				"error":  err.Error(),
			})
			continue
		}

		// Skip engines that are not installed (backward compatible: no state field = load normally)
		var stateCheck struct {
			State struct {
				InstallStatus string `json:"install_status"`
			} `json:"state"`
		}
		if json.Unmarshal(rawCfg, &stateCheck) == nil && stateCheck.State.InstallStatus != "" {
			if stateCheck.State.InstallStatus != InstallStatusInstalled {
				logger.InfoCF("scanner", "Skipping engine (not installed)", map[string]interface{}{
					"engine": name, "status": stateCheck.State.InstallStatus,
				})
				continue
			}
		}

		engines = append(engines, engine)
	}

	sc.engines = engines
	return nil
}

// Start starts all engines in the chain.
func (sc *ScanChain) Start(ctx context.Context) error {
	sc.mu.RLock()
	defer sc.mu.RUnlock()

	for _, engine := range sc.engines {
		if err := engine.Start(ctx); err != nil {
			logger.WarnCF("scanner", "Failed to start engine", map[string]interface{}{
				"engine": engine.Name(),
				"error":  err.Error(),
			})
		}
	}
	return nil
}

// Stop stops all engines in the chain.
func (sc *ScanChain) Stop() {
	sc.mu.RLock()
	defer sc.mu.RUnlock()

	for _, engine := range sc.engines {
		if err := engine.Stop(); err != nil {
			logger.WarnCF("scanner", "Failed to stop engine", map[string]interface{}{
				"engine": engine.Name(),
				"error":  err.Error(),
			})
		}
	}
}

// Engines returns the list of engines in execution order.
func (sc *ScanChain) Engines() []VirusScanner {
	sc.mu.RLock()
	defer sc.mu.RUnlock()
	out := make([]VirusScanner, len(sc.engines))
	copy(out, sc.engines)
	return out
}

// RawConfig returns the raw JSON config for a given engine name.
func (sc *ScanChain) RawConfig(name string) (json.RawMessage, bool) {
	sc.mu.RLock()
	defer sc.mu.RUnlock()
	raw, ok := sc.configs[name]
	return raw, ok
}

// ScanFile scans a file through all engines. Short-circuits on first detection.
func (sc *ScanChain) ScanFile(ctx context.Context, path string) *ScanChainResult {
	start := time.Now()
	sc.mu.RLock()
	engines := sc.engines
	sc.mu.RUnlock()

	if len(engines) == 0 {
		return &ScanChainResult{Clean: true, Duration: time.Since(start)}
	}

	var results []*ScanResult
	for _, engine := range engines {
		if !engine.IsReady() {
			logger.WarnCF("scanner", "Engine not ready, skipping", map[string]interface{}{
				"engine": engine.Name(),
			})
			continue
		}

		result, err := engine.ScanFile(ctx, path)
		if err != nil {
			logger.WarnCF("scanner", "Scan error", map[string]interface{}{
				"engine": engine.Name(),
				"path":   path,
				"error":  err.Error(),
			})
			continue
		}

		results = append(results, result)
		if result.Infected {
			return &ScanChainResult{
				Clean:    false,
				Blocked:  true,
				Engine:   engine.Name(),
				Virus:    result.Virus,
				Path:     path,
				Results:  results,
				Duration: time.Since(start),
			}
		}
	}

	return &ScanChainResult{Clean: true, Results: results, Duration: time.Since(start)}
}

// ScanContent scans byte content through all engines.
func (sc *ScanChain) ScanContent(ctx context.Context, data []byte) *ScanChainResult {
	start := time.Now()
	sc.mu.RLock()
	engines := sc.engines
	sc.mu.RUnlock()

	if len(engines) == 0 {
		return &ScanChainResult{Clean: true, Duration: time.Since(start)}
	}

	var results []*ScanResult
	for _, engine := range engines {
		if !engine.IsReady() {
			continue
		}

		result, err := engine.ScanContent(ctx, data)
		if err != nil {
			logger.WarnCF("scanner", "Content scan error", map[string]interface{}{
				"engine": engine.Name(),
				"error":  err.Error(),
			})
			continue
		}

		results = append(results, result)
		if result.Infected {
			return &ScanChainResult{
				Clean:    false,
				Blocked:  true,
				Engine:   engine.Name(),
				Virus:    result.Virus,
				Results:  results,
				Duration: time.Since(start),
			}
		}
	}

	return &ScanChainResult{Clean: true, Results: results, Duration: time.Since(start)}
}

// ScanDirectory scans a directory through all engines.
func (sc *ScanChain) ScanDirectory(ctx context.Context, dirPath string) *ScanChainResult {
	start := time.Now()
	sc.mu.RLock()
	engines := sc.engines
	sc.mu.RUnlock()

	if len(engines) == 0 {
		return &ScanChainResult{Clean: true, Duration: time.Since(start)}
	}

	var allResults []*ScanResult
	for _, engine := range engines {
		if !engine.IsReady() {
			continue
		}

		results, err := engine.ScanDirectory(ctx, dirPath)
		if err != nil {
			logger.WarnCF("scanner", "Directory scan error", map[string]interface{}{
				"engine": engine.Name(),
				"path":   dirPath,
				"error":  err.Error(),
			})
			continue
		}

		for _, r := range results {
			allResults = append(allResults, r)
			if r.Infected {
				return &ScanChainResult{
					Clean:    false,
					Blocked:  true,
					Engine:   engine.Name(),
					Virus:    r.Virus,
					Path:     r.Path,
					Results:  allResults,
					Duration: time.Since(start),
				}
			}
		}
	}

	return &ScanChainResult{Clean: true, Results: allResults, Duration: time.Since(start)}
}

// GetExtensionRules extracts extension rules from the first engine config
// that has ClamAV-style extension settings. Returns empty rules if none found.
func (sc *ScanChain) GetExtensionRules() ExtensionRules {
	sc.mu.RLock()
	defer sc.mu.RUnlock()

	// Try to find extension rules from any engine config
	for _, raw := range sc.configs {
		var cfg struct {
			ScanExtensions []string `json:"scan_extensions"`
			SkipExtensions []string `json:"skip_extensions"`
		}
		if json.Unmarshal(raw, &cfg) == nil {
			if len(cfg.ScanExtensions) > 0 || len(cfg.SkipExtensions) > 0 {
				return ExtensionRules{
					ScanExtensions: cfg.ScanExtensions,
					SkipExtensions: cfg.SkipExtensions,
				}
			}
		}
	}
	return ExtensionRules{}
}

// GetStats returns aggregated statistics from all engines.
func (sc *ScanChain) GetStats() map[string]interface{} {
	sc.mu.RLock()
	defer sc.mu.RUnlock()

	stats := make(map[string]interface{})
	for _, engine := range sc.engines {
		stats[engine.Name()] = engine.GetStats()
	}
	return stats
}

// ScanToolInvocation checks whether a tool invocation should be blocked.
// It returns (allowed, virusError).
func (sc *ScanChain) ScanToolInvocation(ctx context.Context, toolName string, args map[string]interface{}) (bool, error) {
	sc.mu.RLock()
	engines := sc.engines
	sc.mu.RUnlock()

	if len(engines) == 0 {
		return true, nil
	}

	// Determine the file path from args
	filePath, _ := args["path"].(string)
	if filePath == "" {
		filePath, _ = args["save_path"].(string)
	}

	// Check extension rules before scanning
	rules := sc.GetExtensionRules()
	if filePath != "" && !ShouldScanFile(filePath, rules) {
		return true, nil
	}

	// For write operations, scan the content if present
	switch toolName {
	case "write_file", "edit_file", "append_file":
		if content, ok := args["content"].(string); ok && content != "" {
			result := sc.ScanContent(ctx, []byte(content))
			if result.Blocked {
				return false, fmt.Errorf("virus detected by %s: %s (virus: %s)", result.Engine, filePath, result.Virus)
			}
		}

	case "download":
		if filePath != "" {
			result := sc.ScanFile(ctx, filePath)
			if result.Blocked {
				return false, fmt.Errorf("virus detected by %s: %s (virus: %s)", result.Engine, filePath, result.Virus)
			}
		}

	case "exec", "execute_command":
		if filePath != "" {
			result := sc.ScanFile(ctx, filePath)
			if result.Blocked {
				return false, fmt.Errorf("virus detected by %s: %s (virus: %s)", result.Engine, filePath, result.Virus)
			}
		}
	}

	return true, nil
}
