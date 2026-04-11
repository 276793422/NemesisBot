package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/skills"
)

// Real skills from the project's Skills/ directory
var realSkills = []struct {
	Name        string
	Description string
}{
	{"automated-testing", "完整的自动化测试流程，使用 TestAIServer 作为模拟后端"},
	{"build-project", "定义 NemesisBot 项目的构建流程，包括环境准备、编译构建、结果验证"},
	{"structured-development", "定义完整的结构化开发流程，包括预研、计划、开发、测试、复查、报告等阶段"},
	{"dump-analyze", "分析崩溃转储文件 (.dmp)，使用 cdb/windbg 进行调试"},
	{"wsl-operations", "在 Windows 上运行 WSL 命令，管理 WSL 进程和文件传输"},
	{"desktop-automation", "自动查找浏览器窗口并截取屏幕保存为图片"},
}

// findSkillsDir locates the project's Skills/ directory
func findSkillsDir() string {
	// Try from current working directory upward
	if wd, err := os.Getwd(); err == nil {
		for dir := wd; dir != ""; dir = filepath.Dir(dir) {
			candidate := filepath.Join(dir, "Skills")
			if info, err := os.Stat(filepath.Join(candidate, "automated-testing")); err == nil && info.IsDir() {
				return candidate
			}
			parent := filepath.Dir(dir)
			if parent == dir {
				break
			}
		}
	}
	return ""
}

func main() {
	fmt.Println("============================================")
	fmt.Println("  Skill Registry Install Test (Local)")
	fmt.Println("============================================")
	fmt.Println()

	// Find real skill files
	skillsDir := findSkillsDir()
	if skillsDir == "" {
		fmt.Println("ERROR: Cannot find Skills/ directory")
		os.Exit(1)
	}
	fmt.Printf("Skills source: %s\n", skillsDir)

	// Target workspace
	homeDir, _ := os.UserHomeDir()
	workspace := filepath.Join(homeDir, ".nemesisbot", "workspace")
	os.MkdirAll(workspace, 0755)
	fmt.Printf("Workspace:     %s\n", workspace)
	fmt.Println()

	// Step 1: Start local HTTP server serving real skill files
	fmt.Println("[Step 1] Starting local registry server...")
	server := setupLocalRegistry(skillsDir)
	defer server.Close()
	fmt.Printf("  Server: %s\n", server.URL)
	fmt.Println()

	// Step 2: Create registry manager pointing to local server
	fmt.Println("[Step 2] Creating registry manager...")
	cfg := skills.RegistryConfig{
		GitHub: skills.GitHubConfig{
			Enabled: true,
			BaseURL: server.URL + "/",
		},
	}
	regMgr := skills.NewRegistryManagerFromConfig(cfg)
	fmt.Println("  Registry manager ready")
	fmt.Println()

	// Step 3: Search for a skill
	fmt.Println("[Step 3] Searching for skill 'build'...")
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	results, err := regMgr.SearchAll(ctx, "build", 10)
	cancel()

	if err != nil {
		fmt.Printf("  ERROR: %v\n", err)
		os.Exit(1)
	}
	if len(results) == 0 {
		fmt.Println("  ERROR: No skills found")
		os.Exit(1)
	}

	fmt.Printf("  Found %d result(s):\n", len(results))
	for i, r := range results {
		fmt.Printf("    %d. %s - %s (score: %.1f)\n", i+1, r.Slug, r.Summary, r.Score)
	}

	// Pick the first result
	target := results[0]
	fmt.Printf("\n  Selected: %s\n", target.Slug)
	fmt.Println()

	// Step 4: Install the skill
	fmt.Printf("[Step 4] Installing '%s'...\n", target.Slug)
	installer := skills.NewSkillInstaller(workspace)
	installer.SetRegistryManager(regMgr)

	ctx, cancel = context.WithTimeout(context.Background(), 15*time.Second)
	err = installer.InstallFromRegistry(ctx, "github", target.Slug, "")
	cancel()

	if err != nil {
		fmt.Printf("  ERROR: %v\n", err)
		os.Exit(1)
	}
	fmt.Println()

	// Step 5: Verify installation
	fmt.Println("[Step 5] Verifying installation...")
	skillDir := filepath.Join(workspace, "skills", target.Slug)

	// Directory check
	if info, err := os.Stat(skillDir); err != nil || !info.IsDir() {
		fmt.Printf("  FAIL: directory not found: %s\n", skillDir)
		os.Exit(1)
	}
	fmt.Printf("  Directory:  %s\n", skillDir)

	// SKILL.md check
	skillFile := filepath.Join(skillDir, "SKILL.md")
	content, err := os.ReadFile(skillFile)
	if err != nil {
		fmt.Printf("  FAIL: SKILL.md not readable: %v\n", err)
		os.Exit(1)
	}
	lines := strings.Split(string(content), "\n")
	fmt.Printf("  SKILL.md:   %d lines, %d bytes\n", len(lines), len(content))

	// Preview first few lines
	fmt.Println("  --- Preview ---")
	for i, line := range lines {
		if i >= 6 {
			break
		}
		fmt.Printf("    %s\n", line)
	}
	fmt.Println("  ---")

	// Origin tracking check
	originFile := filepath.Join(skillDir, ".skill-origin.json")
	originContent, err := os.ReadFile(originFile)
	if err != nil {
		fmt.Printf("  WARN: .skill-origin.json not found\n")
	} else {
		var origin map[string]interface{}
		json.Unmarshal(originContent, &origin)
		fmt.Printf("  Origin:     registry=%v, slug=%v, version=%v\n",
			origin["registry"], origin["slug"], origin["installed_version"])
	}

	// SkillsLoader check
	loader := skills.NewSkillsLoader(workspace, "", "")
	skillList := loader.ListSkills()
	for _, s := range skillList {
		if s.Name == target.Slug {
			fmt.Printf("  Loader:     found (source=%s)\n", s.Source)
			break
		}
	}
	fmt.Println()

	// Summary
	fmt.Println("============================================")
	fmt.Println("  INSTALL SUCCESSFUL")
	fmt.Printf("  Skill:    %s\n", target.Slug)
	fmt.Printf("  Location: %s\n", skillDir)
	fmt.Println("============================================")
}

// setupLocalRegistry creates an HTTP server that mimics the GitHub registry API
// but serves real skill files from the local Skills/ directory.
func setupLocalRegistry(skillsDir string) *httptest.Server {
	mux := http.NewServeMux()

	// Build skills.json from real skills
	skillsJSONData := make([]map[string]interface{}, 0, len(realSkills))
	for _, s := range realSkills {
		skillsJSONData = append(skillsJSONData, map[string]interface{}{
			"name":        s.Name,
			"description": s.Description,
			"repository":  "local/skills",
			"author":      "NemesisBot",
			"tags":        []string{},
		})
	}

	// skills.json endpoint (mimics GitHub raw content path)
	mux.HandleFunc("/276793422/nemesisbot-skills/main/skills.json", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(skillsJSONData)
	})

	// Skill file endpoint - serve real SKILL.md files
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		path := r.URL.Path

		// Extract skill name from URL patterns:
		// /276793422/nemesisbot-skills/main/skills/{name}/SKILL.md
		// /{name}/main/SKILL.md
		var skillName string
		if strings.Contains(path, "/skills/") {
			parts := strings.Split(path, "/skills/")
			if len(parts) >= 2 {
				skillName = strings.TrimSuffix(parts[1], "/SKILL.md")
			}
		} else {
			parts := strings.Split(strings.TrimPrefix(path, "/"), "/")
			if len(parts) >= 1 {
				skillName = parts[0]
			}
		}

		if skillName == "" {
			http.NotFound(w, r)
			return
		}

		// Try to serve the real SKILL.md
		skillPath := filepath.Join(skillsDir, skillName, "SKILL.md")
		if _, err := os.Stat(skillPath); err != nil {
			// Try lowercase
			skillPath = filepath.Join(skillsDir, skillName, "skill.md")
		}

		data, err := os.ReadFile(skillPath)
		if err != nil {
			http.NotFound(w, r)
			return
		}

		w.Header().Set("Content-Type", "text/plain")
		w.Write(data)
	})

	return httptest.NewServer(mux)
}
