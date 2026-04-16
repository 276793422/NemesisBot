package command

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/path"
	"github.com/276793422/NemesisBot/module/skills"
	"github.com/276793422/NemesisBot/module/utils"
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

	// Load skills config and set up registry manager
	skillsConfigPath := path.ResolveSkillsConfigPathInWorkspace(workspace)
	if skillsFullCfg, err := config.LoadSkillsConfig(skillsConfigPath); err == nil {
		rm := buildSkillsRegistryManagerFromConfig(skillsFullCfg)
		installer.SetRegistryManager(rm)
	}
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
			fmt.Println("Note:  This is the legacy command. Recommended: nemesisbot skills install clawhub/<skill-name>")
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
	case "add-source":
		if len(os.Args) < 4 {
			fmt.Println("Usage: nemesisbot skills add-source <github-url>")
			fmt.Println()
			fmt.Println("Examples:")
			fmt.Println("  nemesisbot skills add-source https://github.com/openclaw/skills")
			fmt.Println("  nemesisbot skills add-source anthropics/skills")
			return
		}
		cmdSkillsAddSource(os.Args[3])
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
	fmt.Println("  install <registry>/<slug>        Install skill from a registry")
	fmt.Println("  install <github-repo-path>       Install skill from GitHub repo")
	fmt.Println("  install-clawhub <author> <name>  Install skill from ClawHub (legacy)")
	fmt.Println("  install-builtin                  Install all builtin skills to workspace")
	fmt.Println("  list-builtin                     List available builtin skills")
	fmt.Println("  remove <name>                    Remove installed skill")
	fmt.Println("  search [query] [--limit N]       Search available skills across all registries")
	fmt.Println("  cache <stats|clear>              Manage search cache")
	fmt.Println("  add-source <github-url>          Add a GitHub repository as skills source")
	fmt.Println("  show <name>                      Show skill details")
	fmt.Println()
	fmt.Println("Search & Install:")
	fmt.Println("  nemesisbot skills search pdf              # Search across ClawHub + GitHub")
	fmt.Println("  nemesisbot skills search pdf --limit 20  # Limit results to 20")
	fmt.Println("  nemesisbot skills install clawhub/stock-portfolio       # From ClawHub")
	fmt.Println("  nemesisbot skills install anthropics/pdf              # From GitHub (2-layer)")
	fmt.Println("  nemesisbot skills install openclaw/clawcv/pdf-export   # From GitHub (3-layer)")
	fmt.Println("  nemesisbot skills install 276793422/nemesisbot-skills/weather  # Direct GitHub repo")
	fmt.Println()
	fmt.Println("Registries:")
	fmt.Println("  clawhub    ClawHub (clawhub.ai) — 55,000+ community skills with vector search")
	fmt.Println("  anthropics anthropics/skills   — Official Anthropic skills")
	fmt.Println("  openclaw   openclaw/skills     — OpenClaw community skills (65,000+)")
	fmt.Println()
	fmt.Println("Other:")
	fmt.Println("  nemesisbot skills list                       # List installed skills")
	fmt.Println("  nemesisbot skills install-builtin            # Install builtin skills")
	fmt.Println("  nemesisbot skills remove weather             # Remove a skill")
	fmt.Println("  nemesisbot skills cache stats                # View search cache stats")
	fmt.Println("  nemesisbot skills cache clear                # Clear search cache")
	fmt.Println("  nemesisbot skills add-source <github-url>    # Add custom GitHub source")
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
		fmt.Println("Usage: nemesisbot skills install <registry>/<slug> | <github-repo-path>")
		fmt.Println()
		fmt.Println("Examples:")
		fmt.Println("  nemesisbot skills install anthropics/pdf          # Install from registry by slug")
		fmt.Println("  nemesisbot skills install 276793422/nemesisbot-skills/weather  # Install from GitHub repo")
		return
	}

	arg := os.Args[3]
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	// Try registry-based install first: <registry>/<slug> (exactly one slash)
	if parts := strings.SplitN(arg, "/", 2); len(parts) == 2 && installer.HasRegistryManager() {
		registryName, slug := parts[0], parts[1]
		if installer.HasRegistry(registryName) {
			fmt.Printf("Installing skill '%s' from registry '%s'...\n", slug, registryName)
			if err := installer.InstallFromRegistry(ctx, registryName, slug, ""); err != nil {
				fmt.Printf("✗ Failed to install skill: %v\n", err)
				os.Exit(1)
			}
			return
		}
	}

	// Fallback: legacy GitHub repo install
	fmt.Printf("Installing skill from %s...\n", arg)
	if err := installer.InstallFromGitHub(ctx, arg); err != nil {
		fmt.Printf("✗ Failed to install skill: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("✓ Skill '%s' installed successfully!\n", filepath.Base(arg))
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
	limit := 50
	for i := 3; i < len(os.Args); i++ {
		if os.Args[i] == "--limit" && i+1 < len(os.Args) {
			i++
			if n, err := strconv.Atoi(os.Args[i]); err == nil && n > 0 {
				limit = n
			}
		} else if query == "" {
			query = os.Args[i]
		}
	}

	// Note: Search cache functionality is available but not configured via command line
	// The cache will be used when the AI agent uses the find_skills tool

	if query == "" {
		fmt.Println("🔍 Searching for all available skills...")
	} else {
		fmt.Printf("🔍 Searching for skills matching '%s'...\n", query)
	}

	// Use longer timeout for search — openclaw/skills Trees API is ~20MB
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	// Use the enhanced search functionality with registry manager
	if installer.HasRegistryManager() {
		// Use RegistryManager.SearchAll
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

			if result.DisplayName != "" && result.DisplayName != result.Slug {
				fmt.Printf("   Name: %s\n", result.DisplayName)
			}

			if result.Author != "" {
				fmt.Printf("   Author: %s\n", result.Author)
			}

			if result.Downloads > 0 {
				fmt.Printf("   Downloads: %d\n", result.Downloads)
			}

			if result.Summary != "" {
				fmt.Printf("   Description: %s\n", result.Summary)
			}

			// Show install command
			fmt.Printf("   Install: nemesisbot skills install %s/%s\n", result.RegistryName, result.Slug)

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

// buildSkillsRegistryManagerFromConfig builds a skills.RegistryManager from SkillsFullConfig.
func buildSkillsRegistryManagerFromConfig(cfg *config.SkillsFullConfig) *skills.RegistryManager {
	rc := skills.RegistryConfig{
		MaxConcurrentSearches: cfg.MaxConcurrentSearches,
		SearchCache: skills.SearchCacheConfig{
			Enabled: cfg.SearchCache.Enabled,
			MaxSize: cfg.SearchCache.MaxSize,
		},
	}

	if cfg.SearchCache.TTLSeconds > 0 {
		rc.SearchCache.TTL = time.Duration(cfg.SearchCache.TTLSeconds) * time.Second
	}

	for _, src := range cfg.GitHubSources {
		rc.GitHubSources = append(rc.GitHubSources, skills.GitHubSourceConfig{
			Name:             src.Name,
			Repo:             src.Repo,
			Enabled:          src.Enabled,
			Branch:           src.Branch,
			IndexType:        src.IndexType,
			IndexPath:        src.IndexPath,
			SkillPathPattern: src.SkillPathPattern,
			Timeout:          src.Timeout,
			MaxSize:          src.MaxSize,
		})
	}

	rc.ClawHub = skills.ClawHubConfig{
		Enabled:   cfg.ClawHub.Enabled,
		BaseURL:   cfg.ClawHub.BaseURL,
		ConvexURL: cfg.ClawHub.ConvexURL,
		Timeout:   cfg.ClawHub.Timeout,
	}

	return skills.NewRegistryManagerFromConfig(rc)
}

// parseGitHubURL extracts owner and repo from various GitHub URL formats.
func parseGitHubURL(input string) (owner, repo string, err error) {
	input = strings.TrimSpace(input)

	// Remove trailing .git
	input = strings.TrimSuffix(input, ".git")

	// Try parsing as full URL
	if strings.HasPrefix(input, "https://") || strings.HasPrefix(input, "http://") {
		u, parseErr := url.Parse(input)
		if parseErr != nil {
			return "", "", fmt.Errorf("invalid URL: %s\nExamples:\n  https://github.com/openclaw/skills\n  openclaw/skills", input)
		}
		parts := strings.Split(strings.Trim(u.Path, "/"), "/")
		if len(parts) < 2 {
			return "", "", fmt.Errorf("invalid GitHub URL: %s\nExamples:\n  https://github.com/openclaw/skills\n  openclaw/skills", input)
		}
		return parts[0], parts[1], nil
	}

	// Strip "github.com/" prefix if present
	input = strings.TrimPrefix(input, "github.com/")
	input = strings.TrimPrefix(input, "www.github.com/")

	// Treat as owner/repo
	parts := strings.Split(input, "/")
	if len(parts) != 2 || parts[0] == "" || parts[1] == "" {
		return "", "", fmt.Errorf("invalid format: %s\nExamples:\n  https://github.com/openclaw/skills\n  openclaw/skills", input)
	}

	return parts[0], parts[1], nil
}

// skillDetectResult holds the result of auto-detecting a repo's skill structure.
type skillDetectResult struct {
	IndexType string // "github_api" or "skills_json"
	Pattern   string // e.g. "skills/{slug}/SKILL.md"
	Branch    string // detected branch, "main" or "master"
}

// repoCheckResult holds the result of verifying a GitHub repository exists.
type repoCheckResult struct {
	Exists   bool
	Private  bool
	RateLimit bool
	Message  string
}

// verifyRepoExists checks whether a GitHub repository is publicly accessible.
func verifyRepoExists(ctx context.Context, client *http.Client, owner, repo string) repoCheckResult {
	apiURL := fmt.Sprintf("https://api.github.com/repos/%s/%s", owner, repo)
	req, err := http.NewRequestWithContext(ctx, "GET", apiURL, nil)
	if err != nil {
		return repoCheckResult{Message: fmt.Sprintf("failed to create request: %v", err)}
	}
	req.Header.Set("Accept", "application/vnd.github.v3+json")

	resp, err := utils.DoRequestWithRetry(client, req)
	if err != nil {
		return repoCheckResult{Message: fmt.Sprintf("network error: %v", err)}
	}
	defer resp.Body.Close()

	switch resp.StatusCode {
	case http.StatusOK:
		return repoCheckResult{Exists: true}
	case http.StatusNotFound:
		// Could be private or truly absent — distinguish via a second check
		// on raw content. If raw.githubusercontent.com also 404s, assume non-existent.
		rawURL := fmt.Sprintf("https://raw.githubusercontent.com/%s/%s/HEAD/README.md", owner, repo)
		rawReq, rawErr := http.NewRequestWithContext(ctx, "HEAD", rawURL, nil)
		if rawErr == nil {
			rawResp, rawErr := utils.DoRequestWithRetry(client, rawReq)
			if rawErr == nil {
				rawResp.Body.Close()
				if rawResp.StatusCode == http.StatusOK {
					// Repo is accessible via raw but not API — likely a transient API issue
					return repoCheckResult{Exists: true}
				}
			}
		}
		return repoCheckResult{Message: fmt.Sprintf("repository '%s/%s' not found or is private", owner, repo)}
	case http.StatusForbidden:
		return repoCheckResult{RateLimit: true, Message: "GitHub API rate limit exceeded, please try again later or use a personal access token"}
	default:
		return repoCheckResult{Message: fmt.Sprintf("unexpected HTTP %d from GitHub API", resp.StatusCode)}
	}
}

// detectSkillStructure verifies the repo exists and probes its skill layout.
func detectSkillStructure(ctx context.Context, owner, repo string) (*skillDetectResult, error) {
	client := &http.Client{Timeout: 30 * time.Second}

	// Step 0: Verify the repository exists and is accessible
	check := verifyRepoExists(ctx, client, owner, repo)
	if !check.Exists {
		return nil, fmt.Errorf("%s", check.Message)
	}

	// Probe 1: Check skills/ directory via GitHub API
	probe1URL := fmt.Sprintf("https://api.github.com/repos/%s/%s/contents/skills", owner, repo)
	dirs, probeErr := probeGitHubDir(ctx, client, probe1URL)
	if probeErr == nil && len(dirs) > 0 {
		// Pattern A: skills/{slug}/SKILL.md (two-level)
		if verifySkillMDInDirs(ctx, client, owner, repo, "main", "skills", dirs, 5) {
			return &skillDetectResult{
				IndexType: "github_api",
				Pattern:   "skills/{slug}/SKILL.md",
				Branch:    "main",
			}, nil
		}
		// Pattern B: skills/{author}/{slug}/SKILL.md (three-level, e.g. openclaw/skills)
		// Probe only the first author dir via raw URL to confirm structure
		if verifyThreeLevelSkills(ctx, client, owner, repo, "main", dirs) {
			return &skillDetectResult{
				IndexType: "github_api",
				Pattern:   "skills/{author}/{slug}/SKILL.md",
				Branch:    "main",
			}, nil
		}
	}

	// Probe 2: Try skills.json index on main
	probe2URL := fmt.Sprintf("https://raw.githubusercontent.com/%s/%s/main/skills.json", owner, repo)
	if result, ok := probeSkillsJSON(ctx, client, probe2URL); ok {
		result.Branch = "main"
		return result, nil
	}

	// Probe 2b: Try skills.json on master
	probe2bURL := fmt.Sprintf("https://raw.githubusercontent.com/%s/%s/master/skills.json", owner, repo)
	if result, ok := probeSkillsJSON(ctx, client, probe2bURL); ok {
		result.Branch = "master"
		return result, nil
	}

	// Probe 3: Root-level directories with SKILL.md
	probe3URL := fmt.Sprintf("https://api.github.com/repos/%s/%s/contents/", owner, repo)
	if result, err := probeRootLevelSkills(ctx, client, owner, repo, probe3URL); err == nil && result != nil {
		return result, nil
	}

	return nil, fmt.Errorf("repository '%s/%s' exists but contains no detectable skills\nPlease make sure the repo has a skills/ directory with SKILL.md files, or manually edit: %s", owner, repo, GetSkillsConfigPath())
}

// verifySkillMDInDirs checks if any subdirectory under basePath contains a SKILL.md file.
// maxCheck limits how many dirs to probe (avoids exhaustion on repos with thousands of entries).
// Uses GitHub API to list each subdirectory and look for SKILL.md in the file listing.
func verifySkillMDInDirs(ctx context.Context, client *http.Client, owner, repo, branch, basePath string, dirs []string, maxCheck int) bool {
	checked := 0
	for _, dir := range dirs {
		if checked >= maxCheck {
			break
		}
		if containsSkillMD(ctx, client, fmt.Sprintf("https://api.github.com/repos/%s/%s/contents/%s/%s", owner, repo, basePath, dir)) {
			return true
		}
		checked++
	}
	return false
}

// verifyThreeLevelSkills probes the three-level pattern skills/{author}/{slug}/SKILL.md.
// It checks up to maxCheck author dirs by listing their subdirs via GitHub API,
// then verifies SKILL.md exists in the first found subdir.
func verifyThreeLevelSkills(ctx context.Context, client *http.Client, owner, repo, branch string, authorDirs []string) bool {
	maxCheck := 5
	checked := 0
	for _, authorDir := range authorDirs {
		if checked >= maxCheck {
			break
		}
		authorURL := fmt.Sprintf("https://api.github.com/repos/%s/%s/contents/skills/%s", owner, repo, authorDir)
		skillDirs, err := probeGitHubDir(ctx, client, authorURL)
		if err != nil || len(skillDirs) == 0 {
			checked++
			continue
		}
		// Check first subdir for SKILL.md via API
		sd := skillDirs[0]
		if containsSkillMD(ctx, client, fmt.Sprintf("https://api.github.com/repos/%s/%s/contents/skills/%s/%s", owner, repo, authorDir, sd)) {
			return true
		}
		checked++
	}
	return false
}

// containsSkillMD checks if a GitHub API contents URL lists a SKILL.md file.
func containsSkillMD(ctx context.Context, client *http.Client, apiURL string) bool {
	req, err := http.NewRequestWithContext(ctx, "GET", apiURL, nil)
	if err != nil {
		return false
	}
	req.Header.Set("Accept", "application/vnd.github.v3+json")

	resp, err := utils.DoRequestWithRetry(client, req)
	if err != nil {
		return false
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return false
	}

	var entries []struct {
		Type string `json:"type"`
		Name string `json:"name"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&entries); err != nil {
		return false
	}

	for _, e := range entries {
		if e.Name == "SKILL.md" && e.Type == "file" {
			return true
		}
	}
	return false
}

// probeGitHubDir checks if a GitHub API contents URL returns directory entries.
func probeGitHubDir(ctx context.Context, client *http.Client, apiURL string) ([]string, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", apiURL, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Accept", "application/vnd.github.v3+json")

	resp, err := utils.DoRequestWithRetry(client, req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusForbidden {
		return nil, fmt.Errorf("GitHub API rate limit hit, please try again later")
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("HTTP %d", resp.StatusCode)
	}

	var entries []struct {
		Type string `json:"type"`
		Name string `json:"name"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&entries); err != nil {
		return nil, err
	}

	var dirs []string
	for _, e := range entries {
		if e.Type == "dir" {
			dirs = append(dirs, e.Name)
		}
	}
	return dirs, nil
}

// probeSkillsJSON tries to download and parse a skills.json index file.
func probeSkillsJSON(ctx context.Context, client *http.Client, rawURL string) (*skillDetectResult, bool) {
	req, err := http.NewRequestWithContext(ctx, "GET", rawURL, nil)
	if err != nil {
		return nil, false
	}

	resp, err := utils.DoRequestWithRetry(client, req)
	if err != nil {
		return nil, false
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, false
	}

	// Try parsing as JSON array (simple index format)
	var arr []interface{}
	if err := json.NewDecoder(resp.Body).Decode(&arr); err != nil {
		return nil, false
	}

	return &skillDetectResult{
		IndexType: "skills_json",
		Pattern:   "skills/{slug}/SKILL.md",
	}, true
}

// probeRootLevelSkills detects skills at the root level of a repo.
func probeRootLevelSkills(ctx context.Context, client *http.Client, owner, repo, apiURL string) (*skillDetectResult, error) {
	dirs, err := probeGitHubDir(ctx, client, apiURL)
	if err != nil {
		return nil, err
	}

	// Filter out common non-skill directories
	skip := map[string]bool{
		"src": true, "pkg": true, "cmd": true, "internal": true,
		"docs": true, ".github": true, "test": true, "tests": true,
		"scripts": true, "examples": true, "build": true, "dist": true,
		"vendor": true, "node_modules": true, ".git": true, ".vscode": true,
	}

	checked := 0
	for _, dir := range dirs {
		if skip[dir] || strings.HasPrefix(dir, ".") {
			continue
		}
		if checked >= 5 {
			break
		}
		// Verify SKILL.md exists in this directory via GitHub API
		contentsURL := fmt.Sprintf("https://api.github.com/repos/%s/%s/contents/%s", owner, repo, dir)
		if containsSkillMD(ctx, client, contentsURL) {
			return &skillDetectResult{
				IndexType: "github_api",
				Pattern:   "{slug}/SKILL.md",
				Branch:    "main",
			}, nil
		}
		checked++
	}

	return nil, fmt.Errorf("no root-level skills detected")
}


func cmdSkillsAddSource(inputURL string) {
	// 1. Parse URL
	owner, repo, err := parseGitHubURL(inputURL)
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	repoFullName := fmt.Sprintf("%s/%s", owner, repo)
	fmt.Printf("Validating repository %s ...\n", repoFullName)

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// 2. Locate config file
	skillsConfigPath := GetSkillsConfigPath()

	// 3. Load existing config
	skillsCfg, err := config.LoadSkillsConfig(skillsConfigPath)
	if err != nil {
		fmt.Printf("Error loading skills config: %v\n", err)
		os.Exit(1)
	}

	// 4. Check for duplicate
	for _, src := range skillsCfg.GitHubSources {
		if src.Repo == repoFullName {
			fmt.Printf("Repository '%s' already exists as source '%s'.\n", repoFullName, src.Name)
			os.Exit(1)
		}
	}

	// 5. Detect structure (includes repo existence verification)
	result, err := detectSkillStructure(ctx, owner, repo)
	if err != nil {
		fmt.Printf("Error: %v\n", err)
		os.Exit(1)
	}

	// 6. Generate unique name
	name := owner
	for i := 1; ; i++ {
		conflict := false
		for _, src := range skillsCfg.GitHubSources {
			if src.Name == name {
				conflict = true
				break
			}
		}
		if !conflict {
			break
		}
		name = fmt.Sprintf("%s-%d", owner, i)
	}

	// 7. Append source
	newSource := config.GitHubSourceConfig{
		Name:             name,
		Repo:             repoFullName,
		Enabled:          true,
		Branch:           result.Branch,
		IndexType:        result.IndexType,
		SkillPathPattern: result.Pattern,
	}
	skillsCfg.GitHubSources = append(skillsCfg.GitHubSources, newSource)

	// 8. Save
	if err := config.SaveSkillsConfig(skillsConfigPath, skillsCfg); err != nil {
		fmt.Printf("Error saving skills config: %v\n", err)
		os.Exit(1)
	}

	// 9. Print summary
	fmt.Println()
	fmt.Println("Skill source added successfully:")
	fmt.Printf("  Name:     %s\n", name)
	fmt.Printf("  Repo:     %s\n", repoFullName)
	fmt.Printf("  Branch:   %s\n", result.Branch)
	fmt.Printf("  Index:    %s\n", result.IndexType)
	fmt.Printf("  Pattern:  %s\n", result.Pattern)
	fmt.Printf("  Config:   %s\n", skillsConfigPath)
}
