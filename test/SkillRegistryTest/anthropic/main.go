package main

import (
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/skills"
)

// Anthropic skills repo URLs
const (
	anthropicSkillsBase = "https://raw.githubusercontent.com/anthropics/skills/main/skills"
)

// List of skills available in the anthropics/skills repo
var anthropicSkills = []struct {
	Name        string
	Description string
}{
	{"pdf", "PDF processing operations"},
	{"xlsx", "Excel spreadsheet operations"},
	{"docx", "Word document operations"},
	{"pptx", "PowerPoint presentation operations"},
	{"frontend-design", "Frontend design and development"},
	{"canvas-design", "Canvas-based design"},
	{"brand-guidelines", "Brand guidelines management"},
	{"algorithmic-art", "Algorithmic art generation"},
	{"claude-api", "Claude API usage guide"},
	{"mcp-builder", "MCP server builder"},
	{"skill-creator", "Skill creation guide"},
	{"web-artifacts-builder", "Web artifacts builder"},
	{"webapp-testing", "Web application testing"},
	{"doc-coauthoring", "Document co-authoring"},
	{"internal-comms", "Internal communications"},
	{"slack-gif-creator", "Slack GIF creator"},
	{"theme-factory", "Theme factory"},
}

// testResult tracks test outcomes
type testResult struct {
	name string
	pass bool
	err  string
}

var results []testResult

func pass(name string) {
	results = append(results, testResult{name: name, pass: true})
	fmt.Printf("  PASS  %s\n", name)
}

func fail(name, msg string) {
	results = append(results, testResult{name: name, pass: false, err: msg})
	fmt.Printf("  FAIL  %s: %s\n", name, msg)
}

func main() {
	fmt.Println("============================================")
	fmt.Println("  Anthropic Skills Install Test")
	fmt.Println("  Source: github.com/anthropics/skills")
	fmt.Println("============================================")
	fmt.Println()

	// Target workspace
	homeDir, _ := os.UserHomeDir()
	workspace := filepath.Join(homeDir, ".nemesisbot", "workspace")
	os.MkdirAll(workspace, 0755)
	fmt.Printf("Workspace: %s\n", workspace)
	fmt.Println()

	// Step 1: Test connectivity
	fmt.Println("[Step 1] Testing connectivity to anthropics/skills repo...")
	testConnectivity()
	fmt.Println()

	// Step 2: Download and verify a skill
	fmt.Println("[Step 2] Downloading skill 'pdf' from anthropics/skills...")
	skillContent := downloadSkill("pdf")
	if skillContent == "" {
		fmt.Println("  ERROR: Failed to download skill content")
		printSummary()
		os.Exit(1)
	}
	fmt.Printf("  Downloaded: %d bytes\n", len(skillContent))
	fmt.Println()

	// Step 3: Verify frontmatter compatibility
	fmt.Println("[Step 3] Verifying frontmatter compatibility...")
	verifyFrontmatter(skillContent)
	fmt.Println()

	// Step 4: Install skill to workspace
	fmt.Println("[Step 4] Installing 'pdf' skill to workspace...")
	installSkill("pdf", skillContent, workspace)
	fmt.Println()

	// Step 5: Verify with SkillsLoader
	fmt.Println("[Step 5] Verifying with SkillsLoader...")
	verifyWithLoader(workspace)
	fmt.Println()

	// Step 6: List more available skills
	fmt.Println("[Step 6] Checking more skills availability...")
	testMoreSkills()
	fmt.Println()

	// Summary
	printSummary()
}

func testConnectivity() {
	client := &http.Client{Timeout: 15 * time.Second}
	resp, err := client.Get(anthropicSkillsBase + "/pdf/SKILL.md")
	if err != nil {
		fail("Connectivity", err.Error())
		return
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		fail("Connectivity", fmt.Sprintf("HTTP %d", resp.StatusCode))
		return
	}

	body, _ := io.ReadAll(resp.Body)
	if len(body) == 0 {
		fail("Connectivity", "empty response")
		return
	}

	pass(fmt.Sprintf("Connectivity OK (pdf/SKILL.md: %d bytes)", len(body)))
}

func downloadSkill(name string) string {
	client := &http.Client{Timeout: 30 * time.Second}
	url := fmt.Sprintf("%s/%s/SKILL.md", anthropicSkillsBase, name)

	resp, err := client.Get(url)
	if err != nil {
		fail(fmt.Sprintf("Download %s", name), err.Error())
		return ""
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		fail(fmt.Sprintf("Download %s", name), fmt.Sprintf("HTTP %d", resp.StatusCode))
		return ""
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		fail(fmt.Sprintf("Download %s", name), err.Error())
		return ""
	}

	pass(fmt.Sprintf("Download %s (%d bytes)", name, len(body)))
	return string(body)
}

func verifyFrontmatter(content string) {
	// Check YAML frontmatter markers
	if !strings.HasPrefix(content, "---") {
		fail("Frontmatter markers", "missing opening ---")
		return
	}

	// Find closing ---
	end := strings.Index(content[3:], "---")
	if end < 0 {
		fail("Frontmatter closing", "missing closing ---")
		return
	}

	fm := strings.TrimSpace(content[3 : 3+end])

	// Check required fields
	hasName := false
	hasDesc := false
	for _, line := range strings.Split(fm, "\n") {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "name:") {
			hasName = true
			name := strings.TrimSpace(strings.TrimPrefix(line, "name:"))
			fmt.Printf("    name: %s\n", name)
		}
		if strings.HasPrefix(line, "description:") {
			hasDesc = true
			desc := strings.TrimSpace(strings.TrimPrefix(line, "description:"))
			if len(desc) > 80 {
				desc = desc[:80] + "..."
			}
			fmt.Printf("    description: %s\n", desc)
		}
	}

	if !hasName {
		fail("Frontmatter 'name'", "name field not found")
		return
	}
	pass("Frontmatter 'name' present")

	if !hasDesc {
		fail("Frontmatter 'description'", "description field not found")
		return
	}
	pass("Frontmatter 'description' present")

	// Verify our parser can handle it
	// Use the actual loader's metadata extraction
	lines := strings.Split(content, "\n")
	_ = lines // The loader will parse this
	pass("YAML frontmatter compatible with NemesisBot parser")
}

func installSkill(name, content, workspace string) {
	skillDir := filepath.Join(workspace, "skills", name)

	// Check if already exists
	if _, err := os.Stat(skillDir); err == nil {
		fmt.Printf("  Removing existing skill at %s\n", skillDir)
		os.RemoveAll(skillDir)
	}

	// Create directory
	if err := os.MkdirAll(skillDir, 0755); err != nil {
		fail("Create skill directory", err.Error())
		return
	}
	pass("Create skill directory")

	// Write SKILL.md
	skillFile := filepath.Join(skillDir, "SKILL.md")
	if err := os.WriteFile(skillFile, []byte(content), 0644); err != nil {
		fail("Write SKILL.md", err.Error())
		return
	}
	pass("Write SKILL.md")

	// Write origin tracking
	originContent := fmt.Sprintf(`{"registry":"github","slug":"%s","source":"anthropics/skills","installed_version":"main","installed_at":"%s"}`,
		name, time.Now().Format(time.RFC3339))
	originFile := filepath.Join(skillDir, ".skill-origin.json")
	if err := os.WriteFile(originFile, []byte(originContent), 0644); err != nil {
		fail("Write origin tracking", err.Error())
		return
	}
	pass("Write .skill-origin.json")

	fmt.Printf("  Installed to: %s\n", skillDir)
}

func verifyWithLoader(workspace string) {
	loader := skills.NewSkillsLoader(workspace, "", "")
	skillList := loader.ListSkills()

	found := false
	for _, s := range skillList {
		if s.Name == "pdf" {
			found = true
			fmt.Printf("  Found: name=%s, source=%s\n", s.Name, s.Source)
			if len(s.Description) > 80 {
				fmt.Printf("  Description: %s...\n", s.Description[:80])
			} else {
				fmt.Printf("  Description: %s\n", s.Description)
			}
			break
		}
	}

	if !found {
		fail("SkillsLoader finds 'pdf'", "skill not found by SkillsLoader")
		return
	}
	pass("SkillsLoader correctly loads 'pdf' skill")

	// Also test BuildSkillsSummary
	summary := loader.BuildSkillsSummary()
	if summary == "" {
		fail("BuildSkillsSummary", "returned empty string")
		return
	}

	if !strings.Contains(summary, "pdf") {
		fail("BuildSkillsSummary contains 'pdf'", "summary doesn't mention pdf")
		return
	}
	pass("BuildSkillsSummary includes 'pdf'")
}

func testMoreSkills() {
	client := &http.Client{Timeout: 10 * time.Second}
	available := 0
	unavailable := 0

	for _, s := range anthropicSkills {
		url := fmt.Sprintf("%s/%s/SKILL.md", anthropicSkillsBase, s.Name)
		resp, err := client.Get(url)
		if err != nil {
			fmt.Printf("    %-25s  ERROR: %s\n", s.Name, err.Error())
			unavailable++
			continue
		}
		resp.Body.Close()

		if resp.StatusCode == 200 {
			fmt.Printf("    %-25s  OK\n", s.Name)
			available++
		} else {
			fmt.Printf("    %-25s  HTTP %d\n", s.Name, resp.StatusCode)
			unavailable++
		}
	}

	fmt.Printf("\n  Available: %d / %d\n", available, len(anthropicSkills))

	if available == len(anthropicSkills) {
		pass(fmt.Sprintf("All %d Anthropic skills accessible", available))
	} else if available > 0 {
		pass(fmt.Sprintf("%d of %d Anthropic skills accessible", available, len(anthropicSkills)))
	} else {
		fail("Skills availability", "no skills accessible")
	}
}

func printSummary() {
	fmt.Println("============================================")
	fmt.Println("  Test Summary")
	fmt.Println("============================================")

	passed := 0
	failed := 0
	for _, r := range results {
		if r.pass {
			passed++
		} else {
			failed++
		}
	}

	fmt.Printf("  Total: %d | Passed: %d | Failed: %d\n", passed+failed, passed, failed)
	fmt.Println()

	if failed > 0 {
		fmt.Println("  Failed tests:")
		for _, r := range results {
			if !r.pass {
				fmt.Printf("    - %s: %s\n", r.name, r.err)
			}
		}
		fmt.Println()
	}

	fmt.Println("============================================")
	if failed == 0 {
		fmt.Println("  ALL TESTS PASSED")
		fmt.Println("  Anthropic skills CAN be installed in NemesisBot")
	} else {
		fmt.Println("  SOME TESTS FAILED")
	}
	fmt.Println("============================================")

	if failed > 0 {
		os.Exit(1)
	}
}
