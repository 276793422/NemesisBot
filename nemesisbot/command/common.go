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

// SetEmbeddedFS sets the embedded filesystems from main
func SetEmbeddedFS(embedded, defaultFs embed.FS) {
	EmbeddedFiles = embedded
	DefaultFiles = defaultFs
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
	configPath := os.Getenv("NEMESISBOT_CONFIG")
	if configPath != "" {
		return configPath
	}
	homeDir, err := os.UserHomeDir()
	if err != nil {
		return "./config.json"
	}
	return filepath.Join(homeDir, ".nemesisbot", "config.json")
}

// GetSecurityConfigPath returns the security config file path
func GetSecurityConfigPath() string {
	configPath := os.Getenv("NEMESISBOT_SECURITY_CONFIG")
	if configPath != "" {
		return configPath
	}
	homeDir, err := os.UserHomeDir()
	if err != nil {
		return "./config.security.json"
	}
	return filepath.Join(homeDir, ".nemesisbot", "config.security.json")
}

// GetMCPConfigPath returns the MCP config file path
func GetMCPConfigPath() string {
	configPath := os.Getenv("NEMESISBOT_MCP_CONFIG")
	if configPath != "" {
		return configPath
	}
	homeDir, err := os.UserHomeDir()
	if err != nil {
		return "./config.mcp.json"
	}
	return filepath.Join(homeDir, ".nemesisbot", "config.mcp.json")
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
