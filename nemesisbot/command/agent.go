package command

import (
	"context"
	"fmt"
	"os"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/providers"
)

// CmdAgent runs the agent in CLI mode
func CmdAgent() {
	// Check for "set" subcommand first
	if len(os.Args) >= 3 && os.Args[2] == "set" {
		if len(os.Args) < 5 {
			fmt.Println("Usage: nemesisbot agent set llm <provider>/<model>")
			fmt.Println()
			fmt.Println("Examples:")
			fmt.Println("  nemesisbot agent set llm zhipu/glm-4.7-flash")
			fmt.Println("  nemesisbot agent set llm openai/gpt-4o")
			return
		}
		if os.Args[3] == "llm" {
			cmdAgentSetLLM(os.Args[4])
			return
		}
		fmt.Printf("Unknown agent set command: %s\n", os.Args[3])
		fmt.Println("Supported: llm")
		return
	}

	message := ""
	sessionKey := "cli:default"

	args := os.Args[2:]
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--debug", "-d":
			logger.SetLevel(logger.DEBUG)
			fmt.Println("🔍 Debug mode enabled")
		case "-m", "--message":
			if i+1 < len(args) {
				message = args[i+1]
				i++
			}
		case "-s", "--session":
			if i+1 < len(args) {
				sessionKey = args[i+1]
				i++
			}
		}
	}

	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	provider, err := providers.CreateProvider(cfg)
	if err != nil {
		fmt.Printf("Error creating provider: %v\n", err)
		os.Exit(1)
	}

	msgBus := bus.NewMessageBus()
	agentLoop := agent.NewAgentLoop(cfg, msgBus, provider)

	// Print agent startup info (only for interactive mode)
	startupInfo := agentLoop.GetStartupInfo()
	logger.InfoCF("agent", "Agent initialized",
		map[string]interface{}{
			"tools_count":      startupInfo["tools"].(map[string]interface{})["count"],
			"skills_total":     startupInfo["skills"].(map[string]interface{})["total"],
			"skills_available": startupInfo["skills"].(map[string]interface{})["available"],
		})

	if message != "" {
		ctx := context.Background()
		response, err := agentLoop.ProcessDirect(ctx, message, sessionKey)
		if err != nil {
			fmt.Printf("Error: %v\n", err)
			os.Exit(1)
		}
		fmt.Printf("\n%s %s\n", Logo, response)
	} else {
		fmt.Printf("%s Interactive mode (Ctrl+C to exit)\n\n", Logo)
		if err := RunInteractiveMode(agentLoop, sessionKey); err != nil {
			fmt.Printf("Error in interactive mode: %v\n", err)
		}
	}
}

// cmdAgentSetLLM sets the default LLM for the agent
func cmdAgentSetLLM(llmRef string) {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Validate the LLM reference
	_, err = config.ResolveModelConfig(cfg, llmRef)
	if err != nil {
		fmt.Printf("Error: Invalid LLM reference '%s': %v\n", llmRef, err)
		fmt.Println()
		fmt.Println("Format: <model_name> or <vendor/model>")
		fmt.Println("Example: glm-4.7 or zhipu/glm-4.7-flash")
		os.Exit(1)
	}

	// Update the LLM field (new format)
	cfg.Agents.Defaults.LLM = llmRef

	// Save config
	configPath := GetConfigPath()
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("✓ Default LLM set to: %s\n", llmRef)
	fmt.Println()
	fmt.Println("Configuration saved. Restart agent/gateway to apply changes.")
}

// AgentHelp prints agent command help
func AgentHelp() {
	fmt.Println("\nAgent commands:")
	fmt.Println("  nemesisbot agent [options]           Start interactive agent mode")
	fmt.Println("  nemesisbot agent set llm <model>     Set default LLM model")
	fmt.Println()
	fmt.Println("Options:")
	fmt.Println("  -m, --message <text>    Send a single message and exit")
	fmt.Println("  -s, --session <key>     Use specific session key (default: cli:default)")
	fmt.Println("  -d, --debug             Enable debug logging")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot agent                        Interactive mode")
	fmt.Println("  nemesisbot agent -m \"Hello!\"            Single message")
	fmt.Println("  nemesisbot agent set llm zhipu/glm-4.7  Set LLM")
	fmt.Println()
	fmt.Println("LLM format: <vendor>/<model> or <model_name>")
	fmt.Println("  Examples: zhipu/glm-4.7-flash, openai/gpt-4o, claude-sonnet-4")
}
