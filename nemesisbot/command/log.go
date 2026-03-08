package command

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/path"
)

// CmdLog manages logging
func CmdLog() {
	if len(os.Args) < 3 {
		LogHelp()
		return
	}

	subcommand := os.Args[2]

	switch subcommand {
	// LLM logging subcommands
	case "llm":
		cmdLogLLM()

	// Backward compatibility: direct enable/disable default to llm
	case "enable":
		cmdLogLLMEnable()
	case "disable":
		cmdLogLLMDisable()
	case "status":
		cmdLogStatus()
	case "config":
		cmdLogConfig()
	default:
		fmt.Printf("Unknown log command: %s\n", subcommand)
		LogHelp()
	}
}

// LogHelp prints log command help
func LogHelp() {
	fmt.Println("\nManage logging for NemesisBot")
	fmt.Println()
	fmt.Println("Usage:")
	fmt.Println("  nemesisbot log <command> [options]")
	fmt.Println()
	fmt.Println("Commands:")
	fmt.Println("  llm                Manage LLM request logging")
	fmt.Println("  enable              Enable LLM request logging (same as 'log llm enable')")
	fmt.Println("  disable             Disable LLM request logging (same as 'log llm disable')")
	fmt.Println("  status              Show all logging status")
	fmt.Println("  config              Configure LLM logging options")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot log llm enable          # Enable LLM logging")
	fmt.Println("  nemesisbot log llm disable         # Disable LLM logging")
	fmt.Println("  nemesisbot log llm status          # Check LLM logging status")
	fmt.Println("  nemesisbot log enable              # Legacy command (same as 'log llm enable')")
	fmt.Println("  nemesisbot log status              # Show all logging status")
	fmt.Println()
	fmt.Println("Use 'nemesisbot log llm' for LLM logging specific commands.")
}

func cmdLogLLM() {
	if len(os.Args) < 4 {
		LogLLMHelp()
		return
	}

	action := os.Args[3]

	switch action {
	case "enable":
		cmdLogLLMEnable()
	case "disable":
		cmdLogLLMDisable()
	case "status":
		cmdLogLLMStatus()
	default:
		fmt.Printf("Unknown llm command: %s\n", action)
		LogLLMHelp()
	}
}

// LogLLMHelp prints LLM log help
func LogLLMHelp() {
	fmt.Println("\nManage LLM request logging")
	fmt.Println()
	fmt.Println("Usage:")
	fmt.Println("  nemesisbot log llm <command>")
	fmt.Println()
	fmt.Println("Commands:")
	fmt.Println("  enable              Enable LLM request logging")
	fmt.Println("  disable             Disable LLM request logging")
	fmt.Println("  status              Show LLM logging status")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot log llm enable")
	fmt.Println("  nemesisbot log llm disable")
	fmt.Println("  nemesisbot log llm status")
}

func cmdLogLLMEnable() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	if cfg.Logging == nil {
		cfg.Logging = &config.LoggingConfig{}
	}

	if cfg.Logging.LLM == nil {
		cfg.Logging.LLM = &config.LLMLogConfig{}
	}

	cfg.Logging.LLM.Enabled = true

	// Set defaults if not set
	if cfg.Logging.LLM.LogDir == "" {
		cfg.Logging.LLM.LogDir = "logs/request_logs"
	}
	if cfg.Logging.LLM.DetailLevel == "" {
		cfg.Logging.LLM.DetailLevel = "full"
	}

	configPath := GetConfigPath()
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("✅ LLM request logging enabled")
	// Display absolute path to user
	displayLogDir := cfg.Logging.LLM.LogDir
	isUnixStyleAbs := len(displayLogDir) > 0 && (displayLogDir[0] == '/' || displayLogDir[0] == '\\')
	if !filepath.IsAbs(displayLogDir) && !strings.HasPrefix(displayLogDir, "~") && !isUnixStyleAbs {
		displayLogDir = filepath.Join(cfg.WorkspacePath(), displayLogDir)
	}
	fmt.Printf("📁 Log directory: %s\n", displayLogDir)
	fmt.Printf("📝 Detail level: %s\n", cfg.Logging.LLM.DetailLevel)
}

func cmdLogLLMDisable() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	if cfg.Logging == nil {
		cfg.Logging = &config.LoggingConfig{}
	}

	if cfg.Logging.LLM == nil {
		cfg.Logging.LLM = &config.LLMLogConfig{}
	}

	cfg.Logging.LLM.Enabled = false

	configPath := GetConfigPath()
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("❌ LLM request logging disabled")
}

func cmdLogStatus() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("LLM Request Logging Status:")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")

	enabled := false
	pm := path.NewPathManager()
	logDir := filepath.Join(pm.Workspace(), "logs", "request_logs")
	detailLevel := "full"

	if cfg.Logging != nil && cfg.Logging.LLM != nil {
		enabled = cfg.Logging.LLM.Enabled
		if cfg.Logging.LLM.LogDir != "" {
			logDir = cfg.Logging.LLM.LogDir
		}
		if cfg.Logging.LLM.DetailLevel != "" {
			detailLevel = cfg.Logging.LLM.DetailLevel
		}
	}

	if enabled {
		fmt.Println("Status:         ✅ Enabled")
	} else {
		fmt.Println("Status:         ❌ Disabled")
	}
	fmt.Printf("Log Directory:  %s\n", logDir)
	fmt.Printf("Detail Level:   %s\n", detailLevel)
	fmt.Printf("Config File:    %s\n", GetConfigPath())
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")

	// Show recent logs if enabled
	if enabled {
		// Expand ~ in path
		logDirExpanded := logDir
		if strings.HasPrefix(logDir, "~") {
			home, _ := os.UserHomeDir()
			if len(logDir) > 1 && (logDir[1] == '/' || logDir[1] == '\\') {
				logDirExpanded = filepath.Join(home, logDir[2:])
			} else {
				logDirExpanded = home
			}
		}

		// List recent log directories
		entries, err := os.ReadDir(logDirExpanded)
		if err != nil {
			if !os.IsNotExist(err) {
				fmt.Printf("\nWarning: Could not read log directory: %v\n", err)
			}
			return
		}

		// Sort by name (which includes timestamp) and get last 5
		if len(entries) > 0 {
			fmt.Println("\nRecent Logs:")
			count := 0
			for i := len(entries) - 1; i >= 0 && count < 5; i-- {
				entry := entries[i]
				if entry.IsDir() {
					// Count files in directory
					dirPath := filepath.Join(logDirExpanded, entry.Name())
					files, _ := os.ReadDir(dirPath)
					var size int64
					for _, file := range files {
						info, _ := file.Info()
						size += info.Size()
					}
					fmt.Printf("  %s  (%d files, %.1f KB)\n", entry.Name(), len(files), float64(size)/1024)
					count++
				}
			}
		}
	}
}

func cmdLogLLMStatus() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("LLM Request Logging Status:")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")

	enabled := false
	pm := path.NewPathManager()
	logDir := filepath.Join(pm.Workspace(), "logs", "request_logs")
	detailLevel := "full"

	if cfg.Logging != nil && cfg.Logging.LLM != nil {
		enabled = cfg.Logging.LLM.Enabled
		if cfg.Logging.LLM.LogDir != "" {
			logDir = cfg.Logging.LLM.LogDir
		}
		if cfg.Logging.LLM.DetailLevel != "" {
			detailLevel = cfg.Logging.LLM.DetailLevel
		}
	}

	if enabled {
		fmt.Println("Status:         ✅ Enabled")
	} else {
		fmt.Println("Status:         ❌ Disabled")
	}
	fmt.Printf("Log Directory:  %s\n", logDir)
	fmt.Printf("Detail Level:   %s\n", detailLevel)
	fmt.Printf("Config File:    %s\n", GetConfigPath())
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")

	// Show recent logs if enabled
	if enabled {
		// Expand ~ in path
		logDirExpanded := logDir
		if strings.HasPrefix(logDir, "~") {
			home, _ := os.UserHomeDir()
			if len(logDir) > 1 && (logDir[1] == '/' || logDir[1] == '\\') {
				logDirExpanded = filepath.Join(home, logDir[2:])
			} else {
				logDirExpanded = home
			}
		}

		// List recent log directories
		entries, err := os.ReadDir(logDirExpanded)
		if err != nil {
			if !os.IsNotExist(err) {
				fmt.Printf("\nWarning: Could not read log directory: %v\n", err)
			}
			return
		}

		// Sort by name (which includes timestamp) and get last 5
		if len(entries) > 0 {
			fmt.Println("\nRecent Logs:")
			count := 0
			for i := len(entries) - 1; i >= 0 && count < 5; i-- {
				entry := entries[i]
				if entry.IsDir() {
					// Count files in directory
					dirPath := filepath.Join(logDirExpanded, entry.Name())
					files, _ := os.ReadDir(dirPath)
					var size int64
					for _, file := range files {
						info, _ := file.Info()
						size += info.Size()
					}
					fmt.Printf("  %s  (%d files, %.1f KB)\n", entry.Name(), len(files), float64(size)/1024)
					count++
				}
			}
		}
	}
}

func cmdLogConfig() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	if cfg.Logging == nil {
		cfg.Logging = &config.LoggingConfig{}
	}

	if cfg.Logging.LLM == nil {
		cfg.Logging.LLM = &config.LLMLogConfig{}
	}

	// Parse command line options
	detailLevel := cfg.Logging.LLM.DetailLevel
	logDir := cfg.Logging.LLM.LogDir

	for i := 3; i < len(os.Args); i++ {
		arg := os.Args[i]
		if strings.HasPrefix(arg, "--detail-level=") {
			detailLevel = strings.TrimPrefix(arg, "--detail-level=")
			if detailLevel != "full" && detailLevel != "truncated" {
				fmt.Printf("Invalid detail level: %s (must be 'full' or 'truncated')\n", detailLevel)
				os.Exit(1)
			}
		} else if strings.HasPrefix(arg, "--log-dir=") {
			logDir = strings.TrimPrefix(arg, "--log-dir=")
		} else {
			fmt.Printf("Unknown option: %s\n", arg)
			os.Exit(1)
		}
	}

	// Update config
	cfg.Logging.LLM.DetailLevel = detailLevel
	cfg.Logging.LLM.LogDir = logDir

	configPath := GetConfigPath()
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("✅ Configuration updated")
	fmt.Printf("📝 Detail level: %s\n", detailLevel)
	fmt.Printf("📁 Log directory: %s\n", logDir)
}
