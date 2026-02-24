package command

import (
	"fmt"
	"os"

	"github.com/276793422/NemesisBot/module/migrate"
)

// CmdMigrate migrates from OpenClaw to NemesisBot
func CmdMigrate() {
	if len(os.Args) > 2 && (os.Args[2] == "--help" || os.Args[2] == "-h") {
		MigrateHelp()
		return
	}

	opts := migrate.Options{}

	args := os.Args[2:]
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--dry-run":
			opts.DryRun = true
		case "--config-only":
			opts.ConfigOnly = true
		case "--workspace-only":
			opts.WorkspaceOnly = true
		case "--force":
			opts.Force = true
		case "--refresh":
			opts.Refresh = true
		case "--openclaw-home":
			if i+1 < len(args) {
				opts.OpenClawHome = args[i+1]
				i++
			}
		case "--nemesisbot-home":
			if i+1 < len(args) {
				opts.NemesisBotHome = args[i+1]
				i++
			}
		default:
			fmt.Printf("Unknown flag: %s\n", args[i])
			MigrateHelp()
			os.Exit(1)
		}
	}

	result, err := migrate.Run(opts)
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	if !opts.DryRun {
		migrate.PrintSummary(result)
	}
}

// MigrateHelp prints migrate command help
func MigrateHelp() {
	fmt.Println("\nMigrate from OpenClaw to NemesisBot")
	fmt.Println()
	fmt.Println("Usage: nemesisbot migrate [options]")
	fmt.Println()
	fmt.Println("Options:")
	fmt.Println("  --dry-run          Show what would be migrated without making changes")
	fmt.Println("  --refresh          Re-sync workspace files from OpenClaw (repeatable)")
	fmt.Println("  --config-only      Only migrate config, skip workspace files")
	fmt.Println("  --workspace-only   Only migrate workspace files, skip config")
	fmt.Println("  --force            Skip confirmation prompts")
	fmt.Println("  --openclaw-home    Override OpenClaw home directory (default: ~/.openclaw)")
	fmt.Println("  --nemesisbot-home  Override NemesisBot home directory (default: ~/.nemesisbot)")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot migrate              Detect and migrate from OpenClaw")
	fmt.Println("  nemesisbot migrate --dry-run    Show what would be migrated")
	fmt.Println("  nemesisbot migrate --refresh    Re-sync workspace files")
	fmt.Println("  nemesisbot migrate --force      Migrate without confirmation")
}
