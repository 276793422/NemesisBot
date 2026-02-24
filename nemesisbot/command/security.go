package command

import (
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"runtime"
	"strings"

	"github.com/276793422/NemesisBot/module/config"
)

// CmdSecurity manages security settings
func CmdSecurity() {
	if len(os.Args) < 3 {
		SecurityHelp()
		return
	}

	subcommand := os.Args[2]

	switch subcommand {
	case "enable":
		cmdSecurityEnable()
	case "disable":
		cmdSecurityDisable()
	case "status":
		cmdSecurityStatus()
	case "config":
		cmdSecurityConfig()
	case "edit":
		cmdSecurityEdit()
	case "audit":
		cmdSecurityAudit()
	case "approve":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot security approve <request-id>")
			return
		}
		cmdSecurityApprove(os.Args[3])
	case "deny":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot security deny <request-id> [reason]")
			return
		}
		reason := ""
		if len(os.Args) >= 5 {
			reason = strings.Join(os.Args[4:], " ")
		}
		cmdSecurityDeny(os.Args[3], reason)
	case "pending":
		cmdSecurityPending()
	case "rules":
		cmdSecurityRules()
	default:
		fmt.Printf("Unknown security command: %s\n", subcommand)
		SecurityHelp()
	}
}

// SecurityHelp prints security command help
func SecurityHelp() {
	fmt.Println("\nSecurity Module Commands:")
	fmt.Println("  enable              Enable security module")
	fmt.Println("  disable             Disable security module (use with caution!)")
	fmt.Println("  status              Show security status")
	fmt.Println("  config              Configure security settings")
	fmt.Println("  edit                Edit security configuration file")
	fmt.Println("  audit               View and manage audit logs")
	fmt.Println("  rules               Manage security rules")
	fmt.Println("  approve <id>        Approve a pending operation request")
	fmt.Println("  deny <id> [reason]  Deny a pending operation request")
	fmt.Println("  pending             List pending approval requests")
	fmt.Println()
	fmt.Println("NOTE: The 'ask' action in rules is currently treated as 'deny' for security.")
	fmt.Println("      Future versions will support interactive approval prompts.")
	fmt.Println()
	fmt.Println("Config commands:")
	fmt.Println("  nemesisbot security config show       Show current configuration")
	fmt.Println("  nemesisbot security config edit       Edit configuration in $EDITOR")
	fmt.Println("  nemesisbot security config reset      Reset to default configuration")
	fmt.Println()
	fmt.Println("Audit commands:")
	fmt.Println("  nemesisbot security audit             Show recent audit log")
	fmt.Println("  nemesisbot security audit export      Export audit log to file")
	fmt.Println("  nemesisbot security audit denied      Show denied operations only")
	fmt.Println()
	fmt.Println("Rules commands:")
	fmt.Println("  nemesisbot security rules list                    List all rules")
	fmt.Println("  nemesisbot security rules list <type>             List rules for a type")
	fmt.Println("  nemesisbot security rules add <type> <op>         Add a new rule")
	fmt.Println("                        --pattern <pattern> --action <action>")
	fmt.Println("  nemesisbot security rules remove <type> <op> <n>  Remove rule by index")
	fmt.Println("  nemesisbot security rules test <type> <op> <target> Test if target is allowed")
	fmt.Println()
	fmt.Println("Rule types: file, directory, process, network, hardware, registry")
	fmt.Println("Rule operations:")
	fmt.Println("  file:         read, write, delete")
	fmt.Println("  directory:    read, create, delete")
	fmt.Println("  process:      exec, spawn, kill, suspend")
	fmt.Println("  network:      request, download, upload")
	fmt.Println("  hardware:     i2c, spi, gpio")
	fmt.Println("  registry:     read, write, delete")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot security rules list file")
	fmt.Println("  nemesisbot security rules add file write --pattern \"/workspace/**\" --action allow")
	fmt.Println("  nemesisbot security rules add file write --pattern \"*.key\" --action deny")
	fmt.Println("  nemesisbot security rules remove file write 0")
	fmt.Println("  nemesisbot security rules test file write \"/workspace/test.txt\"")
	fmt.Println()
	fmt.Println("Configuration file:")
	fmt.Println("  ~/.nemesisbot/config.security.json")
}

func cmdSecurityEnable() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	if cfg.Security == nil {
		cfg.Security = &config.SecurityFlagConfig{}
	}
	cfg.Security.Enabled = true

	// When security is enabled, disable restrict_to_workspace to allow file operations outside workspace
	// Security module will enforce access through rules instead
	cfg.Agents.Defaults.RestrictToWorkspace = false

	configPath := GetConfigPath()
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	// Ensure security config file exists
	securityConfigPath := GetSecurityConfigPath()
	if _, err := os.Stat(securityConfigPath); os.IsNotExist(err) {
		securityCfg, err := config.LoadSecurityConfig(securityConfigPath)
		if err != nil {
			fmt.Printf("Warning: Could not create security config: %v\n", err)
		} else {
			if err := config.SaveSecurityConfig(securityConfigPath, securityCfg); err != nil {
				fmt.Printf("Warning: Could not save security config: %v\n", err)
			}
		}
	}

	fmt.Println("Security module enabled")
	fmt.Printf("Configuration: %s\n", securityConfigPath)
	fmt.Println("Workspace restriction: disabled (security module enforces rules instead)")
	fmt.Println("\nRestart agent/gateway to apply changes")
}

func cmdSecurityDisable() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	if cfg.Security == nil {
		cfg.Security = &config.SecurityFlagConfig{}
	}
	cfg.Security.Enabled = false

	// When security is disabled, restore restrict_to_workspace to true for safety
	cfg.Agents.Defaults.RestrictToWorkspace = true

	configPath := GetConfigPath()
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("Security module disabled")
	fmt.Println("Workspace restriction: enabled (all operations restricted to workspace)")
	fmt.Println("\nRestart agent/gateway to apply changes")
}

func cmdSecurityStatus() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	securityConfigPath := GetSecurityConfigPath()
	securityCfg, err := config.LoadSecurityConfig(securityConfigPath)
	if err != nil {
		fmt.Printf("Error loading security config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("Security Status:")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")

	enabled := false
	if cfg.Security != nil {
		enabled = cfg.Security.Enabled
	}
	statusSymbol := "❌"
	if enabled {
		statusSymbol = "✅"
	}
	fmt.Printf("Status:      %s %s\n", statusSymbol, map[bool]string{true: "Enabled", false: "Disabled"}[enabled])

	fmt.Println()
	fmt.Println("Configuration:")
	fmt.Printf("  Main Config:      %s\n", GetConfigPath())
	fmt.Printf("  Security Config:  %s\n", securityConfigPath)

	if enabled {
		fmt.Println()
		fmt.Println("Policy Settings:")
		fmt.Printf("  Default Action:    %s\n", strings.ToUpper(securityCfg.DefaultAction))
		fmt.Printf("  Log Operations:    %s\n", map[bool]string{true: "Yes", false: "No"}[securityCfg.LogAllOperations])
		fmt.Printf("  File Log Enabled:  %s\n", map[bool]string{true: "Yes", false: "No"}[securityCfg.AuditLogFileEnabled])
		fmt.Printf("  Approval Timeout:  %d seconds\n", securityCfg.ApprovalTimeout)
		fmt.Printf("  Audit Retention:   %d days\n", securityCfg.AuditLogRetentionDays)

		fmt.Println()
		fmt.Println("Configured Rules:")

		// Count rules for each type
		if securityCfg.FileRules != nil {
			fmt.Printf("  File Rules:         read=%d, write=%d, delete=%d\n",
				len(securityCfg.FileRules.Read), len(securityCfg.FileRules.Write),
				len(securityCfg.FileRules.Delete))
		}
		if securityCfg.DirectoryRules != nil {
			fmt.Printf("  Directory Rules:   read=%d, create=%d, delete=%d\n",
				len(securityCfg.DirectoryRules.Read), len(securityCfg.DirectoryRules.Create),
				len(securityCfg.DirectoryRules.Delete))
		}
		if securityCfg.ProcessRules != nil {
			fmt.Printf("  Process Rules:      exec=%d, spawn=%d, kill=%d, suspend=%d\n",
				len(securityCfg.ProcessRules.Exec), len(securityCfg.ProcessRules.Spawn),
				len(securityCfg.ProcessRules.Kill), len(securityCfg.ProcessRules.Suspend))
		}
		if securityCfg.NetworkRules != nil {
			fmt.Printf("  Network Rules:      request=%d, download=%d, upload=%d\n",
				len(securityCfg.NetworkRules.Request), len(securityCfg.NetworkRules.Download),
				len(securityCfg.NetworkRules.Upload))
		}
		if securityCfg.HardwareRules != nil {
			fmt.Printf("  Hardware Rules:     i2c=%d, spi=%d, gpio=%d\n",
				len(securityCfg.HardwareRules.I2C), len(securityCfg.HardwareRules.SPI),
				len(securityCfg.HardwareRules.GPIO))
		}
		if securityCfg.RegistryRules != nil {
			fmt.Printf("  Registry Rules:     read=%d, write=%d, delete=%d\n",
				len(securityCfg.RegistryRules.Read), len(securityCfg.RegistryRules.Write),
				len(securityCfg.RegistryRules.Delete))
		}
	}
}

func cmdSecurityConfig() {
	if len(os.Args) < 4 {
		SecurityConfigHelp()
		return
	}

	action := os.Args[3]

	switch action {
	case "show":
		cmdSecurityConfigShow()
	case "edit":
		cmdSecurityEdit()
	case "reset":
		cmdSecurityConfigReset()
	default:
		fmt.Printf("Unknown config command: %s\n", action)
		SecurityConfigHelp()
	}
}

func SecurityConfigHelp() {
	fmt.Println("\nSecurity Configuration Commands:")
	fmt.Println("  show       Show current security configuration")
	fmt.Println("  edit       Edit configuration in $EDITOR")
	fmt.Println("  reset      Reset to default configuration")
}

func cmdSecurityConfigShow() {
	securityConfigPath := GetSecurityConfigPath()
	securityCfg, err := config.LoadSecurityConfig(securityConfigPath)
	if err != nil {
		fmt.Printf("Error loading security config: %v\n", err)
		os.Exit(1)
	}

	data, err := json.MarshalIndent(securityCfg, "", "  ")
	if err != nil {
		fmt.Printf("Error formatting config: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("\n%s\n", string(data))
}

func cmdSecurityEdit() {
	securityConfigPath := GetSecurityConfigPath()

	editor := os.Getenv("EDITOR")
	if editor == "" {
		editor = os.Getenv("VISUAL")
	}
	if editor == "" {
		if runtime.GOOS == "windows" {
			editor = "notepad"
		} else {
			editor = "vi"
		}
	}

	if _, err := os.Stat(securityConfigPath); os.IsNotExist(err) {
		securityCfg, _ := config.LoadSecurityConfig(securityConfigPath)
		config.SaveSecurityConfig(securityConfigPath, securityCfg)
	}

	cmd := exec.Command(editor, securityConfigPath)
	cmd.Stdin = os.Stdin
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	if err := cmd.Run(); err != nil {
		fmt.Printf("Error opening editor: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("Configuration saved")
	fmt.Println("Restart agent/gateway to apply changes")
}

func cmdSecurityConfigReset() {
	fmt.Print("This will reset security configuration to defaults. Continue? (y/n): ")
	var response string
	fmt.Scanln(&response)
	if response != "y" {
		fmt.Println("Aborted.")
		return
	}

	securityConfigPath := GetSecurityConfigPath()
	securityCfg, err := config.LoadSecurityConfig(securityConfigPath)
	if err != nil {
		fmt.Printf("Error creating default config: %v\n", err)
		os.Exit(1)
	}

	if err := config.SaveSecurityConfig(securityConfigPath, securityCfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("Security configuration reset to defaults")
}

func cmdSecurityAudit() {
	if len(os.Args) < 4 {
		cmdSecurityAuditShow()
		return
	}

	action := os.Args[3]

	switch action {
	case "export":
		if len(os.Args) < 5 {
			fmt.Println("Usage: nemesisbot security audit export <output-file>")
			return
		}
		cmdSecurityAuditExport(os.Args[4])
	case "denied":
		cmdSecurityAuditDenied()
	default:
		fmt.Printf("Unknown audit command: %s\n", action)
		fmt.Println("Valid commands: show, export, denied")
	}
}

func cmdSecurityAuditShow() {
	fmt.Println("Audit Log Viewer")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println()
	fmt.Println("To view audit logs, the agent/gateway must be running with security enabled.")
	fmt.Println()
	fmt.Println("Options:")
	fmt.Println("  nemesisbot security audit export <file>   Export audit log to file")
	fmt.Println("  nemesisbot security audit denied          Show denied operations")
}

func cmdSecurityAuditExport(outputFile string) {
	fmt.Printf("Exporting audit log to: %s\n", outputFile)
	fmt.Println("This requires the agent/gateway to be running with security enabled.")
	fmt.Println("Use the agent/gateway API or check logs for audit information.")
}

func cmdSecurityAuditDenied() {
	fmt.Println("Denied Operations Log")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println()
	fmt.Println("This requires the agent/gateway to be running with security enabled.")
	fmt.Println("Use 'nemesisbot security audit export <file>' to export the full log.")
}

func cmdSecurityApprove(requestID string) {
	fmt.Printf("Approving request: %s\n", requestID)
	fmt.Println()
	fmt.Println("⚠️  Approval feature is not yet implemented.")
	fmt.Println("   Currently, rules with 'ask' action are treated as 'deny'.")
	fmt.Println("   Future versions will support interactive approval workflow.")
	fmt.Println()
	fmt.Println("To allow the operation, either:")
	fmt.Println("  1. Change the rule action from 'ask' to 'allow' in config.security.json")
	fmt.Println("  2. Wait for the approval UI to be implemented in a future version")
}

func cmdSecurityDeny(requestID, reason string) {
	fmt.Printf("Denying request: %s\n", requestID)
	if reason != "" {
		fmt.Printf("Reason: %s\n", reason)
	}
	fmt.Println()
	fmt.Println("ℹ️  The operation was already blocked (ask rules are treated as deny).")
	fmt.Println("   This command will be functional when the approval workflow is implemented.")
}

func cmdSecurityPending() {
	fmt.Println("Pending Approval Requests")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println()
	fmt.Println("⚠️  Approval tracking is not yet implemented.")
	fmt.Println("   Rules with 'ask' action are currently treated as 'deny'.")
	fmt.Println("   Future versions will track and display pending approval requests.")
	fmt.Println()
	fmt.Println("Note: Operations blocked by 'ask' rules will appear as 'denied' in audit logs.")
}

func cmdSecurityRules() {
	if len(os.Args) < 4 {
		SecurityRulesHelp()
		return
	}

	action := os.Args[3]

	switch action {
	case "list":
		cmdSecurityRulesList()
	case "add":
		cmdSecurityRulesAdd()
	case "remove":
		cmdSecurityRulesRemove()
	case "test":
		cmdSecurityRulesTest()
	default:
		fmt.Printf("Unknown rules command: %s\n", action)
		SecurityRulesHelp()
	}
}

func SecurityRulesHelp() {
	fmt.Println("\nSecurity Rules Commands:")
	fmt.Println("  list                                    List all rules")
	fmt.Println("  list <type>                             List rules for a specific type")
	fmt.Println("  add <type> <op> <args>                    Add a new rule")
	fmt.Println("  remove <type> <op> <index>               Remove rule by index")
	fmt.Println("  test <type> <op> <target>                Test if target is allowed")
	fmt.Println()
	fmt.Println("Rule types: file, directory, process, network, hardware, registry")
	fmt.Println()
	fmt.Println("Operations by type:")
	fmt.Println("  file:         read, write, delete")
	fmt.Println("  directory:    read, create, delete")
	fmt.Println("  process:      exec, spawn, kill, suspend")
	fmt.Println("  network:      request, download, upload")
	fmt.Println("  hardware:     i2c, spi, gpio")
	fmt.Println("  registry:     read, write, delete")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  # List all file rules")
	fmt.Println("  nemesisbot security rules list file")
	fmt.Println()
	fmt.Println("  # Add a file write rule")
	fmt.Println("  nemesisbot security rules add file write --pattern \"/workspace/**\" --action allow")
	fmt.Println()
	fmt.Println("  # Add a deny rule (put specific rules first!)")
	fmt.Println("  nemesisbot security rules add file write --pattern \"*.key\" --action deny")
	fmt.Println()
	fmt.Println("  # Remove rule at index 0")
	fmt.Println("  nemesisbot security rules remove file write 0")
	fmt.Println()
	fmt.Println("  # Test if a path is allowed")
	fmt.Println("  nemesisbot security rules test file write \"/workspace/test.txt\"")
}

func cmdSecurityRulesList() {
	securityConfigPath := GetSecurityConfigPath()
	securityCfg, err := config.LoadSecurityConfig(securityConfigPath)
	if err != nil {
		fmt.Printf("Error loading security config: %v\n", err)
		os.Exit(1)
	}

	// If no type specified, list all
	if len(os.Args) < 5 {
		securityRulesListAll(securityCfg)
		return
	}

	ruleType := os.Args[4]
	securityRulesListByType(securityCfg, ruleType)
}

func securityRulesListAll(cfg *config.SecurityConfig) {
	fmt.Println("\nSecurity Rules")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println()

	if cfg.FileRules != nil {
		fmt.Println("File Rules:")
		printRuleCategory("read", cfg.FileRules.Read)
		printRuleCategory("write", cfg.FileRules.Write)
		printRuleCategory("delete", cfg.FileRules.Delete)
		fmt.Println()
	}

	if cfg.DirectoryRules != nil {
		fmt.Println("Directory Rules:")
		printRuleCategory("read", cfg.DirectoryRules.Read)
		printRuleCategory("create", cfg.DirectoryRules.Create)
		printRuleCategory("delete", cfg.DirectoryRules.Delete)
		fmt.Println()
	}

	if cfg.ProcessRules != nil {
		fmt.Println("Process Rules:")
		printRuleCategory("exec", cfg.ProcessRules.Exec)
		printRuleCategory("spawn", cfg.ProcessRules.Spawn)
		printRuleCategory("kill", cfg.ProcessRules.Kill)
		printRuleCategory("suspend", cfg.ProcessRules.Suspend)
		fmt.Println()
	}

	if cfg.NetworkRules != nil {
		fmt.Println("Network Rules:")
		printRuleCategory("request", cfg.NetworkRules.Request)
		printRuleCategory("download", cfg.NetworkRules.Download)
		printRuleCategory("upload", cfg.NetworkRules.Upload)
		fmt.Println()
	}

	if cfg.HardwareRules != nil {
		fmt.Println("Hardware Rules:")
		printRuleCategory("i2c", cfg.HardwareRules.I2C)
		printRuleCategory("spi", cfg.HardwareRules.SPI)
		printRuleCategory("gpio", cfg.HardwareRules.GPIO)
		fmt.Println()
	}

	if cfg.RegistryRules != nil {
		fmt.Println("Registry Rules:")
		printRuleCategory("read", cfg.RegistryRules.Read)
		printRuleCategory("write", cfg.RegistryRules.Write)
		printRuleCategory("delete", cfg.RegistryRules.Delete)
		fmt.Println()
	}
}

func printRuleCategory(operation string, rules []config.SecurityRule) {
	if len(rules) == 0 {
		fmt.Printf("  %-10s: (none)\n", operation)
		return
	}
	fmt.Printf("  %-10s:\n", operation)
	for i, rule := range rules {
		fmt.Printf("    [%d] pattern: %-30s action: %s\n", i, rule.Pattern, rule.Action)
	}
}

func securityRulesListByType(cfg *config.SecurityConfig, ruleType string) {
	fmt.Printf("\n%s Rules\n", strings.Title(ruleType))
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println()

	switch ruleType {
	case "file":
		if cfg.FileRules == nil {
			fmt.Println("No file rules configured")
			return
		}
		printRuleCategory("read", cfg.FileRules.Read)
		printRuleCategory("write", cfg.FileRules.Write)
		printRuleCategory("delete", cfg.FileRules.Delete)

	case "directory":
		if cfg.DirectoryRules == nil {
			fmt.Println("No directory rules configured")
			return
		}
		printRuleCategory("read", cfg.DirectoryRules.Read)
		printRuleCategory("create", cfg.DirectoryRules.Create)
		printRuleCategory("delete", cfg.DirectoryRules.Delete)

	case "process":
		if cfg.ProcessRules == nil {
			fmt.Println("No process rules configured")
			return
		}
		printRuleCategory("exec", cfg.ProcessRules.Exec)
		printRuleCategory("spawn", cfg.ProcessRules.Spawn)
		printRuleCategory("kill", cfg.ProcessRules.Kill)
		printRuleCategory("suspend", cfg.ProcessRules.Suspend)

	case "network":
		if cfg.NetworkRules == nil {
			fmt.Println("No network rules configured")
			return
		}
		printRuleCategory("request", cfg.NetworkRules.Request)
		printRuleCategory("download", cfg.NetworkRules.Download)
		printRuleCategory("upload", cfg.NetworkRules.Upload)

	case "hardware":
		if cfg.HardwareRules == nil {
			fmt.Println("No hardware rules configured")
			return
		}
		printRuleCategory("i2c", cfg.HardwareRules.I2C)
		printRuleCategory("spi", cfg.HardwareRules.SPI)
		printRuleCategory("gpio", cfg.HardwareRules.GPIO)

	case "registry":
		if cfg.RegistryRules == nil {
			fmt.Println("No registry rules configured")
			return
		}
		printRuleCategory("read", cfg.RegistryRules.Read)
		printRuleCategory("write", cfg.RegistryRules.Write)
		printRuleCategory("delete", cfg.RegistryRules.Delete)

	default:
		fmt.Printf("Unknown rule type: %s\n", ruleType)
		fmt.Println("Valid types: file, directory, process, network, hardware, registry")
	}
}

func cmdSecurityRulesAdd() {
	if len(os.Args) < 8 {
		fmt.Println("Usage: nemesisbot security rules add <type> <operation> --pattern <pattern> --action <action>")
		fmt.Println()
		fmt.Println("Examples:")
		fmt.Println("  nemesisbot security rules add file write --pattern \"/workspace/**\" --action allow")
		fmt.Println("  nemesisbot security rules add directory read --pattern \"/workspace/**\" --action allow")
		return
	}

	ruleType := os.Args[4]
	operation := os.Args[5]

	// Parse flags
	var pattern, action string
	for i := 6; i < len(os.Args); i++ {
		switch os.Args[i] {
		case "--pattern":
			if i+1 < len(os.Args) {
				pattern = os.Args[i+1]
				i++
			}
		case "--action":
			if i+1 < len(os.Args) {
				action = os.Args[i+1]
				i++
			}
		}
	}

	if pattern == "" {
		fmt.Println("Error: --pattern is required")
		return
	}
	if action == "" {
		fmt.Println("Error: --action is required")
		return
	}

	// Validate action
	if action != "allow" && action != "deny" && action != "ask" {
		fmt.Printf("Error: invalid action '%s'. Must be allow, deny, or ask\n", action)
		return
	}

	// Load config
	securityConfigPath := GetSecurityConfigPath()
	securityCfg, err := config.LoadSecurityConfig(securityConfigPath)
	if err != nil {
		fmt.Printf("Error loading security config: %v\n", err)
		os.Exit(1)
	}

	// Add rule
	newRule := config.SecurityRule{Pattern: pattern, Action: action}
	success := addRuleToConfig(securityCfg, ruleType, operation, newRule)

	if !success {
		return
	}

	// Save config
	if err := config.SaveSecurityConfig(securityConfigPath, securityCfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("✓ Rule added: %s %s --pattern %s --action %s\n", ruleType, operation, pattern, action)
	fmt.Println("Restart agent/gateway to apply changes")
}

func cmdSecurityRulesRemove() {
	if len(os.Args) < 7 {
		fmt.Println("Usage: nemesisbot security rules remove <type> <operation> <index>")
		fmt.Println()
		fmt.Println("Examples:")
		fmt.Println("  nemesisbot security rules remove file write 0")
		fmt.Println("  nemesisbot security rules remove directory create 1")
		return
	}

	ruleType := os.Args[4]
	operation := os.Args[5]
	indexStr := os.Args[6]

	var index int
	_, err := fmt.Sscanf(indexStr, "%d", &index)
	if err != nil {
		fmt.Printf("Error: invalid index '%s'. Must be a number\n", indexStr)
		return
	}

	// Load config
	securityConfigPath := GetSecurityConfigPath()
	securityCfg, err := config.LoadSecurityConfig(securityConfigPath)
	if err != nil {
		fmt.Printf("Error loading security config: %v\n", err)
		os.Exit(1)
	}

	// Remove rule
	success := removeRuleFromConfig(securityCfg, ruleType, operation, index)

	if !success {
		return
	}

	// Save config
	if err := config.SaveSecurityConfig(securityConfigPath, securityCfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("✓ Rule removed: %s %s [%d]\n", ruleType, operation, index)
	fmt.Println("Restart agent/gateway to apply changes")
}

func cmdSecurityRulesTest() {
	if len(os.Args) < 7 {
		fmt.Println("Usage: nemesisbot security rules test <type> <operation> <target>")
		fmt.Println()
		fmt.Println("Examples:")
		fmt.Println("  nemesisbot security rules test file write \"/workspace/test.txt\"")
		fmt.Println("  nemesisbot security rules test directory read \"/workspace/src\"")
		return
	}

	ruleType := os.Args[4]
	operation := os.Args[5]
	target := os.Args[6]

	// Map operation to OperationType
	var opType string
	switch ruleType {
	case "file":
		switch operation {
		case "read":
			opType = "file_read"
		case "write":
			opType = "file_write"
		case "delete":
			opType = "file_delete"
		default:
			fmt.Printf("Error: invalid file operation '%s'\n", operation)
			return
		}
	case "directory":
		switch operation {
		case "read":
			opType = "dir_read"
		case "create":
			opType = "dir_create"
		case "delete":
			opType = "dir_delete"
		default:
			fmt.Printf("Error: invalid directory operation '%s'\n", operation)
			return
		}
	default:
		fmt.Printf("Error: invalid rule type '%s'\n", ruleType)
		return
	}

	// Load config
	securityConfigPath := GetSecurityConfigPath()
	securityCfg, err := config.LoadSecurityConfig(securityConfigPath)
	if err != nil {
		fmt.Printf("Error loading security config: %v\n", err)
		os.Exit(1)
	}

	// Get danger level
	dangerLevel := getDangerLevelForOperation(opType)

	// Check if target matches any rule
	allowed, reason := checkRules(securityCfg, opType, target, dangerLevel)

	fmt.Println("\nRule Test Result")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Printf("Type:       %s\n", ruleType)
	fmt.Printf("Operation:  %s\n", operation)
	fmt.Printf("Target:     %s\n", target)
	fmt.Printf("Result:     ")
	if allowed {
		fmt.Println("✅ ALLOWED")
	} else {
		fmt.Println("❌ DENIED")
	}
	fmt.Printf("Reason:     %s\n", reason)
}

func getDangerLevelForOperation(opType string) string {
	switch opType {
	case "file_read", "dir_read":
		return "LOW"
	case "file_write", "file_delete", "dir_create", "dir_delete":
		return "HIGH"
	case "process_exec":
		return "CRITICAL"
	case "network_request", "network_download", "network_upload":
		return "MEDIUM"
	default:
		return "MEDIUM"
	}
}

func checkRules(cfg *config.SecurityConfig, opType, target, dangerLevel string) (bool, string) {
	// Convert operation type to rule category
	var ruleCategory interface{}

	switch opType {
	case "file_read":
		if cfg.FileRules == nil || len(cfg.FileRules.Read) == 0 {
			return false, "No file read rules configured"
		}
		ruleCategory = cfg.FileRules.Read
	case "file_write":
		if cfg.FileRules == nil || len(cfg.FileRules.Write) == 0 {
			return false, "No file write rules configured"
		}
		ruleCategory = cfg.FileRules.Write
	case "file_delete":
		if cfg.FileRules == nil || len(cfg.FileRules.Delete) == 0 {
			return false, "No file delete rules configured"
		}
		ruleCategory = cfg.FileRules.Delete
	case "dir_read":
		if cfg.DirectoryRules == nil || len(cfg.DirectoryRules.Read) == 0 {
			return false, "No directory read rules configured"
		}
		ruleCategory = cfg.DirectoryRules.Read
	case "dir_create":
		if cfg.DirectoryRules == nil || len(cfg.DirectoryRules.Create) == 0 {
			return false, "No directory create rules configured"
		}
		ruleCategory = cfg.DirectoryRules.Create
	case "dir_delete":
		if cfg.DirectoryRules == nil || len(cfg.DirectoryRules.Delete) == 0 {
			return false, "No directory delete rules configured"
		}
		ruleCategory = cfg.DirectoryRules.Delete
	default:
		return false, "Unknown operation type"
	}

	rules, ok := ruleCategory.([]config.SecurityRule)
	if !ok {
		return false, "Invalid rule format"
	}

	// Check rules in order (first match wins)
	for i, rule := range rules {
		matched := matchPattern(rule.Pattern, target)
		if matched {
			action := rule.Action
			if action == "allow" {
				return true, fmt.Sprintf("Matched rule [%d]: %s → %s", i, rule.Pattern, action)
			} else if action == "deny" {
				return false, fmt.Sprintf("Matched rule [%d]: %s → %s", i, rule.Pattern, action)
			} else {
				return false, fmt.Sprintf("Matched rule [%d]: %s → %s (requires approval)", i, rule.Pattern, action)
			}
		}
	}

	return false, "No matching rule found"
}

func matchPattern(pattern, target string) bool {
	// Simple wildcard matching
	// ** matches any number of directories (at least one if between prefix and suffix)
	// * matches any sequence of characters (not including / or \)

	patternParts := strings.Split(pattern, "**")
	if len(patternParts) > 1 {
		// Has ** wildcard
		prefix := patternParts[0]
		suffix := patternParts[1]

		// Check prefix
		if prefix != "" && !strings.HasPrefix(target, prefix) {
			return false
		}

		// For patterns like "/workspace/**", suffix might be empty
		if suffix == "" {
			return true
		}

		// Handle suffix that may contain * wildcard (e.g., "/*.log")
		if strings.HasPrefix(suffix, "/") || strings.HasPrefix(suffix, "\\") {
			// Suffix starts with a separator - ** should have matched directories, now check filename
			// Extract the filename from target (last component)
			lastSlash := strings.LastIndexAny(target, "/\\")
			if lastSlash == -1 {
				return false
			}
			filename := target[lastSlash+1:]

			// Suffix is like "/*.log" - remove leading / and match filename with pattern
			suffixPattern := suffix[1:] // Remove leading /
			if strings.Contains(suffixPattern, "*") {
				// Handle * in suffix pattern
				parts := strings.Split(suffixPattern, "*")
				if len(parts) == 2 {
					// Pattern is like "*.log" - check if filename starts and ends with parts
					if parts[0] != "" && !strings.HasPrefix(filename, parts[0]) {
						return false
					}
					if parts[1] != "" && !strings.HasSuffix(filename, parts[1]) {
						return false
					}
					// Middle part should not contain separators (already checked since we're using filename)
				}
			} else {
				// No wildcard in suffix pattern
				if filename != suffixPattern {
					return false
				}
			}

			// Check that there's at least one directory between prefix and filename
			// Get the part of the path between the prefix and the filename
			afterPrefix := strings.TrimPrefix(target, prefix)
			// Remove filename from afterPrefix to get just the directory path
			dirPath := strings.TrimSuffix(afterPrefix, filename)
			// Check if dirPath contains at least one separator (meaning there's at least one directory level)
			if !strings.Contains(dirPath, "/") && !strings.Contains(dirPath, "\\") {
				return false
			}
		} else {
			// Suffix doesn't start with separator - treat as literal suffix
			if !strings.HasSuffix(target, suffix) {
				return false
			}

			// For ** in the middle, verify there's at least one directory level between
			middle := strings.TrimPrefix(target, prefix)
			middle = strings.TrimSuffix(middle, suffix)
			// Must contain at least one path separator to represent a directory level
			if !strings.Contains(middle, "/") && !strings.Contains(middle, "\\") {
				return false
			}
		}

		return true
	}

	// No ** wildcard, check for * wildcard
	if strings.Contains(pattern, "*") {
		// Simple * wildcard - should not match path separators
		parts := strings.Split(pattern, "*")
		prefix := parts[0]
		suffix := ""
		if len(parts) > 1 {
			suffix = parts[1]
		}

		// Check if target has the prefix
		if !strings.HasPrefix(target, prefix) {
			return false
		}
		if suffix != "" && !strings.HasSuffix(target, suffix) {
			return false
		}

		// Extract the middle part between prefix and suffix
		middle := strings.TrimPrefix(target, prefix)
		if suffix != "" {
			middle = strings.TrimSuffix(middle, suffix)
		}

		// The middle part should not contain path separators
		if strings.Contains(middle, "/") || strings.Contains(middle, "\\") {
			return false
		}

		return true
	}

	// No wildcards, exact match
	return target == pattern
}

func addRuleToConfig(cfg *config.SecurityConfig, ruleType, operation string, rule config.SecurityRule) bool {
	switch ruleType {
	case "file":
		if cfg.FileRules == nil {
			cfg.FileRules = &config.FileSecurityRules{}
		}
		switch operation {
		case "read":
			cfg.FileRules.Read = append(cfg.FileRules.Read, rule)
		case "write":
			cfg.FileRules.Write = append(cfg.FileRules.Write, rule)
		case "delete":
			cfg.FileRules.Delete = append(cfg.FileRules.Delete, rule)
		default:
			fmt.Printf("Error: invalid file operation '%s'. Valid: read, write, delete\n", operation)
			return false
		}

	case "directory":
		if cfg.DirectoryRules == nil {
			cfg.DirectoryRules = &config.DirectorySecurityRules{}
		}
		switch operation {
		case "read":
			cfg.DirectoryRules.Read = append(cfg.DirectoryRules.Read, rule)
		case "create":
			cfg.DirectoryRules.Create = append(cfg.DirectoryRules.Create, rule)
		case "delete":
			cfg.DirectoryRules.Delete = append(cfg.DirectoryRules.Delete, rule)
		default:
			fmt.Printf("Error: invalid directory operation '%s'. Valid: read, create, delete\n", operation)
			return false
		}

	case "process":
		if cfg.ProcessRules == nil {
			cfg.ProcessRules = &config.ProcessSecurityRules{}
		}
		switch operation {
		case "exec":
			cfg.ProcessRules.Exec = append(cfg.ProcessRules.Exec, rule)
		case "spawn":
			cfg.ProcessRules.Spawn = append(cfg.ProcessRules.Spawn, rule)
		case "kill":
			cfg.ProcessRules.Kill = append(cfg.ProcessRules.Kill, rule)
		case "suspend":
			cfg.ProcessRules.Suspend = append(cfg.ProcessRules.Suspend, rule)
		default:
			fmt.Printf("Error: invalid process operation '%s'. Valid: exec, spawn, kill, suspend\n", operation)
			return false
		}

	case "network":
		if cfg.NetworkRules == nil {
			cfg.NetworkRules = &config.NetworkSecurityRules{}
		}
		switch operation {
		case "request":
			cfg.NetworkRules.Request = append(cfg.NetworkRules.Request, rule)
		case "download":
			cfg.NetworkRules.Download = append(cfg.NetworkRules.Download, rule)
		case "upload":
			cfg.NetworkRules.Upload = append(cfg.NetworkRules.Upload, rule)
		default:
			fmt.Printf("Error: invalid network operation '%s'. Valid: request, download, upload\n", operation)
			return false
		}

	case "hardware":
		if cfg.HardwareRules == nil {
			cfg.HardwareRules = &config.HardwareSecurityRules{}
		}
		switch operation {
		case "i2c":
			cfg.HardwareRules.I2C = append(cfg.HardwareRules.I2C, rule)
		case "spi":
			cfg.HardwareRules.SPI = append(cfg.HardwareRules.SPI, rule)
		case "gpio":
			cfg.HardwareRules.GPIO = append(cfg.HardwareRules.GPIO, rule)
		default:
			fmt.Printf("Error: invalid hardware operation '%s'. Valid: i2c, spi, gpio\n", operation)
			return false
		}

	case "registry":
		if cfg.RegistryRules == nil {
			cfg.RegistryRules = &config.RegistrySecurityRules{}
		}
		switch operation {
		case "read":
			cfg.RegistryRules.Read = append(cfg.RegistryRules.Read, rule)
		case "write":
			cfg.RegistryRules.Write = append(cfg.RegistryRules.Write, rule)
		case "delete":
			cfg.RegistryRules.Delete = append(cfg.RegistryRules.Delete, rule)
		default:
			fmt.Printf("Error: invalid registry operation '%s'. Valid: read, write, delete\n", operation)
			return false
		}

	default:
		fmt.Printf("Error: invalid rule type '%s'. Valid: file, directory, process, network, hardware, registry\n", ruleType)
		return false
	}

	return true
}

func removeRuleFromConfig(cfg *config.SecurityConfig, ruleType, operation string, index int) bool {
	switch ruleType {
	case "file":
		if cfg.FileRules == nil {
			fmt.Println("Error: no file rules configured")
			return false
		}
		switch operation {
		case "read":
			if index < 0 || index >= len(cfg.FileRules.Read) {
				fmt.Printf("Error: index %d out of range (0-%d)\n", index, len(cfg.FileRules.Read)-1)
				return false
			}
			cfg.FileRules.Read = append(cfg.FileRules.Read[:index], cfg.FileRules.Read[index+1:]...)
		case "write":
			if index < 0 || index >= len(cfg.FileRules.Write) {
				fmt.Printf("Error: index %d out of range (0-%d)\n", index, len(cfg.FileRules.Write)-1)
				return false
			}
			cfg.FileRules.Write = append(cfg.FileRules.Write[:index], cfg.FileRules.Write[index+1:]...)
		case "delete":
			if index < 0 || index >= len(cfg.FileRules.Delete) {
				fmt.Printf("Error: index %d out of range (0-%d)\n", index, len(cfg.FileRules.Delete)-1)
				return false
			}
			cfg.FileRules.Delete = append(cfg.FileRules.Delete[:index], cfg.FileRules.Delete[index+1:]...)
		default:
			fmt.Printf("Error: invalid file operation '%s'\n", operation)
			return false
		}

	case "directory":
		if cfg.DirectoryRules == nil {
			fmt.Println("Error: no directory rules configured")
			return false
		}
		switch operation {
		case "read":
			if index < 0 || index >= len(cfg.DirectoryRules.Read) {
				fmt.Printf("Error: index %d out of range (0-%d)\n", index, len(cfg.DirectoryRules.Read)-1)
				return false
			}
			cfg.DirectoryRules.Read = append(cfg.DirectoryRules.Read[:index], cfg.DirectoryRules.Read[index+1:]...)
		case "create":
			if index < 0 || index >= len(cfg.DirectoryRules.Create) {
				fmt.Printf("Error: index %d out of range (0-%d)\n", index, len(cfg.DirectoryRules.Create)-1)
				return false
			}
			cfg.DirectoryRules.Create = append(cfg.DirectoryRules.Create[:index], cfg.DirectoryRules.Create[index+1:]...)
		case "delete":
			if index < 0 || index >= len(cfg.DirectoryRules.Delete) {
				fmt.Printf("Error: index %d out of range (0-%d)\n", index, len(cfg.DirectoryRules.Delete)-1)
				return false
			}
			cfg.DirectoryRules.Delete = append(cfg.DirectoryRules.Delete[:index], cfg.DirectoryRules.Delete[index+1:]...)
		default:
			fmt.Printf("Error: invalid directory operation '%s'\n", operation)
			return false
		}

	default:
		fmt.Printf("Error: invalid rule type '%s'\n", ruleType)
		return false
	}

	return true
}
