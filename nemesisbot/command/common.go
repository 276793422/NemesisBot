// Package command implements CLI commands for NemesisBot
package command

import (
	"bufio"
	"context"
	"embed"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/cron"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/path"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/tools"
	"github.com/chzyer/readline"
)

// Version information (set by linker)
var (
	Version   = "dev"
	GitCommit string
	BuildTime string
	GoVersion string
)

const Logo = "🤖"

// Embedded filesystems (must be set by main)
var EmbeddedFiles embed.FS
var DefaultFiles embed.FS
var ConfigFiles embed.FS

// SetEmbeddedFS sets the embedded filesystems from main
func SetEmbeddedFS(embedded, defaultFs, configFs embed.FS) {
	EmbeddedFiles = embedded
	DefaultFiles = defaultFs
	ConfigFiles = configFs

	// Initialize config package with embedded defaults
	if err := initializeConfigDefaults(configFs); err != nil {
		// This should not happen in production, but don't crash
		// The error will be handled when LoadEmbeddedConfig is called
		fmt.Fprintf(os.Stderr, "Warning: Failed to initialize embedded config: %v\n", err)
	}
}

// initializeConfigDefaults initializes the config package with embedded defaults
func initializeConfigDefaults(configFS embed.FS) error {
	// Read config/config.default.json
	data, err := fs.ReadFile(configFS, "config/config.default.json")
	if err != nil {
		return fmt.Errorf("failed to read config/config.default.json: %w", err)
	}

	// Read config/config.mcp.default.json
	mcpData, err := fs.ReadFile(configFS, "config/config.mcp.default.json")
	if err != nil {
		return fmt.Errorf("failed to read config/config.mcp.default.json: %w", err)
	}

	// Read config/config.security.default.json
	securityData, err := fs.ReadFile(configFS, "config/config.security.default.json")
	if err != nil {
		return fmt.Errorf("failed to read config/config.security.default.json: %w", err)
	}

	// Read config/config.cluster.default.json
	clusterData, err := fs.ReadFile(configFS, "config/config.cluster.default.json")
	if err != nil {
		return fmt.Errorf("failed to read config/config.cluster.default.json: %w", err)
	}

	// Initialize config package
	return config.SetEmbeddedDefaults(data, mcpData, securityData, clusterData)
}

// SetVersionInfo sets version information from main
func SetVersionInfo(v, gc, bt, gv string) {
	Version = v
	GitCommit = gc
	BuildTime = bt
	GoVersion = gv
}

// FormatVersion returns the version string with optional git commit
func FormatVersion() string {
	v := Version
	if GitCommit != "" {
		v += fmt.Sprintf(" (git: %s)", GitCommit)
	}
	return v
}

// FormatBuildInfo returns build time and go version info
func FormatBuildInfo() (build string, goVer string) {
	if BuildTime != "" {
		build = BuildTime
	}
	goVer = GoVersion
	if goVer == "" {
		goVer = runtime.Version()
	}
	return
}

// PrintVersion prints version information
func PrintVersion() {
	fmt.Printf("%s nemesisbot %s\n", Logo, FormatVersion())
	build, goVer := FormatBuildInfo()
	if build != "" {
		fmt.Printf("  Build: %s\n", build)
	}
	if goVer != "" {
		fmt.Printf("  Go: %s\n", goVer)
	}
}

// GetConfigPath returns the main config file path
func GetConfigPath() string {
	return path.ResolveConfigPath()
}

// GetSecurityConfigPath returns the security config file path
func GetSecurityConfigPath() string {
	return path.ResolveSecurityConfigPath()
}

// GetMCPConfigPath returns the MCP config file path
func GetMCPConfigPath() string {
	return path.ResolveMCPConfigPath()
}

// CopyDirectory copies a directory recursively
func CopyDirectory(src, dst string) error {
	return filepath.Walk(src, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}

		relPath, err := filepath.Rel(src, path)
		if err != nil {
			return err
		}

		dstPath := filepath.Join(dst, relPath)

		if info.IsDir() {
			return os.MkdirAll(dstPath, info.Mode())
		}

		srcFile, err := os.Open(path)
		if err != nil {
			return err
		}
		defer srcFile.Close()

		dstFile, err := os.OpenFile(dstPath, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, info.Mode())
		if err != nil {
			return err
		}
		defer dstFile.Close()

		_, err = io.Copy(dstFile, srcFile)
		return err
	})
}

// CopyEmbeddedToTarget copies embedded files to target directory
func CopyEmbeddedToTarget(targetDir string) error {
	if err := os.MkdirAll(targetDir, 0755); err != nil {
		return fmt.Errorf("failed to create target directory: %w", err)
	}

	err := fs.WalkDir(EmbeddedFiles, "workspace", func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}

		if d.IsDir() {
			return nil
		}

		data, err := EmbeddedFiles.ReadFile(path)
		if err != nil {
			return fmt.Errorf("failed to read embedded file %s: %w", path, err)
		}

		newPath, err := filepath.Rel("workspace", path)
		if err != nil {
			return fmt.Errorf("failed to get relative path for %s: %v", path, err)
		}

		targetPath := filepath.Join(targetDir, newPath)

		if err := os.MkdirAll(filepath.Dir(targetPath), 0755); err != nil {
			return fmt.Errorf("failed to create directory %s: %w", filepath.Dir(targetPath), err)
		}

		if err := os.WriteFile(targetPath, data, 0644); err != nil {
			return fmt.Errorf("failed to write file %s: %w", targetPath, err)
		}

		return nil
	})

	return err
}

// LoadConfig loads the main configuration
func LoadConfig() (*config.Config, error) {
	configPath := GetConfigPath()
	return config.LoadConfig(configPath)
}

// SetupCronTool creates and configures the cron service
func SetupCronTool(agentLoop *agent.AgentLoop, msgBus *bus.MessageBus, workspace string, restrict bool, execTimeout time.Duration, cfg *config.Config) *cron.CronService {
	cronStorePath := filepath.Join(workspace, "cron", "jobs.json")

	// Create cron service
	cronService := cron.NewCronService(cronStorePath, nil)

	// Create and register CronTool
	cronTool := tools.NewCronTool(cronService, agentLoop, msgBus, workspace, restrict, execTimeout, cfg)
	agentLoop.RegisterTool(cronTool)

	// Set the onJob handler
	cronService.SetOnJob(func(job *cron.CronJob) (string, error) {
		result := cronTool.ExecuteJob(context.Background(), job)
		return result, nil
	})

	return cronService
}

// ShouldSkipHeartbeatForBootstrap checks if BOOTSTRAP.md exists
func ShouldSkipHeartbeatForBootstrap(workspace string) bool {
	bootstrapPath := filepath.Join(workspace, "BOOTSTRAP.md")
	_, err := os.ReadFile(bootstrapPath)
	return err == nil
}

// CreateAgentLoop creates a new agent loop with provider
func CreateAgentLoop(cfg *config.Config) (*agent.AgentLoop, providers.LLMProvider, error) {
	provider, err := providers.CreateProvider(cfg)
	if err != nil {
		return nil, nil, fmt.Errorf("error creating provider: %w", err)
	}

	msgBus := bus.NewMessageBus()
	agentLoop := agent.NewAgentLoop(cfg, msgBus, provider)

	return agentLoop, provider, nil
}

// PrintAgentStartupInfo prints agent initialization info
func PrintAgentStartupInfo(agentLoop *agent.AgentLoop) {
	startupInfo := agentLoop.GetStartupInfo()
	logger.InfoCF("agent", "Agent initialized",
		map[string]interface{}{
			"tools_count":      startupInfo["tools"].(map[string]interface{})["count"],
			"skills_total":     startupInfo["skills"].(map[string]interface{})["total"],
			"skills_available": startupInfo["skills"].(map[string]interface{})["available"],
		})
}

// RunInteractiveMode runs the CLI interactive mode
func RunInteractiveMode(agentLoop *agent.AgentLoop, sessionKey string) error {
	prompt := fmt.Sprintf("%s You: ", Logo)

	rl, err := readline.NewEx(&readline.Config{
		Prompt:          prompt,
		HistoryFile:     filepath.Join(os.TempDir(), ".nemesisbot_history"),
		HistoryLimit:    100,
		InterruptPrompt: "^C",
		EOFPrompt:       "exit",
	})

	if err != nil {
		fmt.Printf("Error initializing readline: %v\n", err)
		fmt.Println("Falling back to simple input mode...")
		return RunSimpleInteractiveMode(agentLoop, sessionKey)
	}
	defer rl.Close()

	for {
		line, err := rl.Readline()
		if err != nil {
			if err == io.EOF || err == readline.ErrInterrupt {
				fmt.Println("\nGoodbye!")
				return nil
			}
			fmt.Printf("Error reading input: %v\n", err)
			continue
		}

		input := strings.TrimSpace(line)
		if input == "" {
			continue
		}

		if input == "exit" || input == "quit" {
			fmt.Println("Goodbye!")
			return nil
		}

		ctx := context.Background()
		response, err := agentLoop.ProcessDirect(ctx, input, sessionKey)
		if err != nil {
			fmt.Printf("Error: %v\n", err)
			continue
		}

		fmt.Printf("\n%s %s\n\n", Logo, response)
	}
}

// RunSimpleInteractiveMode runs a simple CLI interactive mode without readline
func RunSimpleInteractiveMode(agentLoop *agent.AgentLoop, sessionKey string) error {
	reader := bufio.NewReader(os.Stdin)
	for {
		fmt.Print(fmt.Sprintf("%s You: ", Logo))
		line, err := reader.ReadString('\n')
		if err != nil {
			if err == io.EOF {
				fmt.Println("\nGoodbye!")
				return nil
			}
			fmt.Printf("Error reading input: %v\n", err)
			continue
		}

		input := strings.TrimSpace(line)
		if input == "" {
			continue
		}

		if input == "exit" || input == "quit" {
			fmt.Println("Goodbye!")
			return nil
		}

		ctx := context.Background()
		response, err := agentLoop.ProcessDirect(ctx, input, sessionKey)
		if err != nil {
			fmt.Printf("Error: %v\n", err)
			continue
		}

		fmt.Printf("\n%s %s\n\n", Logo, response)
	}
}

// InitLoggerFromConfig initializes the logger from configuration
// Returns a bitmask of what was overridden:
//
//	1 = --debug was used
//	2 = --quiet was used
//	4 = --no-console was used
func InitLoggerFromConfig(cfg *config.Config, checkArgs []string) int {
	// Default values
	enabled := true       // Master switch default: enabled
	enableConsole := true // Console switch default: enabled
	level := "INFO"
	file := ""

	// Read from config file
	if cfg.Logging != nil && cfg.Logging.General != nil {
		generalCfg := cfg.Logging.General

		// Read master switch
		enabled = generalCfg.Enabled

		// Read console switch
		enableConsole = generalCfg.EnableConsole

		// Read log level
		if generalCfg.Level != "" {
			level = generalCfg.Level
		}

		// Read file path
		if generalCfg.File != "" {
			file = generalCfg.File
		}
	}

	// Check command line argument overrides
	overrideFlags := 0

	for _, arg := range checkArgs {
		switch arg {
		case "--quiet", "-q":
			// Completely disable logging (highest priority)
			enabled = false
			overrideFlags |= 2
			fmt.Println("🔇 Logging disabled (quiet mode)")
		case "--no-console":
			// Disable console output
			enableConsole = false
			overrideFlags |= 4
			fmt.Println("📺 Console output disabled")
		case "--debug", "-d":
			// Enable DEBUG level
			level = "DEBUG"
			overrideFlags |= 1
			fmt.Println("🔍 Debug mode enabled")
		}
	}

	// Apply configuration

	// 1. Set master switch
	if enabled {
		logger.EnableLogging()
	} else {
		logger.DisableLogging()
		return overrideFlags // If logging disabled, return immediately
	}

	// 2. Set console switch
	if enableConsole {
		logger.EnableConsole()
	} else {
		logger.DisableConsole()
	}

	// 3. Set log level
	switch strings.ToUpper(level) {
	case "DEBUG":
		logger.SetLevel(logger.DEBUG)
	case "INFO":
		logger.SetLevel(logger.INFO)
	case "WARN":
		logger.SetLevel(logger.WARN)
	case "ERROR":
		logger.SetLevel(logger.ERROR)
	case "FATAL":
		logger.SetLevel(logger.FATAL)
	default:
		logger.SetLevel(logger.INFO)
		if level != "" && level != "INFO" {
			fmt.Printf("⚠️  Invalid log level '%s', using INFO\n", level)
		}
	}

	// 4. Enable file logging
	if file != "" {
		// Expand tilde
		logPath := file
		if strings.HasPrefix(logPath, "~") {
			home, _ := os.UserHomeDir()
			if len(logPath) > 1 && (logPath[1] == '/' || logPath[1] == '\\') {
				logPath = filepath.Join(home, logPath[2:])
			} else {
				logPath = home
			}
		}

		// If relative path, base on workspace
		if !filepath.IsAbs(logPath) && !strings.HasPrefix(logPath, "~") {
			workspace := cfg.WorkspacePath()
			logPath = filepath.Join(workspace, logPath)
		}

		if err := logger.EnableFileLogging(logPath); err != nil {
			fmt.Printf("⚠️  Failed to enable file logging: %v\n", err)
		}
	}

	// Display current configuration status
	if overrideFlags&1 == 0 { // If not --debug override
		fmt.Printf("📊 Logging: %s, Level: %s, Console: %v\n",
			map[bool]string{true: "✅", false: "❌"}[logger.IsLoggingEnabled()],
			level,
			logger.IsConsoleEnabled(),
		)
	}

	return overrideFlags
}
