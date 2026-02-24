package command

import (
	"fmt"
	"os"
	"strings"

	"github.com/276793422/NemesisBot/module/config"
)

// CmdModel manages LLM models
func CmdModel() {
	if len(os.Args) < 3 {
		ModelHelp()
		return
	}

	subcommand := os.Args[2]

	switch subcommand {
	case "list":
		cmdModelList()
	case "add":
		cmdModelAdd()
	case "remove":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot model remove <model_name> [--force]")
			return
		}
		cmdModelRemove(os.Args[3])
	default:
		fmt.Printf("Unknown model command: %s\n", subcommand)
		ModelHelp()
	}
}

// ModelHelp prints model command help
func ModelHelp() {
	fmt.Println("\nManage LLM models")
	fmt.Println()
	fmt.Println("Usage: nemesisbot model <command>")
	fmt.Println()
	fmt.Println("Commands:")
	fmt.Println("  list      List all configured models")
	fmt.Println("  add       Add a new model configuration")
	fmt.Println("  remove    Remove a model configuration")
	fmt.Println()
	fmt.Println("Add options:")
	fmt.Println("  --model <vendor/model>  Model identifier (e.g., openai/gpt-4o)")
	fmt.Println("  --key <api-key>        API key")
	fmt.Println("  --base <url>           API base URL")
	fmt.Println("  --proxy <url>          Proxy URL")
	fmt.Println("  --auth <method>        Authentication method (oauth, token)")
	fmt.Println("  --default              Set as default agent LLM")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot model add --model openai/gpt-4o --key sk-xxx --default")
	fmt.Println("  nemesisbot model add --model anthropic/claude-sonnet-4 --key sk-ant-xxx")
	fmt.Println("  nemesisbot model add --model zhipu/glm-4.7-flash --key xxx --base https://open.bigmodel.cn/api/paas/v4")
	fmt.Println("  nemesisbot model list")
	fmt.Println("  nemesisbot model remove gpt-4o --force")
}

func cmdModelAdd() {
	if len(os.Args) < 4 {
		fmt.Println("Usage: nemesisbot model add --model <vendor/model> [options]")
		fmt.Println()
		fmt.Println("Options:")
		fmt.Println("  --model <vendor/model>  Model identifier (required)")
		fmt.Println("  --key <api-key>        API key")
		fmt.Println("  --base <url>           API base URL")
		fmt.Println("  --proxy <url>          Proxy URL")
		fmt.Println("  --auth <method>        Authentication method (oauth, token)")
		fmt.Println("  --default              Set as default agent LLM")
		return
	}

	// Parse arguments
	modelIdentifier := ""
	apiKey := ""
	apiBase := ""
	proxy := ""
	authMethod := ""
	setAsDefault := false

	args := os.Args[3:]
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--model":
			if i+1 < len(args) {
				modelIdentifier = args[i+1]
				i++
			}
		case "--key":
			if i+1 < len(args) {
				apiKey = args[i+1]
				i++
			}
		case "--base":
			if i+1 < len(args) {
				apiBase = args[i+1]
				i++
			}
		case "--proxy":
			if i+1 < len(args) {
				proxy = args[i+1]
				i++
			}
		case "--auth":
			if i+1 < len(args) {
				authMethod = args[i+1]
				i++
			}
		case "--default":
			setAsDefault = true
		}
	}

	// Validate required fields
	if modelIdentifier == "" {
		fmt.Println("Error: --model is required")
		fmt.Println("Example: nemesisbot model add --model openai/gpt-4o --key sk-xxx")
		os.Exit(1)
	}

	// Parse model identifier to extract vendor and model name
	parts := strings.SplitN(modelIdentifier, "/", 2)
	if len(parts) != 2 {
		fmt.Printf("Error: Invalid model identifier '%s'. Expected format: vendor/model\n", modelIdentifier)
		fmt.Println("Example: openai/gpt-4o, anthropic/claude-sonnet-4")
		os.Exit(1)
	}

	// Generate a model_name alias from the model name
	modelNameAlias := strings.TrimSpace(parts[1])

	// Load config
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Check if model already exists
	for _, m := range cfg.ModelList {
		if m.ModelName == modelNameAlias {
			fmt.Printf("Warning: Model '%s' already exists. Updating...\n", modelNameAlias)
		}
		if m.Model == modelIdentifier {
			fmt.Printf("Warning: Model identifier '%s' already configured as '%s'\n", modelIdentifier, m.ModelName)
		}
	}

	// Create new model config
	newModel := config.ModelConfig{
		ModelName:  modelNameAlias,
		Model:      modelIdentifier,
		APIKey:     apiKey,
		APIBase:    apiBase,
		Proxy:      proxy,
		AuthMethod: authMethod,
	}

	// Add or update model list
	found := false
	for i, m := range cfg.ModelList {
		if m.ModelName == modelNameAlias {
			cfg.ModelList[i] = newModel
			found = true
			break
		}
	}
	if !found {
		cfg.ModelList = append(cfg.ModelList, newModel)
	}

	// Save config
	configPath := GetConfigPath()
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	action := "added"
	if found {
		action = "updated"
	}
	fmt.Printf("✓ Model '%s' %s successfully!\n", modelNameAlias, action)

	// Set as default LLM if --default flag is provided
	if setAsDefault {
		cfg.Agents.Defaults.LLM = modelNameAlias

		if err := config.SaveConfig(configPath, cfg); err != nil {
			fmt.Printf("Warning: Failed to set as default: %v\n", err)
		} else {
			fmt.Printf("✓ Set as default LLM: %s\n", modelNameAlias)
		}
	}
}

func cmdModelList() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Check for verbose flag
	verbose := false
	args := os.Args[3:]
	for _, arg := range args {
		if arg == "--verbose" || arg == "-v" {
			verbose = true
			break
		}
	}

	if len(cfg.ModelList) == 0 {
		fmt.Println("No models configured.")
		fmt.Println("\nAdd a model using: nemesisbot model add --model <vendor/model> [options]")
		return
	}

	// Get current default LLM
	defaultLLM := config.GetEffectiveLLM(cfg)

	fmt.Println("\nConfigured Models:")
	fmt.Println("==================")

	for _, m := range cfg.ModelList {
		isDefault := ""
		if m.ModelName == defaultLLM {
			isDefault = " (default)"
		}

		fmt.Printf("\n%s%s\n", m.ModelName, isDefault)
		fmt.Printf("  Model: %s\n", m.Model)

		if verbose {
			// Show detailed information
			if m.APIKey != "" {
				fmt.Printf("  API Key: ••••••••••••••••\n")
			}
			if m.APIBase != "" {
				fmt.Printf("  API Base: %s\n", m.APIBase)
			}
			if m.Proxy != "" {
				fmt.Printf("  Proxy: %s\n", m.Proxy)
			}
			if m.AuthMethod != "" {
				fmt.Printf("  Auth Method: %s\n", m.AuthMethod)
			}
		}
	}

	fmt.Println()
}

func cmdModelRemove(modelName string) {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Check for --force flag
	force := false
	args := os.Args[4:]
	for _, arg := range args {
		if arg == "--force" || arg == "-f" {
			force = true
			break
		}
	}

	// Find the model
	foundIndex := -1
	for i, m := range cfg.ModelList {
		if m.ModelName == modelName {
			foundIndex = i
			break
		}
	}

	if foundIndex == -1 {
		fmt.Printf("Error: Model '%s' not found\n", modelName)
		fmt.Println("List models using: nemesisbot model list")
		os.Exit(1)
	}

	// Check if this is the current default model
	defaultLLM := config.GetEffectiveLLM(cfg)
	if defaultLLM == modelName {
		fmt.Printf("Error: Cannot remove model '%s' - it is the current default\n", modelName)
		fmt.Println("\nSwitch to another model first:")
		fmt.Println("  nemesisbot agent set llm <model_name>")
		os.Exit(1)
	}

	// Confirm deletion
	if !force {
		fmt.Printf("Remove model '%s'? [y/N]: ", modelName)
		var response string
		fmt.Scanln(&response)
		if strings.ToLower(response) != "y" {
			fmt.Println("Aborted.")
			return
		}
	}

	// Remove model from list
	cfg.ModelList = append(cfg.ModelList[:foundIndex], cfg.ModelList[foundIndex+1:]...)

	// Save config
	configPath := GetConfigPath()
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("✓ Model '%s' removed successfully!\n", modelName)
}
