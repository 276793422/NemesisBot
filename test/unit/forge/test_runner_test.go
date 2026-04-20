package forge_test

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/forge"
)

// === TestRunner Tests (Stage 2) ===

func newTestRunner(t *testing.T) (*forge.TestRunner, string) {
	t.Helper()
	tmpDir := t.TempDir()
	path := filepath.Join(tmpDir, "registry.json")
	registry := forge.NewRegistry(path)
	return forge.NewTestRunner(registry), tmpDir
}

// --- Existing tests (updated for new validation logic) ---

func TestRunnerSkillWithStructure(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	// Create a well-structured skill file with proper frontmatter
	skillDir := filepath.Join(tmpDir, "skills", "test-skill")
	os.MkdirAll(skillDir, 0755)
	skillContent := `---
name: test-skill
description: "A test skill for validation"
---

# Test Skill

## 步骤

1. First step
2. Second step

## Notes

- Note 1
- Note 2
`
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte(skillContent), 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactSkill,
		Name: "test-skill",
		Path: skillPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if !result.Passed {
		t.Errorf("Expected well-structured skill to pass, got errors: %v", result.Errors)
	}
	if result.TestsRun != 5 {
		t.Errorf("Expected 5 tests run, got %d", result.TestsRun)
	}
	if result.TestsPassed != 5 {
		t.Errorf("Expected 5 tests passed, got %d", result.TestsPassed)
	}
}

func TestRunnerSkillMissingStructure(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	skillDir := filepath.Join(tmpDir, "skills", "plain-skill")
	os.MkdirAll(skillDir, 0755)
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte("Just plain text without any structure."), 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactSkill,
		Name: "plain-skill",
		Path: skillPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected skill without structure to fail")
	}
}

func TestRunnerScriptWithTestCases(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	// Create script with test cases
	scriptDir := filepath.Join(tmpDir, "scripts", "utils")
	os.MkdirAll(scriptDir, 0755)
	scriptPath := filepath.Join(scriptDir, "test-script")
	os.WriteFile(scriptPath, []byte("#!/bin/bash\necho hello"), 0644)

	// Create test cases
	testDir := filepath.Join(scriptDir, "tests")
	os.MkdirAll(testDir, 0755)
	testCases := []map[string]interface{}{
		{"name": "test1", "input": "hello", "expected": "hello"},
		{"name": "test2", "input": "world", "expected": "world"},
	}
	testData, _ := json.MarshalIndent(testCases, "", "  ")
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), testData, 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactScript,
		Name: "test-script",
		Path: scriptPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if !result.Passed {
		t.Errorf("Expected script with test cases to pass, got errors: %v", result.Errors)
	}
	if result.TestsPassed != result.TestsRun {
		t.Errorf("Expected all tests to pass: run=%d, passed=%d", result.TestsRun, result.TestsPassed)
	}
}

func TestRunnerScriptMissingTestCases(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	scriptDir := filepath.Join(tmpDir, "scripts", "utils")
	os.MkdirAll(scriptDir, 0755)
	scriptPath := filepath.Join(scriptDir, "no-test-script")
	os.WriteFile(scriptPath, []byte("#!/bin/bash\necho hello"), 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactScript,
		Name: "no-test-script",
		Path: scriptPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected script without test cases to fail")
	}
}

func TestRunnerScriptInvalidTestCases(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	scriptDir := filepath.Join(tmpDir, "scripts", "utils")
	os.MkdirAll(scriptDir, 0755)
	scriptPath := filepath.Join(scriptDir, "bad-json-script")
	os.WriteFile(scriptPath, []byte("#!/bin/bash\necho hello"), 0644)

	testDir := filepath.Join(scriptDir, "tests")
	os.MkdirAll(testDir, 0755)
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), []byte("not valid json"), 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactScript,
		Name: "bad-json-script",
		Path: scriptPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected script with invalid test cases to fail")
	}
}

func TestRunnerMCPWithTests(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	// Create a complete Python MCP module
	mcpDir := filepath.Join(tmpDir, "mcp", "test-mcp")
	os.MkdirAll(mcpDir, 0755)
	pythonCode := `#!/usr/bin/env python3
from mcp.server import Server

server = Server("test-mcp")

@server.tool()
def hello(name: str) -> str:
    return f"Hello, {name}!"

if __name__ == "__main__":
    server.run()
`
	mcpPath := filepath.Join(mcpDir, "server.py")
	os.WriteFile(mcpPath, []byte(pythonCode), 0644)

	// Create requirements.txt
	os.WriteFile(filepath.Join(mcpDir, "requirements.txt"), []byte("mcp>=1.0.0\n"), 0644)

	// Create test cases with full structure
	testDir := filepath.Join(mcpDir, "tests")
	os.MkdirAll(testDir, 0755)
	testCases := []map[string]interface{}{
		{"name": "test1", "input": "hello", "expected": "Hello"},
	}
	testData, _ := json.MarshalIndent(testCases, "", "  ")
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), testData, 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactMCP,
		Name: "test-mcp",
		Path: mcpPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if !result.Passed {
		t.Errorf("Expected MCP with tests to pass, got errors: %v", result.Errors)
	}
	if result.TestsRun != 5 {
		t.Errorf("Expected 5 tests run, got %d", result.TestsRun)
	}
	if result.TestsPassed != 5 {
		t.Errorf("Expected 5 tests passed, got %d", result.TestsPassed)
	}
}

func TestRunnerMCPMissingTests(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	mcpDir := filepath.Join(tmpDir, "mcp", "no-test-mcp")
	os.MkdirAll(mcpDir, 0755)
	mcpPath := filepath.Join(mcpDir, "main.go")
	os.WriteFile(mcpPath, []byte("package main\n\nfunc main() {}\n"), 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactMCP,
		Name: "no-test-mcp",
		Path: mcpPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected MCP without tests to fail")
	}
}

func TestRunnerSkillFileNotFound(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	artifact := &forge.Artifact{
		Type: forge.ArtifactSkill,
		Name: "missing",
		Path: filepath.Join(tmpDir, "nonexistent", "SKILL.md"),
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected missing file to fail")
	}
}

func TestRunnerScriptEmptyTestCases(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	scriptDir := filepath.Join(tmpDir, "scripts", "utils")
	os.MkdirAll(scriptDir, 0755)
	scriptPath := filepath.Join(scriptDir, "empty-tests")
	os.WriteFile(scriptPath, []byte("#!/bin/bash\necho hello"), 0644)

	testDir := filepath.Join(scriptDir, "tests")
	os.MkdirAll(testDir, 0755)
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), []byte("[]"), 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactScript,
		Name: "empty-tests",
		Path: scriptPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected script with empty test cases to fail")
	}
}

// --- New Skill validation tests ---

func TestRunnerSkillValidFrontmatter(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	skillDir := filepath.Join(tmpDir, "skills", "my-valid-skill")
	os.MkdirAll(skillDir, 0755)
	skillContent := `---
name: my-valid-skill
description: "This is a valid skill description"
---

# My Valid Skill

## Steps

1. Do something
2. Do another thing
`
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte(skillContent), 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactSkill,
		Name: "my-valid-skill",
		Path: skillPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if !result.Passed {
		t.Errorf("Expected skill with valid frontmatter to pass, got errors: %v", result.Errors)
	}
	if result.TestsRun != 5 {
		t.Errorf("Expected 5 tests run, got %d", result.TestsRun)
	}
	if result.TestsPassed != 5 {
		t.Errorf("Expected 5 tests passed, got %d", result.TestsPassed)
	}
}

func TestRunnerSkillInvalidName(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	skillDir := filepath.Join(tmpDir, "skills", "bad-name")
	os.MkdirAll(skillDir, 0755)

	// Test underscore in name (not allowed)
	skillContent := `---
name: my_invalid_skill
description: "A skill with invalid name"
---

# Skill with invalid name

- Item 1
`
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte(skillContent), 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactSkill,
		Name: "bad-name",
		Path: skillPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected skill with invalid name (underscore) to fail")
	}

	// Verify the error is specifically about the name
	found := false
	for _, e := range result.Errors {
		if strings.Contains(e, "name") && strings.Contains(e, "不合法") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected error about invalid name, got: %v", result.Errors)
	}
}

func TestRunnerSkillInvalidNameSpace(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	skillDir := filepath.Join(tmpDir, "skills", "space-name")
	os.MkdirAll(skillDir, 0755)
	skillContent := `---
name: "my skill name"
description: "A skill with space in name"
---

# Skill with space in name

- Item 1
`
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte(skillContent), 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactSkill,
		Name: "space-name",
		Path: skillPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected skill with space in name to fail")
	}
}

func TestRunnerSkillMissingDescription(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	skillDir := filepath.Join(tmpDir, "skills", "no-desc")
	os.MkdirAll(skillDir, 0755)
	skillContent := `---
name: no-desc-skill
---

# Skill without description

- Item 1
`
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte(skillContent), 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactSkill,
		Name: "no-desc",
		Path: skillPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected skill without description to fail")
	}

	found := false
	for _, e := range result.Errors {
		if strings.Contains(e, "frontmatter") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected error about frontmatter, got: %v", result.Errors)
	}
}

func TestRunnerSkillNameTooLong(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	skillDir := filepath.Join(tmpDir, "skills", "long-name")
	os.MkdirAll(skillDir, 0755)

	longName := strings.Repeat("a", 65)
	skillContent := `---
name: ` + longName + `
description: "A skill with a very long name"
---

# Skill with long name

- Item 1
`
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte(skillContent), 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactSkill,
		Name: "long-name",
		Path: skillPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected skill with name > 64 chars to fail")
	}
}

func TestRunnerSkillEmptyBody(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	skillDir := filepath.Join(tmpDir, "skills", "empty-body")
	os.MkdirAll(skillDir, 0755)
	skillContent := `---
name: empty-body
description: "A skill with empty body"
---
`
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte(skillContent), 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactSkill,
		Name: "empty-body",
		Path: skillPath,
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected skill with empty body to fail")
	}

	found := false
	for _, e := range result.Errors {
		if strings.Contains(e, "正文为空") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected error about empty body, got: %v", result.Errors)
	}
}

// --- New MCP validation tests ---

func TestRunnerMCPValidPython(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	mcpDir := filepath.Join(tmpDir, "mcp", "valid-py")
	os.MkdirAll(mcpDir, 0755)
	pythonCode := `#!/usr/bin/env python3
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("demo")

@mcp.tool()
def greet(name: str) -> str:
    """Greet someone."""
    return f"Hello, {name}!"

if __name__ == "__main__":
    mcp.run()
`
	os.WriteFile(filepath.Join(mcpDir, "server.py"), []byte(pythonCode), 0644)
	os.WriteFile(filepath.Join(mcpDir, "requirements.txt"), []byte("mcp>=1.0.0\n"), 0644)

	testDir := filepath.Join(mcpDir, "tests")
	os.MkdirAll(testDir, 0755)
	testCases := []map[string]interface{}{
		{"name": "greet", "input": "world", "expected": "Hello, world!"},
	}
	testData, _ := json.MarshalIndent(testCases, "", "  ")
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), testData, 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactMCP,
		Name: "valid-py",
		Path: filepath.Join(mcpDir, "server.py"),
	}

	result := runner.RunTests(context.Background(), artifact)

	if !result.Passed {
		t.Errorf("Expected valid Python MCP to pass, got errors: %v", result.Errors)
	}
	if result.TestsRun != 5 {
		t.Errorf("Expected 5 tests run, got %d", result.TestsRun)
	}
	if result.TestsPassed != 5 {
		t.Errorf("Expected 5 tests passed, got %d", result.TestsPassed)
	}
}

func TestRunnerMCPBracketMismatch(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	mcpDir := filepath.Join(tmpDir, "mcp", "bracket-mismatch")
	os.MkdirAll(mcpDir, 0755)
	// Python code with unbalanced braces
	pythonCode := `#!/usr/bin/env python3
from mcp.server import Server

server = Server("test")

@server.tool()
def broken(name: str) -> str:
    data = {"key": "value"
    return data
`
	os.WriteFile(filepath.Join(mcpDir, "server.py"), []byte(pythonCode), 0644)
	os.WriteFile(filepath.Join(mcpDir, "requirements.txt"), []byte("mcp>=1.0.0\n"), 0644)

	testDir := filepath.Join(mcpDir, "tests")
	os.MkdirAll(testDir, 0755)
	testCases := []map[string]interface{}{
		{"name": "test1", "input": "a", "expected": "b"},
	}
	testData, _ := json.MarshalIndent(testCases, "", "  ")
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), testData, 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactMCP,
		Name: "bracket-mismatch",
		Path: filepath.Join(mcpDir, "server.py"),
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected MCP with bracket mismatch to fail")
	}

	found := false
	for _, e := range result.Errors {
		if strings.Contains(e, "括号不匹配") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected error about bracket mismatch, got: %v", result.Errors)
	}
}

func TestRunnerMCPMissingRequirements(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	mcpDir := filepath.Join(tmpDir, "mcp", "no-reqs")
	os.MkdirAll(mcpDir, 0755)
	pythonCode := `#!/usr/bin/env python3
from mcp.server import Server

server = Server("test")

@server.tool()
def hello(name: str) -> str:
    return f"Hello, {name}!"

if __name__ == "__main__":
    server.run()
`
	os.WriteFile(filepath.Join(mcpDir, "server.py"), []byte(pythonCode), 0644)
	// Deliberately NOT creating requirements.txt

	testDir := filepath.Join(mcpDir, "tests")
	os.MkdirAll(testDir, 0755)
	testCases := []map[string]interface{}{
		{"name": "test1", "input": "a", "expected": "b"},
	}
	testData, _ := json.MarshalIndent(testCases, "", "  ")
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), testData, 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactMCP,
		Name: "no-reqs",
		Path: filepath.Join(mcpDir, "server.py"),
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected MCP without requirements.txt to fail")
	}

	found := false
	for _, e := range result.Errors {
		if strings.Contains(e, "requirements.txt") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected error about missing requirements.txt, got: %v", result.Errors)
	}
}

func TestRunnerMCPServerNoInit(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	mcpDir := filepath.Join(tmpDir, "mcp", "no-init")
	os.MkdirAll(mcpDir, 0755)
	// Python code without Server/FastMCP initialization
	pythonCode := `#!/usr/bin/env python3

def hello(name: str) -> str:
    return f"Hello, {name}!"

if __name__ == "__main__":
    hello("world")
`
	os.WriteFile(filepath.Join(mcpDir, "server.py"), []byte(pythonCode), 0644)
	os.WriteFile(filepath.Join(mcpDir, "requirements.txt"), []byte("mcp>=1.0.0\n"), 0644)

	testDir := filepath.Join(mcpDir, "tests")
	os.MkdirAll(testDir, 0755)
	testCases := []map[string]interface{}{
		{"name": "test1", "input": "a", "expected": "b"},
	}
	testData, _ := json.MarshalIndent(testCases, "", "  ")
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), testData, 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactMCP,
		Name: "no-init",
		Path: filepath.Join(mcpDir, "server.py"),
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected MCP without server initialization to fail")
	}

	found := false
	for _, e := range result.Errors {
		if strings.Contains(e, "Server") || strings.Contains(e, "FastMCP") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected error about missing Server init, got: %v", result.Errors)
	}
}

func TestRunnerMCPNoToolRegistration(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	mcpDir := filepath.Join(tmpDir, "mcp", "no-tool")
	os.MkdirAll(mcpDir, 0755)
	// Python code with server init but no tool registration
	pythonCode := `#!/usr/bin/env python3
from mcp.server import Server

server = Server("test")

if __name__ == "__main__":
    server.run()
`
	os.WriteFile(filepath.Join(mcpDir, "server.py"), []byte(pythonCode), 0644)
	os.WriteFile(filepath.Join(mcpDir, "requirements.txt"), []byte("mcp>=1.0.0\n"), 0644)

	testDir := filepath.Join(mcpDir, "tests")
	os.MkdirAll(testDir, 0755)
	testCases := []map[string]interface{}{
		{"name": "test1", "input": "a", "expected": "b"},
	}
	testData, _ := json.MarshalIndent(testCases, "", "  ")
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), testData, 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactMCP,
		Name: "no-tool",
		Path: filepath.Join(mcpDir, "server.py"),
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected MCP without tool registration to fail")
	}

	found := false
	for _, e := range result.Errors {
		if strings.Contains(e, "工具注册") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected error about missing tool registration, got: %v", result.Errors)
	}
}

func TestRunnerMCPTestCaseStructure(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	mcpDir := filepath.Join(tmpDir, "mcp", "bad-testcases")
	os.MkdirAll(mcpDir, 0755)
	pythonCode := `#!/usr/bin/env python3
from mcp.server import Server

server = Server("test")

@server.tool()
def hello(name: str) -> str:
    return f"Hello, {name}!"

if __name__ == "__main__":
    server.run()
`
	os.WriteFile(filepath.Join(mcpDir, "server.py"), []byte(pythonCode), 0644)
	os.WriteFile(filepath.Join(mcpDir, "requirements.txt"), []byte("mcp>=1.0.0\n"), 0644)

	// Test cases missing "expected" field
	testDir := filepath.Join(mcpDir, "tests")
	os.MkdirAll(testDir, 0755)
	testCases := []map[string]interface{}{
		{"name": "test1", "input": "a"},
	}
	testData, _ := json.MarshalIndent(testCases, "", "  ")
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), testData, 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactMCP,
		Name: "bad-testcases",
		Path: filepath.Join(mcpDir, "server.py"),
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected MCP with incomplete test cases to fail")
	}

	found := false
	for _, e := range result.Errors {
		if strings.Contains(e, "expected") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected error about missing expected field, got: %v", result.Errors)
	}
}

func TestRunnerMCPGoComplete(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	mcpDir := filepath.Join(tmpDir, "mcp", "go-complete")
	os.MkdirAll(mcpDir, 0755)
	goCode := `package main

import (
	"context"
	"fmt"
)

func main() {
	fmt.Println("MCP server running")
}
`
	os.WriteFile(filepath.Join(mcpDir, "main.go"), []byte(goCode), 0644)
	os.WriteFile(filepath.Join(mcpDir, "go.mod"), []byte("module test-mcp\ngo 1.21\n"), 0644)

	testDir := filepath.Join(mcpDir, "tests")
	os.MkdirAll(testDir, 0755)
	testCases := []map[string]interface{}{
		{"name": "test1", "input": "hello", "expected": "hello"},
	}
	testData, _ := json.MarshalIndent(testCases, "", "  ")
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), testData, 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactMCP,
		Name: "go-complete",
		Path: filepath.Join(mcpDir, "main.go"),
	}

	result := runner.RunTests(context.Background(), artifact)

	if !result.Passed {
		t.Errorf("Expected complete Go MCP to pass, got errors: %v", result.Errors)
	}
	if result.TestsRun != 5 {
		t.Errorf("Expected 5 tests run, got %d", result.TestsRun)
	}
	if result.TestsPassed != 5 {
		t.Errorf("Expected 5 tests passed, got %d", result.TestsPassed)
	}
}

func TestRunnerMCPGoMissingGoMod(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	mcpDir := filepath.Join(tmpDir, "mcp", "go-no-mod")
	os.MkdirAll(mcpDir, 0755)
	goCode := `package main

func main() {}
`
	os.WriteFile(filepath.Join(mcpDir, "main.go"), []byte(goCode), 0644)
	// Deliberately NOT creating go.mod

	testDir := filepath.Join(mcpDir, "tests")
	os.MkdirAll(testDir, 0755)
	testCases := []map[string]interface{}{
		{"name": "test1", "input": "hello", "expected": "hello"},
	}
	testData, _ := json.MarshalIndent(testCases, "", "  ")
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), testData, 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactMCP,
		Name: "go-no-mod",
		Path: filepath.Join(mcpDir, "main.go"),
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected Go MCP without go.mod to fail")
	}

	found := false
	for _, e := range result.Errors {
		if strings.Contains(e, "go.mod") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected error about missing go.mod, got: %v", result.Errors)
	}
}

func TestRunnerMCPGoMissingMain(t *testing.T) {
	runner, tmpDir := newTestRunner(t)

	mcpDir := filepath.Join(tmpDir, "mcp", "go-no-main")
	os.MkdirAll(mcpDir, 0755)
	// Go code without func main
	goCode := `package main

import "fmt"

func helper() {
    fmt.Println("helper")
}
`
	os.WriteFile(filepath.Join(mcpDir, "main.go"), []byte(goCode), 0644)
	os.WriteFile(filepath.Join(mcpDir, "go.mod"), []byte("module test-mcp\ngo 1.21\n"), 0644)

	testDir := filepath.Join(mcpDir, "tests")
	os.MkdirAll(testDir, 0755)
	testCases := []map[string]interface{}{
		{"name": "test1", "input": "hello", "expected": "hello"},
	}
	testData, _ := json.MarshalIndent(testCases, "", "  ")
	os.WriteFile(filepath.Join(testDir, "test_cases.json"), testData, 0644)

	artifact := &forge.Artifact{
		Type: forge.ArtifactMCP,
		Name: "go-no-main",
		Path: filepath.Join(mcpDir, "main.go"),
	}

	result := runner.RunTests(context.Background(), artifact)

	if result.Passed {
		t.Error("Expected Go MCP without func main to fail")
	}

	found := false
	for _, e := range result.Errors {
		if strings.Contains(e, "func main") {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("Expected error about missing func main, got: %v", result.Errors)
	}
}
