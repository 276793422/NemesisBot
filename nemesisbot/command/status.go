package command

import (
	"fmt"
	"os"
	"strings"

	"github.com/276793422/NemesisBot/module/auth"
	"github.com/276793422/NemesisBot/module/config"
)

// CmdStatus shows nemesisbot status
func CmdStatus() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		return
	}

	configPath := GetConfigPath()

	fmt.Printf("%s nemesisbot Status\n", Logo)
	fmt.Printf("Version: %s\n", FormatVersion())
	build, _ := FormatBuildInfo()
	if build != "" {
		fmt.Printf("Build: %s\n", build)
	}
	fmt.Println()

	if _, err := os.Stat(configPath); err == nil {
		fmt.Println("Config:", configPath, "✓")
	} else {
		fmt.Println("Config:", configPath, "✗")
	}

	workspace := cfg.WorkspacePath()
	if _, err := os.Stat(workspace); err == nil {
		fmt.Println("Workspace:", workspace, "✓")
	} else {
		fmt.Println("Workspace:", workspace, "✗")
	}

	if _, err := os.Stat(configPath); err == nil {
		defaultLLM := config.GetEffectiveLLM(cfg)
		fmt.Printf("Model: %s\n", defaultLLM)

		// Count configured models by provider
		providerCounts := make(map[string]int)
		providerKeys := make(map[string]bool)
		for _, m := range cfg.ModelList {
			parts := strings.SplitN(m.Model, "/", 2)
			if len(parts) == 2 {
				provider := strings.ToLower(parts[0])
				providerCounts[provider]++
				if m.APIKey != "" {
					providerKeys[provider] = true
				}
			}
		}

		status := func(enabled bool) string {
			if enabled {
				return "✓"
			}
			return "not set"
		}

		fmt.Println("\nConfigured Models:")
		for provider, count := range providerCounts {
			hasKey := providerKeys[provider]
			fmt.Printf("  %s: %d model(s), API key: %s\n", provider, count, status(hasKey))
		}

		store, _ := auth.LoadStore()
		if store != nil && len(store.Credentials) > 0 {
			fmt.Println("\nOAuth/Token Auth:")
			for provider, cred := range store.Credentials {
				authStatus := "authenticated"
				if cred.IsExpired() {
					authStatus = "expired"
				} else if cred.NeedsRefresh() {
					authStatus = "needs refresh"
				}
				fmt.Printf("  %s (%s): %s\n", provider, cred.AuthMethod, authStatus)
			}
		}
	}
}

// StatusHelp prints status command help
func StatusHelp() {
	fmt.Println("\nStatus command shows NemesisBot configuration and runtime status")
	fmt.Println()
	fmt.Println("Usage: nemesisbot status")
	fmt.Println()
	fmt.Println("Displays:")
	fmt.Println("  • Version and build information")
	fmt.Println("  • Configuration file location")
	fmt.Println("  • Workspace directory")
	fmt.Println("  • Default LLM model")
	fmt.Println("  • Configured models by provider")
	fmt.Println("  • OAuth/Token authentication status")
}
