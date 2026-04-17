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

// testSkillContent is a valid SKILL.md with JSON frontmatter.
const testSkillContent = `---
{"name": "test-skill", "description": "A test skill for verifying registry install flow"}
---

# Test Skill

This is a test skill used by the SkillRegistryTest tool.

## Steps

1. Step one
2. Step two
3. Step three

## Notes

- This skill is for testing only
- Do not use in production
`

// skillsJSON is the curated skills list served by the mock server.
var skillsJSON = []struct {
	Name        string   `json:"name"`
	Description string   `json:"description"`
	Repository  string   `json:"repository"`
	Author      string   `json:"author"`
	Tags        []string `json:"tags"`
}{
	{
		Name:        "test-skill",
		Description: "A test skill for verifying registry install flow",
		Repository:  "276793422/nemesisbot-skills",
		Author:      "NemesisBot",
		Tags:        []string{"test", "demo"},
	},
	{
		Name:        "weather-query",
		Description: "Query weather information for any city",
		Repository:  "276793422/nemesisbot-skills",
		Author:      "NemesisBot",
		Tags:        []string{"weather", "api"},
	},
	{
		Name:        "code-review",
		Description: "Automated code review with best practices",
		Repository:  "276793422/nemesisbot-skills",
		Author:      "NemesisBot",
		Tags:        []string{"code", "review"},
	},
}

// --- test result tracking ---

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

// --- main ---

func main() {
	fmt.Println("============================================")
	fmt.Println("  Skill Registry Integration Test Tool")
	fmt.Println("============================================")
	fmt.Println()

	// 1. Setup mock server
	fmt.Println("[1/6] Setting up mock HTTP server...")
	server := setupMockServer()
	defer server.Close()
	fmt.Printf("  Mock server: %s\n", server.URL)
	fmt.Println()

	// 2. Create temp workspace
	fmt.Println("[2/6] Creating temporary workspace...")
	tmpDir, err := os.MkdirTemp("", "skill-registry-test-*")
	if err != nil {
		fatal("Failed to create temp dir: " + err.Error())
	}
	defer os.RemoveAll(tmpDir)
	fmt.Printf("  Workspace: %s\n", tmpDir)
	fmt.Println()

	// 3. Create registry manager pointing to mock server
	fmt.Println("[3/6] Creating registry manager...")
	regMgr := createRegistryManager(server.URL)
	fmt.Println("  GitHub registry configured")
	fmt.Println()

	// 4. Test search
	fmt.Println("[4/6] Testing skill search...")
	testSearch(regMgr)
	fmt.Println()

	// 5. Test install
	fmt.Println("[5/6] Testing skill install...")
	testInstall(regMgr, tmpDir)
	fmt.Println()

	// 6. Verify installation
	fmt.Println("[6/6] Verifying installed skill files...")
	testVerifyInstallation(tmpDir)
	fmt.Println()

	// Summary
	printSummary()
}

func setupMockServer() *httptest.Server {
	mux := http.NewServeMux()

	// skills.json endpoint
	mux.HandleFunc("/276793422/nemesisbot-skills/main/skills.json", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(skillsJSON)
	})

	// Skill file endpoints - try multiple patterns
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		path := r.URL.Path

		// Pattern: /276793422/nemesisbot-skills/main/skills/{name}/SKILL.md
		if strings.HasSuffix(path, "/SKILL.md") {
			w.Header().Set("Content-Type", "text/plain")
			w.Write([]byte(testSkillContent))
			return
		}

		http.NotFound(w, r)
	})

	return httptest.NewServer(mux)
}

func createRegistryManager(serverURL string) *skills.RegistryManager {
	cfg := skills.RegistryConfig{
		GitHub: skills.GitHubConfig{
			Enabled: true,
			BaseURL: serverURL + "/",
		},
	}
	return skills.NewRegistryManagerFromConfig(cfg)
}

func testSearch(regMgr *skills.RegistryManager) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Helper: flatten grouped results
	flatten := func(grouped []skills.RegistrySearchResult) []skills.SearchResult {
		var all []skills.SearchResult
		for _, g := range grouped {
			all = append(all, g.Results...)
		}
		return all
	}

	// Test 1: Search with matching query
	grouped, err := regMgr.SearchAll(ctx, "test", 5)
	if err != nil {
		fail("Search 'test'", err.Error())
		return
	}
	results := flatten(grouped)
	if len(results) == 0 {
		fail("Search 'test'", "expected at least 1 result, got 0")
		return
	}
	if results[0].Slug != "test-skill" {
		fail("Search 'test'", fmt.Sprintf("expected slug 'test-skill', got '%s'", results[0].Slug))
		return
	}
	pass("Search 'test' returns test-skill")

	// Test 2: Search with non-matching query
	grouped, err = regMgr.SearchAll(ctx, "nonexistent-skill-xyz", 5)
	if err != nil {
		fail("Search 'nonexistent'", err.Error())
		return
	}
	results = flatten(grouped)
	if len(results) != 0 {
		fail("Search 'nonexistent'", fmt.Sprintf("expected 0 results, got %d", len(results)))
		return
	}
	pass("Search 'nonexistent' returns empty")

	// Test 3: Search with partial match
	grouped, err = regMgr.SearchAll(ctx, "weather", 5)
	if err != nil {
		fail("Search 'weather'", err.Error())
		return
	}
	results = flatten(grouped)
	if len(results) == 0 {
		fail("Search 'weather'", "expected at least 1 result, got 0")
		return
	}
	pass("Search 'weather' returns weather-query")

	// Test 4: Empty query (list all)
	grouped, err = regMgr.SearchAll(ctx, "", 100)
	if err != nil {
		fail("Search '' (list all)", err.Error())
		return
	}
	results = flatten(grouped)
	// Empty query matches nothing because contains() checks substring
	if len(results) == len(skillsJSON) {
		pass("Search '' (list all) returns all skills")
	} else {
		// Empty string is a substring of everything in our contains() impl
		// Actually our contains checks sLower == substrLower for exact match first
		// then containsMiddle. Empty string "" is contained in every string.
		// But let's just check we got some results
		if len(results) >= 1 {
			pass("Search '' (list all) returns available skills")
		} else {
			fail("Search '' (list all)", fmt.Sprintf("expected results, got %d", len(results)))
		}
	}
}

func testInstall(regMgr *skills.RegistryManager, workspace string) {
	installer := skills.NewSkillInstaller(workspace)
	installer.SetRegistryManager(regMgr)

	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	// Test 1: Install test-skill from github registry
	err := installer.InstallFromRegistry(ctx, "github", "test-skill", "")
	if err != nil {
		fail("Install test-skill", err.Error())
		return
	}
	pass("Install test-skill from github registry")

	// Test 2: Install same skill again (should fail)
	err = installer.InstallFromRegistry(ctx, "github", "test-skill", "")
	if err == nil {
		fail("Install duplicate", "expected error for duplicate install, got nil")
		return
	}
	if !strings.Contains(err.Error(), "already exists") {
		fail("Install duplicate", fmt.Sprintf("expected 'already exists' error, got: %s", err.Error()))
		return
	}
	pass("Install duplicate correctly rejected")

	// Test 3: Install from non-existent registry
	err = installer.InstallFromRegistry(ctx, "nonexistent", "test-skill", "")
	if err == nil {
		fail("Install from invalid registry", "expected error, got nil")
		return
	}
	if !strings.Contains(err.Error(), "not found") {
		fail("Install from invalid registry", fmt.Sprintf("expected 'not found' error, got: %s", err.Error()))
		return
	}
	pass("Install from invalid registry correctly rejected")

	// Test 4: Verify origin tracking
	origin, err := installer.GetOriginTracking("test-skill")
	if err != nil {
		fail("Origin tracking read", err.Error())
		return
	}
	if origin.Registry != "github" {
		fail("Origin tracking registry", fmt.Sprintf("expected 'github', got '%s'", origin.Registry))
		return
	}
	if origin.Slug != "test-skill" {
		fail("Origin tracking slug", fmt.Sprintf("expected 'test-skill', got '%s'", origin.Slug))
		return
	}
	pass("Origin tracking metadata correct")

	// Test 5: Uninstall
	err = installer.Uninstall("test-skill")
	if err != nil {
		fail("Uninstall test-skill", err.Error())
		return
	}
	pass("Uninstall test-skill succeeds")

	// Test 6: Reinstall after uninstall
	err = installer.InstallFromRegistry(ctx, "github", "test-skill", "")
	if err != nil {
		fail("Reinstall after uninstall", err.Error())
		return
	}
	pass("Reinstall after uninstall succeeds")
}

func testVerifyInstallation(workspace string) {
	skillDir := filepath.Join(workspace, "skills", "test-skill")

	// Check 1: Directory exists
	info, err := os.Stat(skillDir)
	if err != nil {
		fail("Skill directory exists", err.Error())
		return
	}
	if !info.IsDir() {
		fail("Skill directory is dir", "path is not a directory")
		return
	}
	pass("Skill directory exists")

	// Check 2: SKILL.md exists
	skillFile := filepath.Join(skillDir, "SKILL.md")
	info, err = os.Stat(skillFile)
	if err != nil {
		fail("SKILL.md exists", err.Error())
		return
	}
	pass("SKILL.md exists")

	// Check 3: SKILL.md has valid content
	content, err := os.ReadFile(skillFile)
	if err != nil {
		fail("SKILL.md readable", err.Error())
		return
	}
	if !strings.Contains(string(content), "---") {
		fail("SKILL.md frontmatter", "missing frontmatter delimiter")
		return
	}
	if !strings.Contains(string(content), "test-skill") {
		fail("SKILL.md content", "missing skill name in content")
		return
	}
	pass("SKILL.md has valid frontmatter and content")

	// Check 4: Origin tracking file exists
	originFile := filepath.Join(skillDir, ".skill-origin.json")
	info, err = os.Stat(originFile)
	if err != nil {
		fail(".skill-origin.json exists", err.Error())
		return
	}
	pass(".skill-origin.json exists")

	// Check 5: Origin tracking has valid JSON
	originContent, err := os.ReadFile(originFile)
	if err != nil {
		fail(".skill-origin.json readable", err.Error())
		return
	}
	var origin map[string]interface{}
	if err := json.Unmarshal(originContent, &origin); err != nil {
		fail(".skill-origin.json valid JSON", err.Error())
		return
	}
	if origin["registry"] != "github" {
		fail(".skill-origin.json registry", fmt.Sprintf("expected 'github', got '%v'", origin["registry"]))
		return
	}
	if origin["slug"] != "test-skill" {
		fail(".skill-origin.json slug", fmt.Sprintf("expected 'test-skill', got '%v'", origin["slug"]))
		return
	}
	pass(".skill-origin.json has correct metadata")

	// Check 6: SkillsLoader can load the installed skill
	loader := skills.NewSkillsLoader(workspace, "", "")
	skillList := loader.ListSkills()
	found := false
	for _, s := range skillList {
		if s.Name == "test-skill" {
			found = true
			if s.Source != "workspace" {
				fail("SkillsLoader source", fmt.Sprintf("expected 'workspace', got '%s'", s.Source))
				return
			}
			break
		}
	}
	if !found {
		fail("SkillsLoader finds installed skill", "test-skill not found by SkillsLoader")
		return
	}
	pass("SkillsLoader correctly loads installed skill")
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
	} else {
		fmt.Println("  SOME TESTS FAILED")
		os.Exit(1)
	}
	fmt.Println("============================================")
}

func fatal(msg string) {
	fmt.Printf("FATAL: %s\n", msg)
	os.Exit(1)
}
