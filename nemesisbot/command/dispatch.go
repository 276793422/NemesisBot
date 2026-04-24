package command

import (
	"fmt"
	"os"

	"github.com/276793422/NemesisBot/nemesisbot/command/test"
)

// Dispatch routes commands to their handlers
func Dispatch() {
	if len(os.Args) < 2 {
		PrintHelp()
		os.Exit(1)
	}

	command := os.Args[1]

	switch command {
	case "onboard":
		// Keep onboard in main.go as it requires embedded files
		return
	case "agent":
		CmdAgent()
	case "gateway":
		CmdGateway()
	case "test":
		// Hidden test command for window testing
		test.CmdWindow()
	case "status":
		CmdStatus()
	case "migrate":
		CmdMigrate()
	case "auth":
		CmdAuth()
	case "cron":
		CmdCron()
	case "mcp":
		CmdMCP()
	case "log":
		CmdLog()
	case "channel":
		CmdChannel()
	case "security":
		CmdSecurity()
	case "cluster":
		CmdCluster()
	case "cors":
		CmdCORS()
	case "model":
		CmdModel()
	case "skills":
		CmdSkills()
	case "forge":
		CmdForge()
	case "workflow":
		CmdWorkflow()
	case "version", "--version", "-v":
		PrintVersion()
	case "help", "--help", "-h":
		PrintHelp()
	default:
		fmt.Printf("Unknown command: %s\n", command)
		PrintHelp()
		os.Exit(1)
	}
}

// PrintHelp prints the main help message
func PrintHelp() {
	fmt.Printf("%s nemesisbot - Personal AI Assistant v%s\n\n", Logo, Version)
	fmt.Println("Usage: nemesisbot <command>")
	fmt.Println()
	fmt.Println("Commands:")
	fmt.Println("  onboard     Initialize nemesisbot configuration and workspace")
	fmt.Println("  agent       Interact with the agent directly")
	fmt.Println("  auth        Manage authentication (login, logout, status)")
	fmt.Println("  gateway     Start nemesisbot gateway")
	fmt.Println("  status      Show nemesisbot status")
	fmt.Println("  channel     Manage communication channels (list, enable, disable, status)")
	fmt.Println("  cluster     Manage bot cluster (status, config, enable, disable)")
	fmt.Println("  cors        Manage CORS configuration (list, add, remove, validate)")
	fmt.Println("  model       Manage LLM models (list, add, remove)")
	fmt.Println("  cron        Manage scheduled tasks")
	fmt.Println("  mcp         Manage MCP servers (list, add, remove, test)")
	fmt.Println("  security    Manage security settings (enable, disable, status, config, audit)")
	fmt.Println("  log         Manage LLM request logging")
	fmt.Println("  migrate     Migrate from OpenClaw to NemesisBot")
	fmt.Println("  skills      Manage skills (install, list, remove)")
	fmt.Println("  forge       Manage self-learning module (status, reflect, list, evaluate)")
	fmt.Println("  workflow    Manage DAG workflows (list, run, status, template)")
	fmt.Println("  version     Show version information")
	fmt.Println()
	fmt.Println("Quick Start:")
	fmt.Println("  nemesisbot onboard default          # 开箱即用（推荐新用户）")
	fmt.Println("  nemesisbot onboard default --local  # 开箱即用，配置在当前目录")
	fmt.Println("  nemesisbot onboard                  # 单步配置，逐步引导")
	fmt.Println("  nemesisbot onboard --local          # 单步配置，配置在当前目录")
	fmt.Println()
	fmt.Println("  nemesisbot model add --model zhipu/glm-4.7 --key YOUR_KEY --default")
	fmt.Println("  nemesisbot gateway                  # 启动服务")
	fmt.Println()
	fmt.Println("Scanner Setup (病毒扫描):")
	fmt.Println("  nemesisbot security scanner enable clamav    # 启用 ClamAV 引擎")
	fmt.Println("  nemesisbot security scanner check            # 检查安装状态")
	fmt.Println("  nemesisbot security scanner install          # 下载安装 + 病毒库")
	fmt.Println("  nemesisbot gateway                           # 启动后自动加载扫描引擎")
	fmt.Println()
	fmt.Println("Docs: https://github.com/276793422/NemesisBot")
}
