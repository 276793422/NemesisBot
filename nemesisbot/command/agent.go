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
			fmt.Println("       nemesisbot agent set concurrent-mode <reject|queue> [--queue-size <size>]")
			fmt.Println()
			fmt.Println("Examples:")
			fmt.Println("  nemesisbot agent set llm zhipu/glm-4.7-flash")
			fmt.Println("  nemesisbot agent set llm openai/gpt-4o")
			fmt.Println("  nemesisbot agent set concurrent-mode reject")
			fmt.Println("  nemesisbot agent set concurrent-mode queue")
			fmt.Println("  nemesisbot agent set concurrent-mode queue --queue-size 16")
			return
		}
		if os.Args[3] == "llm" {
			cmdAgentSetLLM(os.Args[4])
			return
		}
		if os.Args[3] == "concurrent-mode" {
			cmdAgentSetConcurrentMode(os.Args[4:])
			return
		}
		fmt.Printf("Unknown agent set command: %s\n", os.Args[3])
		fmt.Println("Supported: llm, concurrent-mode")
		return
	}

	message := ""
	sessionKey := "cli:default"

	// Load configuration first
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Initialize logger from config
	// Process command line args AFTER loading config
	args := os.Args[2:]
	InitLoggerFromConfig(cfg, args)

	// Parse remaining arguments
	for i := 0; i < len(args); i++ {
		switch args[i] {
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

// cmdAgentSetConcurrentMode sets the concurrent request mode
func cmdAgentSetConcurrentMode(args []string) {
	if len(args) == 0 {
		fmt.Println("Error: concurrent-mode requires a value (reject or queue)")
		fmt.Println()
		fmt.Println("Usage: nemesisbot agent set concurrent-mode <reject|queue> [--queue-size <size>]")
		os.Exit(1)
	}

	mode := args[0]
	if mode != "reject" && mode != "queue" {
		fmt.Printf("Error: Invalid concurrent-mode '%s'. Must be 'reject' or 'queue'\n", mode)
		os.Exit(1)
	}

	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Update the concurrent request mode
	cfg.Agents.Defaults.ConcurrentRequestMode = mode

	// If mode is queue and queue-size is specified, update it
	if mode == "queue" {
		// Default queue size
		queueSize := 8

		// Check for --queue-size flag
		for i := 1; i < len(args); i++ {
			if args[i] == "--queue-size" && i+1 < len(args) {
				size, err := parseIntArg(args[i+1])
				if err != nil || size < 1 {
					fmt.Println("Error: --queue-size must be a positive integer")
					os.Exit(1)
				}
				queueSize = size
				break
			}
		}
		cfg.Agents.Defaults.QueueSize = queueSize
		fmt.Printf("✓ Concurrent request mode set to: %s (queue size: %d)\n", mode, queueSize)
	} else {
		fmt.Printf("✓ Concurrent request mode set to: %s\n", mode)
	}

	// Save config
	configPath := GetConfigPath()
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println()
	fmt.Println("Configuration saved. Restart agent/gateway to apply changes.")
}

// parseIntArg parses a string to int
func parseIntArg(s string) (int, error) {
	var result int
	_, err := fmt.Sscanf(s, "%d", &result)
	return result, err
}

// AgentHelp prints agent command help
func AgentHelp() {
	fmt.Println("\nAgent commands:")
	fmt.Println("  nemesisbot agent [options]                                    Start interactive agent mode")
	fmt.Println("  nemesisbot agent set llm <model>                              Set default LLM model")
	fmt.Println("  nemesisbot agent set concurrent-mode <reject|queue> [options] Set concurrent request mode")
	fmt.Println()
	fmt.Println("Concurrent mode options:")
	fmt.Println("  reject                    Second request returns busy immediately (default)")
	fmt.Println("  queue                     Second request waits in queue")
	fmt.Println("  --queue-size <size>       Queue size (only for queue mode, default: 8)")
	fmt.Println()
	fmt.Println("Agent options:")
	fmt.Println("  -m, --message <text>    Send a single message and exit")
	fmt.Println("  -s, --session <key>     Use specific session key (default: cli:default)")
	fmt.Println("  -d, --debug             Enable debug logging")
	fmt.Println("  -q, --quiet             Disable all logging")
	fmt.Println("      --no-console         Disable console output (file only)")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot agent                                          Interactive mode")
	fmt.Println("  nemesisbot agent -m \"Hello!\"                              Single message")
	fmt.Println("  nemesisbot agent set llm zhipu/glm-4.7                    Set LLM")
	fmt.Println("  nemesisbot agent set concurrent-mode reject               Reject on busy")
	fmt.Println("  nemesisbot agent set concurrent-mode queue                Queue on busy (size: 8)")
	fmt.Println("  nemesisbot agent set concurrent-mode queue --queue-size 16  Queue on busy (size: 16)")
	fmt.Println()
	fmt.Println("LLM format: <vendor>/<model> or <model_name>")
	fmt.Println("  Examples: zhipu/glm-4.7-flash, openai/gpt-4o, claude-sonnet-4")
}
