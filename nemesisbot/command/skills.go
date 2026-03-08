package command

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/skills"
)

// CmdSkills manages skills
func CmdSkills() {
	if len(os.Args) < 3 {
		SkillsHelp()
		return
	}

	subcommand := os.Args[2]

	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	workspace := cfg.WorkspacePath()
	installer := skills.NewSkillInstaller(workspace)
	// global skills: ~/.nemesisbot/workspace/skills/
	// builtin skills: (currently unused, reserved for future embedded skills)
	globalDir := filepath.Dir(GetConfigPath())
	globalSkillsDir := filepath.Join(globalDir, "workspace", "skills")
	builtinSkillsDir := "" // Reserved for embedded skills in the future
	skillsLoader := skills.NewSkillsLoader(workspace, globalSkillsDir, builtinSkillsDir)

	switch subcommand {
	case "list":
		cmdSkillsList(skillsLoader)
	case "install":
		cmdSkillsInstall(installer)
	case "install-clawhub":
		if len(os.Args) < 5 {
			fmt.Println("Usage: nemesisbot skills install-clawhub <author> <skill-name> [output-name]")
			fmt.Println("Example: nemesisbot skills install-clawhub steipete weather")
			fmt.Println("         nemesisbot skills install-clawhub steipete weather weather-clawhub")
			return
		}
		cmdSkillsInstallClawHub(workspace, os.Args[3], os.Args[4])
	case "remove", "uninstall":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot skills remove <skill-name>")
			return
		}
		cmdSkillsRemove(installer, os.Args[3])
	case "install-builtin":
		cmdSkillsInstallBuiltin(workspace)
	case "list-builtin":
		cmdSkillsListBuiltin()
	case "search":
		cmdSkillsSearch(cfg, installer)
	case "cache":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot skills cache <stats|clear>")
			fmt.Println("  stats    Show search cache statistics")
			fmt.Println("  clear    Clear search cache")
			return
		}
		cmdSkillsCache(cfg, os.Args[3])
	case "show":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot skills show <skill-name>")
			return
		}
		cmdSkillsShow(skillsLoader, os.Args[3])
	default:
		fmt.Printf("Unknown skills command: %s\n", subcommand)
		SkillsHelp()
	}
}

// SkillsHelp prints skills command help
func SkillsHelp() {
	fmt.Println("\nSkills commands:")
	fmt.Println("  list                             List installed skills")
	fmt.Println("  install <repo>                   Install skill from GitHub")
	fmt.Println("  install-clawhub <author> <name>  Install skill from ClawHub")
	fmt.Println("  install-builtin                  Install all builtin skills to workspace")
	fmt.Println("  list-builtin                     List available builtin skills")
	fmt.Println("  remove <name>                    Remove installed skill")
	fmt.Println("  search [query]                   Search available skills (supports caching)")
	fmt.Println("  cache <stats|clear>              Manage search cache")
	fmt.Println("  show <name>                      Show skill details")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot skills list")
	fmt.Println("  nemesisbot skills install 276793422/nemesisbot-skills/weather")
	fmt.Println("  nemesisbot skills install-clawhub steipete weather")
	fmt.Println("  nemesisbot skills install-clawhub steipete github github-clawhub")
	fmt.Println("  nemesisbot skills install-builtin")
	fmt.Println("  nemesisbot skills list-builtin")
	fmt.Println("  nemesisbot skills search github")
	fmt.Println("  nemesisbot skills search weather")
	fmt.Println("  nemesisbot skills cache stats")
	fmt.Println("  nemesisbot skills cache clear")
	fmt.Println("  nemesisbot skills remove weather")
	fmt.Println()
	fmt.Println("ClawHub (https://clawhub.ai):")
	fmt.Println("  5,705 community skills from OpenClaw")
	fmt.Println("  Browse: https://github.com/VoltAgent/awesome-openclaw-skills")
}

func cmdSkillsList(loader *skills.SkillsLoader) {
	allSkills := loader.ListSkills()

	if len(allSkills) == 0 {
		fmt.Println("No skills installed.")
		return
	}

	fmt.Println("\nInstalled Skills:")
	fmt.Println("------------------")
	for _, skill := range allSkills {
		fmt.Printf("  ✓ %s (%s)\n", skill.Name, skill.Source)
		if skill.Description != "" {
			fmt.Printf("    %s\n", skill.Description)
		}
	}
}

func cmdSkillsInstall(installer *skills.SkillInstaller) {
	if len(os.Args) < 4 {
		fmt.Println("Usage: nemesisbot skills install <github-repo>")
		fmt.Println("Example: nemesisbot skills install 276793422/nemesisbot-skills/weather")
		return
	}

	repo := os.Args[3]
	fmt.Printf("Installing skill from %s...\n", repo)

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	if err := installer.InstallFromGitHub(ctx, repo); err != nil {
		fmt.Printf("✗ Failed to install skill: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("✓ Skill '%s' installed successfully!\n", filepath.Base(repo))
}

func cmdSkillsInstallClawHub(workspace, author, skillName string) {
	outputName := skillName
	if len(os.Args) >= 6 {
		outputName = os.Args[5]
	}

	skillDir := filepath.Join(workspace, "skills", outputName)

	fmt.Printf("📦 Installing '%s' from '%s' (ClawHub)...\n", skillName, author)
	fmt.Printf("   Source: https://github.com/openclaw/skills/tree/main/skills/%s/%s\n", author, skillName)
	fmt.Printf("   Destination: %s\n", skillDir)
	fmt.Println()

	// Create directory
	if err := os.MkdirAll(skillDir, 0755); err != nil {
		fmt.Printf("✗ Failed to create directory: %v\n", err)
		os.Exit(1)
	}

	// Download SKILL.md
	url := fmt.Sprintf("https://raw.githubusercontent.com/openclaw/skills/main/skills/%s/%s/SKILL.md", author, skillName)
	fmt.Printf("📥 Downloading from: %s\n", url)

	client := &http.Client{Timeout: 30 * time.Second}
	resp, err := client.Get(url)
	if err != nil {
		fmt.Printf("✗ Failed to download: %v\n", err)
		os.Exit(1)
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		fmt.Printf("✗ Failed to download: HTTP %d\n", resp.StatusCode)
		fmt.Println()
		fmt.Println("Possible causes:")
		fmt.Println("  1. Author name is incorrect")
		fmt.Println("  2. Skill name is incorrect")
		fmt.Println("  3. Network connection issue")
		fmt.Println()
		fmt.Println("To find available skills, visit:")
		fmt.Println("  https://github.com/VoltAgent/awesome-openclaw-skills")
		os.Exit(1)
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		fmt.Printf("✗ Failed to read response: %v\n", err)
		os.Exit(1)
	}

	skillPath := filepath.Join(skillDir, "SKILL.md")
	if err := os.WriteFile(skillPath, body, 0644); err != nil {
		fmt.Printf("✗ Failed to write file: %v\n", err)
		os.Exit(1)
	}

	fmt.Println()
	fmt.Printf("✅ Skill '%s' installed successfully!\n", outputName)
	fmt.Println()
	fmt.Println("Next steps:")
	fmt.Printf("  nemesisbot skills list       # Verify installation\n")
	fmt.Printf("  nemesisbot skills show %s    # View skill details\n", outputName)
	fmt.Printf("  nemesisbot agent             # Start using the skill\n")
}

func cmdSkillsRemove(installer *skills.SkillInstaller, skillName string) {
	fmt.Printf("Removing skill '%s'...\n", skillName)

	if err := installer.Uninstall(skillName); err != nil {
		fmt.Printf("✗ Failed to remove skill: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("✓ Skill '%s' removed successfully!\n", skillName)
}

func cmdSkillsInstallBuiltin(workspace string) {
	builtinSkillsDir := "./nemesisbot/skills"
	workspaceSkillsDir := filepath.Join(workspace, "skills")

	fmt.Printf("Copying builtin skills to workspace...\n")

	skillsToInstall := []string{
		"weather",
		"news",
		"stock",
		"calculator",
	}

	for _, skillName := range skillsToInstall {
		builtinPath := filepath.Join(builtinSkillsDir, skillName)
		workspacePath := filepath.Join(workspaceSkillsDir, skillName)

		if _, err := os.Stat(builtinPath); err != nil {
			fmt.Printf("⊘ Builtin skill '%s' not found: %v\n", skillName, err)
			continue
		}

		if err := os.MkdirAll(workspacePath, 0755); err != nil {
			fmt.Printf("✗ Failed to create directory for %s: %v\n", skillName, err)
			continue
		}

		if err := CopyDirectory(builtinPath, workspacePath); err != nil {
			fmt.Printf("✗ Failed to copy %s: %v\n", skillName, err)
		}
	}

	fmt.Println("\n✓ All builtin skills installed!")
	fmt.Println("Now you can use them in your workspace.")
}

func cmdSkillsListBuiltin() {
	cfg, err := LoadConfig()
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		return
	}
	builtinSkillsDir := filepath.Join(filepath.Dir(cfg.WorkspacePath()), "nemesisbot", "skills")

	fmt.Println("\nAvailable Builtin Skills:")
	fmt.Println("-----------------------")

	entries, err := os.ReadDir(builtinSkillsDir)
	if err != nil {
		fmt.Printf("Error reading builtin skills: %v\n", err)
		return
	}

	if len(entries) == 0 {
		fmt.Println("No builtin skills available.")
		return
	}

	for _, entry := range entries {
		if entry.IsDir() {
			skillName := entry.Name()
			skillFile := filepath.Join(builtinSkillsDir, skillName, "SKILL.md")

			description := "No description"
			if _, err := os.Stat(skillFile); err == nil {
				data, err := os.ReadFile(skillFile)
				if err == nil {
					content := string(data)
					if idx := strings.Index(content, "\n"); idx > 0 {
						firstLine := content[:idx]
						if strings.Contains(firstLine, "description:") {
							descLine := strings.Index(content[idx:], "\n")
							if descLine > 0 {
								description = strings.TrimSpace(content[idx+descLine : idx+descLine])
							}
						}
					}
				}
			}
			status := "✓"
			fmt.Printf("  %s  %s\n", status, entry.Name())
			if description != "" {
				fmt.Printf("     %s\n", description)
			}
		}
	}
}

func cmdSkillsSearch(cfg *config.Config, installer *skills.SkillInstaller) {
	// Get query from arguments (optional)
	query := ""
	if len(os.Args) >= 4 {
		query = os.Args[3]
	}

	// Note: Search cache functionality is available but not configured via command line
	// The cache will be used when the AI agent uses the find_skills tool

	if query == "" {
		fmt.Println("🔍 Searching for all available skills...")
	} else {
		fmt.Printf("🔍 Searching for skills matching '%s'...\n", query)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// Use the enhanced search functionality with registry manager
	if installer.HasRegistryManager() {
		// Use RegistryManager.SearchAll
		limit := 20 // Show max 20 results
		results, err := installer.SearchAll(ctx, query, limit)
		if err != nil {
			fmt.Printf("✗ Failed to search skills: %v\n", err)
			return
		}

		if len(results) == 0 {
			fmt.Printf("No skills found matching '%s'\n", query)
			return
		}

		fmt.Printf("\n📦 Found %d skill(s):\n", len(results))
		fmt.Println("-------------------")
		for i, result := range results {
			fmt.Printf("%d. **%s**", i+1, result.Slug)
			if result.Version != "" {
				fmt.Printf(" v%s", result.Version)
			}
			fmt.Printf(" (score: %.2f, registry: %s)\n", result.Score, result.RegistryName)

			if result.DisplayName != "" {
				fmt.Printf("   Name: %s\n", result.DisplayName)
			}

			if result.Summary != "" {
				fmt.Printf("   Description: %s\n", result.Summary)
			}

			fmt.Println()
		}
	} else {
		// Fallback to old ListAvailableSkills method
		availableSkills, err := installer.ListAvailableSkills(ctx)
		if err != nil {
			fmt.Printf("✗ Failed to fetch skills list: %v\n", err)
			return
		}

		if len(availableSkills) == 0 {
			fmt.Println("No skills available.")
			return
		}

		fmt.Printf("\n📦 Available Skills (%d):\n", len(availableSkills))
		fmt.Println("--------------------")
		for _, skill := range availableSkills {
			fmt.Printf("  📦 %s\n", skill.Name)
			fmt.Printf("     %s\n", skill.Description)
			fmt.Printf("     Repo: %s\n", skill.Repository)
			if skill.Author != "" {
				fmt.Printf("     Author: %s\n", skill.Author)
			}
			if len(skill.Tags) > 0 {
				fmt.Printf("     Tags: %v\n", skill.Tags)
			}
			fmt.Println()
		}
	}
}

func cmdSkillsShow(loader *skills.SkillsLoader, skillName string) {
	content, ok := loader.LoadSkill(skillName)
	if !ok {
		fmt.Printf("✗ Skill '%s' not found\n", skillName)
		return
	}

	fmt.Printf("\n📦 Skill: %s\n", skillName)
	fmt.Println("----------------------")
	fmt.Println(content)
}

func cmdSkillsCache(cfg *config.Config, action string) {
	// Create a registry manager with default cache configuration
	// Search cache is available but configured internally, not via config file
	cacheConfig := skills.SearchCacheConfig{
		Enabled: true,
		MaxSize: 50,
		TTL:     5 * 60 * 1000000000, // 5 minutes in nanoseconds
	}
	regConfig := skills.RegistryConfig{
		SearchCache: cacheConfig,
	}
	regMgr := skills.NewRegistryManagerFromConfig(regConfig)

	// Get cache
	cache := regMgr.GetSearchCache()
	if cache == nil {
		fmt.Println("⚠ Search cache is not available.")
		return
	}

	switch action {
	case "stats":
		stats := cache.Stats()

		fmt.Println("\n📊 Search Cache Statistics:")
		fmt.Println("-------------------------")
		fmt.Printf("Entries:      %d / %d\n", stats.Size, stats.MaxSize)
		fmt.Printf("Hit Count:    %d\n", stats.HitCount)
		fmt.Printf("Miss Count:   %d\n", stats.MissCount)
		fmt.Printf("Hit Rate:     %.2f%%\n", stats.HitRate*100)
		fmt.Printf("Memory Usage: ~%d bytes\n", stats.Size*100) // Approximate
		fmt.Println()

		if stats.HitRate >= 0.8 {
			fmt.Println("✅ Cache performance: Excellent")
		} else if stats.HitRate >= 0.5 {
			fmt.Println("🟡 Cache performance: Good")
		} else if stats.HitRate > 0 {
			fmt.Println("🟠 Cache performance: Low - consider increasing TTL")
		} else {
			fmt.Println("🔴 Cache performance: No hits yet")
		}

	case "clear":
		oldStats := cache.Stats()
		cache.Clear()
		fmt.Printf("\n🗑️  Cache cleared!\n")
		fmt.Printf("   Removed %d entries\n", oldStats.Size)
		fmt.Printf("   Freed ~%d bytes\n", oldStats.Size*100)
		fmt.Println()
		fmt.Println("💡 Tip: The cache will rebuild as you search for skills")

	default:
		fmt.Printf("Unknown cache action: %s\n", action)
		fmt.Println("Usage: nemesisbot skills cache <stats|clear>")
	}
}
