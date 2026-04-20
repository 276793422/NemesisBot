package command

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/forge"
)

// CmdForge manages the Forge self-learning module.
func CmdForge() {
	if len(os.Args) < 3 {
		ForgeHelp()
		return
	}

	subcommand := os.Args[2]

	switch subcommand {
	case "status":
		cmdForgeStatus()
	case "reflect":
		cmdForgeReflect()
	case "list":
		cmdForgeList()
	case "enable":
		cmdForgeEnable()
	case "disable":
		cmdForgeDisable()
	case "export":
		cmdForgeExport()
	case "learning":
		cmdForgeLearning()
	default:
		fmt.Printf("Unknown forge command: %s\n", subcommand)
		ForgeHelp()
	}
}

// ForgeHelp prints forge command help.
func ForgeHelp() {
	fmt.Println("\nForge commands (self-learning module):")
	fmt.Println("  status                 Show Forge status and statistics")
	fmt.Println("  reflect                Trigger immediate reflection analysis")
	fmt.Println("  list                   List all Forge artifacts")
	fmt.Println("  enable                 Enable Forge module")
	fmt.Println("  disable                Disable Forge module")
	fmt.Println("  export [artifact-id]   Export artifact(s) to workspace/forge/exports/")
	fmt.Println("  learning               Show closed-loop learning status (Phase 6)")
	fmt.Println("  learning enable        Enable closed-loop learning")
	fmt.Println("  learning disable       Disable closed-loop learning")
	fmt.Println("  learning history       Show recent learning cycles")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot forge status              # View Forge status")
	fmt.Println("  nemesisbot forge enable              # Enable self-learning")
	fmt.Println("  nemesisbot forge reflect             # Trigger reflection now")
	fmt.Println("  nemesisbot forge export              # Export all active artifacts")
	fmt.Println("  nemesisbot forge export mcp-validator # Export specific artifact")
	fmt.Println()
	fmt.Println("Configuration:")
	fmt.Println("  Main config:     config.json → forge.enabled")
	fmt.Println("  Forge settings:  workspace/forge/forge.json")
}

func cmdForgeStatus() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	enabled := cfg.Forge != nil && cfg.Forge.Enabled

	fmt.Println("\n=== Forge Self-Learning Module ===")
	fmt.Println()

	statusStr := "disabled"
	if enabled {
		statusStr = "enabled"
	}
	fmt.Printf("  Status: %s\n", statusStr)

	// Load workspace info
	workspace := cfg.WorkspacePath()
	forgeDir := filepath.Join(workspace, "forge")
	forgeConfigPath := filepath.Join(forgeDir, "forge.json")
	registryPath := filepath.Join(forgeDir, "registry.json")

	fmt.Println()
	fmt.Println("  Configuration:")
	fmt.Printf("    Main Config:     %s (forge.enabled)\n", GetConfigPath())
	fmt.Printf("    Forge Settings:  %s\n", forgeConfigPath)
	fmt.Printf("    Registry:        %s\n", registryPath)
	fmt.Printf("    Experiences:     %s/experiences/\n", forgeDir)
	fmt.Printf("    Reflections:     %s/reflections/\n", forgeDir)

	if !enabled {
		fmt.Println()
		fmt.Println("  Run 'nemesisbot forge enable' to enable self-learning.")
		return
	}

	// Load forge config and show stats
	forgeCfg, err := forge.LoadForgeConfig(forgeConfigPath)
	if err != nil {
		forgeCfg = forge.DefaultForgeConfig()
	}

	fmt.Println()
	fmt.Println("  Settings:")
	fmt.Printf("    Collection interval:    %s\n", forgeCfg.Collection.FlushInterval.String())
	fmt.Printf("    Reflection interval:    %s\n", forgeCfg.Reflection.Interval.String())
	fmt.Printf("    Min experiences:        %d\n", forgeCfg.Reflection.MinExperiences)
	fmt.Printf("    LLM semantic analysis:  %v\n", forgeCfg.Reflection.UseLLM)
	fmt.Printf("    Default artifact status: %s\n", forgeCfg.Artifacts.DefaultStatus)
	fmt.Printf("    Trace collection:       %v\n", forgeCfg.Trace.Enabled)
	fmt.Printf("    Learning (closed-loop): %v\n", forgeCfg.Learning.Enabled)

	// Show registry stats
	registry := forge.NewRegistry(registryPath)
	artifacts := registry.ListAll()
	fmt.Println()
	fmt.Printf("  Artifacts: %d total\n", len(artifacts))
	if len(artifacts) > 0 {
		for _, a := range artifacts {
			fmt.Printf("    - [%s] %s v%s (%s)\n", a.Type, a.Name, a.Version, a.Status)
		}
	}
}

func cmdForgeReflect() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	if cfg.Forge == nil || !cfg.Forge.Enabled {
		fmt.Println("Forge module is not enabled. Run 'nemesisbot forge enable' first.")
		return
	}

	workspace := cfg.WorkspacePath()
	forgeDir := filepath.Join(workspace, "forge")

	// Create a temporary reflector to run reflection
	store := forge.NewExperienceStore(forgeDir, forge.DefaultForgeConfig())
	registry := forge.NewRegistry(filepath.Join(forgeDir, "registry.json"))
	reflector := forge.NewReflector(forgeDir, store, registry, forge.DefaultForgeConfig())

	fmt.Println("Running Forge reflection...")

	reportPath, err := reflector.Reflect(context.Background(), "today", "all")
	if err != nil {
		fmt.Printf("Reflection failed: %v\n", err)
		return
	}

	fmt.Printf("Reflection report generated: %s\n", reportPath)
}

func cmdForgeList() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	workspace := cfg.WorkspacePath()
	registryPath := filepath.Join(workspace, "forge", "registry.json")
	registry := forge.NewRegistry(registryPath)

	artifacts := registry.ListAll()
	if len(artifacts) == 0 {
		fmt.Println("\nNo Forge artifacts found.")
		return
	}

	fmt.Printf("\nForge Artifacts (%d):\n\n", len(artifacts))
	fmt.Println("  ID                               | Type   | Name                 | Version | Status")
	fmt.Println("  ---------------------------------|--------|----------------------|---------|----------")
	for _, a := range artifacts {
		fmt.Printf("  %-32s | %-6s | %-20s | %-7s | %s\n",
			a.ID, a.Type, a.Name, a.Version, a.Status)
	}
}

func cmdForgeEnable() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	if cfg.Forge == nil {
		cfg.Forge = &config.ForgeFlagConfig{}
	}
	cfg.Forge.Enabled = true

	configPath := GetConfigPath()
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	// Create default forge config if not exists
	workspace := cfg.WorkspacePath()
	forgeDir := filepath.Join(workspace, "forge")
	forgeConfigPath := filepath.Join(forgeDir, "forge.json")

	if _, err := os.Stat(forgeConfigPath); err != nil {
		os.MkdirAll(forgeDir, 0755)
		defaultCfg := forge.DefaultForgeConfig()
		if err := forge.SaveForgeConfig(forgeConfigPath, defaultCfg); err != nil {
			fmt.Printf("Warning: Could not create default forge config: %v\n", err)
		} else {
			fmt.Printf("Created default Forge config: %s\n", forgeConfigPath)
		}
	}

	// Create directory structure
	for _, subDir := range []string{"experiences", "reflections", "skills", "scripts", "mcp", "traces", "learning"} {
		os.MkdirAll(filepath.Join(forgeDir, subDir), 0755)
	}
	// Create prompts directory
	os.MkdirAll(filepath.Join(workspace, "prompts"), 0755)

	fmt.Println("Forge self-learning module enabled.")
	fmt.Println("\nRestart agent/gateway to apply changes.")
}

func cmdForgeDisable() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	if cfg.Forge == nil {
		cfg.Forge = &config.ForgeFlagConfig{}
	}
	cfg.Forge.Enabled = false

	configPath := GetConfigPath()
	if err := config.SaveConfig(configPath, cfg); err != nil {
		fmt.Printf("Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("Forge self-learning module disabled.")
	fmt.Println("\nRestart agent/gateway to apply changes.")
}

func cmdForgeExport() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	workspace := cfg.WorkspacePath()
	forgeDir := filepath.Join(workspace, "forge")
	registryPath := filepath.Join(forgeDir, "registry.json")
	registry := forge.NewRegistry(registryPath)
	exporter := forge.NewExporter(workspace, registry)

	targetDir := filepath.Join(forgeDir, "exports")

	// Export specific artifact if ID provided
	if len(os.Args) >= 4 {
		artifactID := os.Args[3]
		fmt.Printf("Exporting artifact: %s\n", artifactID)
		if err := exporter.ExportArtifact(artifactID, targetDir); err != nil {
			fmt.Printf("Export failed: %v\n", err)
			os.Exit(1)
		}
		fmt.Printf("Artifact %s exported to %s\n", artifactID, targetDir)
		return
	}

	// Export all active artifacts
	fmt.Println("Exporting all active artifacts...")
	count, err := exporter.ExportAll(targetDir)
	if err != nil {
		fmt.Printf("Export failed: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("Exported %d artifact(s) to %s\n", count, targetDir)
}

func cmdForgeLearning() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	workspace := cfg.WorkspacePath()
	forgeDir := filepath.Join(workspace, "forge")
	forgeConfigPath := filepath.Join(forgeDir, "forge.json")

	forgeCfg, err := forge.LoadForgeConfig(forgeConfigPath)
	if err != nil {
		forgeCfg = forge.DefaultForgeConfig()
	}

	// Handle sub-commands
	if len(os.Args) >= 4 {
		action := os.Args[3]
		switch action {
		case "enable":
			forgeCfg.Learning.Enabled = true
			// Auto-enable trace
			forgeCfg.Trace.Enabled = true
			if err := forge.SaveForgeConfig(forgeConfigPath, forgeCfg); err != nil {
				fmt.Printf("Error saving config: %v\n", err)
				os.Exit(1)
			}
			fmt.Println("Closed-loop learning enabled. Trace collection also enabled.")
			fmt.Println("Restart agent/gateway to apply changes.")
			return
		case "disable":
			forgeCfg.Learning.Enabled = false
			if err := forge.SaveForgeConfig(forgeConfigPath, forgeCfg); err != nil {
				fmt.Printf("Error saving config: %v\n", err)
				os.Exit(1)
			}
			fmt.Println("Closed-loop learning disabled.")
			fmt.Println("Restart agent/gateway to apply changes.")
			return
		case "history":
			showLearningHistory(forgeDir)
			return
		}
	}

	// Default: show status
	fmt.Println("\n=== Forge Closed-Loop Learning (Phase 6) ===")
	fmt.Println()

	learningStatus := "disabled"
	if forgeCfg.Learning.Enabled {
		learningStatus = "enabled"
	}
	fmt.Printf("  Status: %s\n\n", learningStatus)

	fmt.Println("  Configuration:")
	fmt.Printf("    Min pattern frequency:    %d\n", forgeCfg.Learning.MinPatternFrequency)
	fmt.Printf("    High confidence threshold: %.2f\n", forgeCfg.Learning.HighConfThreshold)
	fmt.Printf("    Max auto creates/cycle:   %d\n", forgeCfg.Learning.MaxAutoCreates)
	fmt.Printf("    Max refine rounds:        %d\n", forgeCfg.Learning.MaxRefineRounds)
	fmt.Printf("    Min outcome samples:      %d\n", forgeCfg.Learning.MinOutcomeSamples)
	fmt.Printf("    Monitor window (days):    %d\n", forgeCfg.Learning.MonitorWindowDays)
	fmt.Printf("    Degrade threshold:        %.2f\n", forgeCfg.Learning.DegradeThreshold)
	fmt.Printf("    Degrade cooldown (days):  %d\n", forgeCfg.Learning.DegradeCooldownDays)
	fmt.Printf("    LLM budget tokens:        %d\n", forgeCfg.Learning.LLMBudgetTokens)

	// Show latest cycle
	showLearningHistory(forgeDir)
}

func showLearningHistory(forgeDir string) {
	learningDir := filepath.Join(forgeDir, "learning")
	if _, err := os.Stat(learningDir); os.IsNotExist(err) {
		fmt.Println("\n  No learning cycles recorded yet.")
		return
	}

	// Read recent cycles
	cycleStore := forge.NewCycleStore(forgeDir, nil)
	cycles, err := cycleStore.ReadCycles(time.Now().UTC().AddDate(0, 0, -30))
	if err != nil || len(cycles) == 0 {
		fmt.Println("\n  No learning cycles recorded yet.")
		return
	}

	fmt.Printf("\n  Recent learning cycles (%d):\n\n", len(cycles))
	for _, cycle := range cycles {
		completed := "running"
		if cycle.CompletedAt != nil {
			completed = cycle.CompletedAt.Format("2006-01-02 15:04")
		}
		fmt.Printf("    [%s] %s → patterns=%d, actions=%d/%d executed, skipped=%d\n",
			cycle.ID[:16], completed,
			cycle.PatternsFound, cycle.ActionsExecuted, cycle.ActionsCreated, cycle.ActionsSkipped)
	}
}
