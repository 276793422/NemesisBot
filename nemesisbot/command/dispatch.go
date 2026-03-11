package command

import (
	"fmt"
	"os"
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
	case "desktop":
		CmdDesktop()
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
	case "model":
		CmdModel()
	case "skills":
		CmdSkills()
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
	fmt.Println("  desktop     Launch NemesisBot desktop UI")
	fmt.Println("  status      Show nemesisbot status")
	fmt.Println("  channel     Manage communication channels (list, enable, disable, status)")
	fmt.Println("  cluster     Manage bot cluster (status, config, enable, disable)")
	fmt.Println("  model       Manage LLM models (list, add, remove)")
	fmt.Println("  cron        Manage scheduled tasks")
	fmt.Println("  mcp         Manage MCP servers (list, add, remove, test)")
	fmt.Println("  security    Manage security settings (enable, disable, status, config, audit)")
	fmt.Println("  log         Manage LLM request logging")
	fmt.Println("  migrate     Migrate from OpenClaw to NemesisBot")
	fmt.Println("  skills      Manage skills (install, list, remove)")
	fmt.Println("  version     Show version information")
}
