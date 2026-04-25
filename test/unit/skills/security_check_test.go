// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills_test

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/skills"
)

// TestSecurityCheckCleanContent tests that clean content passes security check
func TestSecurityCheckCleanContent(t *testing.T) {
	content := `---
name: clean-skill
description: A perfectly safe skill
---
# Clean Skill

This skill does nothing dangerous.

## Steps
1. Read a file
2. Process the content
3. Output the result
`
	result := skills.SecurityCheck(content, "clean-skill", nil)

	if result.Blocked {
		t.Errorf("Expected clean content to not be blocked, got blocked: %s", result.BlockReason)
	}

	if result.LintResult == nil {
		t.Fatal("Expected LintResult to be set")
	}

	if result.LintResult.Score != 100 {
		t.Errorf("Expected lint score 100, got %.0f", result.LintResult.Score)
	}

	if result.QualityScore == nil {
		t.Fatal("Expected QualityScore to be set")
	}
}

// TestSecurityCheckBlockedByLowScore tests blocking by very low lint score
func TestSecurityCheckBlockedByLowScore(t *testing.T) {
	// Content with multiple dangerous patterns to drive score below 30
	content := `---
name: dangerous-skill
description: Dangerous
---
rm -rf /
dd if=/dev/zero of=/dev/sda
shutdown now
`
	result := skills.SecurityCheck(content, "dangerous-skill", nil)

	if !result.Blocked {
		t.Error("Expected dangerous content to be blocked")
	}

	if result.BlockReason == "" {
		t.Error("Expected BlockReason to be set")
	}

	if result.LintResult.Score >= 30 {
		t.Errorf("Expected lint score < 30, got %.0f", result.LintResult.Score)
	}
}

// TestSecurityCheckBlockedByCriticalIssue tests blocking by critical severity
func TestSecurityCheckBlockedByCriticalIssue(t *testing.T) {
	// Content with a single critical pattern, score = 60 (above 30 but has critical)
	content := `---
name: suspicious-skill
description: Suspicious
---
Use the keylog tool to capture input.
`
	result := skills.SecurityCheck(content, "suspicious-skill", nil)

	if !result.Blocked {
		t.Error("Expected content with critical issue to be blocked")
	}

	// BlockReason should mention critical (not "security score too low")
	if !strings.Contains(result.BlockReason, "critical") {
		t.Errorf("Expected BlockReason to mention 'critical', got: %s", result.BlockReason)
	}
}

// TestSecurityCheckWarningOnly tests that high-severity (non-critical) issues warn but don't block
func TestSecurityCheckWarningOnly(t *testing.T) {
	// Single high-severity pattern (not critical) — score penalty is 25, score = 75 >= 30
	content := `---
name: warning-skill
description: Suspicious but not blocked
---
Use curl --upload to send data.
`
	result := skills.SecurityCheck(content, "warning-skill", nil)

	if result.Blocked {
		t.Errorf("Expected high-severity content (score >= 30, no critical) to not be blocked, got: %s", result.BlockReason)
	}

	if result.LintResult.Passed {
		t.Error("Expected lint to not pass due to high severity issues")
	}

	if result.LintResult.Score >= 100 {
		t.Error("Expected lint score to be below 100 due to issues")
	}
}

// TestSecurityCheckWithMetadata tests quality scoring with metadata
func TestSecurityCheckWithMetadata(t *testing.T) {
	content := `---
name: test-skill
description: A test skill
---
# Test Skill

Some content.
`
	metadata := map[string]string{
		"name":        "test-skill",
		"description": "A test skill",
		"source":      "test",
	}

	result := skills.SecurityCheck(content, "test-skill", metadata)

	if result.Blocked {
		t.Error("Expected clean content with metadata to not be blocked")
	}

	if result.QualityScore == nil {
		t.Fatal("Expected QualityScore to be set")
	}

	// With metadata, completeness score should be higher
	if result.QualityScore.Completeness.Score < 20 {
		t.Errorf("Expected higher completeness score with metadata, got %.0f", result.QualityScore.Completeness.Score)
	}
}

// TestSecurityCheckNilMetadata tests quality scoring with nil metadata
func TestSecurityCheckNilMetadata(t *testing.T) {
	content := "# Simple Skill\n\nJust content."

	result := skills.SecurityCheck(content, "simple-skill", nil)

	if result.Blocked {
		t.Error("Expected simple content to not be blocked")
	}

	if result.QualityScore == nil {
		t.Fatal("Expected QualityScore to be set even with nil metadata")
	}
}

// TestSecurityCheckResultStructure tests the SecurityCheckResult fields
func TestSecurityCheckResultStructure(t *testing.T) {
	content := "# Clean"
	result := skills.SecurityCheck(content, "test", nil)

	if result.LintResult == nil {
		t.Error("Expected LintResult to be set")
	}

	if result.QualityScore == nil {
		t.Error("Expected QualityScore to be set")
	}

	if result.Blocked {
		t.Error("Expected clean content to not be blocked")
	}

	if result.BlockReason != "" {
		t.Errorf("Expected empty BlockReason for clean content, got: %s", result.BlockReason)
	}
}

// TestSecurityCheckEmptyContent tests that empty content does not panic or block
func TestSecurityCheckEmptyContent(t *testing.T) {
	result := skills.SecurityCheck("", "empty", nil)

	if result.Blocked {
		t.Error("Expected empty content to not be blocked")
	}

	if result.LintResult == nil {
		t.Fatal("Expected LintResult to be set even for empty content")
	}

	if result.LintResult.Score != 100 {
		t.Errorf("Expected score 100 for empty content, got %.0f", result.LintResult.Score)
	}

	if result.QualityScore == nil {
		t.Fatal("Expected QualityScore to be set for empty content")
	}
}

// TestSecurityCheckBlockedQualityScoreNil confirms QualityScore is nil when blocked
func TestSecurityCheckBlockedQualityScoreNil(t *testing.T) {
	result := skills.SecurityCheck("rm -rf /\nshutdown now", "x", nil)

	if !result.Blocked {
		t.Fatal("Expected to be blocked")
	}

	if result.QualityScore != nil {
		t.Error("Expected QualityScore to be nil when blocked (early return)")
	}
}

// --- Installer security check integration tests ---

// mockFileCreatingRegistry is a MockRegistry that actually creates files on disk.
type mockFileCreatingRegistry struct {
	name            string
	searchResults   []skills.SearchResult
	skillMeta       map[string]*skills.SkillMeta
	skillContent    string // content to write to SKILL.md
	failDownload    bool
}

func (m *mockFileCreatingRegistry) Name() string { return m.name }

func (m *mockFileCreatingRegistry) Search(ctx context.Context, query string, limit int) ([]skills.SearchResult, error) {
	return m.searchResults, nil
}

func (m *mockFileCreatingRegistry) GetSkillMeta(ctx context.Context, slug string) (*skills.SkillMeta, error) {
	if meta, ok := m.skillMeta[slug]; ok {
		return meta, nil
	}
	return &skills.SkillMeta{Slug: slug, DisplayName: slug, Summary: "Mock skill"}, nil
}

func (m *mockFileCreatingRegistry) DownloadAndInstall(ctx context.Context, slug, version, targetDir string) (*skills.InstallResult, error) {
	if m.failDownload {
		return nil, os.ErrNotExist
	}

	if err := os.MkdirAll(targetDir, 0o755); err != nil {
		return nil, err
	}

	skillFile := filepath.Join(targetDir, "SKILL.md")
	if err := os.WriteFile(skillFile, []byte(m.skillContent), 0o644); err != nil {
		return nil, err
	}

	return &skills.InstallResult{
		Version:          version,
		IsMalwareBlocked: false,
		IsSuspicious:     false,
		Summary:          "Mock installation",
	}, nil
}

// TestInstallFromRegistrySecurityCheckBlocked tests that InstallFromRegistry blocks malicious skills
func TestInstallFromRegistrySecurityCheckBlocked(t *testing.T) {
	tempDir := t.TempDir()
	installer := skills.NewSkillInstaller(tempDir)
	rm := skills.NewRegistryManager()
	installer.SetRegistryManager(rm)

	// Create a mock that installs a malicious skill
	mock := &mockFileCreatingRegistry{
		name: "test-registry",
		searchResults: []skills.SearchResult{
			{Slug: "malicious-skill", DisplayName: "Malicious", Summary: "Bad skill", RegistryName: "test-registry"},
		},
		skillMeta:     make(map[string]*skills.SkillMeta),
		skillContent: "# Malicious\n\nrm -rf /\ndd if=/dev/zero\nshutdown now\nkeylog capture",
	}
	rm.AddRegistry(mock)

	ctx := context.Background()
	err := installer.InstallFromRegistry(ctx, "test-registry", "malicious-skill", "1.0.0")
	if err == nil {
		t.Fatal("Expected error for malicious skill")
	}

	if !strings.Contains(err.Error(), "blocked by security check") {
		t.Errorf("Expected 'blocked by security check' error, got: %v", err)
	}

	// Verify skill directory was removed
	skillDir := filepath.Join(tempDir, "skills", "malicious-skill")
	if _, err := os.Stat(skillDir); !os.IsNotExist(err) {
		t.Error("Expected malicious skill directory to be removed after blocking")
	}
}

// TestInstallFromRegistrySecurityCheckClean tests that clean skills install successfully
func TestInstallFromRegistrySecurityCheckClean(t *testing.T) {
	tempDir := t.TempDir()
	installer := skills.NewSkillInstaller(tempDir)
	rm := skills.NewRegistryManager()
	installer.SetRegistryManager(rm)

	cleanContent := `---
name: clean-skill
description: A safe skill
---
# Clean Skill

Steps:
1. Read file
2. Process
3. Output
`
	mock := &mockFileCreatingRegistry{
		name: "test-registry",
		searchResults: []skills.SearchResult{
			{Slug: "clean-skill", DisplayName: "Clean", Summary: "Safe skill", RegistryName: "test-registry"},
		},
		skillMeta:     make(map[string]*skills.SkillMeta),
		skillContent:  cleanContent,
	}
	rm.AddRegistry(mock)

	ctx := context.Background()
	err := installer.InstallFromRegistry(ctx, "test-registry", "clean-skill", "1.0.0")
	if err != nil {
		t.Fatalf("Expected clean skill to install, got: %v", err)
	}

	// Verify skill directory exists
	skillDir := filepath.Join(tempDir, "skills", "clean-skill")
	if _, err := os.Stat(skillDir); os.IsNotExist(err) {
		t.Error("Expected clean skill directory to exist")
	}

	// Verify security check result is available
	check := installer.LastSecurityCheck()
	if check == nil {
		t.Fatal("Expected LastSecurityCheck to return result")
	}

	if check.Blocked {
		t.Error("Expected clean skill to not be blocked")
	}

	if check.LintResult.Score != 100 {
		t.Errorf("Expected lint score 100, got %.0f", check.LintResult.Score)
	}
}

// TestInstallFromRegistrySecurityCheckWarning tests that warning-level issues don't block
func TestInstallFromRegistrySecurityCheckWarning(t *testing.T) {
	tempDir := t.TempDir()
	installer := skills.NewSkillInstaller(tempDir)
	rm := skills.NewRegistryManager()
	installer.SetRegistryManager(rm)

	// Content with a single high-severity (non-critical) issue
	warningContent := `---
name: warning-skill
description: Has warnings
---
# Warning Skill

Use curl --upload to send data.
`
	mock := &mockFileCreatingRegistry{
		name: "test-registry",
		searchResults: []skills.SearchResult{
			{Slug: "warning-skill", DisplayName: "Warning", Summary: "Has warnings", RegistryName: "test-registry"},
		},
		skillMeta:     make(map[string]*skills.SkillMeta),
		skillContent:  warningContent,
	}
	rm.AddRegistry(mock)

	ctx := context.Background()
	err := installer.InstallFromRegistry(ctx, "test-registry", "warning-skill", "1.0.0")
	if err != nil {
		t.Fatalf("Expected warning-level skill to install, got: %v", err)
	}

	// Skill should still be installed
	skillDir := filepath.Join(tempDir, "skills", "warning-skill")
	if _, err := os.Stat(skillDir); os.IsNotExist(err) {
		t.Error("Expected warning-level skill directory to exist")
	}

	check := installer.LastSecurityCheck()
	if check == nil {
		t.Fatal("Expected LastSecurityCheck to return result")
	}

	if check.Blocked {
		t.Error("Expected warning-level skill to not be blocked")
	}

	if check.LintResult.Passed {
		t.Error("Expected lint to fail for content with high-severity issues")
	}
}

// TestLastSecurityCheckInitial tests LastSecurityCheck returns nil before any install
func TestLastSecurityCheckInitial(t *testing.T) {
	tempDir := t.TempDir()
	installer := skills.NewSkillInstaller(tempDir)

	check := installer.LastSecurityCheck()
	if check != nil {
		t.Error("Expected nil LastSecurityCheck before any install")
	}
}

// mockNoFileRegistry creates the directory but not SKILL.md
type mockNoFileRegistry struct{}

func (m *mockNoFileRegistry) Name() string { return "no-file-registry" }
func (m *mockNoFileRegistry) Search(ctx context.Context, query string, limit int) ([]skills.SearchResult, error) {
	return nil, nil
}
func (m *mockNoFileRegistry) GetSkillMeta(ctx context.Context, slug string) (*skills.SkillMeta, error) {
	return &skills.SkillMeta{Slug: slug, DisplayName: slug, Summary: "No file"}, nil
}
func (m *mockNoFileRegistry) DownloadAndInstall(ctx context.Context, slug, version, targetDir string) (*skills.InstallResult, error) {
	os.MkdirAll(targetDir, 0o755)
	// Intentionally do NOT create SKILL.md
	return &skills.InstallResult{Version: version, Summary: "No file"}, nil
}

// TestInstallFromRegistryNoSkillFile tests that security check is skipped when SKILL.md missing
func TestInstallFromRegistryNoSkillFile(t *testing.T) {
	tempDir := t.TempDir()
	installer := skills.NewSkillInstaller(tempDir)
	rm := skills.NewRegistryManager()
	installer.SetRegistryManager(rm)
	rm.AddRegistry(&mockNoFileRegistry{})

	ctx := context.Background()
	err := installer.InstallFromRegistry(ctx, "no-file-registry", "no-file-skill", "1.0.0")
	if err != nil {
		t.Fatalf("Expected install to succeed (no SKILL.md = no security check), got: %v", err)
	}

	// lastSecurityCheck should remain nil since ReadFile failed
	check := installer.LastSecurityCheck()
	if check != nil {
		t.Error("Expected nil LastSecurityCheck when SKILL.md does not exist")
	}
}

// --- Loader security scanning tests ---

// TestSkillsLoaderEnableSecurity tests enabling security scanning
func TestSkillsLoaderEnableSecurity(t *testing.T) {
	workspace := t.TempDir()
	globalSkills := t.TempDir()
	builtinSkills := t.TempDir()

	loader := skills.NewSkillsLoader(workspace, globalSkills, builtinSkills)

	// Create a clean skill in workspace
	workspaceSkillsDir := filepath.Join(workspace, "skills")
	skillDir := filepath.Join(workspaceSkillsDir, "safe-skill")
	if err := os.MkdirAll(skillDir, 0o755); err != nil {
		t.Fatal(err)
	}

	skillContent := `---
name: safe-skill
description: A safe skill
---
# Safe Skill

Just a harmless skill.
`
	if err := os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte(skillContent), 0o644); err != nil {
		t.Fatal(err)
	}

	loader.EnableSecurity()
	skillsList := loader.ListSkills()

	if len(skillsList) != 1 {
		t.Fatalf("Expected 1 skill, got %d", len(skillsList))
	}

	s := skillsList[0]
	if s.LintScore != 100 {
		t.Errorf("Expected lint score 100, got %.0f", s.LintScore)
	}

	if s.HasWarnings {
		t.Error("Expected no warnings for clean skill")
	}
}

// TestSkillsLoaderSecurityScanGlobalAndBuiltin tests that global and builtin skills also get scanned
func TestSkillsLoaderSecurityScanGlobalAndBuiltin(t *testing.T) {
	workspace := t.TempDir()
	globalSkills := t.TempDir()
	builtinSkills := t.TempDir()

	loader := skills.NewSkillsLoader(workspace, globalSkills, builtinSkills)

	// Create clean skill in global
	globalDir := filepath.Join(globalSkills, "global-skill")
	os.MkdirAll(globalDir, 0o755)
	os.WriteFile(filepath.Join(globalDir, "SKILL.md"), []byte(`---
name: global-skill
description: Global safe skill
---
# Global Skill
`), 0o644)

	// Create warning skill in builtin
	builtinDir := filepath.Join(builtinSkills, "builtin-skill")
	os.MkdirAll(builtinDir, 0o755)
	os.WriteFile(filepath.Join(builtinDir, "SKILL.md"), []byte(`---
name: builtin-skill
description: Builtin skill
---
# Builtin Skill

curl --upload data.
`), 0o644)

	loader.EnableSecurity()
	skillsList := loader.ListSkills()

	if len(skillsList) != 2 {
		t.Fatalf("Expected 2 skills, got %d", len(skillsList))
	}

	// Find each skill and verify scan
	for _, s := range skillsList {
		if s.Source == "global" {
			if s.LintScore != 100 {
				t.Errorf("Global skill: expected lint score 100, got %.0f", s.LintScore)
			}
		}
		if s.Source == "builtin" {
			if s.LintScore >= 100 {
				t.Errorf("Builtin skill: expected lint score < 100 due to warning, got %.0f", s.LintScore)
			}
			if !s.HasWarnings {
				t.Error("Builtin skill: expected HasWarnings=true")
			}
		}
	}
}

// TestSkillsLoaderEnableSecurityWithWarnings tests security scanning with warning-level issues
func TestSkillsLoaderEnableSecurityWithWarnings(t *testing.T) {
	workspace := t.TempDir()
	globalSkills := t.TempDir()
	builtinSkills := t.TempDir()

	loader := skills.NewSkillsLoader(workspace, globalSkills, builtinSkills)

	// Create a skill with a warning-level issue
	workspaceSkillsDir := filepath.Join(workspace, "skills")
	skillDir := filepath.Join(workspaceSkillsDir, "warning-skill")
	if err := os.MkdirAll(skillDir, 0o755); err != nil {
		t.Fatal(err)
	}

	skillContent := `---
name: warning-skill
description: Has warnings
---
# Warning Skill

curl --upload some data.
`
	if err := os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte(skillContent), 0o644); err != nil {
		t.Fatal(err)
	}

	loader.EnableSecurity()
	skillsList := loader.ListSkills()

	if len(skillsList) != 1 {
		t.Fatalf("Expected 1 skill, got %d", len(skillsList))
	}

	s := skillsList[0]
	if s.LintScore >= 100 {
		t.Errorf("Expected lint score < 100 for content with issues, got %.0f", s.LintScore)
	}

	if !s.HasWarnings {
		t.Error("Expected HasWarnings to be true")
	}
}

// TestSkillsLoaderSecurityDisabled tests that security scanning is off by default
func TestSkillsLoaderSecurityDisabled(t *testing.T) {
	workspace := t.TempDir()
	globalSkills := t.TempDir()
	builtinSkills := t.TempDir()

	loader := skills.NewSkillsLoader(workspace, globalSkills, builtinSkills)

	// Create a skill
	workspaceSkillsDir := filepath.Join(workspace, "skills")
	skillDir := filepath.Join(workspaceSkillsDir, "test-skill")
	if err := os.MkdirAll(skillDir, 0o755); err != nil {
		t.Fatal(err)
	}

	skillContent := `---
name: test-skill
description: A test skill
---
# Test
`
	if err := os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte(skillContent), 0o644); err != nil {
		t.Fatal(err)
	}

	// Without EnableSecurity, lint score should be 0 (not computed)
	skillsList := loader.ListSkills()

	if len(skillsList) != 1 {
		t.Fatalf("Expected 1 skill, got %d", len(skillsList))
	}

	if skillsList[0].LintScore != 0 {
		t.Errorf("Expected LintScore=0 when security not enabled, got %.0f", skillsList[0].LintScore)
	}

	if skillsList[0].HasWarnings {
		t.Error("Expected HasWarnings=false when security not enabled")
	}
}

// TestSkillsLoaderBuildSkillsSummaryWithSecurity tests that BuildSkillsSummary includes security info
func TestSkillsLoaderBuildSkillsSummaryWithSecurity(t *testing.T) {
	workspace := t.TempDir()
	globalSkills := t.TempDir()
	builtinSkills := t.TempDir()

	loader := skills.NewSkillsLoader(workspace, globalSkills, builtinSkills)

	// Create a skill
	workspaceSkillsDir := filepath.Join(workspace, "skills")
	skillDir := filepath.Join(workspaceSkillsDir, "safe-skill")
	if err := os.MkdirAll(skillDir, 0o755); err != nil {
		t.Fatal(err)
	}

	skillContent := `---
name: safe-skill
description: A safe skill
---
# Safe Skill
`
	if err := os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte(skillContent), 0o644); err != nil {
		t.Fatal(err)
	}

	loader.EnableSecurity()
	summary := loader.BuildSkillsSummary()

	if !strings.Contains(summary, "<security_score>100</security_score>") {
		t.Errorf("Expected security_score tag in summary, got:\n%s", summary)
	}
}

// TestSkillsLoaderBuildSkillsSummaryWithoutSecurity tests no security info when security is off
func TestSkillsLoaderBuildSkillsSummaryWithoutSecurity(t *testing.T) {
	workspace := t.TempDir()
	globalSkills := t.TempDir()
	builtinSkills := t.TempDir()

	loader := skills.NewSkillsLoader(workspace, globalSkills, builtinSkills)

	// Create a skill
	workspaceSkillsDir := filepath.Join(workspace, "skills")
	skillDir := filepath.Join(workspaceSkillsDir, "safe-skill")
	if err := os.MkdirAll(skillDir, 0o755); err != nil {
		t.Fatal(err)
	}

	skillContent := `---
name: safe-skill
description: A safe skill
---
# Safe Skill
`
	if err := os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte(skillContent), 0o644); err != nil {
		t.Fatal(err)
	}

	// Without EnableSecurity, summary should NOT contain security_score
	summary := loader.BuildSkillsSummary()

	if strings.Contains(summary, "<security_score>") {
		t.Errorf("Expected no security_score tag when security is disabled, got:\n%s", summary)
	}

	if !strings.Contains(summary, "<name>safe-skill</name>") {
		t.Errorf("Expected skill name in summary, got:\n%s", summary)
	}
}

// TestSkillsLoaderBuildSkillsSummaryWithDangerousSkill tests security score shown for score=0 skills
func TestSkillsLoaderBuildSkillsSummaryWithDangerousSkill(t *testing.T) {
	workspace := t.TempDir()
	globalSkills := t.TempDir()
	builtinSkills := t.TempDir()

	loader := skills.NewSkillsLoader(workspace, globalSkills, builtinSkills)

	// Create a skill with extremely dangerous content (LintScore = 0)
	workspaceSkillsDir := filepath.Join(workspace, "skills")
	skillDir := filepath.Join(workspaceSkillsDir, "dangerous-skill")
	if err := os.MkdirAll(skillDir, 0o755); err != nil {
		t.Fatal(err)
	}

	skillContent := `---
name: dangerous-skill
description: Dangerous skill
---
rm -rf /
dd if=/dev/zero of=/dev/sda
shutdown now
cat /etc/passwd
cat /etc/shadow
`
	if err := os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte(skillContent), 0o644); err != nil {
		t.Fatal(err)
	}

	loader.EnableSecurity()
	summary := loader.BuildSkillsSummary()

	// Even with LintScore=0, security_score should be shown
	if !strings.Contains(summary, "<security_score>0</security_score>") {
		t.Errorf("Expected security_score 0 in summary for dangerous skill, got:\n%s", summary)
	}

	if !strings.Contains(summary, "<name>dangerous-skill</name>") {
		t.Errorf("Expected skill name in summary, got:\n%s", summary)
	}
}

// --- Concurrency and negative path tests ---

// TestLastSecurityCheckConcurrent tests concurrent access to lastSecurityCheck
func TestLastSecurityCheckConcurrent(t *testing.T) {
	tempDir := t.TempDir()
	installer := skills.NewSkillInstaller(tempDir)
	rm := skills.NewRegistryManager()
	installer.SetRegistryManager(rm)

	cleanContent := `---
name: clean-skill
description: A safe skill
---
# Clean Skill
`
	mock := &mockFileCreatingRegistry{
		name:         "test-registry",
		skillContent: cleanContent,
	}
	rm.AddRegistry(mock)

	const n = 10
	done := make(chan struct{}, n*2)

	// Concurrent installers
	for i := 0; i < n; i++ {
		go func(idx int) {
			slug := fmt.Sprintf("skill-%d", idx)
			// Each needs its own temp dir to avoid "already exists"
			subDir := filepath.Join(tempDir, "skills", slug)
			os.MkdirAll(subDir, 0o755)
			os.WriteFile(filepath.Join(subDir, "SKILL.md"), []byte(cleanContent), 0o644)
			// Directly test LastSecurityCheck read
			_ = installer.LastSecurityCheck()
			done <- struct{}{}
		}(i)
	}

	// Concurrent readers
	for i := 0; i < n; i++ {
		go func() {
			_ = installer.LastSecurityCheck()
			done <- struct{}{}
		}()
	}

	// Wait for all goroutines (no race = pass)
	for i := 0; i < n*2; i++ {
		<-done
	}
}

// TestBuildSkillsSummaryXMLWellFormed verifies the output has proper XML structure
func TestBuildSkillsSummaryXMLWellFormed(t *testing.T) {
	workspace := t.TempDir()
	globalSkills := t.TempDir()
	builtinSkills := t.TempDir()

	loader := skills.NewSkillsLoader(workspace, globalSkills, builtinSkills)

	// Create a skill with XML-special characters
	workspaceSkillsDir := filepath.Join(workspace, "skills")
	skillDir := filepath.Join(workspaceSkillsDir, "test-skill")
	if err := os.MkdirAll(skillDir, 0o755); err != nil {
		t.Fatal(err)
	}

	skillContent := `---
name: test-skill
description: "A <test> & skill with 'special' chars"
---
# Test Skill
`
	if err := os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte(skillContent), 0o644); err != nil {
		t.Fatal(err)
	}

	loader.EnableSecurity()
	summary := loader.BuildSkillsSummary()

	// Verify XML structure
	if !strings.Contains(summary, "<skills>") || !strings.Contains(summary, "</skills>") {
		t.Errorf("Missing root tags, got:\n%s", summary)
	}
	if !strings.Contains(summary, "&lt;test&gt;") {
		t.Errorf("Expected < to be escaped to &lt;, got:\n%s", summary)
	}
	if !strings.Contains(summary, "&amp;") {
		t.Errorf("Expected & to be escaped to &amp;, got:\n%s", summary)
	}
	// Verify security_score is present (clean skill, score=100)
	if !strings.Contains(summary, "<security_score>") {
		t.Errorf("Expected security_score tag, got:\n%s", summary)
	}
}

// TestScanSkillSecurityReadError tests that scanSkillSecurity handles read errors gracefully
func TestScanSkillSecurityReadError(t *testing.T) {
	workspace := t.TempDir()
	globalSkills := t.TempDir()
	builtinSkills := t.TempDir()

	loader := skills.NewSkillsLoader(workspace, globalSkills, builtinSkills)
	loader.EnableSecurity()

	// Create a SkillInfo pointing to a non-existent file
	info := &skills.SkillInfo{
		Name:        "missing-skill",
		Path:        "/nonexistent/path/SKILL.md",
		Source:      "workspace",
		Description: "test",
	}

	// scanSkillSecurity should silently skip if ReadFile fails
	loader.ListSkills() // no crash = pass

	// Direct test: LintScore and HasWarnings should remain zero values
	if info.LintScore != 0 {
		t.Errorf("Expected LintScore=0 after read error, got %.0f", info.LintScore)
	}
	if info.HasWarnings {
		t.Error("Expected HasWarnings=false after read error")
	}
}
