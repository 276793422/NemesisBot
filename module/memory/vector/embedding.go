package vector

import (
	"context"
	"fmt"
	"os"

	chromem "github.com/philippgille/chromem-go"
)

// NewEmbeddingFunc selects the appropriate embedding function based on configuration.
// It guarantees that the returned EmbeddingFunc is NEVER nil.
//
// Priority:
//   Tier 1: ONNX plugin (PluginPath exists) → best quality, fully offline
//   Tier 2: Provider API (APIModel set + provider non-nil) → good quality, costs tokens
//   Tier 3: Local hash (default) → zero cost, fully offline
func NewEmbeddingFunc(cfg StoreConfig, provider EmbeddingProvider, dim int) chromem.EmbeddingFunc {
	tryPlugin := cfg.EmbeddingTier == "plugin" || cfg.EmbeddingTier == "auto" || cfg.EmbeddingTier == ""
	tryAPI := cfg.EmbeddingTier == "api" || cfg.EmbeddingTier == "auto" || cfg.EmbeddingTier == ""

	// Explicit "local" tier skips plugin and API entirely.
	if cfg.EmbeddingTier == "local" {
		tryPlugin = false
		tryAPI = false
	}

	// Tier 1: Plugin
	if tryPlugin && cfg.PluginPath != "" {
		if _, err := os.Stat(cfg.PluginPath); err == nil {
			fn, err := newPluginEmbeddingFunc(cfg.PluginPath, cfg.PluginModelPath, dim)
			if err == nil {
				return fn
			}
			// Plugin load failed, continue to next tier
		}
	}

	// Tier 2: API
	if tryAPI && provider != nil && cfg.APIModel != "" {
		return APIEmbeddingFunc(provider, cfg.APIModel)
	}

	// Tier 3: Local hash (always available)
	return LocalEmbeddingFunc(dim)
}

// newPluginEmbeddingFunc attempts to load a plugin and return an EmbeddingFunc.
func newPluginEmbeddingFunc(pluginPath, modelPath string, dim int) (chromem.EmbeddingFunc, error) {
	plugin, err := LoadPlugin(pluginPath)
	if err != nil {
		return nil, fmt.Errorf("vector: load plugin %s: %w", pluginPath, err)
	}
	if err := plugin.Init(modelPath, dim); err != nil {
		plugin.Close()
		return nil, fmt.Errorf("vector: init plugin: %w", err)
	}
	return func(ctx context.Context, text string) ([]float32, error) {
		return plugin.Embed(text)
	}, nil
}
