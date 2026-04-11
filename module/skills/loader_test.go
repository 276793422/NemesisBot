// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestNewSkillsLoader(t *testing.T) {
	tests := []struct {
		name          string
		workspace     string
		globalSkills  string
		builtinSkills string
	}{
		{
			name:          "all paths provided",
			workspace:     "/workspace",
			globalSkills:  "/global",
			builtinSkills: "/builtin",
		},
		{
			name:          "empty paths",
			workspace:     "",
			globalSkills:  "",
			builtinSkills: "",
		},
		{
			name:          "mixed empty and non-empty",
			workspace:     "/workspace",
			globalSkills:  "",
			builtinSkills: "/builtin",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			sl := NewSkillsLoader(tt.workspace, tt.globalSkills, tt.builtinSkills)

			if sl.workspace != tt.workspace {
				t.Errorf("expected workspace %q, got %q", tt.workspace, sl.workspace)
			}
			if sl.globalSkills != tt.globalSkills {
				t.Errorf("expected globalSkills %q, got %q", tt.globalSkills, sl.globalSkills)
			}
			if sl.builtinSkills != tt.builtinSkills {
				t.Errorf("expected builtinSkills %q, got %q", tt.builtinSkills, sl.builtinSkills)
			}

			// Check workspaceSkills is constructed correctly
			expectedWorkspaceSkills := filepath.Join(tt.workspace, "skills")
			if sl.workspaceSkills != expectedWorkspaceSkills {
				t.Errorf("expected workspaceSkills %q, got %q", expectedWorkspaceSkills, sl.workspaceSkills)
			}
		})
	}
}

func TestSkillInfoValidate(t *testing.T) {
	tests := []struct {
		name        string
		info        SkillInfo
		wantErr     bool
		errContains []string
	}{
		{
			name: "valid skill",
			info: SkillInfo{
				Name:        "test-skill",
				Description: "A test skill",
			},
			wantErr: false,
		},
		{
			name: "empty name",
			info: SkillInfo{
				Name:        "",
				Description: "A test skill",
			},
			wantErr:     true,
			errContains: []string{"name is required"},
		},
		{
			name: "name too long",
			info: SkillInfo{
				Name:        strings.Repeat("a", 65),
				Description: "A test skill",
			},
			wantErr:     true,
			errContains: []string{"name exceeds"},
		},
		{
			name: "invalid name pattern - spaces",
			info: SkillInfo{
				Name:        "test skill",
				Description: "A test skill",
			},
			wantErr:     true,
			errContains: []string{"alphanumeric"},
		},
		{
			name: "invalid name pattern - special chars",
			info: SkillInfo{
				Name:        "test@skill",
				Description: "A test skill",
			},
			wantErr:     true,
			errContains: []string{"alphanumeric"},
		},
		{
			name: "invalid name pattern - starts with hyphen",
			info: SkillInfo{
				Name:        "-test-skill",
				Description: "A test skill",
			},
			wantErr:     true,
			errContains: []string{"alphanumeric"},
		},
		{
			name: "invalid name pattern - ends with hyphen",
			info: SkillInfo{
				Name:        "test-skill-",
				Description: "A test skill",
			},
			wantErr:     true,
			errContains: []string{"alphanumeric"},
		},
		{
			name: "valid name with multiple hyphens",
			info: SkillInfo{
				Name:        "test-skill-name",
				Description: "A test skill",
			},
			wantErr: false,
		},
		{
			name: "empty description",
			info: SkillInfo{
				Name:        "test-skill",
				Description: "",
			},
			wantErr:     true,
			errContains: []string{"description is required"},
		},
		{
			name: "description too long",
			info: SkillInfo{
				Name:        "test-skill",
				Description: strings.Repeat("a", 1025),
			},
			wantErr:     true,
			errContains: []string{"description exceeds"},
		},
		{
			name: "both name and description invalid",
			info: SkillInfo{
				Name:        "",
				Description: "",
			},
			wantErr:     true,
			errContains: []string{"name is required", "description is required"},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := tt.info.validate()
			if (err != nil) != tt.wantErr {
				t.Errorf("validate() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if err != nil {
				for _, expected := range tt.errContains {
					if !strings.Contains(err.Error(), expected) {
						t.Errorf("error message should contain %q, got %q", expected, err.Error())
					}
				}
			}
		})
	}
}

func TestSkillsLoader_ListSkills(t *testing.T) {
	// Create temporary directories for testing
	tempDir := t.TempDir()
	workspaceSkills := filepath.Join(tempDir, "workspace", "skills")
	globalSkills := filepath.Join(tempDir, "global", "skills")
	builtinSkills := filepath.Join(tempDir, "builtin", "skills")

	// Create test skills
	setupSkill := func(base, name, content string) {
		skillDir := filepath.Join(base, name)
		if err := os.MkdirAll(skillDir, 0o755); err != nil {
			t.Fatalf("failed to create skill directory: %v", err)
		}
		skillFile := filepath.Join(skillDir, "SKILL.md")
		if err := os.WriteFile(skillFile, []byte(content), 0o644); err != nil {
			t.Fatalf("failed to write skill file: %v", err)
		}
	}

	tests := []struct {
		name        string
		setup       func()
		wantCount   int
		wantSources map[string]string
		wantNames   []string
	}{
		{
			name: "no skills",
			setup: func() {
				// No skills created
			},
			wantCount:   0,
			wantSources: nil,
		},
		{
			name: "workspace skill only",
			setup: func() {
				// Use YAML frontmatter format
				setupSkill(workspaceSkills, "skill1", `---
name: skill1
description: Workspace skill
---
This is the skill content.`)
			},
			wantCount:   1,
			wantSources: map[string]string{"skill1": "workspace"},
			wantNames:   []string{"skill1"},
		},
		{
			name: "global skill only",
			setup: func() {
				setupSkill(globalSkills, "skill2", `---
name: skill2
description: Global skill
---
Content`)
			},
			wantCount:   1,
			wantSources: map[string]string{"skill2": "global"},
			wantNames:   []string{"skill2"},
		},
		{
			name: "builtin skill only",
			setup: func() {
				setupSkill(builtinSkills, "skill3", `---
name: skill3
description: Builtin skill
---
Content`)
			},
			wantCount:   1,
			wantSources: map[string]string{"skill3": "builtin"},
			wantNames:   []string{"skill3"},
		},
		{
			name: "workspace overrides global",
			setup: func() {
				setupSkill(workspaceSkills, "skill1", `---
name: skill1
description: Workspace
---
Content`)
				setupSkill(globalSkills, "skill1", `---
name: skill1
description: Global
---
Content`)
			},
			wantCount:   1,
			wantSources: map[string]string{"skill1": "workspace"},
			wantNames:   []string{"skill1"},
		},
		{
			name: "workspace and global overrides builtin",
			setup: func() {
				setupSkill(workspaceSkills, "skill1", `---
name: skill1
description: Workspace
---
Content`)
				setupSkill(globalSkills, "skill2", `---
name: skill2
description: Global
---
Content`)
				setupSkill(builtinSkills, "skill1", `---
name: skill1
description: Builtin
---
Content`)
				setupSkill(builtinSkills, "skill2", `---
name: skill2
description: Builtin
---
Content`)
				setupSkill(builtinSkills, "skill3", `---
name: skill3
description: Builtin
---
Content`)
			},
			wantCount:   3,
			wantSources: map[string]string{"skill1": "workspace", "skill2": "global", "skill3": "builtin"},
			wantNames:   []string{"skill1", "skill2", "skill3"},
		},
		{
			name: "invalid skill is skipped",
			setup: func() {
				setupSkill(workspaceSkills, "valid-skill", `---
name: valid-skill
description: A valid skill
---
Content`)
				setupSkill(workspaceSkills, "invalid skill", `---
name: invalid skill
---
Missing description`)
			},
			wantCount:   1,
			wantSources: map[string]string{"valid-skill": "workspace"},
			wantNames:   []string{"valid-skill"},
		},
		{
			name: "skill without SKILL.md is skipped",
			setup: func() {
				skillDir := filepath.Join(workspaceSkills, "noskill")
				if err := os.MkdirAll(skillDir, 0o755); err != nil {
					t.Fatalf("failed to create directory: %v", err)
				}
			},
			wantCount: 0,
		},
		{
			name: "file instead of directory is skipped",
			setup: func() {
				if err := os.MkdirAll(workspaceSkills, 0o755); err != nil {
					t.Fatalf("failed to create directory: %v", err)
				}
				filePath := filepath.Join(workspaceSkills, "notadir")
				if err := os.WriteFile(filePath, []byte("not a directory"), 0o644); err != nil {
					t.Fatalf("failed to create file: %v", err)
				}
			},
			wantCount: 0,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Clean up directories before each test
			os.RemoveAll(workspaceSkills)
			os.RemoveAll(globalSkills)
			os.RemoveAll(builtinSkills)

			tt.setup()

			sl := NewSkillsLoader(filepath.Join(tempDir, "workspace"), globalSkills, builtinSkills)
			skills := sl.ListSkills()

			if len(skills) != tt.wantCount {
				t.Errorf("ListSkills() returned %d skills, want %d", len(skills), tt.wantCount)
			}

			for skillName, wantSource := range tt.wantSources {
				found := false
				for _, s := range skills {
					if s.Name == skillName && s.Source == wantSource {
						found = true
						break
					}
				}
				if !found {
					t.Errorf("Expected skill %q from source %q not found", skillName, wantSource)
				}
			}

			for _, wantName := range tt.wantNames {
				found := false
				for _, s := range skills {
					if s.Name == wantName {
						found = true
						break
					}
				}
				if !found {
					t.Errorf("Expected skill %q not found", wantName)
				}
			}
		})
	}
}

func TestSkillsLoader_LoadSkill(t *testing.T) {
	tempDir := t.TempDir()
	workspaceSkills := filepath.Join(tempDir, "workspace", "skills")
	globalSkills := filepath.Join(tempDir, "global", "skills")
	builtinSkills := filepath.Join(tempDir, "builtin", "skills")

	setupSkill := func(base, name, content string) {
		skillDir := filepath.Join(base, name)
		if err := os.MkdirAll(skillDir, 0o755); err != nil {
			t.Fatalf("failed to create skill directory: %v", err)
		}
		skillFile := filepath.Join(skillDir, "SKILL.md")
		if err := os.WriteFile(skillFile, []byte(content), 0o644); err != nil {
			t.Fatalf("failed to write skill file: %v", err)
		}
	}

	tests := []struct {
		name        string
		setup       func()
		skillName   string
		wantFound   bool
		wantContent string
	}{
		{
			name:      "skill not found",
			setup:     func() {},
			skillName: "nonexistent",
			wantFound: false,
		},
		{
			name: "load from workspace",
			setup: func() {
				setupSkill(workspaceSkills, "skill1", "Workspace content")
			},
			skillName:   "skill1",
			wantFound:   true,
			wantContent: "Workspace content",
		},
		{
			name: "workspace overrides global",
			setup: func() {
				setupSkill(workspaceSkills, "skill1", "Workspace content")
				setupSkill(globalSkills, "skill1", "Global content")
			},
			skillName:   "skill1",
			wantFound:   true,
			wantContent: "Workspace content",
		},
		{
			name: "load from global when workspace doesn't exist",
			setup: func() {
				setupSkill(globalSkills, "skill2", "Global content")
			},
			skillName:   "skill2",
			wantFound:   true,
			wantContent: "Global content",
		},
		{
			name: "load from builtin when workspace and global don't exist",
			setup: func() {
				setupSkill(builtinSkills, "skill3", "Builtin content")
			},
			skillName:   "skill3",
			wantFound:   true,
			wantContent: "Builtin content",
		},
		{
			name: "strip frontmatter",
			setup: func() {
				setupSkill(workspaceSkills, "skill4", `---
name: skill4
description: Test skill
---
Actual content`)
			},
			skillName:   "skill4",
			wantFound:   true,
			wantContent: "Actual content",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Clean up directories before each test
			os.RemoveAll(workspaceSkills)
			os.RemoveAll(globalSkills)
			os.RemoveAll(builtinSkills)

			tt.setup()

			sl := NewSkillsLoader(filepath.Join(tempDir, "workspace"), globalSkills, builtinSkills)
			content, found := sl.LoadSkill(tt.skillName)

			if found != tt.wantFound {
				t.Errorf("LoadSkill() found = %v, want %v", found, tt.wantFound)
				return
			}

			if found && content != tt.wantContent {
				t.Errorf("LoadSkill() content = %q, want %q", content, tt.wantContent)
			}
		})
	}
}

func TestSkillsLoader_LoadSkillsForContext(t *testing.T) {
	tempDir := t.TempDir()
	workspaceSkills := filepath.Join(tempDir, "workspace", "skills")

	setupSkill := func(name, content string) {
		skillDir := filepath.Join(workspaceSkills, name)
		if err := os.MkdirAll(skillDir, 0o755); err != nil {
			t.Fatalf("failed to create skill directory: %v", err)
		}
		skillFile := filepath.Join(skillDir, "SKILL.md")
		if err := os.WriteFile(skillFile, []byte(content), 0o644); err != nil {
			t.Fatalf("failed to write skill file: %v", err)
		}
	}

	tests := []struct {
		name       string
		setup      func()
		skillNames []string
		wantCount  int
		wantEmpty  bool
	}{
		{
			name:       "empty skill list",
			setup:      func() {},
			skillNames: []string{},
			wantEmpty:  true,
		},
		{
			name:       "nil skill list",
			setup:      func() {},
			skillNames: nil,
			wantEmpty:  true,
		},
		{
			name: "single skill",
			setup: func() {
				setupSkill("skill1", "Content 1")
			},
			skillNames: []string{"skill1"},
			wantCount:  1,
		},
		{
			name: "multiple skills",
			setup: func() {
				setupSkill("skill1", "Content 1")
				setupSkill("skill2", "Content 2")
				setupSkill("skill3", "Content 3")
			},
			skillNames: []string{"skill1", "skill2", "skill3"},
			wantCount:  3,
		},
		{
			name: "some skills not found",
			setup: func() {
				setupSkill("skill1", "Content 1")
				setupSkill("skill3", "Content 3")
			},
			skillNames: []string{"skill1", "skill2", "skill3"},
			wantCount:  2, // Only skill1 and skill3 exist
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			os.RemoveAll(workspaceSkills)
			tt.setup()

			sl := NewSkillsLoader(filepath.Join(tempDir, "workspace"), "", "")
			result := sl.LoadSkillsForContext(tt.skillNames)

			if tt.wantEmpty && result != "" {
				t.Errorf("LoadSkillsForContext() should return empty string, got %q", result)
			}

			if !tt.wantEmpty && result == "" {
				t.Errorf("LoadSkillsForContext() should not return empty string")
			}

			// Check that all loaded skills are present
			for _, name := range tt.skillNames {
				expected := fmt.Sprintf("### Skill: %s", name)
				if !strings.Contains(result, expected) {
					// Only check if the skill was supposed to exist
					skillPath := filepath.Join(workspaceSkills, name, "SKILL.md")
					if _, err := os.Stat(skillPath); err == nil {
						t.Errorf("LoadSkillsForContext() should contain %q", expected)
					}
				}
			}

			// Check for separator
			if len(tt.skillNames) > 1 {
				if !strings.Contains(result, "\n---\n") {
					t.Errorf("LoadSkillsForContext() should contain separator")
				}
			}
		})
	}
}

func TestSkillsLoader_BuildSkillsSummary(t *testing.T) {
	tempDir := t.TempDir()
	workspaceSkills := filepath.Join(tempDir, "workspace", "skills")

	setupSkill := func(name, desc string) {
		skillDir := filepath.Join(workspaceSkills, name)
		if err := os.MkdirAll(skillDir, 0o755); err != nil {
			t.Fatalf("failed to create skill directory: %v", err)
		}
		content := fmt.Sprintf(`---
{"name": "%s", "description": "%s"}
---
Content`, name, desc)
		skillFile := filepath.Join(skillDir, "SKILL.md")
		if err := os.WriteFile(skillFile, []byte(content), 0o644); err != nil {
			t.Fatalf("failed to write skill file: %v", err)
		}
	}

	tests := []struct {
		name       string
		setup      func()
		wantEmpty  bool
		wantInXML  []string
		notWantXML []string
	}{
		{
			name:      "no skills",
			setup:     func() {},
			wantEmpty: true,
		},
		{
			name: "single skill",
			setup: func() {
				setupSkill("skill1", "Description 1")
			},
			wantInXML: []string{
				"<skills>",
				"<name>skill1</name>",
				"<description>Description 1</description>",
				"<source>workspace</source>",
				"</skills>",
			},
		},
		{
			name: "multiple skills",
			setup: func() {
				setupSkill("skill1", "Description 1")
				setupSkill("skill2", "Description 2")
			},
			wantInXML: []string{
				"<name>skill1</name>",
				"<name>skill2</name>",
			},
		},
		{
			name: "XML escaping",
			setup: func() {
				setupSkill("test-skill", "Description with <tag> & symbols")
			},
			wantInXML: []string{
				"&lt;tag&gt;",
				"&amp;",
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			os.RemoveAll(workspaceSkills)
			tt.setup()

			sl := NewSkillsLoader(filepath.Join(tempDir, "workspace"), "", "")
			result := sl.BuildSkillsSummary()

			if tt.wantEmpty && result != "" {
				t.Errorf("BuildSkillsSummary() should return empty string, got %q", result)
			}

			if !tt.wantEmpty && result == "" {
				t.Errorf("BuildSkillsSummary() should not return empty string")
			}

			for _, expected := range tt.wantInXML {
				if !strings.Contains(result, expected) {
					t.Errorf("BuildSkillsSummary() should contain %q", expected)
				}
			}

			for _, notExpected := range tt.notWantXML {
				if strings.Contains(result, notExpected) {
					t.Errorf("BuildSkillsSummary() should not contain %q", notExpected)
				}
			}
		})
	}
}

func TestSkillsLoader_GetSkillMetadata(t *testing.T) {
	tempDir := t.TempDir()

	tests := []struct {
		name     string
		content  string
		wantName string
		wantDesc string
	}{
		{
			name: "JSON metadata with frontmatter",
			content: `---
{"name": "test-skill", "description": "A test skill"}
---
Content`,
			wantName: "test-skill",
			wantDesc: "A test skill",
		},
		{
			name: "YAML metadata with frontmatter",
			content: `---
name: test-skill
description: A test skill
---
Content`,
			wantName: "test-skill",
			wantDesc: "A test skill",
		},
		{
			name: "YAML with quotes",
			content: `---
name: "test-skill"
description: "A test skill"
---
Content`,
			wantName: "test-skill",
			wantDesc: "A test skill",
		},
		{
			name: "mixed quotes",
			content: `---
name: 'test-skill'
description: "A test skill"
---
Content`,
			wantName: "test-skill",
			wantDesc: "A test skill",
		},
		{
			name:     "no metadata - use directory name",
			content:  `Just content`,
			wantName: "test-skill", // Will use directory name
			wantDesc: "",
		},
		{
			name: "YAML with comments",
			content: `---
# This is a comment
name: test-skill
# Another comment
description: A test skill
---
Content`,
			wantName: "test-skill",
			wantDesc: "A test skill",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			skillDir := filepath.Join(tempDir, "test-skill")
			if err := os.MkdirAll(skillDir, 0o755); err != nil {
				t.Fatalf("failed to create skill directory: %v", err)
			}
			skillFile := filepath.Join(skillDir, "SKILL.md")
			if err := os.WriteFile(skillFile, []byte(tt.content), 0o644); err != nil {
				t.Fatalf("failed to write skill file: %v", err)
			}

			sl := NewSkillsLoader("", "", "")
			metadata := sl.getSkillMetadata(skillFile)

			if metadata == nil {
				t.Fatal("getSkillMetadata() should not return nil")
			}

			if metadata.Name != tt.wantName {
				t.Errorf("getSkillMetadata() name = %q, want %q", metadata.Name, tt.wantName)
			}

			if metadata.Description != tt.wantDesc {
				t.Errorf("getSkillMetadata() description = %q, want %q", metadata.Description, tt.wantDesc)
			}
		})
	}
}

func TestSkillsLoader_ExtractFrontmatter(t *testing.T) {
	tests := []struct {
		name            string
		content         string
		wantFrontmatter string
	}{
		{
			name:            "no frontmatter",
			content:         `Just content`,
			wantFrontmatter: "",
		},
		{
			name: "Unix line endings",
			content: `---
name: test
description: A test
---
Content`,
			wantFrontmatter: "name: test\ndescription: A test",
		},
		{
			name:            "Windows line endings",
			content:         "---\r\nname: test\r\ndescription: A test\r\n---\r\nContent",
			wantFrontmatter: "name: test\r\ndescription: A test",
		},
		{
			name:            "Classic Mac line endings",
			content:         "---\rname: test\rdescription: A test\r---\rContent",
			wantFrontmatter: "name: test\rdescription: A test",
		},
		{
			name:            "mixed line endings",
			content:         "---\r\nname: test\ndescription: A test\r\n---\nContent",
			wantFrontmatter: "name: test\ndescription: A test",
		},
		{
			name: "multiline frontmatter",
			content: `---
name: test
description: |
  A multi-line
  description
---
Content`,
			wantFrontmatter: "name: test\ndescription: |\n  A multi-line\n  description",
		},
		{
			name: "empty frontmatter",
			content: `---
---
Content`,
			wantFrontmatter: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			sl := NewSkillsLoader("", "", "")
			result := sl.extractFrontmatter(tt.content)
			if result != tt.wantFrontmatter {
				t.Errorf("extractFrontmatter() = %q, want %q", result, tt.wantFrontmatter)
			}
		})
	}
}

func TestSkillsLoader_StripFrontmatter(t *testing.T) {
	tests := []struct {
		name        string
		content     string
		wantContent string
	}{
		{
			name:        "no frontmatter",
			content:     `Just content`,
			wantContent: `Just content`,
		},
		{
			name:        "Unix line endings",
			content:     "---\nname: test\n---\nContent",
			wantContent: "Content",
		},
		{
			name:        "Windows line endings",
			content:     "---\r\nname: test\r\n---\r\nContent",
			wantContent: "Content",
		},
		{
			name:        "with blank line after frontmatter",
			content:     "---\nname: test\n---\n\nContent",
			wantContent: "Content",
		},
		{
			name: "multiline frontmatter",
			content: `---
name: test
description: A test
---
Content here`,
			wantContent: "Content here",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			sl := NewSkillsLoader("", "", "")
			result := sl.stripFrontmatter(tt.content)
			if result != tt.wantContent {
				t.Errorf("stripFrontmatter() = %q, want %q", result, tt.wantContent)
			}
		})
	}
}

func TestSkillsLoader_ParseSimpleYAML(t *testing.T) {
	tests := []struct {
		name      string
		content   string
		wantKey   string
		wantValue string
	}{
		{
			name:      "simple key-value",
			content:   "name: test",
			wantKey:   "name",
			wantValue: "test",
		},
		{
			name:      "with quotes",
			content:   `name: "test skill"`,
			wantKey:   "name",
			wantValue: "test skill",
		},
		{
			name:      "with single quotes",
			content:   `name: 'test skill'`,
			wantKey:   "name",
			wantValue: "test skill",
		},
		{
			name:      "multiple values",
			content:   "name: test\ndescription: A test",
			wantKey:   "description",
			wantValue: "A test",
		},
		{
			name:      "with comments",
			content:   "# Comment\nname: test",
			wantKey:   "name",
			wantValue: "test",
		},
		{
			name:      "empty lines",
			content:   "\nname: test\n",
			wantKey:   "name",
			wantValue: "test",
		},
		{
			name:      "Windows line endings",
			content:   "name: test\r\ndescription: A test",
			wantKey:   "description",
			wantValue: "A test",
		},
		{
			name:      "Classic Mac line endings",
			content:   "name: test\rdescription: A test",
			wantKey:   "description",
			wantValue: "A test",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			sl := NewSkillsLoader("", "", "")
			result := sl.parseSimpleYAML(tt.content)
			if result[tt.wantKey] != tt.wantValue {
				t.Errorf("parseSimpleYAML()[%q] = %q, want %q", tt.wantKey, result[tt.wantKey], tt.wantValue)
			}
		})
	}
}

func TestEscapeXML(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "ampersand",
			input:    "A & B",
			expected: "A &amp; B",
		},
		{
			name:     "less than",
			input:    "A < B",
			expected: "A &lt; B",
		},
		{
			name:     "greater than",
			input:    "A > B",
			expected: "A &gt; B",
		},
		{
			name:     "all special chars",
			input:    "<tag> & \"quoted\"",
			expected: "&lt;tag&gt; &amp; \"quoted\"",
		},
		{
			name:     "no special chars",
			input:    "normal text",
			expected: "normal text",
		},
		{
			name:     "empty string",
			input:    "",
			expected: "",
		},
		{
			name:     "multiple ampersands",
			input:    "A & B & C",
			expected: "A &amp; B &amp; C",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := escapeXML(tt.input)
			if result != tt.expected {
				t.Errorf("escapeXML() = %q, want %q", result, tt.expected)
			}
		})
	}
}
